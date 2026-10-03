//! 会话端点(025 FR-007 ~ FR-011): 列表 / 新建 / 删除 / 取消息 + 出站 SSE 帧流 + 入站一行。
//!
//! 与 CLI 同源: 会话逻辑全在 [`crate::ai::session`], 本模块只做 HTTP 与帧转发 ——
//! 线程里跑的仍是 [`crate::commands::chat::repl`], 本层不复制任何 LLM 调用路径 (D2 / D5)。

use axum::extract::{Path as UrlPath, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get};
use axum::{Json, Router};

use super::WebError;
use super::WebState;
use tokio::sync::broadcast;

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
/// 分流由服务端决定(见 [`super::sink::InputChannel::submit`]), 前端无需自报是密钥还是普通输入。
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

/// 本模块负责的会话路由(挂进 [`super::router`])。
pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/sessions/{id}/messages", get(session_messages))
        // 会话通道(FR-008 / FR-011): 出站 = SSE 帧流, 入站 = 把一行送进 REPL。
        .route("/api/sessions/{id}/events", get(session_events))
        .route("/api/sessions/{id}/input", axum::routing::post(session_input))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::ai::session::SessionSink as _;
    use crate::web::{bind, lock, serve, Starter, WebSink};

    /// FR-011 回归: 一次 `POST /input` **只投一行**。
    ///
    /// 页面开局就连 SSE(`/events` → `attach`), 所以走到 `session_input` 时会话**已经在跑** ——
    /// 曾经的写法在这条热路径上 `submit` 了两次(先判一次、末尾又无条件补一次), 每行输入都进通道
    /// 两遍: 一次输入顶两轮、两次 LLM 调用(2026-09-19 实机 DB: 12 条用户输入无一例外成对)。
    #[tokio::test]
    async fn test_input_posts_one_line_even_when_session_already_attached() {
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
        let store = crate::web::SessionStore::new(db.clone());
        let sid = store.create().await.expect("建会话");
        let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
        let hub = Arc::clone(&state.hub);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let _server = tokio::spawn(serve(listener, state));

        // 开局先连 SSE: 这一步会把会话 attach 上(热路径由此成立)。
        let mut sse =
            tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port)).await.expect("连上 SSE");
        let head = format!(
            "GET /api/sessions/{sid}/events?token=tok-ok HTTP/1.1\r\nHost: {}\r\n\r\n",
            crate::web::BIND_ADDR
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
        let mut post =
            tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port)).await.expect("连上服务");
        let req = format!(
            "POST /api/sessions/{sid}/input?token=tok-ok HTTP/1.1\r\nHost: {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            crate::web::BIND_ADDR,
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

    /// 边界: 未 attach 的会话直接 POST 入站 —— 会就地拉起线程并把这行补送进去, 不丢输入。
    #[tokio::test]
    async fn test_input_without_prior_sse_still_delivers_one_line() {
        use ricow_strategy::Database;

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let starter: Starter = {
            let seen = Arc::clone(&seen);
            Arc::new(move |_id: &str, sink: &mut WebSink| {
                while let Some(line) = sink.input_line("") {
                    lock(&seen).push(line);
                }
                Ok(())
            })
        };
        let root = std::env::temp_dir().join(format!("ricow-web-input2-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时数据目录");
        let db = Database::open_in_memory().await.expect("开内存库");
        let store = crate::web::SessionStore::new(db.clone());
        let sid = store.create().await.expect("建会话");
        let state = WebState::new("tok-ok".to_string(), root, db, store, starter);
        let hub = Arc::clone(&state.hub);
        let (listener, port) = bind(0).await.expect("绑定回环端口");
        let _server = tokio::spawn(serve(listener, state));

        let res = crate::web::test_support::post_raw(
            port,
            &format!("/api/sessions/{sid}/input?token=tok-ok"),
            r#"{"text":"你好"}"#,
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 200"), "入站应 200: {res}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        hub.stop(&sid);
        assert_eq!(*lock(&seen), vec!["你好".to_string()], "补送的那行不该丢");
    }
}
