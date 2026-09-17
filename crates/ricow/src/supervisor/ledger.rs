//! 实例台账与 daemon 元信息文件读写 (008)。
//!
//! 文件布局 (RICOW_ROOT 下):
//! - `run/daemon.json`  daemon 元信息 (pid/port/token), 权限收紧 (unix 0600 / Windows 仅当前用户)
//! - `run/<name>.json`  策略实例台账 (pid/启动时间/模式/上次退出码与原因)
//! - `logs/<name>.log`  策略进程 stdout/stderr 追加日志 (见 `procs`)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// daemon 元信息 (`run/daemon.json`)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub pid: u32,
    pub port: u16,
    /// 控制通道令牌 (随机生成; CLI 必须携带, 防止同机其他进程驱动下单类指令)
    pub token: String,
    pub started_at: String,
}

/// 策略实例台账 (`run/<name>.json`)。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstanceRecord {
    pub name: String,
    /// 进程 pid (停止后保留最后一次的 pid, 便于核对)
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub started_at: Option<String>,
    /// 实际运行模式 (只记录事实: 当前 run 仅 Dry Run)
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub pair: Option<String>,
    #[serde(default)]
    pub market: Option<String>,
    /// 上次退出码 (进程自行结束或经 daemon 停止后写入)
    #[serde(default)]
    pub last_exit: Option<i32>,
    #[serde(default)]
    pub last_exit_at: Option<String>,
    /// 上次退出原因 (事实描述, 如 "停机指令" / "行情流中断 (WebSocket 断开)")
    #[serde(default)]
    pub last_reason: Option<String>,
}

pub fn run_dir(root: &Path) -> PathBuf {
    root.join("run")
}

pub fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}

pub fn ensure_dirs(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(run_dir(root))?;
    std::fs::create_dir_all(logs_dir(root))
}

pub fn daemon_info_path(root: &Path) -> PathBuf {
    run_dir(root).join("daemon.json")
}

pub fn instance_path(root: &Path, name: &str) -> PathBuf {
    run_dir(root).join(format!("{name}.json"))
}

pub fn log_path(root: &Path, name: &str) -> PathBuf {
    logs_dir(root).join(format!("{name}.log"))
}

/// 原子写 (先写临时文件再 rename, 避免读到半截内容)。
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn now_str() -> String {
    now_rfc3339()
}

/// 写 daemon 元信息; token 等同本机操作凭据, 因此收紧文件权限 (unix 0600 / Windows 仅当前用户)。
pub fn write_daemon_info(root: &Path, info: &DaemonInfo) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(info).unwrap_or_default();
    let path = daemon_info_path(root);
    write_atomic(&path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        // Windows 上 `write_atomic` 只能继承父目录 ACL → 同机其它账户可能读到 token。
        // 复用配置文件那条收紧路径 (icacls 断继承 + 只授当前用户)。
        // **尽力而为**: 失败不阻断 daemon 启动, 但如实警告, 不静默假装已保护。
        if let Err(msg) = crate::commands::config_file::harden_secret_file(&path) {
            tracing::warn!(
                target: "supervisor",
                path = %path.display(),
                "未能收紧 daemon.json 的访问权限: {msg}; 该文件含控制通道 token, 请确认其所在目录非共享目录"
            );
        }
    }
    Ok(())
}

pub fn read_daemon_info(root: &Path) -> Option<DaemonInfo> {
    let path = daemon_info_path(root);
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<DaemonInfo>(&text) {
        Ok(info) => Some(info),
        Err(e) => {
            tracing::warn!(target: "supervisor", path = %path.display(), "daemon.json 解析失败, 视为无 daemon: {e}");
            None
        }
    }
}

pub fn remove_daemon_info(root: &Path) {
    let _ = std::fs::remove_file(daemon_info_path(root));
}

pub fn read_instance(root: &Path, name: &str) -> Option<InstanceRecord> {
    let path = instance_path(root, name);
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<InstanceRecord>(&text) {
        Ok(mut rec) => {
            if rec.name.is_empty() {
                rec.name = name.to_string();
            }
            Some(rec)
        }
        Err(e) => {
            tracing::warn!(target: "supervisor", path = %path.display(), "实例台账解析失败, 忽略: {e}");
            None
        }
    }
}

pub fn write_instance(root: &Path, rec: &InstanceRecord) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(rec).unwrap_or_default();
    write_atomic(&instance_path(root, &rec.name), &json)
}

/// 列出全部实例台账 (按文件名排序; 跳过 daemon.json 与坏文件)。
pub fn list_instances(root: &Path) -> Vec<InstanceRecord> {
    let dir = run_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            path.file_stem().and_then(|s| s.to_str()).map(str::to_string)
        })
        .filter(|n| n != "daemon")
        .collect();
    names.sort();
    names.into_iter().filter_map(|n| read_instance(root, &n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-ledger-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create tmp root");
        d
    }

    #[test]
    fn daemon_info_roundtrip_and_permissions() {
        let root = tmp_root("daemon");
        ensure_dirs(&root).expect("ensure dirs");
        let info =
            DaemonInfo { pid: 1234, port: 45678, token: "tok-abc".into(), started_at: now_str() };
        write_daemon_info(&root, &info).expect("write");
        assert_eq!(read_daemon_info(&root), Some(info));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(daemon_info_path(&root)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "daemon.json 必须仅属主可读写");
        }
        remove_daemon_info(&root);
        assert_eq!(read_daemon_info(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn instance_roundtrip_and_bad_file_ignored() {
        let root = tmp_root("instance");
        ensure_dirs(&root).expect("ensure dirs");
        let rec = InstanceRecord {
            name: "demo".into(),
            pid: Some(42),
            started_at: Some(now_str()),
            mode: Some("dry_run".into()),
            pair: Some("ETHUSDT".into()),
            market: Some("spot".into()),
            last_exit: Some(1),
            last_exit_at: Some(now_str()),
            last_reason: Some("行情流中断 (WebSocket 断开)".into()),
        };
        write_instance(&root, &rec).expect("write");
        assert_eq!(read_instance(&root, "demo"), Some(rec.clone()));

        // 坏文件: 忽略而非 panic
        std::fs::write(instance_path(&root, "broken"), "{ not json").unwrap();
        assert_eq!(read_instance(&root, "broken"), None);

        let listed = list_instances(&root);
        assert_eq!(listed.len(), 1, "坏文件与 daemon.json 不应出现在台账列表");
        assert_eq!(listed[0].name, "demo");

        // daemon.json 不参与实例列表
        write_daemon_info(
            &root,
            &DaemonInfo { pid: 1, port: 2, token: "t".into(), started_at: now_str() },
        )
        .unwrap();
        assert_eq!(list_instances(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
