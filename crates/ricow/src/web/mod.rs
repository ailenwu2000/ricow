//! Web UI 服务端 (025): axum + SSE, 只绑 `127.0.0.1`, 一次性 token 鉴权。
//!
//! 与 CLI 同源: 会话逻辑全在 [`crate::ai::session`], 本模块只提供 HTTP 骨架、
//! 会话线程登记与帧转发 —— 线程里跑的仍是 [`crate::commands::chat::repl`],
//! 助手增量仍由 `provider::ask_stream` 逐段产出, 本层不复制任何 LLM 调用路径 (D2 / D5)。

mod backtest_jobs;
mod keyring;
mod keys;
mod markets;
mod runs;
mod sink;
mod store;
mod strategy_io;
mod tail;
mod terms;

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{Database, FillWithMode, OrderRecord, PnlSnapshotRecord, PositionRecord};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::ai::session::SessionSink;
use crate::commands::config_file::{self, SetValue};
use crate::i18n::{t, Lang};
use crate::supervisor::{self, Source};

// 这几个类型出现在本模块的公开签名里(`Starter` / `Hub::attach` / `WebState::new`),
// 装配处(`commands/web.rs`)必须能命名它们 → 用 `pub use` 再导出(仅可见性, 零逻辑改动)。
pub use sink::{Frame, Line, WebSink};
pub use store::SessionStore;

use sink::{FrameSender, InputChannel};

/// 只绑回环地址: 服务不对外网暴露(宪法「完全本地化 / 私钥不出本机」; D3 / R4)。
const BIND_ADDR: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// 帧广播缓冲: 够覆盖一小段突发输出。追不上的连接会收到 `Lagged`, 跳过即可 ——
/// 帧只是实时视图, 补全靠消息接口 (FR-021)。
const FRAME_CAPACITY: usize = 256;

/// 前端静态资产(D4): 全部编译期嵌进二进制, 运行期不依赖工作目录、不依赖外部 CDN。
/// 032 起脚本拆为多文件: 第三方图表库 + 公共件 + 路由 + 对话视图 + 入口(加载顺序见 index.html)。
const INDEX_HTML: &str = include_str!("assets/index.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
/// lightweight-charts v4 UMD(032 T001): 第三方图表库, 文件头保留 Apache-2.0 许可注释。
const LWC_JS: &str = include_str!("assets/lightweight-charts.js");
/// 公共件: TOKEN / api / TEXT 字典 / 通用 i18n 与纯工具(032 T002)。
const COMMON_JS: &str = include_str!("assets/common.js");
/// hash 路由与视图注册表(032 T003)。
const ROUTER_JS: &str = include_str!("assets/router.js");
/// 对话视图: 原 app.js 的会话 / SSE / 三个只读面板逻辑整体迁入(032 T006)。
const CHAT_JS: &str = include_str!("assets/chat.js");
/// 设置视图(032 US1): 市场视野开关; 密钥配置已迁至 033 密钥视图。
const SETTINGS_JS: &str = include_str!("assets/settings.js");
/// 密钥视图(033): 多套 AI / 币安密钥的别名条目 —— 列表 / 新增 / 选用 / 删除, 读写 `/api/keys*`。
const KEYS_JS: &str = include_str!("assets/keys.js");
/// 市场视图(032 US2): 交易对列表 / 订单簿 / K 线, 读写 `/api/markets*`。
const MARKETS_JS: &str = include_str!("assets/markets.js");
/// 策略管理视图(032 US3): 策略列表 / 源码编辑保存 / AI 改 Lua / 回测作业。
const STRATEGIES_JS: &str = include_str!("assets/strategies.js");
/// 运行管理视图(032 US4): 实例一览 / 启停 / 风险确认 / 行内日志。
const RUNS_JS: &str = include_str!("assets/runs.js");
/// 入口: 初始化对话视图 + router 首跳(032 T006)。
const APP_JS: &str = include_str!("assets/app.js");

/// 页面里各静态资源 URL 的 token 占位符, 由 [`index`] 按本次请求的 token 替换。
///
/// 静态资源同样在 token 中间件之后, 而浏览器取 `<link>` / `<script>` 无法自定请求头 ——
/// 只能把 token 拼进查询串, 走的正是 [`token_of`] 的同一条取值路径。
const TOKEN_PLACEHOLDER: &str = "__RICOW_TOKEN__";

/// 会话线程体: 由装配处(`commands/web.rs`)注入 —— 本模块只做 HTTP 与线程登记,
/// 不碰数据目录 / 配置 / 模型, 也不复制会话逻辑。
///
/// 收 `&mut WebSink` 而非拥有它: 线程体可在外层再套一层 sink(如落库包装)。
pub type Starter = Arc<dyn Fn(&str, &mut WebSink) -> CoreResult<()> + Send + Sync>;

/// 会话运行时: **同时只跑一条会话**(单页面, 回复期不并发多轮); 换会话 = 停旧线程、起新线程。
///
/// 用专用 OS 线程而非 tokio 任务: [`SessionSink::input_line`] 是同步阻塞签名(019 已定
/// "允许阻塞等待"), 放线程里阻塞不占 tokio 工作线程。
pub struct Hub {
    starter: Starter,
    /// 数据目录: 会话线程**没起来**时要在对话流里报一句(FR-004), 而那句是页面看得见的宿主
    /// 文案, 得按当前界面语言渲染(FR-031)。语言现读 `ricow.toml`, 不在此另存一份状态。
    root: Arc<PathBuf>,
    active: Mutex<Option<Active>>,
}

struct Active {
    session_id: String,
    frames: FrameSender,
    input: InputChannel,
}

impl Hub {
    pub fn new(starter: Starter, root: Arc<PathBuf>) -> Self {
        Self { starter, root, active: Mutex::new(None) }
    }

    /// 确保该会话在跑, 并返回一条**新的**帧订阅。
    ///
    /// **先订阅再起线程**: 首帧(欢迎语 / 缺密钥提示)不会漏给第一个连上的页面。
    pub fn attach(&self, session_id: &str) -> CoreResult<broadcast::Receiver<Frame>> {
        let mut active = lock(&self.active);
        if let Some(current) = active.as_ref() {
            if current.session_id == session_id {
                return Ok(current.frames.subscribe());
            }
        }
        // 换会话: 丢掉旧通道 → 旧线程的 `recv()` 返回 Err → `repl` 收尾 → 旧页面收到 Closed。
        *active = None;

        let (frames, _) = broadcast::channel(FRAME_CAPACITY);
        let rx = frames.subscribe();
        let (mut web_sink, input) = WebSink::pair(frames.clone());
        let starter = Arc::clone(&self.starter);
        let root = Arc::clone(&self.root);
        let sid = session_id.to_string();
        std::thread::Builder::new()
            .name(format!("ricow-web-{sid}"))
            .spawn(move || {
                if let Err(e) = starter(&sid, &mut web_sink) {
                    // 线程体自己报不了的错(装配 / 开会话失败)在这里兜底, 并留在对话流里可见(FR-004)。
                    // 这一行是**页面看得见的宿主文案** → 按当前语言渲染(FR-031); 其后的 `e` 是既有
                    // `CoreError` 原文, 与终端同源, 不在此翻译。
                    let lang = current_lang(&root).unwrap_or(Lang::Zh);
                    let msg = t(lang, "会话未能启动", "session failed to start");
                    web_sink.error(&format!("{msg}: {e}"));
                }
                // 出作用域即 Drop → 前端收到 Closed。
            })
            .map_err(|e| {
                // 线程起不来时这条会原样进 500 响应体(浏览器可读) → 按当前语言渲染(FR-031);
                // 其后的 `e` 是 `std::io::Error` 原文, 不在此翻译。
                let lang = current_lang(&self.root).unwrap_or(Lang::Zh);
                let msg = t(lang, "启动会话线程失败", "failed to start the session thread");
                CoreError::Network(format!("{msg}: {e}"))
            })?;

        *active = Some(Active { session_id: session_id.to_string(), frames, input });
        Ok(rx)
    }

    /// 把浏览器的一行交给会话; `false` = 该会话不在跑(前端应先 [`Hub::attach`])。
    pub fn submit(&self, session_id: &str, text: String) -> bool {
        let active = lock(&self.active);
        match active.as_ref() {
            Some(current) if current.session_id == session_id => current.input.submit(text),
            _ => false,
        }
    }

    /// 往**当前活跃会话**送一条控制行(FR-030 切语言): 切语言是全局动作, 调用方不必知道哪个
    /// 会话在跑。`false` = 没有会话在跑 —— 此时只写盘, 下次 [`Hub::attach`] 开会话自然用新语言。
    pub fn submit_control(&self, text: String) -> bool {
        let active = lock(&self.active);
        active.as_ref().is_some_and(|current| current.input.submit_control(text))
    }

    /// 收摊指定会话(会话被删除时调用); 丢弃通道即让该线程退出。
    pub fn stop(&self, session_id: &str) {
        let mut active = lock(&self.active);
        if active.as_ref().is_some_and(|current| current.session_id == session_id) {
            *active = None;
        }
    }
}

/// 取锁并**容忍中毒**(同 `supervisor::server::lock_state`): 某次 panic 不该让之后每次请求都失败。
fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 服务共享状态(axum 各 handler 克隆取用)。
#[derive(Clone)]
pub struct WebState {
    token: Arc<String>,
    /// 数据目录: 语言读写与 CLI 共用同一份 `ricow.toml`(D12)。
    root: Arc<PathBuf>,
    /// 只读用的本地库句柄: 与引擎、会话存储**同一份库**(D1 / FR-010)。
    ///
    /// `Database` 内部是连接池, 克隆只是复制一个 `SqlitePool` 句柄(零成本),
    /// 交易端点据此读成交 / 挂单 / 持仓 / PnL 快照。
    db: Database,
    store: SessionStore,
    hub: Arc<Hub>,
    /// 032 US3 (FR-019): 回测异步作业表(纯内存, 进程重启即清空; 同名策略同时只一个 running)。
    jobs: Arc<backtest_jobs::JobStore>,
}

impl WebState {
    pub fn new(
        token: String,
        root: PathBuf,
        db: Database,
        store: SessionStore,
        starter: Starter,
    ) -> Self {
        let root = Arc::new(root);
        // 数据目录交两份(同一块内存的两次克隆): handler 读写语言要用, 会话线程兜底报错也要用。
        let hub = Arc::new(Hub::new(starter, Arc::clone(&root)));
        // 作业表在 new 内部默认构造: 装配方(serve 命令)与全部测试调用点无需改签名。
        Self {
            token: Arc::new(token),
            root,
            db,
            store,
            hub,
            jobs: Arc::new(backtest_jobs::JobStore::new()),
        }
    }
}

/// 一次性随机 token: 进程启动时生成, **不落盘、不进日志**, 服务退出即失效(D3 / R10)。
pub fn new_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 绑定 `127.0.0.1:{port}`; `port == 0` 时由系统分配空闲端口, 一并返回**实际**端口
/// (装配范式照 `commands/daemon.rs` 的前台 daemon)。
pub async fn bind(port: u16) -> CoreResult<(TcpListener, u16)> {
    let listener = TcpListener::bind((BIND_ADDR, port))
        .await
        .map_err(|e| CoreError::Network(format!("绑定 {BIND_ADDR} 失败: {e}")))?;
    let actual = listener
        .local_addr()
        .map_err(|e| CoreError::Network(format!("读取监听地址失败: {e}")))?
        .port();
    Ok((listener, actual))
}

/// 组装路由: **全部**端点(含静态资源)都在 token 中间件之后。
pub fn router(state: WebState) -> Router {
    Router::new()
        // 单页与静态资源(FR-003 / FR-006): 页面本身也要 token, 资源 URL 里的 token 由 `index` 填。
        .route("/", get(index))
        .route("/style.css", get(style_css))
        // 032: 脚本拆分后的五份静态资源(含第三方图表库), 同样挂在 token 中间件之后。
        .route("/lightweight-charts.js", get(lwc_js))
        .route("/common.js", get(common_js))
        .route("/router.js", get(router_js))
        .route("/chat.js", get(chat_js))
        .route("/settings.js", get(settings_js))
        .route("/keys.js", get(keys_js))
        .route("/markets.js", get(markets_js))
        .route("/strategies.js", get(strategies_js))
        .route("/runs.js", get(runs_js))
        .route("/app.js", get(app_js))
        .route("/api/ping", get(ping))
        // 会话 CRUD(FR-007): 列表 / 新建 / 删除 / 取消息。
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/sessions/{id}/messages", get(session_messages))
        // 会话通道(FR-008 / FR-011): 出站 = SSE 帧流, 入站 = 把一行送进 REPL。
        .route("/api/sessions/{id}/events", get(session_events))
        .route("/api/sessions/{id}/input", axum::routing::post(session_input))
        // 术语解释(FR-024): 静态内置数据, 只读。
        .route("/api/terms", get(list_terms))
        // 语言读写(FR-029): 与 CLI 共用 `[ui].lang`。
        .route("/api/lang", get(get_lang).post(set_lang))
        // 密钥配置(032 US1 / FR-006 ~ FR-009): 读只给脱敏尾号, 写与向导共用同一份 ricow.toml。
        .route("/api/config/keys", get(keys::get_keys).post(keys::post_keys))
        // 密钥管理(033): 多套 AI / 币安密钥的别名条目 —— 总览 / 保存 / 选用 / 删除 / 清空生效。
        // 与上面一条的分工: 这里动的是**整条密钥环**(数组表), 上面那个是"外科式改单行 + 市场视野开关"。
        .route("/api/keys", get(keyring::get_keys))
        .route("/api/keys/ai/save", axum::routing::post(keyring::save_ai))
        .route("/api/keys/ai/use", axum::routing::post(keyring::use_ai))
        .route("/api/keys/ai/delete", axum::routing::post(keyring::delete_ai))
        .route("/api/keys/exchange/save", axum::routing::post(keyring::save_exchange))
        .route("/api/keys/exchange/use", axum::routing::post(keyring::use_exchange))
        .route("/api/keys/exchange/delete", axum::routing::post(keyring::delete_exchange))
        .route("/api/keys/clear", axum::routing::post(keyring::clear))
        // 市场浏览(032 US2 / FR-010 ~ FR-014): 交易对视野 + 订单簿 + K 线, 全部免 key 只读公共行情。
        .route("/api/markets", get(markets::list_markets))
        .route("/api/markets/{symbol}/orderbook", get(markets::get_orderbook))
        .route("/api/markets/{symbol}/klines", get(markets::get_klines))
        // 策略目录(031 FR-013 / FR-014): 只读展示策略清单 + 参数 schema。
        .route("/api/strategies", get(list_strategies).post(strategy_io::save_strategy))
        .route("/api/strategies/{id}", get(get_strategy))
        // 策略源码读取与页面保存(032 US3 / FR-015 ~ FR-017): 读 Lua/实例 TOML 原文;
        // POST 保存走命名校验链 → 编译门禁 → 引擎同一落盘内核(免 preview, FR-028)。
        .route("/api/strategies/{id}/source", get(strategy_io::get_strategy_source))
        .route("/api/strategies/{id}/ai-edit", axum::routing::post(strategy_io::ai_edit_strategy))
        // 回测异步作业 (032 US3 / FR-019): POST 202 拿 job_id 后台跑 CLI 同一内核, GET 轮询结果。
        .route("/api/backtest", axum::routing::post(backtest_jobs::start_backtest))
        .route("/api/backtest/{job_id}", get(backtest_jobs::get_backtest))
        // 交易面板数据(026 FR-010 / FR-012): 全部**只读**, 数据来自与引擎同一份本地库(D1);
        // 挂在本 `.layer` 之内 → 与既有端点同一道 token 门禁(D14 / FR-011)。
        .route("/api/trades/fills", get(trades_fills))
        .route("/api/trades/orders", get(trades_orders))
        .route("/api/trades/positions", get(trades_positions))
        .route("/api/trades/pnl", get(trades_pnl))
        // 策略日志(026 FR-015 ~ FR-018): 只读文件, 策略未运行也能看历史;
        // 同样挂在本 `.layer` 之内 → 与既有端点同一道 token 门禁(D14 / T031)。
        .route("/api/logs", get(list_logs))
        .route("/api/logs/{name}/tail", get(log_tail))
        .route("/api/logs/{name}/stream", get(log_stream))
        // 运行控制(032 US4 / FR-022 ~ FR-026): 运行态一览 / 单实例状态 / 拉起 / 停机 / 风险确认。
        // 启停复用 CLI 同一 ctrl 内核, daemon 自举走幂等的 ensure_daemon —— 本层零控制逻辑复制。
        .route("/api/runs", get(runs::list_runs))
        .route("/api/strategies/{id}/status", get(runs::get_status))
        .route("/api/strategies/{id}/start", axum::routing::post(runs::start_strategy))
        .route("/api/strategies/{id}/stop", axum::routing::post(runs::stop_strategy))
        .route("/api/risk-ack", axum::routing::post(runs::post_risk_ack))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

/// 启动服务并阻塞; Ctrl-C 触发优雅停机(停机范式同 `commands/daemon.rs`)。
pub async fn serve(listener: TcpListener, state: WebState) -> CoreResult<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(true);
        }
    });
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let mut rx = shutdown_rx;
            // 收到停机信号, 或信号发送端被丢弃(进程收尾) —— 两种都该收摊。
            let _ = rx.changed().await;
        })
        .await
        .map_err(|e| CoreError::Network(format!("Web 服务异常退出: {e}")))
}

/// 单页首页(FR-003 / FR-006): 把本次 token 填进各静态资源的 URL(见 [`TOKEN_PLACEHOLDER`])。
async fn index(req: Request) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        render_index(&token_of(&req).unwrap_or_default()),
    )
}

/// 样式表: 编译期常量, 不含任何会话内容。
async fn style_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE_CSS)
}

/// 第三方图表库(032 T001): 编译期常量, 不含任何会话内容。
async fn lwc_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], LWC_JS)
}

/// 前端公共件(032 T002): 编译期常量, 不含任何会话内容。
async fn common_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], COMMON_JS)
}

/// 前端 hash 路由(032 T003): 编译期常量, 不含任何会话内容。
async fn router_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], ROUTER_JS)
}

/// 前端对话视图(032 T006): 编译期常量, 不含任何会话内容。
async fn chat_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], CHAT_JS)
}

/// 前端设置视图(032 US1): 编译期常量, 不含任何会话内容。
async fn settings_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], SETTINGS_JS)
}

/// 前端密钥视图(033): 编译期常量, 不含任何会话内容。
async fn keys_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], KEYS_JS)
}

/// 前端市场视图(032 US2): 编译期常量, 不含任何会话内容。
async fn markets_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], MARKETS_JS)
}

/// 前端策略管理视图(032 US3): 编译期常量, 不含任何会话内容。
async fn strategies_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], STRATEGIES_JS)
}

/// 前端运行管理视图(032 US4): 编译期常量, 不含任何会话内容。
async fn runs_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], RUNS_JS)
}

/// 前端入口脚本: 编译期常量, 不含任何会话内容。
async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS)
}

/// 替换页面里的 [`TOKEN_PLACEHOLDER`](单独成函数是为了能直接测)。
fn render_index(token: &str) -> String {
    INDEX_HTML.replace(TOKEN_PLACEHOLDER, token)
}

/// 就绪探针: 无会话内容, 供前端确认「服务在监听且 token 有效」。
async fn ping() -> &'static str {
    "ok"
}

/// 左侧列表一行(FR-007): 标题 + 最后活动时间。
#[derive(serde::Serialize)]
struct SessionRow {
    id: String,
    title: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(serde::Serialize)]
struct SessionId {
    id: String,
}

#[derive(serde::Serialize)]
struct Deleted {
    deleted: bool,
}

/// 回放一行(与 SSE 的 `line` 帧同形: 前端复用同一套渲染与着色)。
#[derive(serde::Serialize)]
struct MessageRow {
    id: i64,
    role: String,
    content: String,
    sev: String,
    created_at: i64,
}

/// 会话列表, 按最后活动倒序。
async fn list_sessions(State(state): State<WebState>) -> Result<Json<Vec<SessionRow>>, WebError> {
    let rows = state.store.list().await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SessionRow {
                id: r.session_id,
                title: r.title,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect(),
    ))
}

/// 新建空会话(标题留给首条用户消息生成, FR-022); 线程等页面用上它时再起。
async fn create_session(State(state): State<WebState>) -> Result<Json<SessionId>, WebError> {
    Ok(Json(SessionId { id: state.store.create().await? }))
}

/// 删除会话: 消息随外键级联删除(FR-019); 若删的正是正在跑的会话, 同时收摊其线程。
async fn delete_session(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Deleted>, WebError> {
    let deleted = state.store.delete(&id).await?;
    state.hub.stop(&id);
    Ok(Json(Deleted { deleted }))
}

/// 某会话全部消息(写入顺序): 切换 / 重开会话时整屏回放(FR-021)。
async fn session_messages(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<Vec<MessageRow>>, WebError> {
    let rows = state.store.messages(&id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MessageRow {
                id: r.message_id,
                role: r.role,
                content: r.content,
                sev: r.severity,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// 出站: 会话帧流(SSE)。帧由会话线程经 sink 产出 —— 助手增量是 `delta`(FR-008),
/// 宿主整行是 `line` + 级别(FR-013), 前端按 `type` 分派、按 `sev` 着色。
async fn session_events(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, WebError> {
    let rx = state.hub.attach(&id)?;
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(frame) => return Some((Ok(Event::default().data(frame.encode())), rx)),
                // 追不上就跳过这些帧: 实时视图宁缺勿错, 补全靠 `messages`。
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                // 会话线程收摊(sink 被丢弃 = 最后一帧 Closed 已发出)。
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(serde::Deserialize)]
struct InputBody {
    text: String,
}

#[derive(serde::Serialize)]
struct Accepted {
    accepted: bool,
}

/// 入站: 把浏览器的一行送进 REPL 循环(FR-011)。
///
/// 页面可能在 SSE 连上之前就送出了第一行, 故这里按需拉起会话线程; 会话正等密钥时,
/// 分流由服务端决定(见 [`InputChannel::submit`]), 前端无需自报是密钥还是普通输入。
async fn session_input(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<InputBody>,
) -> Result<Json<Accepted>, WebError> {
    // 一次入站**只投一行**: 会话在跑就直接送; 不在跑(页面比 SSE 先到)才拉起线程, 拉起后补送这行。
    // 这里必须让 `submit` 的结果决定后续 —— 页面开局就连了 SSE(`/events` → `attach`), 热路径上
    // `submit` 必然成功; 若再无条件补一次, 每行输入就会进通道两遍, 一次输入顶两轮、两次 LLM 调用
    // (2026-09-19 实机 DB 流水: 12 条用户输入无一例外成对, 含同秒重复的裸「确认」)。
    let accepted = if state.hub.submit(&id, body.text.clone()) {
        true
    } else {
        state.hub.attach(&id)?;
        state.hub.submit(&id, body.text)
    };
    Ok(Json(Accepted { accepted }))
}

/// 术语解释(FR-024 / FR-026): 编译期常量, 中英两份一次给全 —— 前端切换语言无需重取,
/// 服务端也不必为它记语言状态。
async fn list_terms() -> Json<Vec<&'static terms::Term>> {
    Json(terms::all())
}

// ---- 策略目录 (031 FR-013 / FR-014): 只读展示策略清单与参数 schema ----

/// 策略列表行(前端策略面板用)。
#[derive(serde::Serialize)]
struct StrategyRow {
    id: String,
    name: String,
    market: String,
    summary: String,
    source: String,
    param_count: usize,
}

/// 策略列表(只读): 内置示例 + 用户自写, 按注册顺序。
async fn list_strategies() -> Json<Vec<StrategyRow>> {
    let rows = crate::strategies::catalog::all()
        .into_iter()
        .map(|e| StrategyRow {
            id: e.manifest.id,
            name: e.manifest.name,
            market: e.manifest.market,
            summary: e.manifest.summary,
            source: match e.source {
                crate::strategies::catalog::Source::Builtin => "builtin".to_string(),
                crate::strategies::catalog::Source::User => "user".to_string(),
            },
            param_count: e.manifest.params.len(),
        })
        .collect();
    Json(rows)
}

/// 策略详情响应(032 T027 扩展): 清单字段整体扁平化(对话视图仍直接读 name/params 等),
/// 另带用户实例的当前交易对 `pair` 与当前参数值 `current`(供控制台表单回填;
/// 内置策略 / 无实例 TOML 时两者为 null, 前端回退清单默认值)。
#[derive(serde::Serialize)]
struct StrategyDetailReply {
    #[serde(flatten)]
    manifest: crate::strategies::catalog::StrategyManifest,
    /// 已部署实例的当前交易对(无实例 → null)。
    pair: Option<String>,
    /// 已部署实例的当前参数值(已剔除 pair/script/script_path; 无实例 → null)。
    current: Option<std::collections::HashMap<String, ricow_strategy::ConfigValue>>,
}

/// 单个策略详情(只读): 完整清单(含参数 schema) + 用户实例当前值, 供前端参数表单渲染。
async fn get_strategy(
    State(state): State<WebState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<StrategyDetailReply>, WebError> {
    let entry = crate::strategies::catalog::find(&id)
        .ok_or_else(|| WebError::bad_request(format!("没有策略 {id}")))?;
    // 内置策略只有编译期嵌入清单, 无用户实例; 用户策略读 strategies/<id>.toml 解析当前值。
    let values = if entry.source == crate::strategies::catalog::Source::Builtin {
        None
    } else {
        strategy_io::load_instance_values(&state.root, &id)?
    };
    Ok(Json(StrategyDetailReply {
        manifest: entry.manifest,
        pair: values.as_ref().and_then(|v| v.pair.clone()),
        current: values.map(|v| v.current),
    }))
}

/// 语言读写(FR-029): 与 CLI 共用 `ricow.toml` 的 `[ui].lang`, 不新增第二处语言状态。
#[derive(serde::Serialize)]
struct LangReply {
    lang: &'static str,
    languages: [&'static str; 2],
}

#[derive(serde::Deserialize)]
struct LangBody {
    lang: String,
}

async fn get_lang(State(state): State<WebState>) -> Result<Json<LangReply>, WebError> {
    Ok(Json(LangReply { lang: current_lang(&state.root)?.code(), languages: SUPPORTED_LANGS }))
}

/// 写入新语言: 非法值**硬失败且不写盘**(023 既有规则, FR-031)。
async fn set_lang(
    State(state): State<WebState>,
    Json(body): Json<LangBody>,
) -> Result<Json<LangReply>, WebError> {
    let Some(lang) = Lang::parse(&body.lang) else {
        // 回执按**当前**界面语言(此刻 `body.lang` 非法, 不能拿它取语言)。
        let lang = current_lang(&state.root).unwrap_or(Lang::Zh);
        return Err(WebError::bad_request(format!(
            "{}: {}",
            t(lang, "不支持的语言", "unsupported language"),
            body.lang
        )));
    };
    // 先确保配置存在(首次运行会生成模板), 再按白名单写回并保留注释(与 `handle_lang` 同路径)。
    config_file::load(&state.root)?;
    config_file::set_values(
        &state.root,
        &[("ui", "lang", SetValue::Str(lang.code().to_string()))],
    )?;
    // 立即生效(FR-030): 写盘只改了配置, **已在跑的会话**手里还攥着旧语言与旧提示词 ——
    // 给它送一条控制行 `/lang`, 走的就是它自己那条"改配置 → 换语言 → 重建 LLM"
    // (`ChatSession::handle_lang` → `rebuild_llm`), 不重启服务、不换会话线程、不丢上下文。
    // (`false` = 当前没有会话在跑: 那就不用管, 下次开会话读配置自然是新语言。)
    state.hub.submit_control(format!("/lang {}", lang.code()));
    Ok(Json(LangReply { lang: lang.code(), languages: SUPPORTED_LANGS }))
}

/// 可选语言(单一来源: 前端语言切换控件按它渲染)。
const SUPPORTED_LANGS: [&str; 2] = ["zh", "en"];

/// 当前界面语言 = 配置里的 `[ui].lang`(缺省中文)。每次现读, 与 CLI 看到的永远一致。
fn current_lang(root: &Path) -> CoreResult<Lang> {
    Ok(crate::i18n::resolve(&config_file::load(root)?))
}

// ---- 交易可见性 (026 FR-010 ~ FR-014; D1 / D10 / D11 / D14) ----
//
// 四个端点全部**只读**: 数据来自 `WebState::db`(与引擎、会话存储同一份本地库, D1),
// 不触发任何交易动作 —— 撤单 / 平仓 / 停机仍走对话确认(D17 / FR-013)。
// 空状态一律**如实**: 响应带 `source` 三态(D10), 停机快照带 `updated_at`(D11)。

/// 交易端点的查询参数: `name` = 策略名(省略 = 全部策略), `limit` = 条数上限。
#[derive(serde::Deserialize)]
struct TradeQuery {
    name: Option<String>,
    limit: Option<i64>,
}

/// 单次条数上限: 与 AI 只读工具同口径(`ai::tools::trade_args`), 请求超限一律夹取。
const TRADE_LIMIT_MAX: i64 = 200;

impl TradeQuery {
    /// 策略名 → `strategy_id`(与 `format_fills` / AI 工具同口径: 能读到 TOML 就用配置里的 `name`)。
    fn strategy_id(&self, root: &Path) -> Option<String> {
        self.name.as_deref().map(|n| {
            crate::commands::read_strategy_config_in(root, n)
                .map(|c| c.name)
                .unwrap_or_else(|| n.to_string())
        })
    }

    /// `limit` 缺省取 `default`, 一律夹到 `1..=200`。
    fn limit(&self, default: i64) -> i64 {
        self.limit.unwrap_or(default).clamp(1, TRADE_LIMIT_MAX)
    }
}

/// 只读交易响应: 除明细外**必须**带来源三态与最后写入时间 ——
/// 前端据此把"确实没有"、"daemon 未运行, 此刻状态不可知"、"读不到(原因)"分开呈现(FR-014)。
#[derive(serde::Serialize)]
struct TradeReply<T> {
    /// `ok` / `daemon_down` / `unreadable`(D10 的稳定短串, 见 [`Source::as_str`])。
    source: &'static str,
    /// 读不到时的**实证原因**; 仅 `source == "unreadable"` 时有值。
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// 数据最后写入时间(毫秒); 无数据 = `null`。daemon 未运行时即"截至 <时间>"(D11)。
    updated_at: Option<i64>,
    items: Vec<T>,
}

/// 把 [`supervisor::classify`] 的三态结果折叠成响应体: 读不到 → `items` 为空 **且** 带原因,
/// 绝不把它说成"没有"。
fn trade_reply<T>(source: Source, items: Vec<T>, updated_at: Option<i64>) -> TradeReply<T> {
    let reason = match &source {
        Source::Unreadable(msg) => Some(msg.clone()),
        Source::Ok | Source::DaemonDown => None,
    };
    TradeReply { source: source.as_str(), reason, updated_at, items }
}

/// 成交明细项: 数值一律给字符串(十进制金额不失精度), 前端只做展示。
#[derive(serde::Serialize)]
struct FillItem {
    strategy_id: String,
    pair: String,
    side: String,
    fill_price: String,
    fill_size: String,
    fee: String,
    timestamp: i64,
    /// 运行模式(`dry_run` / `demo` / `live`); `null` = 关联不到订单, **未知**(不猜, D5)。
    mode: Option<String>,
}

impl From<&FillWithMode> for FillItem {
    fn from(f: &FillWithMode) -> Self {
        Self {
            strategy_id: f.strategy_id.clone(),
            pair: f.pair.clone(),
            side: f.side.clone(),
            fill_price: f.fill_price.to_string(),
            fill_size: f.fill_size.to_string(),
            fee: f.fee.to_string(),
            timestamp: f.timestamp,
            mode: f.mode.as_ref().filter(|m| !m.is_empty()).cloned(),
        }
    }
}

/// 订单项: **一单一行当前状态**(不是流水), 撤单 / 部分成交后的状态就在这里更新(D7)。
#[derive(serde::Serialize)]
struct OrderItem {
    strategy_id: String,
    exchange_order_id: String,
    client_order_id: String,
    pair: String,
    side: String,
    price: String,
    size: String,
    filled_size: String,
    /// 交易所订单状态(`open` / `partially_filled` / `filled` / `canceled` ...), 原样透出。
    status: String,
    mode: String,
    created_at: i64,
    updated_at: i64,
}

impl From<&OrderRecord> for OrderItem {
    fn from(o: &OrderRecord) -> Self {
        Self {
            strategy_id: o.strategy_id.clone(),
            exchange_order_id: o.exchange_order_id.clone(),
            client_order_id: o.client_order_id.clone(),
            pair: o.pair.clone(),
            side: o.side.clone(),
            price: o.price.to_string(),
            size: o.size.to_string(),
            filled_size: o.filled_size.to_string(),
            status: o.status.clone(),
            mode: o.mode.clone(),
            created_at: o.created_at,
            updated_at: o.updated_at,
        }
    }
}

/// 持仓项: **每策略每交易对一行**的当前持仓(D7)。
#[derive(serde::Serialize)]
struct PositionItem {
    strategy_id: String,
    pair: String,
    size: String,
    entry_price: String,
    mode: String,
    updated_at: i64,
}

impl From<&PositionRecord> for PositionItem {
    fn from(p: &PositionRecord) -> Self {
        Self {
            strategy_id: p.strategy_id.clone(),
            pair: p.pair.clone(),
            size: p.size.to_string(),
            entry_price: p.entry_price.to_string(),
            mode: p.mode.clone(),
            updated_at: p.updated_at,
        }
    }
}

/// PnL 快照项: 每笔成交后一条, 永久保留(FR-007)。
#[derive(serde::Serialize)]
struct PnlItem {
    strategy_id: String,
    timestamp: i64,
    realized_pnl: String,
    fees: String,
    net_pnl: String,
    trade_count: i64,
}

impl From<&PnlSnapshotRecord> for PnlItem {
    fn from(s: &PnlSnapshotRecord) -> Self {
        Self {
            strategy_id: s.strategy_id.clone(),
            timestamp: s.timestamp,
            realized_pnl: s.realized_pnl.to_string(),
            fees: s.fees.to_string(),
            net_pnl: s.net_pnl.to_string(),
            trade_count: s.trade_count,
        }
    }
}

/// 最近成交 (FR-010 / FR-012): 只读, 支持 `?name=<策略>&limit=<N>`。
async fn trades_fills(
    State(state): State<WebState>,
    Query(q): Query<TradeQuery>,
) -> Json<TradeReply<FillItem>> {
    let (sid, limit) = (q.strategy_id(&state.root), q.limit(50));
    let read =
        state.db.recent_fills_with_mode(sid.as_deref(), limit).await.map_err(|e| e.to_string());
    let (source, rows) = supervisor::classify(&state.root, read).await;
    let updated_at = rows.as_ref().and_then(|r| r.first()).map(|f| f.timestamp);
    let items = rows.unwrap_or_default().iter().map(FillItem::from).collect();
    Json(trade_reply(source, items, updated_at))
}

/// 最近订单 (FR-010 / FR-012): 只读, 按 `updated_at` 倒序 —— 挂单视图由此而来。
async fn trades_orders(
    State(state): State<WebState>,
    Query(q): Query<TradeQuery>,
) -> Json<TradeReply<OrderItem>> {
    let (sid, limit) = (q.strategy_id(&state.root), q.limit(50));
    let read = state.db.recent_orders(sid.as_deref(), limit).await.map_err(|e| e.to_string());
    let (source, rows) = supervisor::classify(&state.root, read).await;
    let updated_at = rows.as_ref().and_then(|r| r.first()).map(|o| o.updated_at);
    let items = rows.unwrap_or_default().iter().map(OrderItem::from).collect();
    Json(trade_reply(source, items, updated_at))
}

/// 当前持仓 (FR-010 / FR-012): 只读。表是"每策略每对一行"的当前状态, 全量本就很小。
async fn trades_positions(
    State(state): State<WebState>,
    Query(q): Query<TradeQuery>,
) -> Json<TradeReply<PositionItem>> {
    let (sid, limit) = (q.strategy_id(&state.root), q.limit(TRADE_LIMIT_MAX));
    let read = state.db.current_positions(sid.as_deref()).await.map_err(|e| e.to_string());
    let (source, rows) = supervisor::classify(&state.root, read).await;
    // 停机快照的"截至" = 这批持仓里**最近一次写入**(查询无序, 取最大值而非首行, D11)。
    let updated_at = rows.as_ref().and_then(|r| r.iter().map(|p| p.updated_at).max());
    let mut items: Vec<PositionItem> =
        rows.unwrap_or_default().iter().map(PositionItem::from).collect();
    items.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Json(trade_reply(source, items, updated_at))
}

/// PnL 快照 (FR-010 / FR-012): 只读, 按时间倒序。
async fn trades_pnl(
    State(state): State<WebState>,
    Query(q): Query<TradeQuery>,
) -> Json<TradeReply<PnlItem>> {
    let (sid, limit) = (q.strategy_id(&state.root), q.limit(20));
    let read =
        state.db.recent_pnl_snapshots(sid.as_deref(), limit).await.map_err(|e| e.to_string());
    let (source, rows) = supervisor::classify(&state.root, read).await;
    let updated_at = rows.as_ref().and_then(|r| r.first()).map(|p| p.timestamp);
    let items = rows.unwrap_or_default().iter().map(PnlItem::from).collect();
    Json(trade_reply(source, items, updated_at))
}

// ---- 策略日志 (026 FR-015 ~ FR-018; D2 / D12 / D13 / D14) ----
//
// 与 025 的会话 SSE **各走各的**(R5): 独立端点、独立流, 不共用会话 `Hub` 的任何通道 ——
// 会话链路一行不动。这一侧只**读** `logs/<name>.log`(由 `supervisor::procs` 写),
// 不写、不改、不轮转它; 策略**没在跑**照样能看历史日志(FR-017)。

/// 尾读的缺省条数: 与既有 AI `logs_tail` 同口径(D12)。
const LOG_DEFAULT_LINES: i64 = 50;

/// SSE 轮询间隔(D12): 服务端尾读驱动 —— 新行在下一轮里推出。
const LOG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// 日志端点的查询参数。
#[derive(serde::Deserialize)]
struct LogQuery {
    /// 打开时先给最近多少行: 缺省 50, 一律夹到 `1..=200`(D12 / FR-015 / SC-008)。
    lines: Option<i64>,
}

impl LogQuery {
    fn lines(&self) -> usize {
        let max = tail::TAIL_MAX_LINES as i64;
        self.lines.unwrap_or(LOG_DEFAULT_LINES).clamp(1, max) as usize
    }
}

/// 策略名 → 日志文件路径; 只接受**单段**名字(与 AI 工具 `safe_strategy_name` 同口径:
/// 禁路径分隔与 `..`, 防目录穿越)。非法 → `None`。
fn log_path_of(root: &Path, name: &str) -> Option<PathBuf> {
    let unsafe_name = name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.');
    (!unsafe_name).then(|| supervisor::ledger::log_path(root, name))
}

/// 日志清单一行(FR-017): 面板据此列出**当前有日志**的策略。
#[derive(serde::Serialize)]
struct LogFileItem {
    /// 策略名(日志文件的主名)。
    name: String,
    /// 当前字节数。
    size: u64,
    /// 最后写入时间(毫秒); 取不到为 `null`。
    updated_at: Option<i64>,
}

/// 日志清单: 现读 `logs/*.log`(FR-017 —— 只认文件, 不认进程; 文件在就该看得到)。
/// 目录不存在 → 空清单(还没跑过任何策略), 不是错误。
async fn list_logs(State(state): State<WebState>) -> Json<Vec<LogFileItem>> {
    Json(log_files(&state.root))
}

/// 列 `logs/*.log`(按名字排序); 轮转备份 `.log.1` 不在其中(D20)。
fn log_files(root: &Path) -> Vec<LogFileItem> {
    let Ok(entries) = std::fs::read_dir(supervisor::ledger::logs_dir(root)) else {
        return Vec::new();
    };
    let mut items: Vec<LogFileItem> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("log") {
                return None;
            }
            let name = path.file_stem().and_then(|s| s.to_str())?.to_string();
            let meta = e.metadata().ok();
            let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
            let updated_at = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_millis()).ok());
            Some(LogFileItem { name, size, updated_at })
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

/// 日志答复的三态(体例同 D10, 取值按日志的实际情形):
/// - `ok`: 文件在, `lines` 就是它的**原样**内容(可能为空 = 策略还没输出, FR-018);
/// - `missing`: 还没有日志文件(该策略从未启动过)—— **不是**"读不到";
/// - `unreadable`: 读不到(权限 / IO 错误), 附 `reason`; 不得说成"没有日志"(FR-020)。
#[derive(serde::Serialize)]
struct LogTailReply {
    name: String,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    lines: Vec<String>,
}

/// 日志尾部(FR-015 / FR-017): 只认文件 —— 策略没在跑也能看历史。
async fn log_tail(
    State(state): State<WebState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<LogQuery>,
) -> Result<Json<LogTailReply>, WebError> {
    let path = log_path_of(&state.root, &name)
        .ok_or_else(|| WebError::bad_request(format!("非法策略名: {name}")))?;
    let reply = match tail::tail_since(&path, 0, q.lines()) {
        Ok(got) => LogTailReply { name, source: "ok", reason: None, lines: got.lines },
        // 文件不存在 = 从来没启动过(既不是读不到, 也不代表"没有交易")。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            LogTailReply { name, source: "missing", reason: None, lines: Vec::new() }
        }
        // 其余 IO 错误: 如实说读不到 + 实证原因。
        Err(e) => LogTailReply {
            name,
            source: "unreadable",
            reason: Some(format!("{}: {e}", path.display())),
            lines: Vec::new(),
        },
    };
    Ok(Json(reply))
}

/// 日志流的一帧。`line` 的 `text` 是文件里的那一行, **原样**(FR-018);
/// `rotated` / `unreadable` 是宿主对"轮转"与"读不到"的如实提示, 不是日志内容。
#[derive(serde::Serialize)]
struct LogEvent {
    kind: &'static str,
    text: String,
}

/// 服务端尾读游标: 只记 (路径, 上次 offset, 首读行数) —— 每轮现读文件, 不在内存里留日志内容。
struct LogTail {
    path: PathBuf,
    offset: u64,
    lines: usize,
    lang: Lang,
}

impl LogTail {
    /// 读一轮: `Ok` = 该推的帧(可能为空), `Err` = 读不到(实证原因)。
    ///
    /// 文件不存在 → `Ok(空)`: 策略还没启动(或日志已被清理), 不算错 —— 文件一旦出现,
    /// 下一轮 offset 仍是 0, 自然按"最近 N 行"补上首屏(FR-016 / FR-017)。
    fn poll(&mut self) -> Result<Vec<LogEvent>, String> {
        let got = match tail::tail_since(&self.path, self.offset, self.lines) {
            Ok(got) => got,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        self.offset = got.offset;
        let mut out = Vec::new();
        if got.rotated {
            // D13: 轮转/截断后不许静默跳读 —— 先如实说一句, 再给重读到的内容。
            out.push(LogEvent {
                kind: "rotated",
                text: t(
                    self.lang,
                    "日志已轮转(或被截断), 已从头重读 —— 不丢行、不重复",
                    "log rotated (or truncated); re-read from the start - no line dropped or repeated",
                )
                .to_string(),
            });
        }
        out.extend(got.lines.into_iter().map(|text| LogEvent { kind: "line", text }));
        Ok(out)
    }

    /// 读不到时给一帧如实提示(流不断, 下一轮继续试)。
    fn unreadable(&self, reason: String) -> Vec<LogEvent> {
        vec![LogEvent { kind: "unreadable", text: reason }]
    }
}

/// 日志实时流(FR-016 / D2): 首屏 = 最近 `lines` 行, 之后每 500ms 尾读一次增量(D12);
/// 页面刷新重连即重建。**独立端点、独立流** —— 不共用 025 会话 `Hub`(R5)。
async fn log_stream(
    State(state): State<WebState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<LogQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, WebError> {
    let path = log_path_of(&state.root, &name)
        .ok_or_else(|| WebError::bad_request(format!("非法策略名: {name}")))?;
    let lang = current_lang(&state.root).unwrap_or(Lang::Zh);
    let mut tail = LogTail { path, offset: 0, lines: q.lines(), lang };
    // 首屏在返回响应前读一次: 读不到就**当场**给提示帧, 而不是开一条永远不说话的流。
    let head = match tail.poll() {
        Ok(events) => events,
        Err(reason) => tail.unreadable(reason),
    };
    let stream =
        futures::stream::unfold((tail, head.into_iter()), |(mut tail, mut pending)| async move {
            loop {
                if let Some(event) = pending.next() {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok(Event::default().data(data)), (tail, pending)));
                }
                tokio::time::sleep(LOG_POLL_INTERVAL).await;
                pending = match tail.poll() {
                    Ok(events) => events.into_iter(),
                    Err(reason) => tail.unreadable(reason).into_iter(),
                };
            }
        });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// handler 的错误出口: 只回状态码与一句原因 —— **不回任何会话内容**(D3 / FR-002)。
///
/// 032 US3 起错误体统一为 data-model §9 的 `{ "error": 中文, "code"?: 机器码 }` JSON;
/// `code` / `line` 缺省(既有端点)序列化时跳过 —— 既有端点的中文文案一字不改。
struct WebError {
    status: StatusCode,
    body: WebErrorBody,
}

/// 统一错误响应体 (data-model §9): 机器码 / 行号可选, 缺省即不出现。
#[derive(serde::Serialize)]
struct WebErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    /// 编译错误行号等定位信息(目前仅策略保存的编译失败带)。
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
}

impl WebError {
    fn new(
        status: StatusCode,
        reason: impl Into<String>,
        code: Option<String>,
        line: Option<u32>,
    ) -> Self {
        Self { status, body: WebErrorBody { error: reason.into(), code, line } }
    }

    fn bad_request(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, reason, None, None)
    }

    /// 400 + 机器码(如策略名非法 `invalid_name` / 编译失败 `compile`)。
    fn bad_request_code(reason: impl Into<String>, code: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, reason, Some(code.to_string()), None)
    }

    /// 404 + 中文一句(如未知策略 id)。
    fn not_found(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, reason, None, None)
    }

    /// 403 需前置动作 + 机器码(032 §6: AI 改 Lua 未配密钥 → `need_keys`, 引导先去设置页)。
    fn forbidden(reason: impl Into<String>, code: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, reason, Some(code.to_string()), None)
    }

    /// 409 冲突 + 机器码(策略保存: `prefix` / `reserved` / `running` / `exists`)。
    fn conflict(reason: impl Into<String>, code: &str) -> Self {
        Self::new(StatusCode::CONFLICT, reason, Some(code.to_string()), None)
    }

    /// 编译门禁失败: 400 + `code:"compile"`, 尽量带 mlua 行号 (data-model §5)。
    fn compile_failed(reason: impl Into<String>, line: Option<u32>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, reason, Some("compile".to_string()), line)
    }

    /// 上游(交易所公共行情)失败: 502 + 中文一句(可带上游原文, FR-014)。
    fn bad_gateway(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, reason, None, None)
    }
}

impl From<CoreError> for WebError {
    fn from(e: CoreError) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None, None)
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// token 校验中间件: 缺失或错误**一律 `401` 且响应体为空**(不回任何内容, D3 / FR-002)。
async fn require_token(State(state): State<WebState>, req: Request, next: Next) -> Response {
    match token_of(&req) {
        Some(token) if ct_eq(&token, &state.token) => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// 从请求中取 token: 先 `Authorization: Bearer <token>`, 再查询参数 `?token=<token>`。
///
/// 两种都收是因为浏览器 `EventSource` **无法自定请求头**, 只能把 token 挂在 URL 上。
fn token_of(req: &Request) -> Option<String> {
    if let Some(value) = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(token) = value.strip_prefix("Bearer ") {
            return Some(token.to_string());
        }
    }
    req.uri().query()?.split('&').find_map(|kv| {
        let (key, value) = kv.split_once('=')?;
        (key == "token").then(|| value.to_string())
    })
}

/// 常量时间比较 (Web token) —— **逐字搬抄** `supervisor/server.rs` 的同名函数 (D3),
/// 因 `supervisor` 为 crate 私有模块, 跨模块无法复用, 故在源处同步维护这份实现。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ct_eq_matches_only_identical() {
        assert!(ct_eq("", ""));
        assert!(ct_eq("3f2a-9c", "3f2a-9c"));
        assert!(!ct_eq("3f2a-9c", "3f2a-9d"));
        assert!(!ct_eq("3f2a-9c", "3f2a-9c-"));
        assert!(!ct_eq("", "3f2a-9c"));
    }

    /// 静态资源与首页(D4): 032 拆分后的各份资产都非空; 首页里每个资源 URL 必须带上本次 token ——
    /// 浏览器取 `<link>` / `<script>` 带不上请求头, 只能靠这里填进去。
    #[test]
    fn test_index_fills_token_into_asset_urls() {
        assert!(
            !INDEX_HTML.is_empty()
                && !STYLE_CSS.is_empty()
                && !LWC_JS.is_empty()
                && !COMMON_JS.is_empty()
                && !ROUTER_JS.is_empty()
                && !CHAT_JS.is_empty()
                && !SETTINGS_JS.is_empty()
                && !MARKETS_JS.is_empty()
                && !STRATEGIES_JS.is_empty()
                && !RUNS_JS.is_empty()
                && !APP_JS.is_empty()
        );
        for asset in [
            "/style.css",
            "/lightweight-charts.js",
            "/common.js",
            "/router.js",
            "/chat.js",
            "/settings.js",
            "/markets.js",
            "/strategies.js",
            "/runs.js",
            "/app.js",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("{asset}?token=")),
                "首页须引用带 token 的 {asset}"
            );
        }
        let html = render_index("tok-123");
        assert!(html.contains("tok-123"), "资源 URL 应带上本次 token");
        assert!(!html.contains(TOKEN_PLACEHOLDER), "占位符必须全部替换掉");
    }

    /// SC-011(浏览器侧): 页面不往 `localStorage` / `sessionStorage` 写任何东西 —— 明文密钥与
    /// 对话内容都不该在浏览器里留存; 密钥提示一到就切遮蔽输入, 提交时也不画用户气泡。
    ///
    /// 遮蔽**只能**走 CSS class: 输入区是多行 `<textarea>`, 它的 `type` 是只读属性, 赋值会抛
    /// `TypeError: Cannot set property type of #<HTMLTextAreaElement> which has only a getter`
    /// 并把 `openSession` / `submitText` 打断(2026-09-19 实机走查发现)。
    #[test]
    fn test_frontend_stores_nothing_and_masks_secret_input() {
        // 032 脚本拆分后, 扫描覆盖**全部**前端脚本(含第三方图表库 UMD): 一份都不许碰浏览器存储。
        let js_assets = [
            LWC_JS,
            COMMON_JS,
            ROUTER_JS,
            CHAT_JS,
            SETTINGS_JS,
            MARKETS_JS,
            STRATEGIES_JS,
            RUNS_JS,
            APP_JS,
        ];
        for store in ["localStorage", "sessionStorage"] {
            for js in js_assets {
                assert!(!js.contains(store), "前端不得使用 {store}(SC-011)");
            }
        }
        assert!(CHAT_JS.contains(r#"case "secret_prompt""#), "前端要处理密钥提示帧(FR-012)");
        assert!(
            CHAT_JS.contains(r#"els.input.classList.toggle("masked""#),
            "密钥录入期间输入框须遮蔽回显(SC-011)"
        );
        for js in js_assets {
            assert!(
                !js.contains("input.type =") && !js.contains("input.type="),
                "`<textarea>` 的 type 只读, 赋值会抛 TypeError"
            );
        }
        assert!(STYLE_CSS.contains("#input.masked"), "遮蔽样式须随前端一并内嵌(D4)");
        assert!(
            CHAT_JS.contains("!value.trim() && !state.secret"),
            "密钥期须放行空行(提示语承诺的\"回车放弃\", 服务端按空值不改配置)"
        );
        assert!(
            COMMON_JS.contains("const suffix = R.lang;"),
            "`dataset` 键名是首字母小写驼峰, 取大写得 undefined(静态文案整片空白 / placeholder 变字面量)"
        );
    }

    /// D17 / FR-013(只读红线): 右侧两块面板**只读** —— 页面上没有任何直连交易所的按钮, 前端也
    /// 不对交易 / 日志端点发写请求; 撤单 / 平仓 / 停机仍然只能在对话里确认(D19 不做交易所直连)。
    #[test]
    fn test_frontend_panels_are_read_only() {
        // 先确认面板真在, 否则下面全是空转(032 后面板整体迁入 #view-chat, id 不变)。
        assert!(
            INDEX_HTML.contains(r#"id="trade-panel""#) && INDEX_HTML.contains(r#"id="log-panel""#)
        );
        assert!(CHAT_JS.contains(r#"api("/api/trades/positions")"#), "交易面板数据须来自只读端点");

        // 面板区里没有按钮、没有内联事件 —— 只展示, 不动作。
        let start = INDEX_HTML.find(r#"<aside id="panels">"#).expect("右栏面板");
        let end = INDEX_HTML[start..].find("</aside>").expect("面板收尾") + start;
        let panels = &INDEX_HTML[start..end];
        assert!(!panels.contains("<button"), "面板是只读的, 不得有按钮(D17)");
        assert!(!panels.contains("onclick"), "面板是只读的, 不得有内联事件(D17)");

        // 这两组端点在对话视图里只以 GET 出现(032: 逻辑迁入 chat.js)。
        for line in CHAT_JS.lines().filter(|l| l.contains("/api/trades") || l.contains("/api/logs"))
        {
            assert!(!line.contains("method:"), "交易 / 日志端点只读, 不得带写方法: {line}");
        }
        // 前端**全部脚本**(含第三方图表库)里不出现交易所写动作的入口。
        for js in [
            LWC_JS,
            COMMON_JS,
            ROUTER_JS,
            CHAT_JS,
            SETTINGS_JS,
            MARKETS_JS,
            STRATEGIES_JS,
            RUNS_JS,
            APP_JS,
        ] {
            for banned in ["cancel_order", "cancelOrder", "close_position", "place_order"] {
                assert!(!js.contains(banned), "前端不得出现交易所写动作 `{banned}`(D17 / FR-013)");
            }
        }
    }

    #[test]
    fn test_token_of_reads_header_then_query() {
        let req = |uri: &str, auth: Option<&str>| {
            let mut builder = Request::builder().uri(uri);
            if let Some(auth) = auth {
                builder = builder.header(header::AUTHORIZATION, auth);
            }
            builder.body(axum::body::Body::empty()).expect("构造请求")
        };
        // Authorization: Bearer 优先。
        assert_eq!(token_of(&req("/api/ping", Some("Bearer abc"))).as_deref(), Some("abc"));
        assert_eq!(token_of(&req("/api/ping", Some("Basic abc"))), None);
        // 无头时退回查询参数; 只有名为 token 的那个键算数。
        assert_eq!(token_of(&req("/api/ping?token=xyz", None)).as_deref(), Some("xyz"));
        assert_eq!(token_of(&req("/api/ping?a=b&token=xyz", None)).as_deref(), Some("xyz"));
        assert_eq!(token_of(&req("/api/ping?tokens=xyz", None)), None);
        assert_eq!(token_of(&req("/api/ping", None)), None);
    }

    #[tokio::test]
    async fn test_bind_default_port_is_loopback() {
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        assert!(port > 0, "port=0 应由系统分配实际端口");
        let addr = listener.local_addr().expect("读取监听地址");
        assert!(addr.ip().is_loopback(), "服务只应绑回环地址, 实际 {addr}");
        assert_eq!(addr.port(), port);
    }

    /// 取 HTTP 响应体(首个空行之后); 供鉴权用例断言「响应体为空 / 不含会话内容」。
    fn body_of(res: &str) -> &str {
        res.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    /// SC-005 / FR-002: 无 token 与错 token 一律 `401` 且**响应体不携带任何会话内容**;
    /// 带对 token 才拿得到响应。走真实监听套接字 —— 中间件、路由、响应体一体验证。
    #[tokio::test]
    async fn test_missing_or_wrong_token_is_401_without_leaking_content() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use ricow_strategy::Database;

        /// 裸 HTTP/1.1 请求(本 crate 没有 HTTP 客户端依赖, 直接写套接字)。
        async fn get(port: u16, path: &str) -> String {
            let mut stream =
                tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
            let req =
                format!("GET {path} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nConnection: close\r\n\r\n");
            stream.write_all(req.as_bytes()).await.expect("发出请求");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_string()
        }

        const SECRET: &str = "TOP-SECRET-CONTENT";
        let root = std::env::temp_dir().join(format!("ricow-web-auth-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时数据目录");
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = SessionStore::new(db.clone());
        let sid = store.create().await.expect("建会话");
        store.append_user(&sid, SECRET).await.expect("落一条会话内容");

        let starter: Starter = Arc::new(|_id: &str, _sink: &mut WebSink| Ok(()));
        let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let _server = tokio::spawn(serve(listener, state));

        // 无 token: 首页与接口一律 401, 且响应体为空、不带任何会话内容。
        for path in ["/", "/api/ping", "/api/config/keys", &format!("/api/sessions/{sid}/messages")]
        {
            let res = get(port, path).await;
            assert!(res.starts_with("HTTP/1.1 401"), "无 token 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {res}");
            assert!(!res.contains(SECRET), "401 不得泄漏会话内容: {res}");
        }
        // 032 T007: 脚本拆分后的各静态资源同样挂在 token 门禁之后 —— 无 / 错 token 一律 401。
        for path in [
            "/style.css",
            "/lightweight-charts.js",
            "/common.js",
            "/router.js",
            "/chat.js",
            "/settings.js",
            "/markets.js",
            "/strategies.js",
            "/runs.js",
            "/app.js",
        ] {
            let res = get(port, path).await;
            assert!(res.starts_with("HTTP/1.1 401"), "无 token 取 {path} 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {path}");
            let res = get(port, &format!("{path}?token=wrong")).await;
            assert!(res.starts_with("HTTP/1.1 401"), "错 token 取 {path} 应 401, 实际: {res}");
        }
        // 对 token 取得到新资源(证明上面拦住的不是"路由不存在")。
        let res = get(port, "/common.js?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "对 token 应取到 common.js, 实际: {res}");
        // 错 token: 即便路径与会话 id 都正确, 同样 401 且不回内容。
        let res = get(port, &format!("/api/sessions/{sid}/messages?token=wrong")).await;
        assert!(res.starts_with("HTTP/1.1 401"), "错 token 应 401, 实际: {res}");
        assert!(!res.contains(SECRET), "错 token 不得泄漏会话内容: {res}");
        // 对 token: 才拿得到内容(证明上面拦住的不是「路由不存在」)。
        let res = get(port, "/api/ping?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "对 token 应 200, 实际: {res}");
        assert_eq!(body_of(&res), "ok");

        // 032 US1: 密钥端点的 POST 同样在 token 门禁之后 —— 无 / 错 token 一律 401、空体;
        // 写请求体里放一把假密钥, 断言它不会被 401 响应原样带回。
        async fn post_raw(port: u16, path: &str, token: Option<&str>, body: &str) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let target = match token {
                Some(tok) => format!("{path}?token={tok}"),
                None => path.to_string(),
            };
            let mut stream =
                tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
            let req = format!(
                "POST {target} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(req.as_bytes()).await.expect("发出 POST");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_string()
        }
        const FAKE_KEY: &str = "FAKE-KEY-MUST-NOT-ECHO";
        let body = format!(
            "{{\"updates\":[{{\"section\":\"exchange\",\"key\":\"binance_key\",\"value\":\"{FAKE_KEY}\"}}]}}"
        );
        for probe in [None, Some("wrong")] {
            let res = post_raw(port, "/api/config/keys", probe, &body).await;
            assert!(res.starts_with("HTTP/1.1 401"), "无/错 token POST 应 401, 实际: {res}");
            assert!(body_of(&res).is_empty(), "401 响应体必须为空: {res}");
            assert!(!res.contains(FAKE_KEY), "401 不得回显请求中的密钥: {res}");
        }
        // 对 token: GET 密钥端点应 200 且响应不含任何密钥全文(空模板, 全部未配置)。
        let res = get(port, "/api/config/keys?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "对 token GET 密钥端点应 200, 实际: {res}");
        let body_txt = body_of(&res);
        assert!(body_txt.contains(r#""configured":false"#), "空模板应全部未配置: {body_txt}");
        assert!(!body_txt.contains(FAKE_KEY), "读响应不得含密钥全文: {body_txt}");
    }

    /// FR-011 回归: 一次 `POST /input` **只投一行**。
    ///
    /// 页面开局就连 SSE(`/events` → `attach`), 所以走到 `session_input` 时会话**已经在跑** ——
    /// 曾经的写法在这条热路径上 `submit` 了两次(先判一次、末尾又无条件补一次), 每行输入都进通道
    /// 两遍: 一次输入顶两轮、两次 LLM 调用(2026-09-19 实机 DB: 12 条用户输入无一例外成对)。
    #[tokio::test]
    async fn test_input_posts_one_line_even_when_session_already_attached() {
        use std::time::Duration;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        use ricow_strategy::Database;

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let starter: Starter = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_id: &str, sink: &mut WebSink| {
                // 模拟 REPL: 收到一行记一笔, 直到通道断开(`hub.stop` 或页面走人)。
                while let Some(line) = sink.input_line("") {
                    lock(&seen).push(line);
                }
                Ok(())
            })
        };

        let root = std::env::temp_dir().join(format!("ricow-web-input-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时数据目录");
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = SessionStore::new(db.clone());
        let sid = store.create().await.expect("建会话");
        let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
        let hub = Arc::clone(&state.hub);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let _server = tokio::spawn(serve(listener, state));

        // 开局先连 SSE: 这一步会把会话 attach 上(热路径由此成立)。
        let mut sse = tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上 SSE");
        let head = format!(
            "GET /api/sessions/{sid}/events?token=tok-ok HTTP/1.1\r\nHost: {BIND_ADDR}\r\n\r\n"
        );
        sse.write_all(head.as_bytes()).await.expect("发 SSE 请求");
        let mut buf = [0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(2), sse.read(&mut buf))
            .await
            .expect("等首帧超时")
            .expect("读到首帧");
        assert!(n > 0, "SSE 应至少回一帧");

        // 一次入站。`submit` 在 handler 内同步入队, 故 200 回来时该投的行都已进通道。
        let body = serde_json::json!({ "text": "确认" }).to_string();
        let mut post = tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
        let req = format!(
            "POST /api/sessions/{sid}/input?token=tok-ok HTTP/1.1\r\nHost: {BIND_ADDR}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        );
        post.write_all(req.as_bytes()).await.expect("发 POST");
        let mut res = Vec::new();
        post.read_to_end(&mut res).await.expect("读回响应");
        assert!(
            String::from_utf8_lossy(&res).starts_with("HTTP/1.1 200"),
            "入站应 200, 实际: {}",
            String::from_utf8_lossy(&res)
        );

        // 给会话线程一点时间把已入队的行记完, 再收摊(断开通道 → 记录循环退出)。
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub.stop(&sid);

        let got = lock(&seen).clone();
        assert_eq!(got, vec!["确认".to_string()], "一次 POST 只应投一行, 实际: {got:?}");
    }

    /// 收一帧(最多 2 秒); 收不到即失败 —— 线程行为不符合预期时不让测试挂死。
    fn expect_frame(rx: &mut broadcast::Receiver<Frame>) -> Frame {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match rx.try_recv() {
                Ok(frame) => return frame,
                Err(broadcast::error::TryRecvError::Empty) => {
                    assert!(std::time::Instant::now() < deadline, "等帧超时");
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(e) => panic!("取帧失败: {e}"),
            }
        }
    }

    /// 运行时登记: 同时只跑一条会话, 输入只认当前会话; 换会话即停旧线程。
    #[test]
    fn test_hub_keeps_one_active_session_and_routes_input() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let starter: Starter = {
            let seen = Arc::clone(&seen);
            Arc::new(move |session_id: &str, sink: &mut WebSink| {
                // 模拟 REPL: 收一行就收摊(收不到 = 通道断开)。
                if let Some(line) = sink.input_line("") {
                    lock(&seen).push(format!("{session_id}:{line}"));
                }
                Ok(())
            })
        };
        let hub = Hub::new(starter, Arc::new(std::env::temp_dir()));

        // 起会话 a: 线程先发 TurnEnd(表示已就绪), 再阻塞等输入。
        let mut rx = hub.attach("a").expect("启动会话 a");
        assert_eq!(expect_frame(&mut rx), Frame::TurnEnd);
        // 别的会话不在跑, 输入不该被接受。
        assert!(!hub.submit("b", "走错门".into()));
        assert!(hub.submit("a", "你好".into()));
        // 线程取到那一行后收摊 → 前端收到 Closed。
        assert_eq!(expect_frame(&mut rx), Frame::Closed);
        assert_eq!(*lock(&seen), vec!["a:你好".to_string()]);

        // 换会话: 旧会话随即失效(新线程同理)。
        let _b = hub.attach("b").expect("启动会话 b");
        assert!(!hub.submit("a", "旧会话已停".into()));
        hub.stop("b");
        assert!(!hub.submit("b", "已收摊".into()));
    }

    /// T030 / FR-031: 会话线程**没起来**时兜底那一行是页面看得见的宿主文案 —— 得跟着
    /// `[ui].lang` 走, 不能写死中文。
    #[test]
    fn test_attach_error_line_follows_configured_language() {
        let root = std::env::temp_dir().join(format!("ricow-web-lang-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时数据目录");
        config_file::ensure_template(&root).expect("生成配置模板");
        config_file::set_values(&root, &[("ui", "lang", SetValue::Str("en".into()))])
            .expect("写入语言");

        let starter: Starter =
            Arc::new(|_id: &str, _sink: &mut WebSink| Err(CoreError::Auth("boom".into())));
        let hub = Hub::new(starter, Arc::new(root));
        let mut rx = hub.attach("s1").expect("启动会话线程");

        match expect_frame(&mut rx) {
            Frame::Line { text, sev } => {
                assert!(
                    text.starts_with("session failed to start"),
                    "应为英文兜底行, 实际: {text}"
                );
                assert_eq!(sev, "error", "兜底行按错误着色");
            }
            other => panic!("应收到错误行, 实际: {other:?}"),
        }
    }

    // ── 026 T025 / T026: 交易端点鉴权 + 空状态三态 (SC-007 / FR-011 / FR-014)──────────

    /// 裸 HTTP/1.1 GET(本 crate 没有 HTTP 客户端依赖, 直接写回环套接字)。
    async fn get_raw(port: u16, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut stream = tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
        let req = format!("GET {path} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.expect("发出请求");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("读回响应");
        String::from_utf8_lossy(&buf).to_string()
    }

    /// 起一个真实监听的服务(中间件 + 路由 + 响应体一体验证), 返回系统分配的端口。
    ///
    /// 顺手断言监听地址仍是 `127.0.0.1`: FR-011 要求新增端点**不**改变这张网的面 ——
    /// 依旧只在回环上服务。
    async fn serve_trade_test(root: PathBuf, db: Database) -> u16 {
        let store = SessionStore::new(db.clone());
        let starter: Starter = Arc::new(|_id: &str, _sink: &mut WebSink| Ok(()));
        let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let addr = listener.local_addr().expect("读取监听地址");
        assert_eq!(addr.ip(), std::net::IpAddr::V4(BIND_ADDR), "只应绑回环, 实际 {addr}");
        tokio::spawn(serve(listener, state));
        port
    }

    /// 干净的临时数据目录(`daemon.json` 的有无由用例掌握)。
    fn trade_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ricow-web-trade-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时数据目录");
        dir
    }

    /// 往库里种成交 / 订单 / 持仓 / PnL 各一行 —— 让「401 不泄漏交易数据」有真数据可泄漏。
    async fn seed_trade_rows(db: &Database) {
        use ricow_core::{OrderFill, OrderSide};

        const TS: i64 = 1_700_000_000_000;
        let at = chrono::DateTime::from_timestamp_millis(TS).expect("时间戳");
        let dec = |s: &str| s.parse::<rust_decimal::Decimal>().expect("Decimal");

        db.insert_fill(
            "s1",
            &OrderFill {
                trade_id: None,
                exchange_order_id: "EX-SECRET".into(),
                client_order_id: "C-1".into(),
                pair: "BTCUSDT".into(),
                side: OrderSide::Buy,
                fill_price: dec("100"),
                fill_size: dec("2"),
                fee: dec("0.1"),
                timestamp: at,
            },
        )
        .await
        .expect("落成交");
        db.upsert_order(&OrderRecord {
            strategy_id: "s1".into(),
            exchange_order_id: "EX-SECRET".into(),
            client_order_id: "C-1".into(),
            pair: "BTCUSDT".into(),
            side: "buy".into(),
            price: dec("100"),
            size: dec("2"),
            filled_size: dec("2"),
            status: "filled".into(),
            mode: "demo".into(),
            created_at: TS,
            updated_at: TS,
        })
        .await
        .expect("落订单");
        db.upsert_position(&PositionRecord {
            strategy_id: "s1".into(),
            pair: "BTCUSDT".into(),
            size: dec("2"),
            entry_price: dec("100"),
            mode: "demo".into(),
            updated_at: TS,
        })
        .await
        .expect("落持仓");
        db.insert_pnl_snapshot(&PnlSnapshotRecord {
            strategy_id: "s1".into(),
            timestamp: TS,
            realized_pnl: dec("5"),
            fees: dec("0.1"),
            net_pnl: dec("4.9"),
            trade_count: 1,
        })
        .await
        .expect("落 PnL 快照");
    }

    /// 造一个「daemon 在线」的最小替身: 绑本机端口 + 落 `run/daemon.json`。
    ///
    /// 探活口径 = `read_daemon_info` + TCP connect(唯一口径, D10), 握手成功即算在跑 ——
    /// 所以只需保住监听器(连上即丢), 不必应答任何协议。
    async fn seed_online_daemon(root: &Path) {
        use crate::supervisor::ledger;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑本机端口");
        let port = listener.local_addr().expect("读端口").port();
        ledger::ensure_dirs(root).expect("建 run/logs 目录");
        ledger::write_daemon_info(
            root,
            &ledger::DaemonInfo {
                pid: std::process::id(),
                port,
                token: "tok-daemon".into(),
                started_at: ledger::now_str(),
            },
        )
        .expect("写 run/daemon.json");
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                drop(sock);
            }
        });
    }

    /// SC-007 / FR-011: 四个交易端点**逐个**过一遍 —— 无 token / 错 token 一律 `401` 且
    /// 响应体**不含任何交易数据**; 带对 token 才 `200`。库里种了真数据, 泄漏断言才成立。
    #[tokio::test]
    async fn test_trade_endpoints_require_token_and_never_leak_rows() {
        /// 库里那行成交 / 订单 / 持仓的可辨识字样 —— 出现在 401 响应里就是泄漏。
        const MARKS: [&str; 2] = ["EX-SECRET", "BTCUSDT"];

        let root = trade_root("auth");
        let db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&db).await;
        let port = serve_trade_test(root, db).await;

        let paths =
            ["/api/trades/fills", "/api/trades/orders", "/api/trades/positions", "/api/trades/pnl"];
        for path in paths {
            for probe in [path.to_string(), format!("{path}?token=wrong")] {
                let res = get_raw(port, &probe).await;
                assert!(res.starts_with("HTTP/1.1 401"), "无/错 token 应 401: {probe} → {res}");
                assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe} → {res}");
                for mark in MARKS {
                    assert!(!res.contains(mark), "401 不得泄漏交易数据({mark}): {res}");
                }
            }
            // 对 token: 才 `200`(证明上面拦住的不是"路由不存在")。
            let res = get_raw(port, &format!("{path}?token=tok-ok")).await;
            assert!(res.starts_with("HTTP/1.1 200"), "对 token 应 200: {path} → {res}");
            assert!(!body_of(&res).is_empty(), "对 token 应有响应体: {path}");
        }

        // 库中那行必须真给得出来 —— 否则上面的「不泄漏」只是空谈。
        let fills = get_raw(port, "/api/trades/fills?token=tok-ok").await;
        assert!(fills.contains("BTCUSDT"), "成交端点应给出库中成交: {fills}");
        let orders = get_raw(port, "/api/trades/orders?token=tok-ok").await;
        assert!(orders.contains("EX-SECRET"), "订单端点应给出库中订单: {orders}");
    }

    /// FR-014 / D10: 库**读不到**时不得说成"没有" —— 响应带 `source=unreadable` + 实证原因,
    /// 且不给任何数据; 状态码仍是 `200`(如实回话, 不是 5xx)。
    #[tokio::test]
    async fn test_trade_unreadable_db_says_reason_not_empty() {
        let root = trade_root("unreadable");
        let db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&db).await;
        let closer = db.clone();
        let port = serve_trade_test(root, db).await;
        // 关池 → 之后任何读都报错 = "读不到"的实证(生产路径不会走到)。
        closer.close_pool_for_test().await;

        for path in
            ["/api/trades/fills", "/api/trades/orders", "/api/trades/positions", "/api/trades/pnl"]
        {
            let res = get_raw(port, &format!("{path}?token=tok-ok")).await;
            assert!(res.starts_with("HTTP/1.1 200"), "读不到也应 200: {path} → {res}");
            let body = body_of(&res).to_string();
            assert!(body.contains(r#""source":"unreadable""#), "{path} 必须标 unreadable: {body}");
            assert!(!body.contains(r#""reason":null"#), "{path} 必须带上读不到的原因: {body}");
            assert!(!body.contains(r#""reason":"""#), "{path} 原因不得为空串: {body}");
            assert!(body.contains(r#""items":[]"#), "{path} 读不到时不得给任何数据: {body}");
        }
    }

    /// FR-014 / D10 / D11: 空状态三态必须**分开** ——
    /// `daemon_down` + 快照 = 带 `updated_at`(前端标"截至 <时间>"), **不**谎称"当前";
    /// `ok` + 真空 = 这时才叫"确实没有"。
    #[tokio::test]
    async fn test_trade_distinguishes_daemon_down_snapshot_from_true_empty() {
        // ① daemon 未运行(无 run/daemon.json) + 库里有最后一次快照。
        let down_root = trade_root("down");
        let down_db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&down_db).await;
        let down_port = serve_trade_test(down_root, down_db).await;

        let res = get_raw(down_port, "/api/trades/positions?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"daemon_down""#), "daemon 不在须标 daemon_down: {body}");
        assert!(body.contains("BTCUSDT"), "最后一次快照仍应给出: {body}");
        assert!(!body.contains(r#""updated_at":null"#), "快照必须带'截至 <时间>'的落点: {body}");

        // ② daemon 在线 + 库确实空。
        let up_root = trade_root("up");
        seed_online_daemon(&up_root).await;
        let up_db = Database::open_in_memory().await.expect("开内存库");
        let up_port = serve_trade_test(up_root, up_db).await;

        let res = get_raw(up_port, "/api/trades/positions?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"ok""#), "daemon 在线应为 ok: {body}");
        assert!(body.contains(r#""updated_at":null"#), "真空时没有'截至'可标: {body}");
        assert!(body.contains(r#""items":[]"#), "确认真空时才说'没有': {body}");
    }

    // ── 026 T029 / T030 / T031: 日志端点鉴权 + 尾读上限 + 三态 (SC-007 / SC-008)──────────

    /// 写一份策略日志(模拟 `supervisor::procs` 写的 `logs/<name>.log`)。**不启任何进程** ——
    /// FR-017 要的正是"策略没在跑也看得到历史日志"。
    fn write_log(root: &Path, name: &str, lines: &[String]) {
        let dir = crate::supervisor::ledger::logs_dir(root);
        std::fs::create_dir_all(&dir).expect("建 logs 目录");
        std::fs::write(dir.join(format!("{name}.log")), lines.join("\n") + "\n").expect("写日志");
    }

    /// FR-017 / FR-018: 清单与尾部**只认文件** —— 没有 daemon、没有台账, 一样列得出、看得到;
    /// 非 `.log` 与轮转备份 `.log.1` 不进清单(D20)。
    #[tokio::test]
    async fn test_log_listing_and_tail_need_no_running_strategy() {
        let root = trade_root("logs-list");
        let lines: Vec<String> = (1..=3).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "alpha", &lines);
        write_log(&root, "beta", &["ONLY-ONE".to_string()]);
        // 干扰项: 非日志文件、轮转备份 —— 都不该出现在清单里。
        std::fs::write(crate::supervisor::ledger::logs_dir(&root).join("notes.txt"), "x")
            .expect("写干扰文件");
        std::fs::write(crate::supervisor::ledger::logs_dir(&root).join("alpha.log.1"), "x")
            .expect("写轮转备份");

        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_trade_test(root, db).await;

        let res = get_raw(port, "/api/logs?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "清单应 200: {res}");
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""name":"alpha""#), "有日志的策略就该列出: {body}");
        assert!(body.contains(r#""name":"beta""#), "有日志的策略就该列出: {body}");
        assert!(!body.contains("notes"), "非 .log 不进清单: {body}");
        assert!(!body.contains("alpha.log"), ".log.1 备份不进清单(D20): {body}");

        let res = get_raw(port, "/api/logs/alpha/tail?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "尾部应 200: {res}");
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"ok""#), "文件在就是 ok: {body}");
        for line in ["LINE-0001", "LINE-0002", "LINE-0003"] {
            assert!(body.contains(line), "应原样给出该行({line}): {body}");
        }
    }

    /// SC-007 / D14: 三个日志端点**逐个**过一遍 —— 无 token / 错 token 一律 `401` 且响应体为空、
    /// **不含任何日志内容**(日志里种了可辨识字样, 泄漏断言才成立)。
    #[tokio::test]
    async fn test_log_endpoints_require_token_and_never_leak_lines() {
        const SECRET: &str = "LOG-SECRET-LINE";

        let root = trade_root("logs-auth");
        write_log(&root, "demo", &[SECRET.to_string()]);
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_trade_test(root, db).await;

        let paths = ["/api/logs", "/api/logs/demo/tail", "/api/logs/demo/stream"];
        for path in paths {
            for probe in [path.to_string(), format!("{path}?token=wrong")] {
                let res = get_raw(port, &probe).await;
                assert!(res.starts_with("HTTP/1.1 401"), "无/错 token 应 401: {probe} → {res}");
                assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe} → {res}");
                assert!(!res.contains(SECRET), "401 不得泄漏日志内容: {res}");
            }
        }
        // 对 token: 才 `200`(证明上面拦住的不是"路由不存在")。
        for path in ["/api/logs", "/api/logs/demo/tail"] {
            let res = get_raw(port, &format!("{path}?token=tok-ok")).await;
            assert!(res.starts_with("HTTP/1.1 200"), "对 token 应 200: {path} → {res}");
            assert!(body_of(&res).contains("demo"), "对 token 应给出该策略的日志: {res}");
        }
    }

    /// SC-008 / FR-015: 超大 `lines` 一律夹到上限 200(只给最近的行); 同时 `missing`(从没启动过)
    /// 与 `unreadable`(读不到) 必须**分开** —— FR-020: 读不到不许说成"没有"。
    #[tokio::test]
    async fn test_log_tail_clamps_lines_and_separates_missing_from_unreadable() {
        let root = trade_root("logs-clamp");
        let lines: Vec<String> = (1..=300).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "big", &lines);
        // 目录冒充日志文件 → 读它必然报错(非 NotFound) = "读不到"的实证。
        std::fs::create_dir_all(crate::supervisor::ledger::logs_dir(&root).join("locked.log"))
            .expect("建同名目录");

        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_trade_test(root, db).await;

        let res = get_raw(port, "/api/logs/big/tail?lines=100000&token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"ok""#), "文件在就是 ok: {body}");
        assert!(
            body.contains("LINE-0101") && body.contains("LINE-0300"),
            "应给最近 200 行: {body}"
        );
        assert!(!body.contains("LINE-0100"), "超出上限的旧行不得给出(SC-008): {body}");
        assert!(!body.contains("LINE-0001"), "超出上限的旧行不得给出(SC-008): {body}");

        // 从没启动过 = 没有日志文件; 不是"读不到"。
        let res = get_raw(port, "/api/logs/ghost/tail?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"missing""#), "没文件应标 missing: {body}");
        assert!(body.contains(r#""lines":[]"#), "missing 时不给任何行: {body}");

        // 读不到: 如实说 + 带上实证原因, 不得说成"没有日志"。
        let res = get_raw(port, "/api/logs/locked/tail?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"unreadable""#), "读不到应标 unreadable: {body}");
        assert!(!body.contains(r#""reason":null"#), "必须带上读不到的原因: {body}");
        assert!(!body.contains(r#""reason":"""#), "原因不得为空串: {body}");
    }

    /// FR-016 / D2 / R5: 日志 SSE **独立端点** —— 首屏就把既有行推出来, 不必等新行、不必等进程。
    #[tokio::test]
    async fn test_log_stream_pushes_first_screen_without_a_process() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let root = trade_root("logs-stream");
        let lines: Vec<String> = (1..=2).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "live", &lines);
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_trade_test(root, db).await;

        let mut stream = tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
        let req =
            format!("GET /api/logs/live/stream?token=tok-ok HTTP/1.1\r\nHost: {BIND_ADDR}\r\n\r\n");
        stream.write_all(req.as_bytes()).await.expect("发出请求");

        // 流不会自行结束: 读到首屏两行即停, 用超时兜住"一条都不推"的情况。
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&buf).contains("LINE-0002") {
                let n = stream.read(&mut chunk).await.expect("读回响应");
                assert_ne!(n, 0, "流不应在首屏之前关闭");
                buf.extend_from_slice(&chunk[..n]);
            }
        })
        .await;
        assert!(read.is_ok(), "日志流应在首屏推出既有行");

        let text = String::from_utf8_lossy(&buf).to_string();
        assert!(text.starts_with("HTTP/1.1 200"), "流应 200: {text}");
        assert!(text.contains("text/event-stream"), "应为 SSE: {text}");
        assert!(text.contains("LINE-0001"), "首屏应给最近的行: {text}");
    }

    // ── 032 US2: 市场端点鉴权 + 参数硬失败 (FR-004 / FR-014; T015)──────────

    /// 三个市场端点的 401 矩阵 + 非法参数 400。
    ///
    /// **不发真实网络**: 401 在中间件层短路; 对 token 的用例全部走"参数非法 → 400"路径 —
    /// handler 严格按"先校验参数, 后取行情"排序, 400 在任何交易所请求之前返回,
    /// 既证明路由真实存在(不是 404), 又让单测离线可跑。
    #[tokio::test]
    async fn test_markets_endpoints_require_token_and_reject_bad_params_offline() {
        let root = trade_root("markets-auth");
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_trade_test(root, db).await;

        // ① 无 / 错 token: 三端点一律 401 且空体(带齐参数也会在中间件被拦, 不触达行情)。
        let guarded = [
            "/api/markets",
            "/api/markets/BTCUSDT/orderbook?market=spot&depth=20",
            "/api/markets/BTCUSDT/klines?market=spot&interval=1h&limit=200",
        ];
        for path in guarded {
            let sep = if path.contains('?') { "&" } else { "?" };
            for probe in [path.to_string(), format!("{path}{sep}token=wrong")] {
                let res = get_raw(port, &probe).await;
                assert!(res.starts_with("HTTP/1.1 401"), "无/错 token 应 401: {probe} → {res}");
                assert!(body_of(&res).is_empty(), "401 响应体必须为空: {probe}");
            }
        }

        // ② 对 token + 非法参数: 一律 400 中文硬失败, 且不联网(handler 内校验先于取数)。
        let bad_requests = [
            ("/api/markets?all=2&token=tok-ok", "all 只支持 0/1"),
            ("/api/markets?market=bogus&token=tok-ok", "market 只支持 spot / futures"),
            ("/api/markets/BTCUSDT/orderbook?token=tok-ok", "market 必填"),
            (
                "/api/markets/BTCUSDT/orderbook?market=fx&depth=20&token=tok-ok",
                "market 只支持 spot / futures",
            ),
            ("/api/markets/BTCUSDT/orderbook?market=spot&depth=99&token=tok-ok", "1..=50"),
            ("/api/markets/BTCUSDT/orderbook?market=spot&depth=0&token=tok-ok", "1..=50"),
            ("/api/markets/BTCUSDT/klines?token=tok-ok", "market 必填"),
            ("/api/markets/BTCUSDT/klines?market=spot&interval=2h&token=tok-ok", "interval"),
            ("/api/markets/BTCUSDT/klines?market=futures&limit=501&token=tok-ok", "1..=500"),
        ];
        for (path, expect) in bad_requests {
            let res = get_raw(port, path).await;
            assert!(res.starts_with("HTTP/1.1 400"), "{path} 应 400, 实际: {res}");
            let body = body_of(&res);
            assert!(body.contains(expect), "{path} 错误体应含「{expect}」: {body}");
        }

        // ③ /markets.js 静态资源对 token 可取(证明 401 拦的不是"路由不存在")。
        let res = get_raw(port, "/markets.js?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "对 token 应取到 markets.js: {res}");
    }
}
