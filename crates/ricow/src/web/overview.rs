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
    /// 064: 交易对视野规模(bStock 覆盖卡)。**取不到 = None**(显示"—", 不当 0)。
    pub scope: Option<ScopeCounts>,
    /// 064: 最近操作卡要的**最近几个会话**(按 `updated_at` 倒序, 最多 3 个)。
    ///
    /// **必须走这个聚合端点, 不许前端另发 `/api/sessions`** —— 概览页常驻占着
    /// 会话 SSE + 每 5 秒三路轮询, 浏览器同源连接上限只有 6, 再并发一个请求就会排队
    /// (062 实测: 四格一起显示"—"而 curl 毫秒级返回)。故这里一并给全。
    pub recent_sessions: Vec<RecentSession>,
    /// 064: 告警格要的**被门禁拦住的策略**(最多 3 个)。
    ///
    /// 空数组 = 现在没有需要处理的事(这是**正常状态**, 不是"读不到")。
    pub alerts: Vec<AlertItem>,
}

/// 064: **交易对视野规模** —— 概览页「bStock 覆盖」卡要它。
///
/// 这是 ricow 对 fmz 的**差异化卖点**: 币安官方的机器人功能只开放少数几个 stock,
/// 而 ricow 的视野判据(`market_class::bstock_spot_pool`)走 **fapi EQUITY base 白名单的
/// 全量枚举** —— 交易所上了哪个美股代币, ricow 就能跑哪个。
///
/// 取数走 `pairs::current_view(root, false)` —— **与 `GET /api/markets` 同源**,
/// 不新造第二套白名单(那必然漂移)。
///
/// `filtered` = 当前是否处于"仅股票类"的过滤视野(`[market].show_all_pairs = false`)。
/// 前端据此说"股票类视野"还是"全部交易对" —— 两个数字**不说明**口径, 必须靠这个标志。
#[derive(Debug, Serialize)]
pub(super) struct ScopeCounts {
    /// 视野内现货交易对数。
    pub spot: u64,
    /// 视野内合约交易对数。
    pub futures: u64,
    /// 是否处于"仅股票类"过滤视野(默认 true)。
    pub filtered: bool,
    /// 066: 现货那半**取到了没有**。`Some(false)` = 取不到 ⇒ 前端显示「—」,
    /// **不显示 0**(把"没数到"说成"0 个"是假绿)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spot_ok: Option<bool>,
    /// 066: 合约那半**取到了没有**。语义同 [`ScopeCounts::spot_ok`]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub futures_ok: Option<bool>,
}

/// 064: 概览页「最近操作」卡的一行 —— **一个会话**。
///
/// 为什么用会话当"最近操作": ricow 本地**真正持久化**的"人最近干了什么"只有两处:
/// ① `web_sessions`(会话, 带 `updated_at`);
/// ② `previews`(待确认的策略草稿, 带 `expires_at`)。
/// 回测作业**只留 5 分钟且不落库** —— 拿它当"最近操作", 用户 5 分钟后回来看就是空的,
/// 那是个会骗人的卡。故用会话: 它持久、有真实时间戳、且点进去真能接着干。
#[derive(Debug, Serialize)]
pub(super) struct RecentSession {
    /// 会话 id(前端据此跳 `#chat/{id}`)。
    pub id: String,
    /// 会话标题(**首条用户消息生成**, 可能为空 —— 空就是空, 不编个标题)。
    pub title: String,
    /// 最近一次活动的 Unix 秒(前端转相对时间)。
    pub updated_at: i64,
}

/// 064: 概览页「告警」格的一条 —— **一个被门禁拦住的策略**。
///
/// 数据源 = `catalog::all()` 里 `blocks_run()` 为真的条目: 未声明清单的裸 `.lua`
/// 且与内置脚本逐字相同(N1 冗余副本门禁)。这是 ricow 本地**真实的、需要用户动手的**事,
/// 不是凭空造的告警 —— 概览上出现它的意义就是"你有东西跑不起来, 点进去看"。
#[derive(Debug, Serialize)]
pub(super) struct AlertItem {
    /// 策略 id(前端据此跳 `#strategies/{id}`)。
    pub id: String,
    /// 它复制的那个内置 id(没有则空串)。
    pub duplicate_of: String,
}

/// `GET /api/overview` —— 一次拿全概览的四项计数。
///
/// **任何一项取不到都不影响其它项**(各自独立 `if let Ok`), 只把那项留 `None`。
/// 只有"全部取不到"才返回错误 —— 那说明服务本身有问题, 前端该显示错误态而不是四个「—」。
pub(super) async fn get_overview(
    State(state): State<WebState>,
) -> Result<Json<OverviewReply>, WebError> {
    // 五份并行取: 服务端并发没有浏览器那样的连接池压力。
    let (strategies, runs, keys, sessions, scope) = tokio::join!(
        super::strategies::list_strategies(State(state.clone())),
        super::runs::list_runs(State(state.clone())),
        super::keyring::get_keys(State(state.clone())),
        super::sessions::list_sessions(State(state.clone())),
        // 064: 视野规模。**与 /api/markets 同源**(同一个 current_view) —— 两边数字必须一致,
        // 否则用户会在概览看到一个数、市场视图看到另一个数, 那是两套白名单的典型症状。
        //
        // **带 5 秒超时**: 它要联网拉 exchangeInfo(币安客户端自身超时 30 秒),
        // 而概览端点的设计原则是"小到可以每次进页面都拉"。币安不通时若不等它,
        // 整页会卡到 30 秒后连四项计数都拿不到(真机实测: overview HTTP 000)。
        // 超时 → scope 为 None, 前端显示「—」并说明原因, 主计数照常可用。
        async {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                crate::commands::pairs::current_view(&state.root, false),
            )
            .await
            .unwrap_or_else(|_| Err(ricow_core::CoreError::Network("视野快照超时".to_string())))
        },
    );

    // 策略: 只要**用户自写**的那些, 内置示例另计。
    let strategy_count = strategies.0.iter().filter(|s| s.is_user()).count() as u64;

    // 密钥: AI 条目 + 交易所条目。**只数条目数**, 不看是否填了值 ——
    // "我配了几把钥匙"是条目事实, "能不能用"是另一回事(各视图另有生效摘要)。
    let key_count = keys.as_ref().ok().map(|v| v.entry_count() as u64).unwrap_or(0);

    // 064: 告警 = 被门禁拦住的策略(N1 冗余副本: 未声明清单且与内置逐字相同)。
    // 直接走 `catalog::all()`(与 `/api/strategies` 同一个目录源), 用**它自己的**
    // `blocks_run()` 判据 —— 不经由 `StrategyRow`(那层的字段是私有的, 且它多包了一层
    // Json)。代价是多扫一次目录; 收益是判据只有一处, 不会两边漂移。
    let alerts: Vec<AlertItem> = crate::strategies::catalog::all()
        .into_iter()
        .filter(|e| e.blocks_run())
        .take(3)
        .map(|e| AlertItem { id: e.manifest.id, duplicate_of: e.duplicate_of.unwrap_or_default() })
        .collect();

    // 064: 最近操作 = 最近的几个会话(updated_at 倒序, 取前 3)。
    // 取不到会话列表时给**空数组**而不是 None: 这张卡的含义是"最近没什么操作",
    // 与"读不到"是两件事 —— 但为了不让它拖累主计数, 这里宁空不报错。
    let recent_sessions: Vec<RecentSession> = sessions
        .as_ref()
        .ok()
        .map(|v| {
            let mut rows: Vec<&crate::web::sessions::SessionRow> = v.0.iter().collect();
            // 倒序(最近活动的在前): `Reverse` 包一下, clippy 认 `sort_by_key` 这一形。
            rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
            rows.into_iter()
                .take(3)
                .map(|r| RecentSession {
                    id: r.id.clone(),
                    title: r.title.clone(),
                    updated_at: r.updated_at,
                })
                .collect()
        })
        .unwrap_or_default();

    let reply = OverviewReply {
        strategies: Some(strategy_count),
        runs: runs.as_ref().ok().map(|v| v.0.len() as u64),
        runs_active: runs
            .as_ref()
            .ok()
            .map(|v| v.0.iter().filter(|r| r.is_running()).count() as u64),
        keys: Some(key_count),
        sessions: sessions.ok().map(|v| v.0.len() as u64),
        // 视野取不到(交易所快照拉不到) → None; 前端显示"—"而不是 0。
        // **不因为视野取不到就整份报错** —— 四项计数仍然有效, 概览照样能看。
        scope: scope.ok().map(|v| ScopeCounts {
            spot: v.spot.len() as u64,
            futures: v.futures.len() as u64,
            filtered: v.filtered,
            // 066: 两半的可用性一起带出。**降级时那一半的列表是空的** ——
            // 不带这个标志, 前端会把空列表显示成"0 个"(假信息)。
            spot_ok: v.spot_ok,
            futures_ok: v.futures_ok,
        }),
        recent_sessions,
        alerts,
    };

    // 主计数全都取不到 => 服务/依赖确实有问题, 如实报错(前端显示错误态 + 重试按钮)。
    // **scope 不参与这个判据**: 它是附加信息(交易所快照拉不到很常见, 比如离线),
    // 为它把整页打成错误态, 会让"明明计数都读到了"的用户白看一次错误。
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
    use crate::web::test_support::{body_of, get_raw, serve_test, tmp_root};

    /// 062: `/api/overview` —— 一次拿全四项计数, 且**每项都能是 0**(不是"没取到")。
    ///
    /// 为什么单独钉一条: 概览页把"0"与"—"(取不到)严格区分 —— 那个区分**只在前端**,
    /// 后端这边若某项错了(比如把取不到当 0), 前端看不出来, 只能靠这条断言拦。
    #[tokio::test]
    async fn test_overview_returns_all_four_counts() {
        // 064: 这里原先有本地的 `raw`/`get` 两个闭包, 后来改用共享的 `get_raw` ——
        // 闭包就没人用了, clippy 报 dead_code。**删掉, 别留着**: 死代码会让
        // "这个测试到底走哪条 HTTP 路径"变得不确定(后来者可能改回本地版本)。
        let root = tmp_root("overview");
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        let res = get_raw(port, "/api/overview?token=tok-ok").await;
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

    /// 064: `/api/overview` 的响应体里**恒有** `recent_sessions` 与 `alerts` 两个数组字段
    /// (以及 `scope`, 取不到为 null)。
    ///
    /// 为什么钉"恒有": 前端按 `d.recent_sessions || []` 取, 字段缺失时静默走空态 ——
    /// 于是"后端忘了给这个字段"这件事**在前端看不出来**（卡永远是空的, 看着像"真没数据"）。
    /// 与 062 那条"每项都能是 0"同一用意: 把只在前端做的区分, 在后端也拦一道。
    #[tokio::test]
    async fn test_overview_always_carries_activity_and_alert_arrays() {
        let root = tmp_root("ov-064-arrays");
        let db = ricow_strategy::Database::open_in_memory().await.expect("开内存库");
        let port = serve_test(root.clone(), db).await;

        let res = get_raw(port, "/api/overview?token=tok-ok").await;
        // 401 之外的正常响应(干净 root 上四项计数都是 0, 但字段须在)。
        let body = body_of(&res);
        for key in [
            "\"recent_sessions\"",
            "\"alerts\"",
            "\"scope\"",
            "\"strategies\"",
            "\"runs\"",
            "\"sessions\"",
        ] {
            assert!(body.contains(key), "overview 响应须含 {key}: {body}");
        }
        // 两个数组字段必须是数组(不是 null)—— 前端不判 null, 会当空列表用。
        assert!(body.contains("\"recent_sessions\":["), "recent_sessions 须是数组: {body}");
        assert!(body.contains("\"alerts\":["), "alerts 须是数组: {body}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
