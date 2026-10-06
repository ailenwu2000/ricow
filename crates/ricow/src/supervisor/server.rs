//! daemon 主体: 实例调度 + 控制通道服务 + 子进程监控 (008)。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// 启动中占位标记 (防并发双实例): `start` 从锁内检查"是否在跑"到锁外完成
    /// 进程拉起之间有 1~3s 窗口 (TOML 校验 / pair 视野网络调用 / spawn_blocking /
    /// 600ms 存活探测), 只查 `children` 时第二个同名 Start 会穿过检查, 造成
    /// **同一策略两个子进程同时交易** (双份下单、client_order_id 前缀相同导致
    /// 成交事件互相污染)。占位标记在锁内完成 check+insert, 窗口期内的重复请求一律拒绝。
    starting: HashSet<String>,
    /// 崩溃自动重启策略 (038 P1-C, 来自 `ricow.toml` 的 `[supervisor]` 段)。
    /// 默认 [`RestartPolicy::None`] = 行为与没有这一项时完全一致。
    restart: RestartPolicy,
    /// 各实例**已自动重启过几次** (键 = 策略名)。成功跑满
    /// [`RESTART_HEALTHY_SECS`] 后清零, 免得长跑之后的偶发崩溃被历史计数卡死。
    retries: HashMap<String, u32>,
    /// 最多自动重启几次 (默认 3)。
    restart_max: u32,
    /// 退避基数秒 (默认 5; 第 N 次重启前等 `base × N` 秒)。
    restart_backoff_secs: u64,
}

/// 崩溃自动重启策略 (038 P1-C)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    /// 不自动重启 (默认 —— 与加这项之前的行为一字不差)。
    #[default]
    None,
    /// 异常退出 (退出码非 0 / 被信号杀死) 后自动重启, 最多 `max_retries` 次。
    OnFailure,
}

/// 跑满这么久就认为"这次运行是健康的", 自动重启计数**清零**。
///
/// 固定常量, 不新增配置面: 没有它的话, "崩 → 重启 → 跑三天 → 又崩" 会因为三次重试已用完
/// 而拒绝重启 —— 那不是用户想要的语义。60s 取自"短于一次的行情重连周期"。
const RESTART_HEALTHY_SECS: u64 = 60;

/// 判断这次子进程退出**该不该**自动重启 (纯函数, 可单测)。
///
/// - `code`: 退出码; `None` = 被信号杀死 (按异常处理)。
/// - `retries_done`: 已经自动重启过几次。
pub fn restart_decision(
    policy: RestartPolicy,
    code: Option<i32>,
    retries_done: u32,
    max_retries: u32,
) -> bool {
    match policy {
        RestartPolicy::None => false,
        RestartPolicy::OnFailure => code != Some(0) && retries_done < max_retries,
    }
}

/// 到第 `attempt` 次重启前应等多久 (线性退避)。
pub fn restart_backoff(attempt: u32, base_secs: u64) -> Duration {
    Duration::from_secs(base_secs.saturating_mul(attempt.max(1) as u64))
}

/// 控制通道服务 (可克隆; 连接处理与监控共用同一状态)。
#[derive(Clone)]
pub struct Server {
    state: Arc<Mutex<State>>,
    token: String,
    shutdown_tx: watch::Sender<bool>,
}

/// 从数据根的 `ricow.toml` 读 `[supervisor]` 段, 折成 `(策略, 最大重试, 退避基数秒)`
/// (038 P1-C)。
///
/// **读不到 / 解析失败 → `(None, 3, 5)` 并 warn**: daemon 不该因为一份写坏的配置文件起不来
/// (其它入口会在真正需要时给出明确的配置错误); 静默降级到"不重启"是安全方向 —— 少做一次动作
/// 总好过在配置没确认的情况下自动拉起实盘进程。
fn load_restart_policy(root: &std::path::Path) -> (RestartPolicy, u32, u64) {
    let (policy, max, backoff) = match crate::commands::config_file::load(root) {
        Ok(f) => (
            match f.supervisor.restart_policy.as_deref() {
                Some("on-failure") => RestartPolicy::OnFailure,
                _ => RestartPolicy::None,
            },
            f.supervisor.max_retries.unwrap_or(3).clamp(0, 100) as u32,
            f.supervisor.backoff_secs.unwrap_or(5).clamp(0, 3600) as u64,
        ),
        Err(e) => {
            tracing::warn!(target: "supervisor", "读取 ricow.toml 失败, 自动重启按默认(关闭)处理: {e}");
            (RestartPolicy::None, 3, 5)
        }
    };
    if policy == RestartPolicy::OnFailure {
        tracing::info!(
            target: "supervisor",
            max_retries = max, backoff_secs = backoff,
            "已启用崩溃自动重启 (on-failure): 主动停机不会触发, 重启前会走启动挂单接管"
        );
    }
    (policy, max, backoff)
}

/// 取状态锁, **容忍锁中毒**。
///
/// 持锁线程 panic 后 `Mutex` 会进入 poisoned 状态, 默认做法 `expect(...)` 会让之后每一次取锁
/// 都继续 panic —— 一次局部异常会放大成 "list/stop/shutdown 全部不可用, 只能重启 daemon"。
/// 这里的内部状态是 `root`/`exe`/`children` 三样简单数据, 不存在"被写坏到不能用"的不变量,
/// 因此中毒后继续沿用内部值, 把影响限制在真正出问题的那一次请求上。
fn lock_state(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `starting` 占位标记的 RAII 清理。
///
/// `start` 在锁内登记占位后即释放锁去跑网络/进程操作, 期间有**多条早期失败返回路径**
/// (TOML 不存在 / 校验失败 / 缺实盘确认 / spawn 失败 / 秒退)。用 Drop guard 保证
/// 任何一条路径返回都会摘除标记, 不留"该策略永远启动中"的死锁占位。
/// 成功路径的 drop 同样摘除 —— 届时 `children` 已登记, 占位的历史使命完成。
struct StartingGuard {
    state: Arc<Mutex<State>>,
    name: String,
}

impl Drop for StartingGuard {
    fn drop(&mut self) {
        lock_state(&self.state).starting.remove(&self.name);
    }
}

/// 常量时间比较 (控制通道 token)。
///
/// 逐字节短路比较 (字符串 `==`) 会按第一个不同的字节提前返回, 在回环网络上仍可能被
/// 反复试探出前缀; 这里对全长度做定长累加, 不提前退出。长度不等直接判否 (长度本身不敏感)。
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl Server {
    /// 取状态锁 (同 [lock_state]: 锁中毒后沿用内部数据, 不把 panic 扩散到后续请求)。
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        lock_state(&self.state)
    }

    pub fn new(
        root: PathBuf,
        exe: PathBuf,
        token: String,
        shutdown_tx: watch::Sender<bool>,
    ) -> Self {
        // 038 P1-C: 监督策略在 daemon 启动时读一次。读失败或没写 → `none`(保持既有行为),
        // 只 warn —— 配置文件的问题不该让 daemon 起不来(那是 `ricow` 其它入口的职责)。
        let (restart, restart_max, restart_backoff_secs) = load_restart_policy(&root);
        let state = Arc::new(Mutex::new(State {
            root,
            exe,
            children: HashMap::new(),
            starting: HashSet::new(),
            restart,
            retries: HashMap::new(),
            restart_max,
            restart_backoff_secs,
        }));
        Self { state, token, shutdown_tx }
    }

    /// 处理一条请求 (纯逻辑入口, 单测与集成测共用)。
    pub async fn handle(&self, env: Envelope) -> Response {
        if !ct_eq(&env.token, &self.token) {
            tracing::warn!(target: "supervisor", "拒绝请求: 令牌不匹配");
            return Response::err("令牌不匹配 (控制通道拒绝)");
        }
        match env.request {
            Request::Ping => {
                Response::ok(Some(serde_json::json!({ "pong": true, "pid": std::process::id() })))
            }
            Request::List => {
                Response::ok(Some(serde_json::json!({ "instances": self.list_views() })))
            }
            Request::Info { name } => match self.view_of(&name) {
                Some(v) => Response::ok(Some(serde_json::json!(v))),
                None => Response::err(format!("未找到实例 {name} (未在运行且无台账)")),
            },
            Request::Start { name, live, demo, confirmed } => {
                self.start(&name, live, demo, confirmed).await
            }
            Request::Stop { name, close_all } => self.stop(&name, close_all).await,
            Request::Shutdown => {
                let _ = self.shutdown_tx.send(true);
                Response::ok(Some(serde_json::json!({ "shutting_down": true })))
            }
        }
    }

    /// 运行中实例 + 台账并集 (CLI 再与已部署清单合并)。
    fn list_views(&self) -> Vec<InstanceView> {
        let state = self.state();
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
    ///
    /// 并发防重 (审计 H-1): "是否已在跑"的检查与 `children.insert` 登记之间隔着
    /// 整个启动流程 (含网络调用), 锁内只做检查会留下 1~3s 的双实例窗口。
    /// 故锁内同步完成 **检查 + 占位登记**, 后续任何路径返回时由 [`StartingGuard`]
    /// 摘除占位; 占位期间的重复 Start 一律拒绝。
    async fn start(&self, name: &str, live: bool, demo: bool, confirmed: bool) -> Response {
        let (root, exe) = {
            let mut state = self.state();
            if state.children.contains_key(name) {
                return Response::err(format!("策略 {name} 已在运行, 不重复拉起"));
            }
            if state.starting.contains(name) {
                return Response::err(format!(
                    "策略 {name} 正在启动中, 请等待本次启动结束再试 (防重复拉起)"
                ));
            }
            state.starting.insert(name.to_string());
            (state.root.clone(), state.exe.clone())
        };
        // 作用域存续到函数结束: 任何 return 路径都会摘除占位标记。
        let _starting = StartingGuard { state: Arc::clone(&self.state), name: name.to_string() };

        // 策略 TOML 校验 (enabled=false / 解析失败 / 缺脚本在此拒绝), 并取运行元数据
        // 目录与台账/子进程同源(daemon 的 root), 不用进程全局 `strategies_dir()` ——
        // 否则 `RICOW_STATE_ROOT` 之类把 data root 指到别处时, 会拿另一个目录的 TOML 去启动。
        // #013: 不存在的策略先给出期望路径与下一步, 不冒"读取策略 xxx 失败"的 IO 原文。
        let dir = root.join("strategies");
        let toml_path = dir.join(format!("{name}.toml"));
        if !toml_path.exists() {
            return Response::err(format!(
                "策略 {name} 不存在: 期望 {} (无此文件)。下一步: `ricow list` 查看已部署策略; 用 `ricow create`(或让 AI 写)新建。",
                toml_path.display()
            ));
        }
        let config = match crate::commands::load_strategy_toml(&dir, name) {
            Ok(c) => c,
            Err(e) => return Response::err(format!("{e}")),
        };
        let pair = config.get_str("pair").unwrap_or("").to_string();
        let market = config.market.clone();
        // #025: 交易路径(dry_run/demo/实盘)同样受交易对视野约束 —— 与回测/查询同源判定,
        // 避免"列表里看不见却下得出去"。快照拉取失败时 warn 放行, 与其它入口同一降级语义。
        if !pair.is_empty() {
            if let Err(e) =
                crate::commands::pairs::ensure_pair_in_scope(&root, &pair, &market).await
            {
                return Response::err(format!("{e}"));
            }
        }
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

        // #028: spawn 后**短等并确认子进程存活**再回报"已启动" —— 缺凭据/坏配置的子进程
        // 会秒退(stdout/stderr 已重定向到日志, 终端看不见), 不做这一步就会出现
        // "报成功但进程已死"的假成功, 在实盘语境下是危险信号。
        // 失败即退出本身是安全行为, 这里只负责让它**前台可见地失败**。
        let probe = tokio::task::spawn_blocking(move || {
            let mut h = handle;
            std::thread::sleep(std::time::Duration::from_millis(600));
            let early_exit = h.child.try_wait().ok().flatten();
            (h, early_exit)
        })
        .await;
        let (handle, early_exit) = match probe {
            Ok(v) => v,
            Err(e) => return Response::err(format!("启动存活校验失败: {e}")),
        };
        if let Some(status) = early_exit {
            return Response::err(format!(
                "策略 {name} 进程启动后立即退出 (exit={}): 多为凭据缺失或配置错误。\
                 详情见 logs/{name}.log; demo/实盘需先在 ricow.toml 配好对应凭据。",
                status.code().map(|c| c.to_string()).unwrap_or_else(|| "非零(信号)".into())
            ));
        }

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
        self.state().children.insert(name.to_string(), handle);
        tracing::info!(target: "supervisor", name = %name, pid = ?view.pid, "策略已启动");
        Response::ok(Some(serde_json::json!(view)))
    }

    /// 停止策略: 名字判定 → 下发停机指令 → 等待退出 → 台账记录; 超时如实报告且不静默强杀。
    async fn stop(&self, name: &str, close_all: bool) -> Response {
        let handle = { self.state().children.remove(name) };
        let Some(mut handle) = handle else {
            // 句柄缺失分两种: 名字根本不存在, 或存在但本来就没在跑 (027)。
            // 停机是风险动作: 名字不存在时一律如实失败 —— 绝不回"已停止"这种可能被误读为
            // "风险已解除"的假成功回执。判定与文案取自唯一来源 `commands::instances`。
            let root = self.state().root.clone();
            if !crate::commands::instances::strategy_name_exists(&root, name) {
                return Response::err(format!(
                    "{}; 可用 ricow list 查看现有实例",
                    crate::commands::instances::unknown_name_message(name)
                ));
            }
            // 存在但没在跑: 只陈述"未在运行 (无需停止)"这一个事实 (未下发指令 / 未等待 / 未写台账 / 未清理)
            let report = StopReport {
                name: name.to_string(),
                exited: true,
                graceful: true,
                exit_code: None,
                waited_ms: 0,
                note: Some("该策略未在运行 (无需停止)".into()),
                already_stopped: true,
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

        let root = self.state().root.clone();
        // 清理提示: 依据运行模式与脚本是否定义 on_stop 如实说明 (不臆测清理结果)
        let cleanup_hint = cleanup_hint_for(&root, name, handle.view.mode.as_deref(), close_all);
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
            // 超时: 放回状态表, 避免"摘除后失联"。
            // 防孤儿 (审计 H-1 关联): 停机等待最长 30s, 期间同名实例可能已被重新拉起
            // (stop 已把旧句柄摘除, 新 Start 的占位检查拦不住它)。此时**不能**无条件
            // 覆盖 —— 覆盖会把新进程的句柄换掉, 新进程从此无人监控也永远停不掉。
            let mut state = self.state();
            if state.children.contains_key(name) {
                tracing::warn!(target: "supervisor", name = %name, pid = ?handle.pid(),
                    "停机超时且同名实例已被重新启动: 旧句柄弃置 (旧进程请按 pid 手工核对), 不覆盖新实例");
            } else {
                state.children.insert(name.to_string(), handle);
            }
        }

        let report = StopReport {
            name: name.to_string(),
            exited,
            graceful: exited,
            exit_code: code,
            waited_ms: waited.as_millis() as u64,
            note: note.clone(),
            already_stopped: false,
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
        // 监控循环拿一份 `Server`(而非只有 State): 崩溃自动重启要复用 `start()` 的
        // 全部既有校验(TOML / 视野 / 并发占位锁), 那是个 `&self` 方法。
        // 另收一份 `shutdown` 接收器: 停机过程中一律不重启(见 `handle_child_exit`)。
        let monitor = tokio::spawn(monitor_loop(self.clone(), shutdown.clone()));
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
        ledger::remove_daemon_info(&self.state().root);
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

/// 子进程监控: 发现自行退出 (崩溃/行情流中断) → 落台账, 供 `list` 如实展示退出码;
/// 并按 [`procs::LOG_ROTATE_INTERVAL`] 周期检查日志体积 (只在启动时查会让长运行策略的
/// 日志无限增长 —— 10MB 阈值形同虚设)。
async fn monitor_loop(server: Server, mut shutdown: watch::Receiver<bool>) {
    // `None` = 尚未轮转过 → 首轮即视为到期 (等价于旧写法的 `now - INTERVAL`, 但**不下溢**)。
    //
    // 旧写法 `Instant::now() - LOG_ROTATE_INTERVAL` 在系统开机不足 INTERVAL(60s)时会 panic
    // (`overflow when subtracting duration from instant`): Windows 的 `Instant` 锚在启动
    // 时刻, 干净 CI runner / 刚重启的机器都可能触发。用 `Option` 表达"还没到点"既保住
    // "启动即检查一次"的语义, 又与时钟起点无关。
    let mut last_rotate: Option<Instant> = None;
    loop {
        tokio::time::sleep(Duration::from_millis(1000)).await;

        let mut exited: Vec<(String, Option<i32>, InstanceView, PathBuf)> = Vec::new();
        let mut rotate_targets: Vec<PathBuf> = Vec::new();
        {
            let mut guard = lock_state(&server.state);
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
            // 只给**还在运行**的策略轮转日志 (已退出的进程不再写, 没有增长压力)。
            if rotate_due(last_rotate) {
                for name in guard.children.keys() {
                    rotate_targets.push(ledger::log_path(&root, name));
                }
            }
        }

        for path in rotate_targets {
            // 轮转要读写文件, 放到阻塞线程池: 别让每 60s 的一次 I/O 卡住整个监控循环。
            let _ = tokio::task::spawn_blocking(move || procs::rotate_large_log(&path)).await;
        }

        for (name, code, view, root) in exited {
            handle_child_exit(&server, &mut shutdown, &name, code, view, root).await;
        }

        if rotate_due(last_rotate) {
            last_rotate = Some(Instant::now());
        }
    }
}

/// 一个子进程自行退出后的处置: 落台账 + (按策略)自动重启 (038 P1-C)。
///
/// **主动停机不会走到这里**: `stop_all` 会先把 `children` 整体 `drain` 走, 监控循环根本看不到
/// 那些实例 —— 这是"stop 不会被自动重启顶回来"的结构性保证(有单测锁定)。
async fn handle_child_exit(
    server: &Server,
    shutdown: &mut watch::Receiver<bool>,
    name: &str,
    code: Option<i32>,
    view: InstanceView,
    root: PathBuf,
) {
    // 停机过程中一律不重启: 此刻 `stop_all` 正在(或已经)收摊, 再拉起一个子进程会**逃出**
    // 停机清单, 变成 daemon 退出后仍在跑的孤儿(虽然它靠 stdin EOF 会自愈退出, 但窗口内
    // 它仍可能在交易)。
    let shutting_down = *shutdown.borrow();
    // 跑满 RESTART_HEALTHY_SECS 的实例视为"健康过" → 清零历史重试计数, 免得
    // "崩→重启→长跑三天→再崩"被前面那几次计数卡死(那显然不是用户想要的语义)。
    let ran_secs = uptime_secs(&view);
    let (policy, max, backoff_base, retries_done) = {
        let mut guard = lock_state(&server.state);
        if ran_secs >= RESTART_HEALTHY_SECS && guard.retries.contains_key(name) {
            guard.retries.remove(name);
        }
        let done = guard.retries.get(name).copied().unwrap_or(0);
        (guard.restart, guard.restart_max, guard.restart_backoff_secs, done)
    };
    let will_restart = !shutting_down && restart_decision(policy, code, retries_done, max);

    let reason = if will_restart {
        format!("进程自行退出 (exit={:?}), 即将自动重启 #{}", code, retries_done + 1)
    } else if shutting_down {
        "进程自行退出 (daemon 正在停机, 不重启)".to_string()
    } else if policy == RestartPolicy::OnFailure && code != Some(0) && retries_done >= max {
        format!("进程自行退出 (exit={code:?}); 已达最大重试次数 {max}, 停止自动重启, 需人工介入")
    } else {
        "进程自行退出".to_string()
    };
    write_exit_record(&root, name, &view, code, &reason);

    if !will_restart {
        if policy == RestartPolicy::OnFailure && code != Some(0) && retries_done >= max {
            tracing::error!(
                target: "supervisor", name = %name,
                "策略 {name} 连续异常退出已达上限 {max} 次, 不再自动重启 —— 请查看日志排查后手工启动"
            );
        }
        return;
    }

    if retries_done > 0 {
        let wait = restart_backoff(retries_done + 1, backoff_base);
        tracing::warn!(target: "supervisor", name = %name, secs = wait.as_secs(), "自动重启退避中");
        tokio::time::sleep(wait).await;
    }

    // 重启复用 `start()` —— 它带**全部**既有校验(TOML / 交易对视野 / 并发占位锁),
    // 比自己拼一遍 spawn 安全得多; 且子进程会在启动段执行 037 P0-A 的挂单接管,
    // 所以重启不会让敞口翻倍。
    let mode = view.mode.clone().unwrap_or_default();
    let (live, demo) = (mode == "live", mode == "demo");
    tracing::warn!(
        target: "supervisor", name = %name, mode = %mode,
        "策略 {name} 非正常退出, 自动重启中 (第 {} 次; 重启会先撤销本实例遗留挂单)",
        retries_done + 1
    );
    // 实盘确认在**首次**启动时已由用户在交互终端逐字完成; 自动重启沿用该次确认(配置里
    // 显式开了 on-failure 就是用户对"崩了帮我拉起来"的授权)。子进程侧仍走原有的
    // `RICOW_DAEMON_SPAWNED` 标记路径, 不绕过任何门禁 —— 只是不再重复索要 stdin 确认。
    let resp = server.start(name, live, demo, true).await;
    let mut guard = lock_state(&server.state);
    if resp.ok {
        *guard.retries.entry(name.to_string()).or_insert(0) += 1;
    } else {
        // 启动失败(TOML 没了 / 视野越界 / 秒退): 不臆造次数, 如实记日志。
        tracing::error!(
            target: "supervisor", name = %name,
            "策略 {name} 自动重启失败: {}", resp.error.unwrap_or_else(|| "未知原因".into())
        );
    }
}

/// 统计某实例本次已连续运行多久 (秒); 取不到视图 → 0。
///
/// 只用于"跑够久就重置重试计数"这一个判断 —— 拿不到就不重置(保守: 不重置只会更早停手)。
fn uptime_secs(view: &InstanceView) -> u64 {
    view.uptime_secs.unwrap_or(0)
}

/// 是否到达下一次日志轮转时刻: 从未轮转过(首轮) 或已满 [`procs::LOG_ROTATE_INTERVAL`]。
fn rotate_due(last: Option<Instant>) -> bool {
    match last {
        None => true,
        Some(t) => t.elapsed() >= procs::LOG_ROTATE_INTERVAL,
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
        let mut guard = lock_state(state);
        guard.children.drain().collect()
    };
    let root = lock_state(state).root.clone();
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
/// - 实盘 / 测试网(demo): 引擎按订单号前缀撤单兜底, `close_all` 时平仓; 真实结果以日志与
///   交易所状态为准 —— demo 走测试网、与真实资金无关, 措辞必须与实盘分开 (否则跑测试网的人
///   会以为动了真钱);
/// - Dry Run: 订单是虚拟撮合, 交易所侧无本策略挂单, 只有脚本自身 `on_stop` 的逻辑。
///
/// `root` = daemon 自己的数据根: 脚本要从**同一份**策略目录读, 否则会去判断另一份 TOML
/// 有没有 `on_stop`(提示与实际运行的策略不符)。
fn cleanup_hint_for(
    root: &std::path::Path,
    name: &str,
    mode: Option<&str>,
    close_all: bool,
) -> String {
    let action = if close_all {
        "撤单兜底 + 平掉策略持仓"
    } else {
        "撤单兜底 (未带 --close-all, 持仓保留)"
    };
    match mode {
        Some("live") => {
            return format!(
                "实盘停机(**真实资金**): 引擎执行{action}; 详细结果见 logs/{name}.log 的停机清理段落, 请以交易所账户实际状态为准"
            )
        }
        Some("demo") => {
            return format!(
                "测试网停机(模拟盘, 与真实资金无关): 引擎执行{action}; 详细结果见 logs/{name}.log 的停机清理段落, 请以测试网账户实际状态为准"
            )
        }
        _ => {}
    }
    let dir = root.join("strategies");
    match crate::commands::load_strategy_toml(&dir, name) {
        Ok(config) => match ricow_engine::load_strategy(&config) {
            Ok(strategy) if strategy.has_on_stop() => format!(
                "该策略实现了清理 (on_stop); 执行结果以 logs/{name}.log 为准 (引擎未代策略臆测清理结果)"
            ),
            Ok(_) => "该策略未实现清理 (on_stop); 如仍有挂单或持仓需手工处理 (请以交易所实际状态为准)".to_string(),
            Err(e) => format!("无法判定清理实现 (策略装载失败: {e}); 请手工核对挂单与持仓"),
        },
        Err(e) => format!("无法判定清理实现 ({e}); 请手工核对挂单与持仓"),
    }
}

/// 计算运行时长 (基于 started_at)。
pub fn with_uptime(view: &InstanceView) -> InstanceView {
    let mut v = view.clone();
    v.uptime_secs =
        v.started_at.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(
            |t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0) as u64,
        );
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

        // 不存在的名字: stop 必须如实失败 (027) —— 旧实现在这里回 "已停止" 的假成功
        for close_all in [false, true] {
            let err = client
                .call_ok(Request::Stop { name: "nope".into(), close_all })
                .await
                .expect_err("不存在的名字必须失败 (--close-all 不改变名字判定)");
            let msg = err.to_string();
            assert!(msg.contains("未找到策略或实例 nope"), "实际错误: {msg}");
            assert!(msg.contains("ricow list"), "报错要给可执行的下一步: {msg}");
            for forbidden in ["已停止", "未在运行", "无需停止"] {
                assert!(!msg.contains(forbidden), "不存在分支不得出现 {forbidden}: {msg}");
            }
        }

        // 名字存在但从未启动 (仅有 strategies/<n>.toml): 成功且只陈述"未在运行 (无需停止)" (027)
        std::fs::create_dir_all(root.join("strategies")).unwrap();
        std::fs::write(root.join("strategies").join("only-toml.toml"), "name = \"only-toml\"\n")
            .unwrap();
        let data = client
            .call_ok(Request::Stop { name: "only-toml".into(), close_all: false })
            .await
            .expect("名字存在时必须成功");
        assert_eq!(data["exited"], true);
        assert_eq!(data["already_stopped"], true);
        assert_eq!(data["note"], "该策略未在运行 (无需停止)");

        // 停机: serve 应返回且清理 daemon.json
        client.call_ok(Request::Shutdown).await.expect("shutdown");
        let served = tokio::time::timeout(Duration::from_secs(10), handle).await;
        assert!(served.is_ok(), "serve 应在收到 shutdown 后退出");
        assert_eq!(ledger::read_daemon_info(&root), None, "退出后应清理 daemon.json");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 停机清理提示必须按**真实模式**措辞: 测试网(demo)不得印"实盘停机" (跑测试网的人会以为
    /// 动了真钱), 实盘必须点明真实资金; 未定义 `on_stop` 的 Dry Run 走"该策略未实现清理"分支。
    #[test]
    fn cleanup_hint_wording_matches_mode() {
        let root = tmp_root("cleanup-hint");
        let demo = cleanup_hint_for(&root, "s1", Some("demo"), false);
        assert!(demo.contains("测试网停机"), "{demo}");
        assert!(demo.contains("与真实资金无关"), "{demo}");
        assert!(!demo.contains("实盘"), "demo 不得出现实盘措辞: {demo}");

        let live = cleanup_hint_for(&root, "s1", Some("live"), false);
        assert!(live.contains("实盘停机"), "{live}");
        assert!(live.contains("真实资金"), "{live}");

        let live_close = cleanup_hint_for(&root, "s1", Some("live"), true);
        assert!(live_close.contains("平掉策略持仓"), "{live_close}");
        let demo_close = cleanup_hint_for(&root, "s1", Some("demo"), true);
        assert!(demo_close.contains("平掉策略持仓"), "{demo_close}");

        // 无策略 TOML → 走 Dry Run 分支 (如实说明无法判定, 不臆测)
        let dry = cleanup_hint_for(&root, "s1", Some("dry_run"), false);
        assert!(dry.contains("无法判定清理实现"), "{dry}");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn start_envelope(name: &str) -> Envelope {
        Envelope {
            token: "t".into(),
            request: Request::Start {
                name: name.into(),
                live: false,
                demo: false,
                confirmed: false,
            },
        }
    }

    fn minimal_strategy_toml(root: &std::path::Path, name: &str) {
        std::fs::create_dir_all(root.join("strategies")).unwrap();
        std::fs::write(
            root.join("strategies").join(format!("{name}.toml")),
            // 合法可装载的最小 TOML ([strategy] 段 + lua script), pair 留空跳过视野网络检查
            format!(
                "[strategy]\nname = \"{name}\"\ntype = \"lua\"\nenabled = true\nexchange = \"binance\"\n\n[strategy.params]\nscript = \"function on_tick() end\"\n"
            ),
        )
        .unwrap();
    }

    /// 审计 H-1 回归: 启动中占位标记必须拒绝并发的同名 Start。
    ///
    /// `start` 的"是否在跑"检查与 `children.insert` 登记之间隔着整个启动流程 (含网络
    /// 调用与进程拉起), 只查 children 会留下 1~3s 双实例窗口 —— 同一策略两个子进程
    /// 同时交易。占位标记在锁内原子完成 check+insert, 窗口内的重复 Start 一律拒绝。
    #[tokio::test]
    async fn concurrent_start_is_rejected_by_starting_marker() {
        let root = tmp_root("starting-marker");
        ledger::ensure_dirs(&root).unwrap();
        let (tx, _rx) = watch::channel(false);
        let server =
            Server::new(root.clone(), PathBuf::from("definitely-not-an-exe"), "t".into(), tx);
        minimal_strategy_toml(&root, "s1");

        // 占位期间 (模拟另一请求正在启动流程中): handle(Start) 必须被拒, 且消息如实说明。
        lock_state(&server.state).starting.insert("s1".into());
        let resp = server.handle(start_envelope("s1")).await;
        let body = serde_json::to_string(&resp).unwrap();
        assert!(body.contains("正在启动中"), "占位期间的重复 Start 应被拒: {body}");

        // 手工占位没有 guard 跟随, 测试自行摘除后再测真实启动路径。
        lock_state(&server.state).starting.remove("s1");

        // 失败路径不留死标记: 直接走完整 start (spawn 必败, exe 不存在), 返回后占位必须被清空。
        let resp = server.handle(start_envelope("s1")).await;
        let body = serde_json::to_string(&resp).unwrap();
        assert!(body.contains("启动失败"), "exe 不存在应启动失败: {body}");
        assert!(lock_state(&server.state).starting.is_empty(), "失败后不得残留占位标记");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 审计 H-1 回归: 两个并发 Start **绝不能都成功** (双实例 = 双份下单)。
    ///
    /// 占位 check+insert 在同一锁段内完成, 故第二个请求必然看到标记;
    /// 无论调度顺序如何, 结果只能是"一个成功一个被拒"或"都失败 (spawn 必败)"。
    #[tokio::test]
    async fn concurrent_starts_never_both_succeed() {
        let root = tmp_root("concurrent-start");
        ledger::ensure_dirs(&root).unwrap();
        let (tx, _rx) = watch::channel(false);
        let server =
            Server::new(root.clone(), PathBuf::from("definitely-not-an-exe"), "t".into(), tx);
        minimal_strategy_toml(&root, "s1");

        let (r1, r2) =
            tokio::join!(server.handle(start_envelope("s1")), server.handle(start_envelope("s1")));
        let ok = |r: &Response| serde_json::to_string(r).unwrap().contains("\"ok\":true");
        assert!(!(ok(&r1) && ok(&r2)), "并发 Start 不得双双成功 (双实例同时交易): {r1:?} / {r2:?}");
        let state = lock_state(&server.state);
        assert!(state.starting.is_empty(), "结束后不得残留占位标记");
        assert!(state.children.len() <= 1, "不得登记两个子进程句柄");
        drop(state);

        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 038 P1-C: 崩溃自动重启的判定(纯逻辑) ----

    /// **零行为变更的核心断言**: 默认策略下, 任何退出码都不重启。
    #[test]
    fn restart_decision_default_policy_never_restarts() {
        for code in [None, Some(0), Some(1), Some(101), Some(137)] {
            assert!(!restart_decision(RestartPolicy::default(), code, 0, 100), "{code:?}");
            assert!(!restart_decision(RestartPolicy::None, code, 0, 100), "{code:?}");
        }
    }

    #[test]
    fn restart_decision_on_failure_restarts_only_unexpected_exits() {
        assert!(restart_decision(RestartPolicy::OnFailure, Some(1), 0, 3), "非零退出应重启");
        assert!(
            restart_decision(RestartPolicy::OnFailure, None, 0, 3),
            "被信号杀死(无退出码)同样算异常"
        );
        assert!(
            !restart_decision(RestartPolicy::OnFailure, Some(0), 0, 3),
            "退出码 0 = 正常结束, 不该被顶回来"
        );
    }

    #[test]
    fn restart_decision_respects_max_retries() {
        assert!(restart_decision(RestartPolicy::OnFailure, Some(1), 2, 3), "未到上限应重启");
        assert!(!restart_decision(RestartPolicy::OnFailure, Some(1), 3, 3), "到上限即停手");
        assert!(!restart_decision(RestartPolicy::OnFailure, Some(1), 9, 3), "超限也不重启");
        // max_retries = 0 是"显式关掉"的一种写法。
        assert!(!restart_decision(RestartPolicy::OnFailure, Some(1), 0, 0));
    }

    #[test]
    fn restart_backoff_is_linear_and_never_zero() {
        assert_eq!(restart_backoff(1, 5), Duration::from_secs(5));
        assert_eq!(restart_backoff(3, 5), Duration::from_secs(15));
        // attempt 0 也至少等一个基数: 退化成"立即重启"就是忙循环。
        assert_eq!(restart_backoff(0, 5), Duration::from_secs(5));
        // 基数 0(用户显式关退避) 不 panic。
        assert_eq!(restart_backoff(5, 0), Duration::from_secs(0));
        // 极大 attempt 走 saturating, 不 panic。
        assert!(restart_backoff(u32::MAX, 5).as_secs() > 0);
    }

    /// 配置 → 策略 的整链: 写了 `on-failure` 才启用; 缺省/写坏一律回到"不重启"。
    #[test]
    fn restart_policy_is_read_from_config_file() {
        let root = tmp_root("restart-cfg");
        std::fs::create_dir_all(&root).unwrap();
        let cfg = root.join("ricow.toml");

        // ① 缺省(文件不存在 → 模板): none + 默认上限/退避。
        let (p, max, back) = load_restart_policy(&root);
        assert_eq!(p, RestartPolicy::None, "默认必须是不重启");
        assert_eq!((max, back), (3, 5));

        // ② 显式 on-failure + 自定义上限与退避。
        std::fs::write(
            &cfg,
            "schema_version = 1\n[supervisor]\nrestart_policy = \"on-failure\"\nmax_retries = 7\nbackoff_secs = 12\n",
        )
        .unwrap();
        let (p, max, back) = load_restart_policy(&root);
        assert_eq!(p, RestartPolicy::OnFailure);
        assert_eq!((max, back), (7, 12));

        // ③ 拼错的策略名: `load` 直接拒绝 → 降级为不重启(安全方向: 少做动作)。
        std::fs::write(&cfg, "[supervisor]\nrestart_policy = \"onfail\"\n").unwrap();
        let (p, max, back) = load_restart_policy(&root);
        assert_eq!(p, RestartPolicy::None, "非法值不得被当成启用");
        assert_eq!((max, back), (3, 5));

        // ④ 类型写错(max_retries 给字符串): 同样硬拒并降级, 不静默丢值。
        std::fs::write(
            &cfg,
            "[supervisor]\nrestart_policy = \"on-failure\"\nmax_retries = \"三\"\n",
        )
        .unwrap();
        let (p, _, _) = load_restart_policy(&root);
        assert_eq!(p, RestartPolicy::None, "类型错的配置不得被当成启用");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 起一个**立刻正常退出**的真子进程当替身 —— 只走 `std::process`, 不涉交易、不碰网络。
    ///
    /// 两个平台各给一份: 早先这里只有 `#[cfg(unix)]` 分支, 于是 Windows 上 `children` 恒为空,
    /// 下面那条 `graceful == 1` 的断言在 windows-latest 上**必然为 0** —— 测试成了平台相关,
    /// 而 CI 是三平台矩阵。要测的是"停机把 children 收干净"这一条结构性事实, 与平台无关。
    ///
    /// stdin 给 `null` 而非 `piped`: 本测试要验的是 `stop_all` 的 drain + `wait_exit`, 不该顺带
    /// 依赖"往子进程 stdin 写停机指令"那条链路(那条由 `procs` 的「子进程 stdin 停机链路」测试组
    /// 覆盖, 三平台都跑),
    /// 也免得这条断言多背一个环境依赖。
    ///
    /// **实测备注 (2026-10-06, 探针 `tmp/pipe_probe2.rs` / `tmp/pipe_probe.rs`)**: 在本机
    /// **agent 进程树内**, 给子进程 `stdin(Stdio::piped())` 会让 `spawn` 直接失败并返回
    /// `ERROR_PIPE_BUSY(231)`("所有的管道范例都在使用中")。触发面**仅是 Rust std 为子进程
    /// stdin 走的那条 `NtCreateNamedPipeFile` 路径**
    /// (`library/std/src/sys/process/windows/child_pipe.rs` 已明确弃用 `CreatePipe`);
    /// 同源却**不受影响**的有: `stdout` / `stderr` piped、`stdin = null | File`、
    /// 以及 Win32 `CreatePipe` 造的匿名管道(`std::io::pipe()` 实测正常)。
    /// 故"匿名管道被拦"是**不准确**的描述; 与 ricow 代码无关, 属环境侧注入 DLL 的行为
    /// (Rust 上游 issue #143078 记录的正是这类 hook)。用户普通终端无此现象。
    fn quick_exit_child() -> std::process::Child {
        use std::process::{Command, Stdio};
        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg("exit 0");
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.arg("/C").arg("exit 0");
            c
        };
        #[cfg(not(any(unix, windows)))]
        let mut cmd = Command::new("true");
        // 不用 `.ok()`: 起不来时必须把真实 errno 打出来, 否则又一次只能靠猜。
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("起替身进程失败: {e} (raw_os_error={:?})", e.raw_os_error()))
    }

    /// 主动停机**结构性**不会被自动重启顶回来: `stop_all` 先把 `children` 整体 drain 走,
    /// 监控循环里根本不会出现这些实例 —— 它们不会走到 `handle_child_exit`。
    #[tokio::test]
    async fn intentional_stop_removes_children_so_no_restart_can_fire() {
        let root = tmp_root("restart-stop");
        let (_tx, rx) = watch::channel(false);
        let server = Server::new(root.clone(), PathBuf::from("/bin/true"), "t".into(), _tx);
        {
            let mut st = lock_state(&server.state);
            st.restart = RestartPolicy::OnFailure;
            st.restart_max = 9;
        }
        // 模拟"实例在跑": 用一个立刻结束的真子进程当替身。
        let mut c = quick_exit_child();
        let stdin = c.stdin.take();
        let view = InstanceView {
            name: "s1".into(),
            running: true,
            pid: Some(c.id()),
            started_at: Some(ledger::now_str()),
            uptime_secs: Some(0),
            mode: Some("dry_run".into()),
            pair: Some("ETHUSDT".into()),
            market: Some("spot".into()),
            last_exit: None,
            last_exit_at: None,
            last_reason: None,
        };
        lock_state(&server.state)
            .children
            .insert("s1".into(), ChildHandle { child: c, stdin, view });

        // 主动停机: drain 后监控循环再也看不到它 → 不可能触发重启。
        let (graceful, _) = stop_all(&server.state);
        assert_eq!(graceful, 1, "应正常收摊这一个实例");
        assert!(lock_state(&server.state).children.is_empty(), "停完不得残留句柄");
        // 重试计数也不该因此增长(没有任何一次"自动重启")。
        assert!(lock_state(&server.state).retries.is_empty(), "主动停机不得计入重试次数");
        drop(rx);
        let _ = std::fs::remove_dir_all(&root);
    }
}
