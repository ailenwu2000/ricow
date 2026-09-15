//! 策略子进程生命周期: 启动 / 停机指令 / 等待退出 / 终止 / 日志重定向 (008)。
//!
//! 关键约定 (决策 P1/P4/P5):
//! - 用 `std::process` spawn (不用 `tokio::process`: 实测后者在启动器退出时会带走子进程)
//! - 子进程与 daemon 同生命周期; daemon 消失时子进程靠 stdin 管道 EOF 自愈退出
//! - 停机 = 向子进程 stdin 写 `stop` (跨平台一致, 不依赖信号语义)

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crate::supervisor::{ledger, proto::InstanceView};

/// 日志文件轮转阈值 (启动时若超过则轮转为 `.log.1`, 单份备份)。
const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// 停机等待上限 (清理可能调用交易所, 故不用短超时)。
pub const STOP_WAIT: Duration = Duration::from_secs(30);

/// 运行中的策略实例句柄。
pub struct ChildHandle {
    pub child: Child,
    pub stdin: Option<ChildStdin>,
    /// 运行视图 (pid/启动时间/模式/交易对/市场), 直接用于 list/info
    pub view: InstanceView,
}

impl ChildHandle {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// 启动 `ricow run <name>` 子进程: stdin 管道 (停机指令), stdout/stderr 追加到日志文件。
///
/// `live=true` 时透传 `--live` (实盘仍由子进程侧的门禁双条件复核, 见 011 FR-013)。
pub fn spawn_strategy(
    root: &Path,
    exe: &Path,
    name: &str,
    mode: &str,
    pair: &str,
    market: &str,
    live: bool,
    demo: bool,
) -> std::io::Result<ChildHandle> {
    ledger::ensure_dirs(root)?;
    let log_path = ledger::log_path(root, name);
    rotate_log(&log_path);

    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    let err = log.try_clone()?;

    let mut cmd = Command::new(exe);
    cmd.arg("run").arg(name);
    if live {
        cmd.arg("--live");
        // 实盘确认已由唤醒它的交互终端完成(019 T030): 子进程不再索要, 否则会吃掉 daemon 的 stdin 停机指令
        cmd.arg("--live-confirmed");
    }
    if demo {
        cmd.arg("--demo");
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::from(log)).stderr(Stdio::from(err));

    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take();
    let pid = child.id();
    let view = InstanceView {
        name: name.to_string(),
        running: true,
        pid: Some(pid),
        started_at: Some(ledger::now_str()),
        uptime_secs: Some(0),
        mode: Some(mode.to_string()),
        pair: Some(pair.to_string()),
        market: Some(market.to_string()),
        last_exit: None,
        last_exit_at: None,
        last_reason: None,
    };
    Ok(ChildHandle { child, stdin, view })
}

/// 停机指令文本 (008 管道通道; 011 增加平仓开关):
/// `stop` = 撤单兜底; `stop --close-all` = 撤单兜底 + 平掉策略持仓。
pub fn stop_instruction(close_all: bool) -> &'static str {
    if close_all {
        "stop --close-all\n"
    } else {
        "stop\n"
    }
}

/// 下发停机指令。子进程未接 stdin 时返回错误, 由调用方如实报告。
pub fn request_stop(stdin: &mut Option<ChildStdin>, close_all: bool) -> std::io::Result<()> {
    let Some(s) = stdin.as_mut() else {
        return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "子进程 stdin 不可用"));
    };
    s.write_all(stop_instruction(close_all).as_bytes())?;
    s.flush()
}

/// 阻塞等待子进程退出 (调用方应在 spawn_blocking 中执行)。
/// 返回 (是否已退出, 退出码, 实际等待时长)。
pub fn wait_exit(child: &mut Child, timeout: Duration) -> (bool, Option<i32>, Duration) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (true, status.code(), start.elapsed()),
            Ok(None) => {}
            Err(_) => return (false, None, start.elapsed()),
        }
        if start.elapsed() >= timeout {
            return (false, None, start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 日志超过阈值则轮转为 `<name>.log.1` (写入者是自己, 重命名安全; 失败不影响启动)。
pub fn rotate_log(path: &Path) -> Option<PathBuf> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size <= LOG_ROTATE_BYTES {
        return None;
    }
    let rotated = path.with_extension("log.1");
    let _ = std::fs::remove_file(&rotated);
    match std::fs::rename(path, &rotated) {
        Ok(()) => Some(rotated),
        Err(e) => {
            tracing::warn!(target: "supervisor", path = %path.display(), "日志轮转失败: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-procs-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create tmp root");
        d
    }

    #[test]
    fn rotate_log_over_threshold() {
        let root = tmp_root("rotate");
        ledger::ensure_dirs(&root).unwrap();
        let log = ledger::log_path(&root, "demo");

        // 未超阈值: 不轮转
        std::fs::write(&log, vec![b'x'; 1024]).unwrap();
        assert_eq!(rotate_log(&log), None);
        assert!(log.exists());

        // 超阈值: 轮转为 .log.1 且原文件让位
        std::fs::write(&log, vec![b'x'; (LOG_ROTATE_BYTES + 1) as usize]).unwrap();
        let rotated = rotate_log(&log).expect("应轮转");
        assert!(rotated.to_string_lossy().ends_with("demo.log.1"));
        assert!(rotated.exists());
        assert!(!log.exists());

        // 再写小文件: 不覆盖已有 .log.1 之外的内容 (轮转幂等)
        std::fs::write(&log, b"new").unwrap();
        std::fs::write(&log, vec![b'y'; (LOG_ROTATE_BYTES + 1) as usize]).unwrap();
        assert!(rotate_log(&log).is_some());
        assert_eq!(std::fs::read(&rotated).unwrap().len(), (LOG_ROTATE_BYTES + 1) as usize);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn stop_instruction_and_wait_exit() {
        // 用 sh 模拟策略进程: 读到 stop 行后执行"清理"再退出 (与 run 的 stdin 监听同构)
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("while read line; do [ \"$line\" = stop ] && exit 0; done; exit 7")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let mut stdin = child.stdin.take();

        // 发送停机指令 → 优雅退出 (exit 0)
        request_stop(&mut stdin, false).expect("send stop");
        let (exited, code, _) = wait_exit(&mut child, Duration::from_secs(10));
        assert!(exited, "应观察到退出");
        assert_eq!(code, Some(0), "收到 stop 后按 0 退出");

        // stdin 关闭 (EOF) → 子进程读到 EOF 退出 (自愈路径的同构)
        let mut child2 = Command::new("sh")
            .arg("-c")
            .arg("while read line; do :; done; exit 3")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sh2");
        let stdin2 = child2.stdin.take();
        drop(stdin2); // 关闭管道写端 = daemon 消失
        let (exited2, code2, _) = wait_exit(&mut child2, Duration::from_secs(10));
        assert!(exited2, "EOF 后应退出");
        assert_eq!(code2, Some(3), "EOF 路径退出码由策略进程决定");
    }

    #[cfg(unix)]
    #[test]
    fn wait_exit_times_out_without_exit() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 5")
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn");
        let (exited, code, waited) = wait_exit(&mut child, Duration::from_millis(300));
        assert!(!exited, "超时应返回未退出");
        assert_eq!(code, None);
        assert!(waited >= Duration::from_millis(300));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn request_stop_without_stdin_reports_error() {
        let mut none: Option<ChildStdin> = None;
        let err = request_stop(&mut none, false).expect_err("无 stdin 应报错");
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn stop_instruction_carries_close_all_intent() {
        assert_eq!(stop_instruction(false), "stop\n");
        assert_eq!(stop_instruction(true), "stop --close-all\n");
        assert!(stop_instruction(true).contains("close-all"), "平仓意图须体现在指令文本");
    }
}
