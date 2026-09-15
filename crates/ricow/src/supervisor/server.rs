//! daemon 主体: 实例调度 + 控制通道服务 + 子进程监控 (008)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ricow_core::CoreResult;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::supervisor::procs::{self, ChildHandle};
use crate::supervisor::proto::{Envelope, InstanceView, Request, Response, StopReport};
use crate::supervisor::{ledger, proto};

/// daemon 内部状态。
struct State {
    root: PathBuf,
    exe: PathBuf,
    children: HashMap<String, ChildHandle>,
}

/// 控制通道服务 (可克隆; 连接处理与监控共用同一状态)。
#[derive(Clone)]
pub struct Server {
    state: Arc<Mutex<State>>,
    token: String,
    shutdown_tx: watch::Sender<bool>,
}

impl Server {
    pub fn new(root: PathBuf, exe: PathBuf, token: String, shutdown_tx: watch::Sender<bool>) -> Self {
        let state = Arc::new(Mutex::new(State { root, exe, children: HashMap::new() }));
        Self { state, token, shutdown_tx }
    }

    /// 处理一条请求 (纯逻辑入口, 单测与集成测共用)。
    pub async fn handle(&self, env: Envelope) -> Response {
        if env.token != self.token {
            tracing::warn!(target: "supervisor", "拒绝请求: 令牌不匹配");
            return Response::err("令牌不匹配 (控制通道拒绝)");
        }
        match env.request {
            Request::Ping => {
                Response::ok(Some(serde_json::json!({ "pong": true, "pid": std::process::id() })))
            }
            Request::List => Response::ok(Some(serde_json::json!({ "instances": self.list_views() }))),
            Request::Info { name } => match self.view_of(&name) {
                Some(v) => Response::ok(Some(serde_json::json!(v))),
                None => Response::err(format!("未找到实例 {name} (未在运行且无台账)")),
            },
            Request::Start { name, live, demo, confirmed } => self.start(&name, live, demo, confirmed).await,
            Request::Stop { name, close_all } => self.stop(&name, close_all).await,
            Request::Shutdown => {
                let _ = self.shutdown_tx.send(true);
                Response::ok(Some(serde_json::json!({ "shutting_down": true })))
            }
        }
    }

    /// 运行中实例 + 台账并集 (CLI 再与已部署清单合并)。
    fn list_views(&self) -> Vec<InstanceView> {
        let state = self.state.lock().expect("state lock");
        let mut views: Vec<InstanceView> = Vec::new();
        for rec in ledger::list_instances(&state.root) {
            match state.children.get(&rec.name) {
                Some(h) => views.push(with_uptime(&h.view)),
                None => views.push(InstanceView {
                    name: rec.name.clone(),
                    running: false,
                    pid: rec.pid,
                    started_at: rec.started_at,
                    uptime_secs: None,
                    mode: rec.mode,
                    pair: rec.pair,
                    market: rec.market,
                    last_exit: rec.last_exit,
                    last_exit_at: rec.last_exit_at,
                    last_reason: rec.last_reason,
                }),
            }
        }
        // 运行中但台账缺失的实例 (台账写入失败等极端情况) 也不能漏报
        for (name, h) in state.children.iter() {
            if !views.iter().any(|v| &v.name == name) {
                views.push(with_uptime(&h.view));
            }
        }
        views.sort_by(|a, b| a.name.cmp(&b.name));
        views
    }

    fn view_of(&self, name: &str) -> Option<InstanceView> {
        self.list_views().into_iter().find(|v| v.name == name)
    }

    /// 启动策略: 台账校验 → spawn → 台账落盘。
    async fn start(&self, name: &str, live: bool, demo: bool, confirmed: bool) -> Response {
        let (root, exe) = {
            let state = self.state.lock().expect("state lock");
            if state.children.contains_key(name) {
                return Response::err(format!("策略 {name} 已在运行, 不重复拉起"));
            }
            (state.root.clone(), state.exe.clone())
        };

        // 策略 TOML 校验 (enabled=false / 解析失败 / 缺脚本在此拒绝), 并取运行元数据
        let dir = crate::commands::strategies_dir();
        let config = match crate::commands::load_strategy_toml(&dir, name) {
            Ok(c) => c,
            Err(e) => return Response::err(format!("{e}")),
        };
        let pair = config.get_str("pair").unwrap_or("").to_string();
        let market = config.market.clone();
        // 如实记录实际运行器 (011): 仅当命令行要求实盘 **且** 配置声明实盘时才写 live;
        // 子进程侧仍会走同一门禁复核 (缺一即回落 Dry Run), 台账与子进程行为不会不一致
        // 实盘二次分离(019 T030): 未携带用户确认的实盘请求一律拒绝(**不静默降级** —— 用户已明确要实盘)
        if live && !confirmed {
            return Response::err(format!(
                "缺少实盘确认: 请在交互终端执行 `ricow start {name} --live` 并逐字输入确认短语(确认不会经 daemon 传递 stdin)。"
            ));
        }
        // demo(测试网)只看命令行开关: 它不涉真实资金, 不需要 TOML 声明 live_enabled(那是主网保护)。
        let live = live && config.live_enabled;
        let mode = if demo {
            "demo"
        } else if live {
            "live"
        } else {
            "dry_run"
        };

        let root2 = root.clone();
        let exe2 = exe.clone();
        let name2 = name.to_string();
        let pair2 = pair.clone();
        let market2 = market.clone();
        let spawned = tokio::task::spawn_blocking(move || {
            procs::spawn_strategy(&root2, &exe2, &name2, mode, &pair2, &market2, live, demo)
        })
        .await;

        let handle = match spawned {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => return Response::err(format!("启动失败: {e}")),
            Err(e) => return Response::err(format!("启动任务失败: {e}")),
        };

        let rec = ledger::InstanceRecord {
            name: name.to_string(),
            pid: Some(handle.pid()),
            started_at: handle.view.started_at.clone(),
            mode: Some(mode.to_string()),
            pair: Some(pair.clone()),
            market: Some(market.clone()),
            last_exit: None,
            last_exit_at: None,
            last_reason: None,
        };
        if let Err(e) = ledger::write_instance(&root, &rec) {
            tracing::warn!(target: "supervisor", name = %name, "台账写入失败: {e}");
        }

        let view = with_uptime(&handle.view);
        self.state.lock().expect("state lock").children.insert(name.to_string(), handle);
        tracing::info!(target: "supervisor", name = %name, pid = ?view.pid, "策略已启动");
        Response::ok(Some(serde_json::json!(view)))
    }

    /// 停止策略: 下发停机指令 → 等待退出 → 台账记录; 超时如实报告且不静默强杀。
    async fn stop(&self, name: &str, close_all: bool) -> Response {
        let handle = { self.state.lock().expect("state lock").children.remove(name) };
        let Some(mut handle) = handle else {
            let report = StopReport {
                name: name.to_string(),
                exited: true,
                graceful: true,
                exit_code: None,
                waited_ms: 0,
                note: Some("该策略未在运行 (无需停止)".into()),
            };
            return Response::ok(Some(serde_json::json!(report)));
        };

        let name_in_task = name.to_string();
        let joined = tokio::task::spawn_blocking(move || {
            let sent = procs::request_stop(&mut handle.stdin, close_all);
            if let Err(e) = sent {
                tracing::warn!(target: "supervisor", name = %name_in_task, "停机指令下发失败: {e}");
            }
            let (exited, code, waited) = procs::wait_exit(&mut handle.child, procs::STOP_WAIT);
            (handle, exited, code, waited)
        })
        .await;
        let (handle, exited, code, waited) = match joined {
            Ok(v) => v,
            Err(e) => return Response::err(format!("停机任务失败: {e}")),
        };

        let root = self.state.lock().expect("state lock").root.clone();
        // 清理提示: 依据运行模式与脚本是否定义 on_stop 如实说明 (不臆测清理结果)
        let cleanup_hint = cleanup_hint_for(name, matches!(handle.view.mode.as_deref(), Some("live") | Some("demo")), close_all);
        let note = if !exited {
            Some(format!(
                "停机超时 ({}s) 未观测到退出; 未强制终止, 请手工核对 (pid {:?})",
                procs::STOP_WAIT.as_secs(),
                handle.pid()
            ))
        } else {
            Some(cleanup_hint)
        };

        if exited {
            write_exit_record(&root, name, &handle.view, code, "停机指令");
        } else {
            // 超时: 放回状态表, 避免"摘除后失联"
            self.state.lock().expect("state lock").children.insert(name.to_string(), handle);
        }

        let report = StopReport {
            name: name.to_string(),
            exited,
            graceful: exited,
            exit_code: code,
            waited_ms: waited.as_millis() as u64,
            note: note.clone(),
        };
        if let Some(n) = note {
            tracing::warn!(target: "supervisor", name = %name, "{n}");
        } else {
            tracing::info!(target: "supervisor", name = %name, code = ?code, "策略已停止");
        }
        Response::ok(Some(serde_json::json!(report)))
    }

    /// 服务循环: 接受连接 + 响应停机信号; 退出前优雅停全部策略。
    pub async fn serve(
        self,
        listener: TcpListener,
        mut shutdown: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        let monitor = tokio::spawn(monitor_loop(self.state.clone()));
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((socket, _peer)) => {
                            let server = self.clone();
                            tokio::spawn(async move { serve_conn(socket, server).await });
                        }
                        Err(e) => {
                            tracing::warn!(target: "supervisor", "accept 失败: {e}");
                        }
                    }
                }
            }
        }

        monitor.abort();
        let state = self.state.clone();
        let stopped = tokio::task::spawn_blocking(move || stop_all(&state)).await;
        if let Ok((graceful, timed_out)) = stopped {
            tracing::info!(target: "supervisor", graceful, timed_out, "daemon 停机完成");
            if timed_out > 0 {
                tracing::warn!(
                    target: "supervisor",
                    "有 {timed_out} 个策略停机超时未退出 (未强制终止), 请手工核对"
                );
            }
        }
        ledger::remove_daemon_info(&self.state.lock().expect("state lock").root);
        Ok(())
    }
}

/// 连接处理: 一行一请求, 一行一响应。
async fn serve_conn(socket: tokio::net::TcpStream, server: Server) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = socket.into_split();
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(target: "supervisor", "读请求失败: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match proto::decode_envelope(&line) {
            Ok(env) => server.handle(env).await,
            Err(e) => Response::err(e),
        };
        if writer.write_all(proto::encode(&resp).as_bytes()).await.is_err() {
            break;
        }
        if writer.flush().await.is_err() {
            break;
        }
    }
}

/// 子进程监控: 发现自行退出 (崩溃/行情流中断) → 落台账, 供 `list` 如实展示退出码。
async fn monitor_loop(state: Arc<Mutex<State>>) {
    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let mut exited: Vec<(String, Option<i32>, InstanceView, PathBuf)> = Vec::new();
        {
            let mut guard = state.lock().expect("state lock");
            let root = guard.root.clone();
            let names: Vec<String> = guard.children.keys().cloned().collect();
            for name in names {
                if let Some(h) = guard.children.get_mut(&name) {
                    if let Ok(Some(status)) = h.child.try_wait() {
                        exited.push((name, status.code(), h.view.clone(), root.clone()));
                    }
                }
            }
            for (name, _, _, _) in exited.iter() {
                guard.children.remove(name);
            }
        }
        for (name, code, view, root) in exited {
            write_exit_record(&root, &name, &view, code, "进程自行退出");
        }
    }
}

/// 写"进程已退出"台账 (退出码 + 时间 + 原因)。
fn write_exit_record(
    root: &std::path::Path,
    name: &str,
    view: &InstanceView,
    code: Option<i32>,
    reason: &str,
) {
    let rec = ledger::InstanceRecord {
        name: name.to_string(),
        pid: view.pid,
        started_at: view.started_at.clone(),
        mode: view.mode.clone(),
        pair: view.pair.clone(),
        market: view.market.clone(),
        last_exit: code,
        last_exit_at: Some(ledger::now_str()),
        last_reason: Some(reason.to_string()),
    };
    if let Err(e) = ledger::write_instance(root, &rec) {
        tracing::warn!(target: "supervisor", name = %name, "退出台账写入失败: {e}");
    }
}

/// 停机: 逐个下发停机指令并等待 (不强制终止, 超时如实计数)。
fn stop_all(state: &Arc<Mutex<State>>) -> (usize, usize) {
    let handles: Vec<(String, ChildHandle)> = {
        let mut guard = state.lock().expect("state lock");
        guard.children.drain().collect()
    };
    let root = state.lock().expect("state lock").root.clone();
    let (mut graceful, mut timed_out) = (0usize, 0usize);
    for (name, mut h) in handles {
        let _ = procs::request_stop(&mut h.stdin, false);
        let (exited, code, _) = procs::wait_exit(&mut h.child, procs::STOP_WAIT);
        if exited {
            write_exit_record(&root, &name, &h.view, code, "daemon 退出");
            graceful += 1;
        } else {
            timed_out += 1;
        }
    }
    (graceful, timed_out)
}

/// 停机清理提示 (如实说明, 不臆测清理结果)。
///
/// - 实盘: 引擎按订单号前缀撤单兜底, `close_all` 时平仓; 真实结果以日志与交易所状态为准;
/// - Dry Run: 订单是虚拟撮合, 交易所侧无本策略挂单, 只有脚本自身 `on_stop` 的逻辑。
fn cleanup_hint_for(name: &str, live: bool, close_all: bool) -> String {
    if live {
        let action = if close_all {
            "撤单兜底 + 平掉策略持仓"
        } else {
            "撤单兜底 (未带 --close-all, 持仓保留)"
        };
        return format!(
            "实盘停机: 引擎执行{action}; 详细结果见 logs/{name}.log 的停机清理段落, 请以交易所账户实际状态为准"
        );
    }
    let dir = crate::commands::strategies_dir();
    match crate::commands::load_strategy_toml(&dir, name) {
        Ok(config) => match ricow_engine::load_strategy(&config) {
            Ok(strategy) if strategy.has_on_stop() => format!(
                "该策略实现了清理 (on_stop); 执行结果以 logs/{name}.log 为准 (引擎未代策略臆测清理结果)"
            ),
            Ok(_) => format!(
                "该策略未实现清理 (on_stop); 如仍有挂单或持仓需手工处理 (请以交易所实际状态为准)"
            ),
            Err(e) => format!("无法判定清理实现 (策略装载失败: {e}); 请手工核对挂单与持仓"),
        },
        Err(e) => format!("无法判定清理实现 ({e}); 请手工核对挂单与持仓"),
    }
}

/// 计算运行时长 (基于 started_at)。
pub fn with_uptime(view: &InstanceView) -> InstanceView {
    let mut v = view.clone();
    v.uptime_secs = v
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0) as u64);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::client::Client;
    use std::path::PathBuf;

    fn tmp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ricow-server-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create tmp root");
        d
    }

    /// 真实 TCP 回环集成测试 (非 mock): 提供服务 → 客户端带/不带 token 请求 → 校验响应与拒绝 → 停机。
    #[tokio::test]
    async fn serve_and_client_over_real_tcp() {
        let root = tmp_root("serve");
        ledger::ensure_dirs(&root).unwrap();
        let token = "test-token-1234".to_string();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        ledger::write_daemon_info(
            &root,
            &ledger::DaemonInfo {
                pid: std::process::id(),
                port,
                token: token.clone(),
                started_at: ledger::now_str(),
            },
        )
        .unwrap();

        let (tx, rx) = watch::channel(false);
        let server = Server::new(root.clone(), PathBuf::from("/bin/true"), token.clone(), tx);
        let handle = tokio::spawn(server.serve(listener, rx));

        // 客户端: ping / list 正常
        let client = Client::connect(&root).await.expect("connect");
        assert_eq!(client.port(), port);
        let pong = client.call_ok(Request::Ping).await.expect("ping");
        assert_eq!(pong["pong"], true);

        let data = client.call_ok(Request::List).await.expect("list");
        assert!(data["instances"].as_array().expect("instances 数组").is_empty());

        // 令牌错误: 必须被拒绝 (不能凭端口即操作)
        let mut info = ledger::read_daemon_info(&root).expect("daemon.json");
        info.token = "wrong-token".into();
        let bad = Client { info };
        let err = bad.call_ok(Request::Ping).await.expect_err("错 token 应被拒");
        assert!(err.to_string().contains("令牌不匹配"), "实际错误: {err}");

        // 未运行的策略: stop 幂等返回 ok
        let data = client
            .call_ok(Request::Stop { name: "nope".into(), close_all: false })
            .await
            .expect("stop 幂等");
        assert_eq!(data["exited"], true);

        // 停机: serve 应返回且清理 daemon.json
        client.call_ok(Request::Shutdown).await.expect("shutdown");
        let served = tokio::time::timeout(Duration::from_secs(10), handle).await;
        assert!(served.is_ok(), "serve 应在收到 shutdown 后退出");
        assert_eq!(ledger::read_daemon_info(&root), None, "退出后应清理 daemon.json");

        let _ = std::fs::remove_dir_all(&root);
    }
}
