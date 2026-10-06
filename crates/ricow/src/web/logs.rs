//! 策略日志端点(026 FR-015 ~ FR-018; D2 / D12 / D13 / D14)。
//!
//! 与 025 的会话 SSE **各走各的**(R5): 独立端点、独立流, 不共用会话 `Hub` 的任何通道 ——
//! 会话链路一行不动。这一侧只**读** `logs/<name>.log`(由 `supervisor::procs` 写),
//! 不写、不改、不轮转它; 策略**没在跑**照样能看历史日志(FR-017)。

use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};

use super::settings::current_lang;
use super::tail;
use super::WebError;
use super::WebState;
use crate::i18n::{t, Lang};

/// 尾读的缺省条数: 与既有 AI `logs_tail` 同口径(D12)。
const LOG_DEFAULT_LINES: i64 = 50;

/// SSE 轮询间隔(D12): 服务端尾读驱动 —— 新行在下一轮里推出。
const LOG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// 断线重连的退避起点(SSE `retry:` 字段, 036): 浏览器默认 ~3s, 这里给个更贴合的 2s。
const LOG_SSE_RETRY: std::time::Duration = std::time::Duration::from_millis(2_000);

/// 断线续传用的请求头(SSE 规范): 浏览器重连带上一帧的 `id`, 据此接着读。
const LAST_EVENT_ID: &str = "last-event-id";

/// 日志端点的查询参数。
#[derive(serde::Deserialize)]
struct LogQuery {
    /// 打开时先给最近多少行: 缺省 50, 一律夹到 `1..=200`(D12 / FR-015 / SC-008)。
    lines: Option<i64>,
    /// 续传位置(036): 前端统一 SSE 打开器重开的是**新** `EventSource` 对象, 浏览器
    /// 不会自动带 `Last-Event-ID` 头, 故以查询参数续传 —— 与头同义, 头优先。
    last_event_id: Option<u64>,
}

impl LogQuery {
    fn lines(&self) -> usize {
        let max = tail::TAIL_MAX_LINES as i64;
        self.lines.unwrap_or(LOG_DEFAULT_LINES).clamp(1, max) as usize
    }
}

/// 策略名 → 日志文件路径 (审计 安全-12): 收敛到 `ricow_strategy::validate_strategy_name()`
/// 白名单 (`[A-Za-z0-9_-]`、≤24 字符), 与 AI 工具 `safe_strategy_name` / 写盘入口同口径。
/// 此前自维护的黑名单不拦 `:`, Windows 上 `name="C:x"` 经 `Path::join` 会替换整个路径,
/// 读到 C 盘任意 `.log` 文件(`logs_tail` 是免审批 L0 工具, 可被 prompt 注入操纵)。
/// 非法 → `None`(端点按 404 处理)。
fn log_path_of(root: &Path, name: &str) -> Option<PathBuf> {
    ricow_strategy::validate_strategy_name(name)
        .ok()
        .map(|_| crate::supervisor::ledger::log_path(root, name))
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
    let Ok(entries) = std::fs::read_dir(crate::supervisor::ledger::logs_dir(root)) else {
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
            //
            // 注意: `supervisor::procs::rotate_large_log` 用的是"保留末尾窗口"而不是
            // 直接截空, 所以它**不会**让文件变短、不会把这里推成 rotated; 走到这里的是
            // 真·截断/清空(人工操作或异常)。
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

/// 组装一帧: `data` 是原样 JSON, `id` = **文件字节 offset**(续传游标), `retry` = 重连退避起点。
fn log_event_frame(data: String, offset: u64) -> Event {
    Event::default().id(offset.to_string()).retry(LOG_SSE_RETRY).data(data)
}

/// 日志实时流(FR-016 / D2; 断线续传 036): 首屏 = 最近 `lines` 行, 之后每 500ms 尾读一次
/// 增量(D12); 页面刷新重连即重建。
///
/// **续传与去重(036)**: 每帧的 `id` 就是该帧读完之后文件里的字节 offset。浏览器
/// `EventSource` 重连时会自动把最后一帧的 `id` 放进 `Last-Event-ID` 请求头 —— 我们据此
/// **从断点接着读**, 而不是从 offset 0 重来, 所以重连既不丢行也不重发已给过的行。
/// (重连与退避本身由浏览器 `EventSource` 负责, `retry:` 字段只调它的起始等待。)
async fn log_stream(
    State(state): State<WebState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<LogQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, WebError> {
    let path = log_path_of(&state.root, &name)
        .ok_or_else(|| WebError::bad_request(format!("非法策略名: {name}")))?;
    let lang = current_lang(&state.root).unwrap_or(Lang::Zh);
    let resume = headers
        .get(LAST_EVENT_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or(q.last_event_id);
    let mut tail = LogTail { path, offset: resume.unwrap_or(0), lines: q.lines(), lang };
    // 首屏在返回响应前读一次: 读不到就**当场**给提示帧, 而不是开一条永远不说话的流。
    let head = match tail.poll() {
        Ok(events) => events,
        Err(reason) => tail.unreadable(reason),
    };
    let head_id = tail.offset;
    let stream = futures::stream::unfold(
        (tail, head.into_iter(), head_id),
        |(mut tail, mut pending, mut id)| async move {
            loop {
                if let Some(event) = pending.next() {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    return Some((Ok(log_event_frame(data, id)), (tail, pending, id)));
                }
                tokio::time::sleep(LOG_POLL_INTERVAL).await;
                pending = match tail.poll() {
                    Ok(events) => events.into_iter(),
                    Err(reason) => tail.unreadable(reason).into_iter(),
                };
                id = tail.offset;
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// 本模块负责的日志路由(挂进 [`super::router`])。
pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/logs", get(list_logs))
        .route("/api/logs/{name}/tail", get(log_tail))
        .route("/api/logs/{name}/stream", get(log_stream))
}

#[cfg(test)]
mod tests {
    use ricow_strategy::Database;

    use crate::web::test_support::{body_of, get_raw, serve_test, tmp_root, write_log};

    /// FR-017 / FR-018: 清单与尾部**只认文件** —— 没有 daemon、没有台账, 一样列得出、看得到;
    /// 非 `.log` 与轮转备份 `.log.1` 不进清单(D20)。
    #[tokio::test]
    async fn test_log_listing_and_tail_need_no_running_strategy() {
        let root = tmp_root("logs-list");
        let lines: Vec<String> = (1..=3).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "alpha", &lines);
        write_log(&root, "beta", &["ONLY-ONE".to_string()]);
        // 干扰项: 非日志文件、轮转备份 —— 都不该出现在清单里。
        std::fs::write(crate::supervisor::ledger::logs_dir(&root).join("notes.txt"), "x")
            .expect("写干扰文件");
        std::fs::write(crate::supervisor::ledger::logs_dir(&root).join("alpha.log.1"), "x")
            .expect("写轮转备份");

        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root, db).await;

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

        let root = tmp_root("logs-auth");
        write_log(&root, "demo", &[SECRET.to_string()]);
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root, db).await;

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
        let root = tmp_root("logs-clamp");
        let lines: Vec<String> = (1..=300).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "big", &lines);
        // 目录冒充日志文件 → 读它必然报错(非 NotFound) = "读不到"的实证。
        std::fs::create_dir_all(crate::supervisor::ledger::logs_dir(&root).join("locked.log"))
            .expect("建同名目录");

        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root, db).await;

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

        let root = tmp_root("logs-stream");
        let lines: Vec<String> = (1..=2).map(|i| format!("LINE-{i:04}")).collect();
        write_log(&root, "live", &lines);
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root, db).await;

        let mut stream =
            tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port)).await.expect("连上服务");
        let req = format!(
            "GET /api/logs/live/stream?token=tok-ok HTTP/1.1\r\nHost: {}\r\n\r\n",
            crate::web::BIND_ADDR
        );
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

    /// 036 断线续传: 每帧带 `id`(文件 offset); 重连带 `Last-Event-ID` 时**从断点接着读** ——
    /// 既不重发已给过的行(去重), 也不漏掉断线期间写进来的行。
    #[tokio::test]
    async fn test_log_stream_resumes_from_last_event_id_without_duplicates() {
        use std::io::Write;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        /// 从 SSE 响应文本里取最后一帧的 `id:` 值。
        fn last_id(text: &str) -> u64 {
            text.lines()
                .filter_map(|l| l.strip_prefix("id: "))
                .filter_map(|v| v.trim().parse::<u64>().ok())
                .next_back()
                .unwrap_or_else(|| panic!("响应里应带 id 帧: {text}"))
        }

        let root = tmp_root("logs-resume");
        write_log(&root, "live", &["LINE-0001".to_string()]);
        let db = Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        // 第一次连接: 首屏给 LINE-0001, 并带上续传游标。
        let mut first =
            tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port)).await.expect("连上服务");
        let req = format!(
            "GET /api/logs/live/stream?token=tok-ok HTTP/1.1\r\nHost: {}\r\n\r\n",
            crate::web::BIND_ADDR
        );
        first.write_all(req.as_bytes()).await.expect("发出请求");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&buf).contains("LINE-0001") {
                let n = first.read(&mut chunk).await.expect("读回响应");
                assert_ne!(n, 0, "流不应提前关闭");
                buf.extend_from_slice(&chunk[..n]);
            }
        })
        .await
        .expect("首屏应推出来");
        let text = String::from_utf8_lossy(&buf).to_string();
        let offset = last_id(&text);
        assert_eq!(offset, "LINE-0001\n".len() as u64, "id 应是读完首屏后的文件字节 offset");
        drop(first);

        // 断线期间写进两行。
        let path = crate::supervisor::ledger::log_path(&root, "live");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).expect("打开追加");
        f.write_all(b"LINE-0002\nLINE-0003\n").expect("追加");
        drop(f);

        // 重连(带 Last-Event-ID): 只应给断线期间的新行, 不重发 LINE-0001。
        let mut again =
            tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port)).await.expect("连上服务");
        let req = format!(
            "GET /api/logs/live/stream?token=tok-ok HTTP/1.1\r\nHost: {}\r\n\
             Last-Event-ID: {offset}\r\n\r\n",
            crate::web::BIND_ADDR
        );
        again.write_all(req.as_bytes()).await.expect("发出重连请求");
        let mut buf2 = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&buf2).contains("LINE-0003") {
                let n = again.read(&mut chunk).await.expect("读回响应");
                assert_ne!(n, 0, "流不应提前关闭");
                buf2.extend_from_slice(&chunk[..n]);
            }
        })
        .await
        .expect("增量应推出来");
        let text2 = String::from_utf8_lossy(&buf2).to_string();
        assert!(text2.contains("LINE-0002") && text2.contains("LINE-0003"), "新行不得漏: {text2}");
        assert!(!text2.contains("LINE-0001"), "已给过的行不得重发(去重): {text2}");
    }
}
