//! Web UI 服务端 (025): axum + SSE, 只绑 `127.0.0.1`, 一次性 token 鉴权。
//!
//! 与 CLI 同源: 会话逻辑全在 [`crate::ai::session`], 本模块只提供 HTTP 骨架、
//! 会话线程登记与帧转发 —— 线程里跑的仍是 [`crate::commands::chat::repl`],
//! 助手增量仍由 `provider::ask_stream` 逐段产出, 本层不复制任何 LLM 调用路径 (D2 / D5)。
//!
//! 036 起按职责拆文件: 各端点族的 handler / 路由 / 测试随各自子模块走, 本文件只留
//! 服务骨架(`bind` / `router` / `serve` / `Hub` / `WebState` / `WebError`)与端点装配。

mod assets;
mod auth;
mod backtest_jobs;
mod keyring;
mod keys;
mod logs;
mod markets;
mod runs;
mod sessions;
mod settings;
mod sink;
mod store;
mod strategies;
mod strategy_io;
mod tail;
mod terms;
#[cfg(test)]
mod test_support;
mod trades;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::ai::session::SessionSink;
use crate::i18n::{t, Lang};

use settings::current_lang;

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
///
/// 036 起各端点族自带 `routes()`(`sessions` / `strategies` / `settings` / `trades` / `logs`),
/// 这里只做装配 —— 静态资源与仍属本文件的散点(`ping` / `terms` / 其余子模块)单独挂。
pub fn router(state: WebState) -> Router {
    Router::new()
        // 单页与静态资源(FR-003 / FR-006): 页面本身也要 token, 资源 URL 里的 token 由 `index` 填。
        .route("/", get(assets::index))
        .route("/style.css", get(assets::style_css))
        // 032: 脚本拆分后的五份静态资源(含第三方图表库), 同样挂在 token 中间件之后。
        .route("/lightweight-charts.js", get(assets::lwc_js))
        .route("/common.js", get(assets::common_js))
        .route("/router.js", get(assets::router_js))
        .route("/chat.js", get(assets::chat_js))
        .route("/settings.js", get(assets::settings_js))
        .route("/keys.js", get(assets::keys_js))
        .route("/markets.js", get(assets::markets_js))
        .route("/strategies.js", get(assets::strategies_js))
        .route("/runs.js", get(assets::runs_js))
        .route("/app.js", get(assets::app_js))
        .route("/api/ping", get(ping))
        // 会话端点族(FR-007 ~ FR-011): CRUD + 出入站通道。
        .merge(sessions::routes())
        // 术语解释(FR-024): 静态内置数据, 只读。
        .route("/api/terms", get(list_terms))
        // 语言 / 主题读写(FR-029 / 034): 与 CLI 共用 `[ui].lang` / `[ui].theme`。
        .merge(settings::routes())
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
        .merge(strategies::routes())
        // 策略源码读取与页面保存(032 US3 / FR-015 ~ FR-017): 读 Lua/实例 TOML 原文;
        // POST 保存走命名校验链 → 编译门禁 → 引擎同一落盘内核(免 preview, FR-028)。
        .route("/api/strategies", axum::routing::post(strategy_io::save_strategy))
        .route("/api/strategies/{id}/source", get(strategy_io::get_strategy_source))
        .route("/api/strategies/{id}/ai-edit", axum::routing::post(strategy_io::ai_edit_strategy))
        // 用户策略清单(manifest)编辑(P2-8): 声明/维护参数 schema, 点亮参数表单。
        .route(
            "/api/strategies/{id}/manifest",
            axum::routing::post(strategy_io::save_strategy_manifest),
        )
        // AI 从零生成策略(P0-3): 结构化意图 → AI 参照 lua-api 规范生成 → 编译门禁, 不落盘。
        .route(
            "/api/strategies/ai-generate",
            axum::routing::post(strategy_io::ai_generate_strategy),
        )
        // 回测异步作业 (032 US3 / FR-019): POST 202 拿 job_id 后台跑 CLI 同一内核, GET 轮询结果。
        // 参数寻优 (P2-7): 单参数网格扫描, 作业生命周期与轮询端点复用。
        .route("/api/backtest/sweep", axum::routing::post(backtest_jobs::start_sweep))
        .route("/api/backtest", axum::routing::post(backtest_jobs::start_backtest))
        .route("/api/backtest/{job_id}", get(backtest_jobs::get_backtest))
        // 交易面板数据(026 FR-010 / FR-012): 全部**只读**, 数据来自与引擎同一份本地库(D1)。
        .merge(trades::routes())
        // 策略日志(026 FR-015 ~ FR-018): 只读文件, 策略未运行也能看历史。
        .merge(logs::routes())
        // 运行控制(032 US4 / FR-022 ~ FR-026): 运行态一览 / 单实例状态 / 拉起 / 停机 / 风险确认。
        // 启停复用 CLI 同一 ctrl 内核, daemon 自举走幂等的 ensure_daemon —— 本层零控制逻辑复制。
        .route("/api/runs", get(runs::list_runs))
        .route("/api/strategies/{id}/status", get(runs::get_status))
        .route("/api/strategies/{id}/start", axum::routing::post(runs::start_strategy))
        .route("/api/strategies/{id}/stop", axum::routing::post(runs::stop_strategy))
        .route("/api/risk-ack", axum::routing::post(runs::post_risk_ack))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_token))
        // 安全响应头 (审计 低危 #2): 挂在**最外层**, 保证 401/403/404 等早退响应也带上加固头。
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// 安全响应头 (审计 低危 #2): 全部响应统一注入。
///
/// 这是纯纵深加固 —— 服务只绑回环、页面无第三方内容, 不是"堵某个已证实的洞":
/// - `X-Content-Type-Options: nosniff` —— 禁内容嗅探, 静态资源不会被当成 HTML 执行;
/// - `Referrer-Policy: no-referrer` —— 页面导航外跳时不带 Referer。**关键**: token 会出现在
///   URL 查询串里, 若 Referer 外流, token 就跟着漏(与"导航 302 去 token"是同一威胁面);
/// - `X-Frame-Options: DENY` + CSP `frame-ancestors 'none'` —— 禁被 iframe 嵌套(点击劫持);
/// - `Content-Security-Policy` —— 限死脚本/样式/连接来源为 `'self'`. 前端只加载同源脚本与
///   `lightweight-charts.js`, SSE 走同源 `EventSource`, 故 `default-src 'self'` 足够;
///   `style-src` 放宽到 `'unsafe-inline'` 是因为图表库与主题切换会写内联 style 属性
///   (属性写法不受 `style-src` 的 `'unsafe-inline'` 约束, 但内联 `<style>` 需要它)。
async fn security_headers(req: axum::extract::Request, next: middleware::Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    for (name, value) in [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        // 标准头 + 旧名一并给(FrameOptions 已废弃但旧浏览器仍认)。
        (header::X_FRAME_OPTIONS, "DENY"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; connect-src 'self'; font-src 'self'; \
             object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    ] {
        h.insert(name, header::HeaderValue::from_static(value));
    }
    resp
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

/// 就绪探针: 无会话内容, 供前端确认「服务在监听且 token 有效」。
async fn ping() -> &'static str {
    "ok"
}

/// 术语解释(FR-024 / FR-026): 编译期常量, 中英两份一次给全 —— 前端切换语言无需重取,
/// 服务端也不必为它记语言状态。
async fn list_terms() -> Json<Vec<&'static terms::Term>> {
    Json(terms::all())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use ricow_strategy::Database;
    use tokio::sync::broadcast;

    use super::*;
    use crate::commands::config_file::{self, SetValue};

    /// 取 HTTP 响应体(首个空行之后); 供鉴权用例断言「响应体为空 / 不含会话内容」。
    fn body_of(res: &str) -> &str {
        res.split_once("\r\n\r\n").map_or("", |(_, body)| body)
    }

    #[tokio::test]
    async fn test_bind_default_port_is_loopback() {
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        assert!(port > 0, "port=0 应由系统分配实际端口");
        let addr = listener.local_addr().expect("读取监听地址");
        assert!(addr.ip().is_loopback(), "服务只应绑回环地址, 实际 {addr}");
        assert_eq!(addr.port(), port);
    }

    /// SC-005 / FR-002: 无 token 与错 token 一律 `401` 且**响应体不携带任何会话内容**;
    /// 带对 token 才拿得到响应。走真实监听套接字 —— 中间件、路由、响应体一体验证。
    #[tokio::test]
    async fn test_missing_or_wrong_token_is_401_without_leaking_content() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    /// 审计 低危 #2: 全部响应都带安全头, **包括早退的 401** —— 所以中间件必须挂在最外层。
    #[tokio::test]
    async fn test_security_headers_present_on_200_and_401() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn get(port: u16, path: &str) -> String {
            let mut stream =
                tokio::net::TcpStream::connect((BIND_ADDR, port)).await.expect("连上服务");
            let req =
                format!("GET {path} HTTP/1.1\r\nHost: {BIND_ADDR}\r\nConnection: close\r\n\r\n");
            stream.write_all(req.as_bytes()).await.expect("发出请求");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_ascii_lowercase()
        }

        let root = std::env::temp_dir().join(format!("ricow-web-sechdr-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时数据目录");
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = SessionStore::new(db.clone());
        let starter: Starter = Arc::new(|_id: &str, _sink: &mut WebSink| Ok(()));
        let state = WebState::new("tok-sec".to_string(), root, db, store, starter);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let _server = tokio::spawn(serve(listener, state));

        // 200 与 401 两条路径都必须带齐四类加固头。
        for (path, want_status) in [("/api/ping?token=tok-sec", "200"), ("/api/ping", "401")] {
            let res = get(port, path).await;
            // 注意: `get` 已把整段响应小写化(便于比头名), 状态行也要按小写比。
            assert!(
                res.starts_with(&format!("http/1.1 {want_status}")),
                "{path}: 期望 {want_status}, 实际: {res}"
            );
            assert!(res.contains("x-content-type-options: nosniff"), "{path} 缺 nosniff: {res}");
            assert!(
                res.contains("referrer-policy: no-referrer"),
                "{path} 缺 Referrer-Policy: {res}"
            );
            assert!(res.contains("x-frame-options: deny"), "{path} 缺 X-Frame-Options: {res}");
            assert!(
                res.contains("content-security-policy: default-src 'self'"),
                "{path} 缺 CSP: {res}"
            );
            assert!(res.contains("frame-ancestors 'none'"), "{path} CSP 应禁嵌套: {res}");
        }
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

    // ── 032 US2: 市场端点鉴权 + 参数硬失败 (FR-004 / FR-014; T015)──────────

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
