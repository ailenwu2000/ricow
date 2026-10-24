//! 概览聚合端点(062): 一次请求返回概览页要的全部**计数**。
//!
//! ── 为什么要有这个端点 ────────────────────────────────────────────────────────
//! 概览页(前端 `dashboard.js`)要四个数字: 策略数 / 运行实例数 / 密钥数 / 会话数。
//! 早先它发**四个** `/api/*` 请求 —— 但浏览器的同源 HTTP/1.1 **连接数上限是 6**,
//! 而本页面**长期占着好几个**: 会话 SSE 一条、交易面板每 5 秒轮询一次且**一次并发 3 路**
//! (`/api/trades/positions|orders|fills`)。于是概览的请求要排队, 排不到就超时 ——
//! 实测现象是概览四格一起显示「—」(读不到), 而同样的接口用 curl 打是**毫秒级**返回
//! (curl 不占浏览器的连接池)。
//!
//! 聚合端点把「四个并发请求」变成「一个」: 浏览器只占一个连接位, 服务端内部并行取四份。
//! 服务端没有浏览器的连接池限制, 并行取没有代价。
//!
//! ── 只给计数, 不给明细 ─────────────────────────────────────────────────────────
//! 概览页要的是「一眼看到现状」, 不是明细。明细各视图自己取(且已有各自的缓存/轮询策略),
//! 这里再给一遍只会白白增大响应体 —— 概览的响应要小到可以每次进页面都拉。

use axum::extract::State;
use axum::routing::get;
use axum::Json;
use serde::Serialize;

use super::{WebError, WebState};

/// 概览页要的四项计数。
///
/// `Option<u64>` 的含义是「**取不到**」—— 与 `Some(0)`("确实是 0 个")**必须能区分**。
/// 前端据此显示"—"而不是 0: 把"没数到"说成"0 个"是假绿(见 062 的假绿排查清单)。
#[derive(Debug, Serialize)]
pub(super) struct OverviewReply {
    /// 用户自写的策略数(内置示例另计 —— 混在一起会让人以为内置的也是自己写的)。
    pub strategies: Option<u64>,
    /// 运行实例总数。
    pub runs: Option<u64>,
    /// **正在运行**的实例数(概览页的"运行状态"卡要它)。
    pub runs_active: Option<u64>,
    /// 密钥条目数(AI 条目 + 交易所条目)。
    pub keys: Option<u64>,
    /// 会话数。
    pub sessions: Option<u64>,
}

/// `GET /api/overview` —— 一次拿全概览的四项计数。
///
/// **任何一项取不到都不影响其它项**(各自独立 `if let Ok`), 只把那项留 `None`。
/// 只有"全部取不到"才返回错误 —— 那说明服务本身有问题, 前端该显示错误态而不是四个「—」。
pub(super) async fn get_overview(
    State(state): State<WebState>,
) -> Result<Json<OverviewReply>, WebError> {
    // 四份并行取: 服务端并发没有浏览器那样的连接池压力。
    let (strategies, runs, keys, sessions) = tokio::join!(
        super::strategies::list_strategies(State(state.clone())),
        super::runs::list_runs(State(state.clone())),
        super::keyring::get_keys(State(state.clone())),
        super::sessions::list_sessions(State(state.clone())),
    );

    // 策略: 只要**用户自写**的那些, 内置示例另计。
    let strategy_count = strategies.0.iter().filter(|s| s.is_user()).count() as u64;

    // 密钥: AI 条目 + 交易所条目。**只数条目数**, 不看是否填了值 ——
    // "我配了几把钥匙"是条目事实, "能不能用"是另一回事(各视图另有生效摘要)。
    let key_count = keys.as_ref().ok().map(|v| v.entry_count() as u64).unwrap_or(0);

    let reply = OverviewReply {
        strategies: Some(strategy_count),
        runs: runs.as_ref().ok().map(|v| v.0.len() as u64),
        runs_active: runs
            .as_ref()
            .ok()
            .map(|v| v.0.iter().filter(|r| r.is_running()).count() as u64),
        keys: Some(key_count),
        sessions: sessions.ok().map(|v| v.0.len() as u64),
    };

    // 全都取不到 => 服务/依赖确实有问题, 如实报错(前端显示错误态 + 重试按钮)。
    if reply.strategies.is_none()
        && reply.runs.is_none()
        && reply.keys.is_none()
        && reply.sessions.is_none()
    {
        return Err(WebError::bad_gateway("概览数据读取失败".to_string()));
    }
    Ok(Json(reply))
}

/// 本模块负责的设置路由(挂进 [`super::router`]。
pub(super) fn routes() -> axum::Router<WebState> {
    axum::Router::new().route("/api/overview", get(get_overview))
}

#[cfg(test)]
mod tests {
    use crate::web::test_support::{body_of, serve_test, tmp_root};

    /// 062: `/api/overview` —— 一次拿全四项计数, 且**每项都能是 0**(不是"没取到")。
    ///
    /// 为什么单独钉一条: 概览页把"0"与"—"(取不到)严格区分 —— 那个区分**只在前端**,
    /// 后端这边若某项错了(比如把取不到当 0), 前端看不出来, 只能靠这条断言拦。
    #[tokio::test]
    async fn test_overview_returns_all_four_counts() {
        async fn raw(port: u16, req: String) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut stream = tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port))
                .await
                .expect("连上服务");
            stream.write_all(req.as_bytes()).await.expect("发出请求");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_string()
        }
        async fn get(port: u16, path: &str) -> String {
            raw(
                port,
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
            )
            .await
        }

        let root = tmp_root("overview");
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        let res = get(port, "/api/overview?token=tok-ok").await;
        assert!(res.starts_with("HTTP/1.1 200"), "GET /api/overview 应 200, 实际: {res}");
        let body = body_of(&res);
        // 四个键都要在(缺一个前端就少一格)。
        for key in ["\"strategies\"", "\"runs\"", "\"runs_active\"", "\"keys\"", "\"sessions\""] {
            assert!(body.contains(key), "概览响应须含 {key}: {body}");
        }
        // 空 root 下: 策略/实例/密钥/会话都是 0 —— 但**是 0 不是 null**(确实是 0 个)。
        assert!(body.contains("\"strategies\":0"), "无策略时应是 0: {body}");
        assert!(body.contains("\"runs\":0"), "无实例时应是 0: {body}");
        assert!(body.contains("\"sessions\":0"), "无会话时应是 0: {body}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 062: `/api/overview` 与其它端点一样受 token 门禁保护(无 token 一律 401 空体)。
    #[tokio::test]
    async fn test_overview_requires_token() {
        async fn raw(port: u16, req: String) -> String {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let mut stream = tokio::net::TcpStream::connect((crate::web::BIND_ADDR, port))
                .await
                .expect("连上服务");
            stream.write_all(req.as_bytes()).await.expect("发出请求");
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.expect("读回响应");
            String::from_utf8_lossy(&buf).to_string()
        }

        let root = tmp_root("overview-noauth");
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        let res = raw(
            port,
            "GET /api/overview HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
                .to_string(),
        )
        .await;
        assert!(res.starts_with("HTTP/1.1 401"), "无 token 应 401, 实际: {res}");
        assert!(body_of(&res).trim().is_empty(), "401 响应体须为空(不回任何内容): {res}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
