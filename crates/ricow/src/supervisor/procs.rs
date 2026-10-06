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

/// 日志文件轮转阈值 (超过则轮转为 `.log.1`, 单份备份)。
///
/// 对 crate 内可见: `web::tail` 的跨模块测试要据此断言"轮转后文件不会缩到很小"。
pub(crate) const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// 日志轮转的**周期**(监控循环节拍): 只在启动时检查会让长运行策略的日志无限增长,
/// 因此运行期同样按此间隔检查 (见 [`rotate_large_log`])。
pub const LOG_ROTATE_INTERVAL: Duration = Duration::from_secs(60);

/// 轮转时为了把切口对齐到行边界而多读的字节数 (见 [`tail_window`])。
const LINE_SCAN_SLACK_BYTES: u64 = 64 * 1024;

/// 停机等待上限 (清理可能调用交易所, 故不用短超时)。
pub const STOP_WAIT: Duration = Duration::from_secs(30);

/// daemon 派生标记 (019 T030): daemon 派生策略子进程时注入, 子进程据此认定
/// "实盘确认已在交互终端逐字完成", 不再索要 stdin 确认 (那里只收停机指令)。
///
/// 只注入给 daemon 自己派生的子进程; 用户手工在 shell 里跑 `ricow run --live` 没有这个标记,
/// 因此即使凑巧传了内部标志 `--live-confirmed` 也仍会被要求逐字确认 —— 内部通道不会被误走成绕过。
pub const DAEMON_SPAWN_ENV: &str = "RICOW_DAEMON_SPAWNED";

/// 本进程是否由 daemon 派生 (见 [`DAEMON_SPAWN_ENV`])。
pub fn spawned_by_daemon() -> bool {
    std::env::var_os(DAEMON_SPAWN_ENV).is_some()
}

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
#[allow(clippy::too_many_arguments)] // 参数聚合重构另行立项(021 只清存量告警, 不改结构)
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
    // 启动即轮转一次: 上次运行残留的超大日志先进备份, 新进程从干净文件开始写。
    rotate_large_log(&log_path);

    let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)?;
    let err = log.try_clone()?;

    let mut cmd = Command::new(exe);
    cmd.arg("run").arg(name);
    if live {
        cmd.arg("--live");
        // 实盘确认已由唤醒它的交互终端完成(019 T030): 子进程不再索要, 否则会吃掉 daemon 的 stdin 停机指令
        cmd.arg("--live-confirmed");
        // 光有标志不够, 子进程还要看到这个 daemon 注入的标记才会认账(见 `DAEMON_SPAWN_ENV`)
        cmd.env(DAEMON_SPAWN_ENV, "1");
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

/// 计算轮转后要保留的日志窗口: 从 `len` 字节的末尾**对齐到行边界**取不超过
/// `max_bytes` 的一段, 返回 `(起始偏移, 窗口字节数)`。
///
/// 先多读 `max_bytes + LINE_SCAN_SLACK_BYTES` 然后回退到最后一个 `\n` 之后, 是为了让
/// "按字节估的窗口"能容下一行的长度, 避免切口正好落在超长行中间导致被对齐成极短窗口。
/// 文件里没有换行符(单行超长日志)时无从对齐, 只能从估算位置硬切。
///
/// **返回的窗口长度可能超过 `max_bytes`**(多出最多一个行长), 由调用方裁到阈值内。
fn tail_window(len: u64, max_bytes: u64) -> (u64, usize) {
    let rounded = len.min(max_bytes);
    let read_len = rounded.saturating_add(LINE_SCAN_SLACK_BYTES).min(len);
    let start = len - read_len;
    (start, read_len as usize)
}

/// 把窗口裁到 `max_bytes` 之内, **保留末尾**(最新日志)并保证首尾都落在行边界上。
///
/// 这是轮转语义的关键一步: 超出阈值时必须丢**最旧**的那段, 而不是最新的。早先的实现
/// 直接 `truncate(max_bytes)` —— 在"整个文件就是窗口"的情形下(文件刚过阈值一点点),
/// 那等于把**最新的日志**切进备份、把**最旧的**留在原文件, 尾读会突然跳到很旧的位置。
///
/// - 先按需从**头部**推进起点(跳过若干字节, 再对齐到下一个 `\n` 之后);
/// - 只要还有空间就继续, 直到长度 <= `max_bytes`;
/// - 无 `\n` 可对齐(单行超长)时退化为按字节从头部硬切。
fn trim_window_to_budget(window: &mut Vec<u8>, max_bytes: u64) {
    let max = max_bytes as usize;
    if window.len() <= max {
        return;
    }
    // 需要丢掉的字节数(至少这么多), 从头部丢。
    let mut drop = window.len() - max;
    // 对齐到 `\n` 之后: 从 drop 位置往后找第一个换行, 丢掉它及其之前的内容。
    // 没有换行时只能硬切(整段都是同一超长行的一部分), 保持 `drop` 不动。
    if let Some(i) = window[drop..].iter().position(|b| *b == b'\n') {
        drop += i + 1;
    }
    window.drain(..drop);
    // 对齐后可能仍略超(丢掉的字节少于需要), 再从头硬切一次兜底。
    if window.len() > max {
        let extra = window.len() - max;
        match window[extra..].iter().position(|b| *b == b'\n') {
            Some(i) => {
                window.drain(..extra + i + 1);
            }
            None => {
                window.drain(..extra);
            }
        }
    }
}

/// 日志超阈值则轮转为 `<name>.log.1` (当前写入者是自己, 重命名安全)。
///
/// 关键点 (**Q3: 必须是"轮转"而不是"截断"**): 若直接清空/截断, Web 只读尾读
/// (`web::tail::tail_since`) 会因为"文件比上次 offset 短"而误报 `rotated`。虽然报
/// `rotated` 是如实的, 但它会把刚过去的那段日志从客户端视野里抹掉 —— 所以这里统一
/// 采用"保留末尾窗口 + 其余进 `.log.1`"的语义: 文件长度**单调不减**, 中间无空档,
/// 尾读视角里既没有跳变、也没有新写入的行被当成旧内容。
///
/// 失败不影响策略运行 (日志不是交易通道)。
pub fn rotate_large_log(path: &Path) -> Option<PathBuf> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size <= LOG_ROTATE_BYTES {
        return None;
    }

    // 1. 对齐行边界算出要留在原文件里的窗口, 只读这一段(不限流整份日志)。
    let (start, window_len) = tail_window(size, LOG_ROTATE_BYTES);
    let mut window = match read_window(path, start, window_len) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(target: "supervisor", path = %path.display(), "日志轮转读取窗口失败: {e}");
            return None;
        }
    };

    // 对齐行边界后可能超出阈值一个行长(slack 只保证有整行可对齐)。硬上限**优先**于
    // 行对齐, 但必须丢**最旧**的那段: 尾部保留的是最新日志, 且首尾都落在行边界上,
    // 这样尾读既不会看到半截行, 也不会因"最新日志被切走"而跳回很旧的位置。
    trim_window_to_budget(&mut window, LOG_ROTATE_BYTES);

    let rotated = path.with_extension("log.1");
    let tmp = path.with_extension("log.rot");

    // 2. 先把窗口落成临时文件并 fsync —— 此刻原文件还没被动过, 任何一步失败都能安全放弃。
    let write_tmp = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&window)?;
        f.sync_all()
    };
    if let Err(e) = write_tmp() {
        tracing::warn!(target: "supervisor", path = %path.display(), "日志轮转写临时文件失败: {e}");
        let _ = std::fs::remove_file(&tmp);
        return None;
    }

    // 3. 备份旧备份 → 原文件让位给 `.log.1` → 临时文件顶替原路径。
    //    中间任一步失败都尽量把窗口放回原路径, 不留"日志文件不存在"的状态。
    if let Err(e) = std::fs::rename(path, &rotated) {
        tracing::warn!(target: "supervisor", path = %path.display(), "日志轮转改名失败: {e}");
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => {
            tracing::info!(
                target: "supervisor",
                path = %path.display(),
                kept_bytes = window.len(),
                backup = %rotated.display(),
                "日志已轮转 (保留末尾窗口, 其余进备份)"
            );
            Some(rotated)
        }
        Err(e) => {
            // 尽力恢复: 把窗口写回原路径, 否则该策略日志就没文件了。
            tracing::warn!(target: "supervisor", path = %path.display(), "日志轮转替换失败: {e}");
            if std::fs::rename(&tmp, path).is_err() {
                if let Ok(w) = read_window(&rotated, 0, window.len().min(LOG_ROTATE_BYTES as usize))
                {
                    if std::fs::write(path, &w).is_ok() {
                        tracing::warn!(target: "supervisor", "已从备份恢复该轮窗口, 备份保留待人工核对");
                    }
                }
            }
            None
        }
    }
}

/// 读 `[start, start+len)` 这一段 (文件可能比预期短, 按实际读到的算)。
fn read_window(path: &Path, start: u64, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    let mut filled = 0usize;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    buf.truncate(filled);
    if let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n') {
        buf.truncate(last_nl + 1); // 对齐到行边界: 不把半截行带进窗口
    } else if start > 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "窗口内没有换行符, 无法对齐行边界",
        ));
    }
    Ok(buf)
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
    fn rotate_large_log_keeps_tail_window_and_backs_up() {
        let root = tmp_root("rotate");
        ledger::ensure_dirs(&root).unwrap();
        let log = ledger::log_path(&root, "demo");

        // 未超阈值: 不轮转, 也不产生备份。
        std::fs::write(&log, vec![b'x'; 1024]).unwrap();
        assert_eq!(rotate_large_log(&log), None);
        assert!(log.exists());
        assert!(!log.with_extension("log.1").exists());

        // 超阈值: 备份收走旧内容, 原文件保留"末尾窗口"(行边界对齐) —— 长度单调不减。
        // 每行 23 字节 → 50 万行 ≈ 11.5MB, 稳稳超过 10MB 阈值。
        let lines: Vec<String> =
            (0..500_000).map(|i| format!("line-{i:06}-aaaaaaaaaa\n")).collect();
        std::fs::write(&log, lines.concat()).unwrap();
        let before = std::fs::metadata(&log).unwrap().len();
        assert!(before > LOG_ROTATE_BYTES);

        let rotated = rotate_large_log(&log).expect("应轮转");
        assert!(rotated.to_string_lossy().ends_with("demo.log.1"));
        assert!(rotated.exists());

        let kept = std::fs::read(&log).unwrap();
        assert!(kept.len() as u64 <= LOG_ROTATE_BYTES, "窗口须在阈值内");
        assert!(
            kept.len() as u64 > LOG_ROTATE_BYTES - LINE_SCAN_SLACK_BYTES,
            "窗口不应被对齐切得太短"
        );
        assert!(kept.ends_with(b"\n"), "窗口必须以整行结尾 (不留下半截行)");
        assert!(kept.starts_with(b"line-"), "窗口首字节也是行首 (切口对齐行边界)");
        assert!(before > kept.len() as u64, "必然丢弃了一部分最旧的日志");

        // **核心语义**: 保留的是**最新**的那段 —— 窗口末行必须等于原文件的末行。
        // (早先的实现会丢错方向: 把最新日志切进备份、把最旧的留在原文件。)
        let kept_text = String::from_utf8_lossy(&kept).to_string();
        let kept_last = kept_text.lines().last().expect("窗口有行").to_string();
        assert_eq!(kept_last, "line-499999-aaaaaaaaaa", "窗口末行必须是原日志的末行(保留最新)");
        // 首行应是 499999 - (行数 - 1) —— 即窗口覆盖的是原日志末尾连续的一段。
        let kept_lines = kept_text.lines().count();
        assert_eq!(
            kept_text.lines().next().unwrap(),
            format!("line-{:06}-aaaaaaaaaa", 500_000 - kept_lines),
            "窗口须是原日志末尾的连续一段(收紧后从更早处切开也不许有缺口)"
        );

        // 备份里是完整原文(不丢内容), 且没有残留临时文件。
        let backup = std::fs::read(&rotated).unwrap();
        assert_eq!(backup, lines.concat().into_bytes(), "备份须逐字保留原日志");
        assert!(!log.with_extension("log.rot").exists(), "轮转后不应残留临时文件");

        // 幂等: 未长到阈值前再调用仍是 no-op。
        assert_eq!(rotate_large_log(&log), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 轮转决策的纯逻辑: 短于阈值不动; 长于阈值时窗口 = 末尾 `max_bytes`(先对齐行边界)。
    #[test]
    fn tail_window_is_bounded_and_line_aligned_inputs() {
        // 未超阈值 → 调用方会提前返回, 这里只校验边界算法自身。
        assert_eq!(tail_window(100, 1024), (0, 100));
        // 超阈值: 多读 slack 后回退到行边界, 窗口不超过"估算上限 + slack"。
        let (start, len) = tail_window(10 * 1024 * 1024, 1024);
        assert_eq!(start + len as u64, 10 * 1024 * 1024, "窗口必须贴住文件末尾");
        assert_eq!(len as u64, 1024 + LINE_SCAN_SLACK_BYTES);
    }

    /// `trim_window_to_budget`: 超预算时丢**头部**(最旧), 保留**尾部**(最新),
    /// 且裁完首字节落在行首。这是"轮转保留最新日志"的关键保证。
    #[test]
    fn trim_window_to_budget_drops_oldest_not_newest() {
        let mk = |n: usize| -> Vec<u8> {
            let mut v = Vec::new();
            for i in 0..n {
                v.extend_from_slice(format!("L{i:05}\n").as_bytes());
            }
            v
        };

        // 未超预算: 原样不动。
        let mut w = mk(10);
        let orig = w.clone();
        trim_window_to_budget(&mut w, 10_000);
        assert_eq!(w, orig, "未超预算不该改动");

        // 超预算: 头部被丢, 尾部保留, 首字节是行首, 且长度 <= 预算。
        let budget = 20usize; // 每行 7 字节 → 至少要丢到 <=20 字节, 即 2 行(14 字节)
        let mut w = mk(10);
        trim_window_to_budget(&mut w, budget as u64);
        assert!(w.len() <= budget, "裁完不得超过预算: {}", w.len());
        assert!(w.starts_with(b"L"), "首字节须落在行首");
        assert!(w.ends_with(b"\n"), "尾字节须是行尾");
        let text = String::from_utf8(w.clone()).unwrap();
        assert_eq!(text.lines().last().unwrap(), "L00009", "必须保留最新的那行");
        assert!(!text.contains("L00000"), "最旧的那行必须被丢掉");

        // 单行超长(无换行可对齐): 退化为按字节从头部硬切, 不 panic。
        let mut w = vec![b'x'; 100];
        trim_window_to_budget(&mut w, 30);
        assert_eq!(w.len(), 30);
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
