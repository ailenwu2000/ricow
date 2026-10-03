//! 交易可见性端点(026 FR-010 ~ FR-014; D1 / D10 / D11 / D14)。
//!
//! 四个端点全部**只读**: 数据来自 [`WebState`] 的本地库(与引擎、会话存储同一份, D1),
//! 不触发任何交易动作 —— 撤单 / 平仓 / 停机仍走对话确认(D17 / FR-013)。
//! 空状态一律**如实**: 响应带 `source` 三态(D10), 停机快照带 `updated_at`(D11)。

use std::path::Path;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use ricow_strategy::{FillWithMode, OrderRecord, PnlSnapshotRecord, PositionRecord};

use super::WebState;
use crate::supervisor::{self, Source};

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

/// 本模块负责的交易可见性路由(挂进 [`super::router`])。
///
/// 全部**只读**且与既有端点同一道 token 门禁(D14 / FR-011)。
pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/api/trades/fills", get(trades_fills))
        .route("/api/trades/orders", get(trades_orders))
        .route("/api/trades/positions", get(trades_positions))
        .route("/api/trades/pnl", get(trades_pnl))
}

#[cfg(test)]
mod tests {
    use ricow_strategy::Database;

    use crate::web::test_support::{
        body_of, get_raw, seed_online_daemon, seed_trade_rows, serve_test, tmp_root,
    };

    /// SC-007 / FR-011: 四个交易端点**逐个**过一遍 —— 无 token / 错 token 一律 `401` 且
    /// 响应体**不含任何交易数据**; 带对 token 才 `200`。库里种了真数据, 泄漏断言才成立。
    #[tokio::test]
    async fn test_trade_endpoints_require_token_and_never_leak_rows() {
        /// 库里那行成交 / 订单 / 持仓的可辨识字样 —— 出现在 401 响应里就是泄漏。
        const MARKS: [&str; 2] = ["EX-SECRET", "BTCUSDT"];

        let root = tmp_root("trades-auth");
        let db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&db).await;
        let port = serve_test(root, db).await;

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
        let root = tmp_root("trades-unreadable");
        let db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&db).await;
        let closer = db.clone();
        let port = serve_test(root, db).await;
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
        let down_root = tmp_root("trades-down");
        let down_db = Database::open_in_memory().await.expect("开内存库");
        seed_trade_rows(&down_db).await;
        let down_port = serve_test(down_root, down_db).await;

        let res = get_raw(down_port, "/api/trades/positions?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"daemon_down""#), "daemon 不在须标 daemon_down: {body}");
        assert!(body.contains("BTCUSDT"), "最后一次快照仍应给出: {body}");
        assert!(!body.contains(r#""updated_at":null"#), "快照必须带'截至 <时间>'的落点: {body}");

        // ② daemon 在线 + 库确实空。
        let up_root = tmp_root("trades-up");
        seed_online_daemon(&up_root).await;
        let up_db = Database::open_in_memory().await.expect("开内存库");
        let up_port = serve_test(up_root, up_db).await;

        let res = get_raw(up_port, "/api/trades/positions?token=tok-ok").await;
        let body = body_of(&res).to_string();
        assert!(body.contains(r#""source":"ok""#), "daemon 在线应为 ok: {body}");
        assert!(body.contains(r#""updated_at":null"#), "真空时没有'截至'可标: {body}");
        assert!(body.contains(r#""items":[]"#), "确认真空时才说'没有': {body}");
    }
}
