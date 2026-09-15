//! `ricow daemon` — 策略进程管理器 (008): start(自后台化) / stop / status / run(前台)。
//!
//! 生命周期 (决策 D1/P2): daemon 常驻并持有策略子进程; `daemon stop` 先优雅停全部策略再退出;
//! daemon 被强杀时, 策略进程靠 stdin 管道 EOF 自愈退出 (不依赖 OS 带走子进程)。

use std::process::{Command, Stdio};
use std::time::Duration;

use clap::{Args, Subcommand};
use ricow_core::{CoreError, CoreResult};
use tokio::net::TcpListener;

use crate::supervisor::client::Client;
use crate::supervisor::proto::{InstanceView, Request};
use crate::supervisor::{ledger, server::Server};

#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: DaemonAction,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// 后台启动 daemon (自后台化; 已在运行则报错)
    Start,
    /// 停止 daemon (先优雅停全部策略, 超时如实报告)
    Stop,
    /// 查看 daemon 状态
    Status,
    /// 前台运行 daemon (start 内部调用; 也可手工前台调试)
    Run,
}

pub async fn run(args: DaemonArgs) -> CoreResult<()> {
    match args.action {
        DaemonAction::Start => start().await,
        DaemonAction::Stop => stop().await,
        DaemonAction::Status => status().await,
        DaemonAction::Run => run_foreground().await,
    }
}

fn io_err(context: &str, e: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidArgument(format!("{context}: {e}"))
}

/// 若存在可达的 daemon, 返回其元信息 (连不上则清理陈旧文件并返回 None)。
async fn live_daemon(root: &std::path::Path) -> Option<ledger::DaemonInfo> {
    let info = ledger::read_daemon_info(root)?;
    match Client::connect(root).await {
        Ok(_) => Some(info),
        Err(_) => {
            println!("提示: 清理陈旧 run/daemon.json (pid={} 未在监听)", info.pid);
            ledger::remove_daemon_info(root);
            None
        }
    }
}

async fn start() -> CoreResult<()> {
    let root = crate::commands::project_root();
    ledger::ensure_dirs(&root).map_err(|e| io_err("创建 run/ logs/ 目录失败", e))?;

    if let Some(info) = live_daemon(&root).await {
        return Err(CoreError::InvalidArgument(format!(
            "daemon 已在运行 (pid={}, 端口={}); 如需重启请先 ricow daemon stop",
            info.pid, info.port
        )));
    }

    let exe = std::env::current_exe().map_err(|e| io_err("定位 ricow 可执行文件失败", e))?;
    let log = ledger::logs_dir(&root).join("daemon.log");
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| io_err("打开 daemon 日志失败", e))?;
    let err = out.try_clone().map_err(|e| io_err("复制日志句柄失败", e))?;

    let mut cmd = Command::new(&exe);
    cmd.arg("daemon").arg("run").stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    detach(&mut cmd);
    let child = cmd.spawn().map_err(|e| io_err("启动 daemon 进程失败", e))?;
    let child_pid = child.id();

    // 等待 daemon 写入 daemon.json (就绪信号)
    for _ in 0..50 {
        if let Some(info) = ledger::read_daemon_info(&root) {
            println!(
                "daemon 已启动: pid={} (子进程 pid={}) 端口={} 日志={}",
                info.pid,
                child_pid,
                info.port,
                log.display()
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(CoreError::InvalidArgument(format!(
        "daemon 启动超时 (5s 未就绪); 请查看日志 {}",
        log.display()
    )))
}

async fn stop() -> CoreResult<()> {
    let root = crate::commands::project_root();
    let client = Client::connect(&root).await?;
    let pid = client.pid();
    client.call_ok(Request::Shutdown).await?;

    // 等待退出: 每个策略停机等待上限 30s, 故给足余量
    for _ in 0..900 {
        if ledger::read_daemon_info(&root).is_none() {
            println!("daemon (pid={pid}) 已停止");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(CoreError::InvalidArgument(
        "daemon 停止超时 (90s); 请检查 logs/daemon.log 与策略进程状态".into(),
    ))
}

async fn status() -> CoreResult<()> {
    let root = crate::commands::project_root();
    let Some(info) = ledger::read_daemon_info(&root) else {
        println!("daemon: 未运行");
        return Ok(());
    };
    match Client::connect(&root).await {
        Ok(client) => {
            let views: Vec<InstanceView> = match client.call_ok(Request::List).await {
                Ok(data) => serde_json::from_value(
                    data.get("instances").cloned().unwrap_or(serde_json::json!([])),
                )
                .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let running = views.iter().filter(|v| v.running).count();
            println!(
                "daemon: 运行中 (pid={} 端口={} 启动于 {})",
                info.pid,
                client.port(),
                info.started_at
            );
            println!("托管实例: {} 个运行中 / 共 {} 个台账实例", running, views.len());
            for v in views.iter().filter(|v| v.running) {
                println!("  - {} (pid={:?})", v.name, v.pid);
            }
            Ok(())
        }
        Err(e) => {
            println!("daemon: run/daemon.json 存在 (pid={}) 但无法连接 —— {e}", info.pid);
            Ok(())
        }
    }
}

/// 前台 daemon: 绑定随机端口 → 写 daemon.json → 服务循环 (Ctrl-C 亦走优雅停机)。
async fn run_foreground() -> CoreResult<()> {
    let root = crate::commands::project_root();
    ledger::ensure_dirs(&root).map_err(|e| io_err("创建 run/ logs/ 目录失败", e))?;

    if let Some(info) = live_daemon(&root).await {
        return Err(CoreError::InvalidArgument(format!(
            "daemon 已在运行 (pid={}, 端口={})",
            info.pid, info.port
        )));
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| io_err("绑定 127.0.0.1 失败", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| io_err("读取监听地址失败", e))?
        .port();
    let token = uuid::Uuid::new_v4().to_string();
    let info = ledger::DaemonInfo {
        pid: std::process::id(),
        port,
        token: token.clone(),
        started_at: ledger::now_str(),
    };
    ledger::write_daemon_info(&root, &info).map_err(|e| io_err("写 run/daemon.json 失败", e))?;
    println!("daemon 前台运行中: pid={} 端口={} (控制通道仅本机 + token 校验)", info.pid, port);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let tx = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = tx.send(true);
        }
    });

    let exe = std::env::current_exe().map_err(|e| io_err("定位 ricow 可执行文件失败", e))?;
    let server = Server::new(root, exe, token, shutdown_tx);
    server.serve(listener, shutdown_rx).await
}

/// daemon 自身只脱离一次终端 (策略子进程与 daemon 同生命周期, 不脱离)。
#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}
