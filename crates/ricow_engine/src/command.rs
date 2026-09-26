//! 命令分发: 组合 loader / market / backtest / keys 完成引擎动作。

use std::collections::HashMap;
use std::sync::Arc;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use futures::{Stream, StreamExt};
use ricow_core::{
    Balance, CoreError, CoreResult, Exchange, Kline, Market, OrderAck, OrderFill, OrderRequest,
    OrderSide, OrderStatus, OrderUpdate, Position, UserEvent,
};
use ricow_strategy::{
    is_owned, ConfigValue, Context, Database, DryRunContext, LiveContext, OrderRecord,
    PnlSnapshotRecord, PnlTracker, PositionRecord, Strategy, StrategyConfig,
};
use rust_decimal::Decimal;

use crate::backtest_runner::run_backtest;
use crate::confirm::create_preview;
use crate::live::{plan_cleanup, residual_owned, CleanupOutcome, OnceGate};
use crate::loader::load_strategy;
use crate::market;
use crate::notify::{Notifier, NotifyEvent};

/// 引擎入口 — 无状态命令分发。
pub struct Engine;

/// 停机原因 (由调用方注入的信号决定; 用于日志与实例台账)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// 收到停机指令 (`ricow stop` / daemon 下发)
    Requested,
    /// 管理器已不在 (stdin 管道 EOF) —— 自愈路径
    ManagerGone,
    /// 前台调试被中断 (Ctrl-C)
    Interrupted,
    /// 行情流结束 (交易所连接断开, 视为异常)
    StreamEnded,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StopReason::Requested => "停机指令",
            StopReason::ManagerGone => "管理器已退出 (管道 EOF)",
            StopReason::Interrupted => "用户中断 (Ctrl-C)",
            StopReason::StreamEnded => "行情流中断",
        };
        write!(f, "{s}")
    }
}

/// 停机请求 (来自 stdin 指令 / daemon 下发 / Ctrl-C)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopRequest {
    /// 停机原因 (None = 尚未请求)
    pub reason: Option<StopReason>,
    /// 是否在停机清理时平掉策略持仓 (`stop --close-all`)
    pub close_all: bool,
}

/// 停机信号接收端: `Some(req)` 表示请求停机。
pub type StopSignal = tokio::sync::watch::Receiver<Option<StopRequest>>;

/// 引擎运行模式 (026 FR-002/D5)。
///
/// `as_str()` 是**落库口径** (`orders.mode` / `positions.mode`): 稳定短串, 与界面文案解耦;
/// `label()` 是**面向用户的标签**, 与 CLI `Mode::label()` 逐字一致 (日志文案零变化)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    DryRun,
    Demo,
    Live,
}

impl RunMode {
    /// 落库口径。改动它会打断既有数据, 不得随文案调整。
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::DryRun => "dry_run",
            RunMode::Demo => "demo",
            RunMode::Live => "live",
        }
    }

    /// 面向用户的模式名 (打印时必须如实, 绝不把 demo 说成实盘)。
    pub fn label(self) -> &'static str {
        match self {
            RunMode::DryRun => "dry run",
            RunMode::Demo => "测试网模拟盘(demo)",
            RunMode::Live => "实盘",
        }
    }
}

/// 停机清理后吸干用户流的窗口: 兜底平仓的成交只能靠用户流送达 (少了它会漏记成交)。
const CLEANUP_DRAIN_WINDOW: Duration = Duration::from_secs(5);

/// 资金费补拉回溯窗口 (014 D3): 7 天 —— 资金费 8h 一条, 足够覆盖停机时长。
const FUNDING_LOOKBACK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 资金费增量拉取间隔 (014 D4): 30 分钟 —— 资金费 8h 结算一次, 该粒度足够且无变现频压力。
const FUNDING_POLL_SECS: u64 = 1800;

/// 实盘行情静默期兜底心跳间隔 (035, 竞品优势 #10): quote 流**未断但静默**时也周期性决策,
/// 防网格挂单空窗/重挂被无限期推迟。行情正常时心跳空转冗余 (策略自身幂等), 语义无损。
const QUOTE_HEARTBEAT_SECS: u64 = 30;

/// 实盘事件 (行情 / 用户数据流 / 停机信号 / 资金费对账 / 静默心跳)。
enum LiveEvent {
    Stop(Option<StopRequest>),
    Quote(Option<ricow_core::OrderBookUpdate>),
    User(Option<UserEvent>),
    /// 资金费增量拉取 (014 FR-003): 不驱动策略 tick, 只对账落库。
    Funding,
    /// 035 行情静默兜底心跳: 用最后一次 orderbook 调 on_tick, 不虚构行情事实。
    Heartbeat,
}

/// 拉取资金费流水并落库 (014 FR-003): 水位 = db `MAX(funding_time)`(无记录则回溯 `FUNDING_LOOKBACK_MS`)。
///
/// 资金费是**事后对账**, 不参与下单决策 —— 失败只 warn, 绝不中断交易循环。返回新插入条数。
async fn poll_funding_income(
    exchange: &Arc<dyn Exchange>,
    db: &Database,
    strategy_name: &str,
) -> u64 {
    let start = match db.latest_funding_time().await {
        // 减 1ms 避免与已入库的同一条重复请求 (落库本身幂等, 这里只是省一次往返)
        Ok(Some(t)) => t.saturating_sub(1),
        Ok(None) => Utc::now().timestamp_millis() - FUNDING_LOOKBACK_MS,
        Err(e) => {
            tracing::warn!(target: "engine", name = %strategy_name, "资金费水位查询失败: {e}");
            return 0;
        }
    };
    let rows = match exchange.funding_income(start, 1000).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "engine", name = %strategy_name, "资金费拉取失败: {e}");
            return 0;
        }
    };
    let mut inserted = 0u64;
    for r in &rows {
        match db.insert_funding_fee(strategy_name, r).await {
            Ok(true) => inserted += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(target: "engine", name = %strategy_name, "资金费落库失败: {e}")
            }
        }
    }
    inserted
}

/// 持仓距强平告警 (014 FR-005): 低于阈值 → `warn`; 交易所未给强平价 → 如实标注"未知"(不猜)。
/// **只提示不动作** —— 自动减仓/平仓不在本职责范围 (风控护栏 004 的边界不加宽)。
fn warn_near_liquidation(
    positions: &[Position],
    strategy_name: &str,
    pair: &str,
    threshold: f64,
    notifier: Option<&Notifier>,
) {
    for p in positions {
        let mark = if p.mark_price > Decimal::ZERO { p.mark_price } else { p.entry_price };
        let side_tag = format!("{:?}", p.side);
        match p.liquidation_price {
            Some(liq) => match crate::live::liquidation_distance(mark, liq, p.side) {
                Some(d) if d < threshold => {
                    tracing::warn!(
                        target: "risk", name = %strategy_name, pair = %pair, side = ?p.side, size = %p.size,
                        liq = %liq, mark = %mark,
                        "接近强平: 距离 {:.2}% (阈值 {:.1}%, 已穿越为负)", d * 100.0, threshold * 100.0
                    );
                    // 003: 出站通知 (同 pair/方向去重, 距离恢复后由下方 clear_liq 复位)
                    if let Some(n) = notifier {
                        n.notify(NotifyEvent::LiqWarn {
                            pair: pair.to_string(),
                            side: side_tag,
                            mark,
                            liq,
                            distance_pct: d,
                            threshold_pct: threshold,
                        });
                    }
                }
                Some(d) => {
                    tracing::debug!(
                        target: "risk", name = %strategy_name, pair = %pair, "距强平 {:.2}%", d * 100.0
                    );
                    // 距离恢复到阈值外 → 复位去重标记, 下次再逼近可再告警
                    if let Some(n) = notifier {
                        n.clear_liq(pair, &side_tag);
                    }
                }
                None => {}
            },
            None => tracing::info!(
                target: "risk", name = %strategy_name, pair = %pair, side = ?p.side,
                "持仓未提供强平价, 距离未知"
            ),
        }
    }
}

/// 停机清理后短暂消费用户流: 把清理期间产生的成交 (兜底平仓) 回写上下文并落库。
///
/// 返回吸干到的**本实例**成交笔数。窗口内无事件即提前结束 (撤单不产生成交, 不会被白等满)。
#[allow(clippy::too_many_arguments)] // 参数聚合重构另行立项(021 只清存量告警, 不改结构)
async fn drain_user_events(
    stream: &mut Pin<Box<dyn Stream<Item = UserEvent> + Send>>,
    ctx: &mut LiveContext,
    strategy: &mut dyn Strategy,
    db: Option<&Database>,
    strategy_name: &str,
    prefix: &str,
    outcome: &mut RunOutcome,
    window: Duration,
    notifier: Option<&Notifier>,
    mode: RunMode,
) -> u64 {
    let deadline = tokio::time::Instant::now() + window;
    let mut n = 0u64;
    loop {
        let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remain.is_zero() {
            break;
        }
        match tokio::time::timeout(remain, stream.next()).await {
            Ok(Some(UserEvent::Fill(fill))) => {
                if !is_owned(&fill.client_order_id, prefix) {
                    continue;
                }
                n += 1;
                ctx.record_fill(&fill);
                outcome.fills += 1;
                if let Some(db) = db {
                    if let Err(e) = db.insert_fill(strategy_name, &fill).await {
                        outcome.persist_errors += 1;
                        let msg = e.to_string();
                        tracing::error!(target: "engine", name = %strategy_name, "清理成交落库失败: {msg}");
                        outcome.last_error = Some(format!("清理成交落库失败: {msg}"));
                    }
                    // 026 时点②: 订单状态 + PnL 快照 (与成交同批落库)
                    persist_fill_facts(db, outcome, strategy_name, &fill, ctx.pnl(), mode).await;
                }
                if let Some(n) = notifier {
                    n.notify(NotifyEvent::Fill {
                        pair: fill.pair.clone(),
                        side: format!("{:?}", fill.side),
                        price: fill.fill_price,
                        size: fill.fill_size,
                        fee: fill.fee,
                    });
                }
                strategy.on_fill(ctx, fill);
            }
            // 026 时点③: 清理期间的订单状态变化 (撤单/过期) 同样要可见 —— 此前被静默丢弃
            Ok(Some(UserEvent::Order(upd))) => {
                if !is_owned(&upd.client_order_id, prefix) {
                    continue;
                }
                if let Some(db) = db {
                    persist_order_update(db, outcome, strategy_name, &upd, mode).await;
                }
            }
            Ok(None) => break,
            // 窗口用尽 (无更多事件)
            Err(_) => break,
        }
    }
    n
}

/// 定向持仓的可读描述 (日志/快照用)。
fn describe_positions(positions: &[Position]) -> String {
    if positions.is_empty() {
        return "无持仓".to_string();
    }
    positions
        .iter()
        .map(|p| format!("{} {} @{}", p.side, p.size, p.entry_price))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 时点① 下单回执 → 订单当前状态行 (026 FR-003/T010)。
///
/// `price` / `size` 就是**委托价 / 委托量**: 只有此刻拿得到, 之后各时点不得改写 (见 `upsert_order`)。
fn order_row_of_ack(
    strategy_name: &str,
    ack: &OrderAck,
    mode: RunMode,
    now_ms: i64,
) -> OrderRecord {
    OrderRecord {
        strategy_id: strategy_name.to_string(),
        exchange_order_id: ack.exchange_order_id.clone(),
        client_order_id: ack.client_order_id.clone(),
        pair: ack.pair.clone(),
        side: ack.side.to_string(),
        price: ack.price,
        size: ack.size,
        filled_size: ack.filled_size,
        status: ack.status.to_string(),
        mode: mode.as_str().to_string(),
        created_at: now_ms,
        updated_at: now_ms,
    }
}

/// 时点② 成交回报 → 订单当前状态行 (026 FR-003/T011)。
///
/// **如实登记的局限 (plan P2)**: 成交消息不带委托价/委托量, 交易所原文里的累计成交量与订单状态
/// 在 `Exchange::subscribe_user_events` 就已被丢弃 —— 故此处按本次成交写 `filled`,
/// `filled_size` = 本次成交量。市价单/全额成交正确; **分笔部分成交**会显示为已全成且累计量偏小,
/// 随后由时点③ 的 `OrderUpdate`(带累计量) 纠正。行不存在时(早于本特性下的单 / 重启后)
/// `price`/`size` 只能以成交价/成交量为近似。
fn order_row_of_fill(
    strategy_name: &str,
    fill: &OrderFill,
    mode: RunMode,
    now_ms: i64,
) -> OrderRecord {
    OrderRecord {
        strategy_id: strategy_name.to_string(),
        exchange_order_id: fill.exchange_order_id.clone(),
        client_order_id: fill.client_order_id.clone(),
        pair: fill.pair.clone(),
        side: fill.side.to_string(),
        price: fill.fill_price,
        size: fill.fill_size,
        filled_size: fill.fill_size,
        status: OrderStatus::Filled.to_string(),
        mode: mode.as_str().to_string(),
        created_at: now_ms,
        updated_at: fill.timestamp.timestamp_millis(),
    }
}

/// 时点③ 订单状态变化 → 订单当前状态行 (026 FR-003/T012): 撤单/过期/部分成交此前完全不可见。
///
/// `OrderUpdate` 不带方向/委托价/委托量: `side` 留空(空串 = 未知, upsert 不改写既有行的 `side`),
/// `price` 取成交均价, `size` 取 `filled_size + remaining_size`(该单总量)。
fn order_row_of_update(
    strategy_name: &str,
    upd: &OrderUpdate,
    mode: RunMode,
    now_ms: i64,
) -> OrderRecord {
    OrderRecord {
        strategy_id: strategy_name.to_string(),
        exchange_order_id: upd.exchange_order_id.clone(),
        client_order_id: upd.client_order_id.clone(),
        pair: upd.pair.clone(),
        side: String::new(),
        price: upd.avg_price.unwrap_or(Decimal::ZERO),
        size: upd.filled_size + upd.remaining_size,
        filled_size: upd.filled_size,
        status: upd.status.to_string(),
        mode: mode.as_str().to_string(),
        created_at: now_ms,
        updated_at: upd.timestamp.timestamp_millis(),
    }
}

/// 时点④ 持仓事实 → 净持仓行 (026 FR-003/T013)。
///
/// 净仓 = 多 − 空 (与 `LiveContext::position` 同口径, 空为负); 多空并存时 `entry_price`
/// 取首个非零成本价 (如实近似, 不虚构加权算法)。空切片 = 已平仓 → 落一行 `size = 0`。
fn position_row_of(
    strategy_name: &str,
    pair: &str,
    positions: &[Position],
    mode: RunMode,
    now_ms: i64,
) -> PositionRecord {
    let mut size = Decimal::ZERO;
    let mut entry_price = Decimal::ZERO;
    for p in positions {
        match p.side {
            OrderSide::Buy => size += p.size,
            OrderSide::Sell => size -= p.size,
        }
        if entry_price.is_zero() && !p.entry_price.is_zero() {
            entry_price = p.entry_price;
        }
    }
    PositionRecord {
        strategy_id: strategy_name.to_string(),
        pair: pair.to_string(),
        size,
        entry_price,
        mode: mode.as_str().to_string(),
        updated_at: now_ms,
    }
}

/// 时点① 落库 (T010)。错误处置与 `insert_fill` 逐字同构 (T014):
/// 只计数 + error 日志 + 记 `last_error`, 不上抛、不阻塞、不改执行结果。
async fn persist_order_ack(
    db: &Database,
    outcome: &mut RunOutcome,
    strategy_name: &str,
    ack: &OrderAck,
    mode: RunMode,
) {
    let rec = order_row_of_ack(strategy_name, ack, mode, Utc::now().timestamp_millis());
    if let Err(e) = db.upsert_order(&rec).await {
        outcome.persist_errors += 1;
        let msg = e.to_string();
        tracing::error!(target: "engine", name = %strategy_name, "下单落库失败: {msg}");
        outcome.last_error = Some(format!("下单落库失败: {msg}"));
    }
}

/// 时点② 落库 (T011): 订单状态 + PnL 快照 (取 `ctx.pnl()`, **同源不重算**)。
async fn persist_fill_facts(
    db: &Database,
    outcome: &mut RunOutcome,
    strategy_name: &str,
    fill: &OrderFill,
    pnl: &PnlTracker,
    mode: RunMode,
) {
    let now_ms = Utc::now().timestamp_millis();
    let rec = order_row_of_fill(strategy_name, fill, mode, now_ms);
    if let Err(e) = db.upsert_order(&rec).await {
        outcome.persist_errors += 1;
        let msg = e.to_string();
        tracing::error!(target: "engine", name = %strategy_name, "成交订单落库失败: {msg}");
        outcome.last_error = Some(format!("成交订单落库失败: {msg}"));
    }
    let snap = PnlSnapshotRecord {
        strategy_id: strategy_name.to_string(),
        timestamp: now_ms,
        realized_pnl: pnl.realized_pnl(),
        fees: pnl.total_fees(),
        net_pnl: pnl.net_pnl(),
        trade_count: pnl.trade_count() as i64,
    };
    if let Err(e) = db.insert_pnl_snapshot(&snap).await {
        outcome.persist_errors += 1;
        let msg = e.to_string();
        tracing::error!(target: "engine", name = %strategy_name, "盈亏快照落库失败: {msg}");
        outcome.last_error = Some(format!("盈亏快照落库失败: {msg}"));
    }
}

/// 时点③ 落库 (T012)。
async fn persist_order_update(
    db: &Database,
    outcome: &mut RunOutcome,
    strategy_name: &str,
    upd: &OrderUpdate,
    mode: RunMode,
) {
    let rec = order_row_of_update(strategy_name, upd, mode, Utc::now().timestamp_millis());
    if let Err(e) = db.upsert_order(&rec).await {
        outcome.persist_errors += 1;
        let msg = e.to_string();
        tracing::error!(target: "engine", name = %strategy_name, "订单状态落库失败: {msg}");
        outcome.last_error = Some(format!("订单状态落库失败: {msg}"));
    }
}

/// 时点④ 落库 (T013)。
async fn persist_position(
    db: &Database,
    outcome: &mut RunOutcome,
    strategy_name: &str,
    pair: &str,
    positions: &[Position],
    mode: RunMode,
) {
    let rec = position_row_of(strategy_name, pair, positions, mode, Utc::now().timestamp_millis());
    if let Err(e) = db.upsert_position(&rec).await {
        outcome.persist_errors += 1;
        let msg = e.to_string();
        tracing::error!(target: "engine", name = %strategy_name, "持仓落库失败: {msg}");
        outcome.last_error = Some(format!("持仓落库失败: {msg}"));
    }
}

/// 034 事件驱动决策 (dry-run): 下单出口 (on_tick 与 on_fill 返回的订单共用)。
/// 只提交 + 拒单回传 + 落库, **不** drain 撮合成交 —— 调用方在合适时点 drain 并派发。
async fn submit_orders_dry(
    ctx: &mut DryRunContext,
    strategy: &mut dyn Strategy,
    orders: Vec<OrderRequest>,
    outcome: &mut RunOutcome,
    db: Option<&Database>,
    strategy_name: &str,
) {
    for req in orders {
        outcome.orders_submitted += 1;
        match ctx.place_order(req) {
            Ok(ack) if ack.status == OrderStatus::Rejected => {
                // 风控/参数对齐拒单: 与"下单失败"区分计数 (策略循环不中断)
                // 026 时点① 只记**已提交**的单 —— 拒单未到交易所, 不产生订单行
                outcome.rejections += 1;
                // 审计 #3: 拒单回传给策略。
                strategy.on_order_update(ctx, ack.to_update(Utc::now()));
            }
            Ok(ack) => {
                if ack.status == OrderStatus::Cancelled {
                    strategy.on_order_update(ctx, ack.to_update(Utc::now()));
                }
                if let Some(db) = db {
                    persist_order_ack(db, outcome, strategy_name, &ack, RunMode::DryRun).await;
                }
            }
            Err(e) => {
                // 下单失败如实记录并计数 (不再静默吞掉)
                outcome.order_errors += 1;
                let msg = e.to_string();
                tracing::error!(target: "engine", name = %strategy_name, "下单失败: {msg}");
                outcome.last_error = Some(format!("下单失败: {msg}"));
            }
        }
    }
}

/// 034 事件驱动决策 (dry-run): 单笔成交的完整处置 + on_fill 后续订单的有界递归。
///
/// 手工 Box 装箱以支持 async 递归: on_fill 返回的订单立即提交, 其即时成交在同一
/// 事件内继续派发 (深度上限 [`crate::backtest_runner::MAX_FILL_DECISION_DEPTH`],
/// 防"成交即市价反手"病态策略无限递归); on_fill 返回空 = 旧行为, 零递归。
#[allow(clippy::too_many_arguments)]
fn handle_dry_fill<'a>(
    ctx: &'a mut DryRunContext,
    strategy: &'a mut dyn Strategy,
    fill: OrderFill,
    depth: u32,
    outcome: &'a mut RunOutcome,
    db: Option<&'a Database>,
    notifier: Option<&'a Notifier>,
    strategy_name: &'a str,
    pair: &'a str,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth > crate::backtest_runner::MAX_FILL_DECISION_DEPTH {
            tracing::warn!(
                target: "engine",
                depth,
                name = %strategy_name,
                "on_fill 递归决策深度超限, 停止派发 (疑似'成交即反手'病态逻辑)"
            );
            return;
        }
        outcome.fills += 1;
        if let Some(db) = db {
            if let Err(e) = db.insert_fill(strategy_name, &fill).await {
                outcome.persist_errors += 1;
                let msg = e.to_string();
                tracing::error!(target: "engine", name = %strategy_name, "成交落库失败: {msg}");
                outcome.last_error = Some(format!("成交落库失败: {msg}"));
            }
            // 026 时点②: 订单状态 + PnL 快照 (与成交同批落库)
            persist_fill_facts(db, outcome, strategy_name, &fill, ctx.pnl(), RunMode::DryRun).await;
        }
        if let Some(n) = notifier {
            n.notify(NotifyEvent::Fill {
                pair: fill.pair.clone(),
                side: format!("{:?}", fill.side),
                price: fill.fill_price,
                size: fill.fill_size,
                fee: fill.fee,
            });
        }
        let follow_ups = strategy.on_fill(ctx, fill);
        // 030 断点续接: 成交后立即持久化策略状态(关机/中止后重启可继续)。
        if let Some(db) = db {
            for (k, v) in strategy.state_snapshot() {
                if let Err(e) = db.strategy_state_set(strategy_name, &k, &v).await {
                    outcome.persist_errors += 1;
                    tracing::error!(target: "engine", key = %k, "策略状态落库失败: {e}");
                }
            }
        }
        // 026 时点④: dry run 的持仓事实 = 虚拟持仓 (None = 已平 → 落 size 0)
        if let Some(db) = db {
            persist_position(
                db,
                outcome,
                strategy_name,
                pair,
                ctx.position(pair).as_slice(),
                RunMode::DryRun,
            )
            .await;
        }
        if follow_ups.is_empty() {
            return;
        }
        submit_orders_dry(ctx, strategy, follow_ups, outcome, db, strategy_name).await;
        let next_fills = ctx.drain_fills();
        for f in next_fills {
            handle_dry_fill(
                ctx,
                strategy,
                f,
                depth + 1,
                outcome,
                db,
                notifier,
                strategy_name,
                pair,
            )
            .await;
        }
    })
}

/// 035 实盘决策 tick 的共用出口 (Quote 与 Heartbeat 共用, FR-002):
/// on_tick 产单 → 提交 (计数/落库/拒单回传) → 下单即成交则立刻对齐本地快照 (016 FR-B)。
#[allow(clippy::too_many_arguments)] // 与 refresh_positions 同口径, 参数聚合另行立项
async fn live_decision_tick(
    ctx: &mut LiveContext,
    strategy: &mut dyn Strategy,
    outcome: &mut RunOutcome,
    db: Option<&Database>,
    strategy_name: &str,
    mode: RunMode,
    exchange: &Arc<dyn Exchange>,
    market: &Market,
    pair: &str,
    is_futures: bool,
    liq_warn_threshold: f64,
    notifier: Option<&Notifier>,
) {
    let orders = strategy.on_tick(ctx);
    let mut any_filled = false;
    for req in orders {
        outcome.orders_submitted += 1;
        match ctx.place_order(req) {
            Ok(ack) if ack.status == OrderStatus::Rejected => {
                outcome.rejections += 1;
                strategy.on_order_update(ctx, ack.to_update(Utc::now()));
            }
            Ok(ack) => {
                // 026 时点①: 委托价/委托量在提交时落库
                if ack.status == OrderStatus::Cancelled {
                    strategy.on_order_update(ctx, ack.to_update(Utc::now()));
                }
                if let Some(db) = db {
                    persist_order_ack(db, outcome, strategy_name, &ack, mode).await;
                }
                if ack.filled_size > Decimal::ZERO {
                    any_filled = true;
                }
            }
            Err(e) => {
                outcome.order_errors += 1;
                let msg = e.to_string();
                tracing::error!(target: "engine", name = %strategy_name, "下单失败: {msg}");
                outcome.last_error = Some(format!("下单失败: {msg}"));
            }
        }
    }
    // 下单即有成交 → 立刻对齐本地快照 (016 FR-B): 成交回写走用户流有延迟, 期间策略会按陈旧
    // 持仓/现金重复下单 (demo 实测: 1.4s 内同价同量 3 次 → 2 成交 + 1 次资金不足报错)。
    if any_filled {
        let refreshed = refresh_positions(
            exchange,
            market,
            pair,
            ctx,
            is_futures,
            strategy_name,
            liq_warn_threshold,
            notifier,
        )
        .await;
        // 026 时点④: 只落**查到的事实**; None = 查询失败 → 不落库 (不拿陈旧快照冒充现状)
        if let (Some(db), Some(positions)) = (db, refreshed) {
            persist_position(db, outcome, strategy_name, pair, &positions, mode).await;
        }
    }
}

/// 成交后刷新持仓: 合约走定向持仓覆盖; 现货走 base 可用余额包装 (空余额 = 清仓, 不留陈旧仓)。
///
/// 返回**已写入上下文的持仓事实**: `Some(空切片)` = 确实无仓(已平), `None` = 查询失败
/// (无法确定, 调用方据此**不落库**, 绝不把"查不到"写成本地"已平仓")。
#[allow(clippy::too_many_arguments)] // 参数聚合重构另行立项(021 只清存量告警, 不改结构)
async fn refresh_positions(
    exchange: &Arc<dyn Exchange>,
    market: &Market,
    pair: &str,
    ctx: &LiveContext,
    is_futures: bool,
    strategy_name: &str,
    liq_warn_threshold: f64,
    notifier: Option<&Notifier>,
) -> Option<Vec<Position>> {
    if is_futures {
        match exchange.get_positions_directional(pair).await {
            Ok(ps) => {
                warn_near_liquidation(&ps, strategy_name, pair, liq_warn_threshold, notifier);
                ctx.set_positions(pair, &ps);
                Some(ps)
            }
            Err(e) => {
                tracing::warn!(target: "engine", pair = %pair, "成交后刷新合约持仓失败: {e}");
                None
            }
        }
    } else {
        let positions: Vec<Position> =
            spot_position_of(exchange, market, pair).await.into_iter().collect();
        ctx.set_positions(pair, &positions);
        // 现货: **现金与持仓都要刷** —— 策略按 `equity = 现金 + 持仓价值` 定目标比例,
        // 只刷持仓不刷现金会让它以为"只有币没有钱"(现金停在启动快照), 从而每 tick 再卖一半,
        // 几何级数把持仓卖光 (2026-09-13 demo 实测: 2.016 ETH 被 1.0079→0.5039→0.252→… 清空,
        // 而策略意图只是卖一半回到 50%)。合约侧现金在钱包/保证金模型里, 不走这条。
        for asset in [market.base_asset.clone(), market.quote_asset.clone()] {
            match exchange.get_balance(&asset).await {
                Ok(bal) => ctx.update_balance(&asset, bal),
                Err(e) => tracing::warn!(
                    target: "engine", pair = %pair,
                    "成交后刷新现货余额失败 ({asset}): {e}"
                ),
            }
        }
        Some(positions)
    }
}

/// 现货持仓快照: base 资产可用余额包装为多头持仓 (现货无成本价概念, entry/mark 记 0); 空仓 → None。
async fn spot_position_of(
    exchange: &Arc<dyn Exchange>,
    market: &Market,
    pair: &str,
) -> Option<Position> {
    let bal = exchange.get_balance(&market.base_asset).await.ok()?;
    if bal.free <= Decimal::ZERO {
        return None;
    }
    Some(Position {
        pair: pair.to_string(),
        side: OrderSide::Buy,
        size: bal.free,
        entry_price: Decimal::ZERO,
        mark_price: Decimal::ZERO,
        liquidation_price: None,
        unrealized_pnl: Decimal::ZERO,
        leverage: None,
    })
}

/// 一次运行的结果 (供 CLI 输出与实例台账记录)。
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub stop_reason: Option<StopReason>,
    /// 收到的行情 tick 数
    pub ticks: u64,
    /// 策略提交的订单数 (含失败)
    pub orders_submitted: u64,
    /// 下单失败数 (不再静默吞掉, 见 008 T004)
    pub order_errors: u64,
    /// 成交笔数 (含停机清理产生的)
    pub fills: u64,
    /// 被拒订单数 (风控拒单 / 交易所最小数量与名义不足的对齐拒单)
    pub rejections: u64,
    /// 落库失败数 (成交 / 订单 / 持仓 / 盈亏快照)
    pub persist_errors: u64,
    /// 最近一次错误信息 (如实反映, 不夸大)
    pub last_error: Option<String>,
    /// 策略是否实现了停机清理 (`on_stop`); false 时需提示用户手工处理
    pub on_stop_implemented: bool,
    /// 实盘停机清理结果 (撤单兜底/平仓/残留); Dry Run 无交易所挂单, 恒 None
    pub cleanup: Option<CleanupOutcome>,
}

impl Engine {
    pub fn new() -> Self {
        Self
    }

    /// 运行 Dry Run: 行情驱动策略, 虚拟撮合。
    ///
    /// - `db`: 成交落库目标; `None` 时仅内存运行 (不落库)。
    /// - `stop`: 停机信号; `None` 时只在行情流结束或错误时退出。
    pub async fn run_dry_run(
        &self,
        config: StrategyConfig,
        exchange: Arc<dyn Exchange>,
        initial_balance: Balance,
        db: Option<&Database>,
        mut stop: Option<StopSignal>,
    ) -> CoreResult<RunOutcome> {
        // 出站通知 (003): 未配 `params.notify_webhook` → None (不打通道)
        let notifier = Notifier::from_config(&config);
        let pair = config.get_str("pair").unwrap_or("ETH").to_string();
        let strategy_name = config.name.clone();
        // 026: dry run 的模式是固定的 —— 不对外暴露 mode 参数 (调用方无从传错), 落库口径由 RunMode 决定
        let mode = RunMode::DryRun;

        let mut ctx = DryRunContext::new(exchange.clone(), config.clone(), initial_balance);
        // 预装 K 线历史见 on_init 之后 —— 须先跑声明阶段收集 need_klines。

        let mut strategy = load_strategy(&config)?;
        // 030 断点续接: 把上次会话保存的策略状态注回 (无记录 = 首次运行)。
        if let Some(db) = db {
            match db.strategy_state_all(&strategy_name).await {
                Ok(items) if !items.is_empty() => {
                    tracing::info!(target: "engine", name = %strategy_name, keys = items.len(), "注入已保存的策略状态");
                    strategy.state_restore(items);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(target: "engine", name = %strategy_name, "读策略状态失败: {e}");
                }
            }
        }
        strategy.on_init(&mut ctx);

        // 按策略声明 (need_klines) 预装 K 线历史 —— 不读任何策略参数名。
        {
            let mut by_tf: std::collections::HashMap<String, (bool, u32)> =
                std::collections::HashMap::new();
            for d in ctx.declarations() {
                let e = by_tf.entry(d.tf.clone()).or_insert((false, 0));
                e.0 |= d.role == "primary";
                e.1 = e.1.max(d.min_bars);
            }
            for (tf, (is_primary, need)) in by_tf {
                match exchange.get_klines(&pair, &tf, need).await {
                    Ok(bars) if !bars.is_empty() => {
                        let tf_ms = ricow_strategy::tf_ms_of(&tf).unwrap_or(3_600_000) as i64;
                        let complete = ricow_strategy::resample_complete(&bars, tf_ms);
                        tracing::info!(target: "engine", tf = %tf, n = complete.len(), "预装 K 线历史");
                        ctx.set_tf_klines(&pair, &tf, complete);
                        if is_primary {
                            ctx.update_klines(&pair, bars);
                        }
                    }
                    Ok(_) => tracing::warn!(target: "engine", tf = %tf, "K 线历史为空"),
                    Err(e) => tracing::warn!(target: "engine", tf = %tf, "取 K 线历史失败: {e}"),
                }
            }
        }

        let mut stream = market::subscribe_orderbook(&exchange, &pair).await?;
        tracing::info!(target: "engine", name = %config.name, pair = %pair, "dry run started");

        let mut outcome = RunOutcome::default();

        loop {
            let update = match stop.as_mut() {
                Some(rx) => {
                    tokio::select! {
                        biased;
                        res = rx.changed() => {
                            // 停机信号: 记原因后跳出 (不处理本次行情)
                            outcome.stop_reason = match res {
                                Ok(()) => rx.borrow().as_ref().and_then(|r| r.reason),
                                // 发送端已关闭 = 管理器进程消失
                                Err(_) => Some(StopReason::ManagerGone),
                            };
                            break;
                        }
                        upd = stream.next() => upd,
                    }
                }
                None => stream.next().await,
            };

            let Some(update) = update else { break };

            let ob = market::to_orderbook(update);
            ctx.update_orderbook(&pair, ob);
            outcome.ticks += 1;

            // 030: 用最新价刷新"当前未收盘"K 线 —— 这是实盘侧 ctx:klines 从空到有的来源。
            if let Some(px) = ctx.price(&pair) {
                ctx.tick_kline(&pair, px, chrono::Utc::now().timestamp_millis());
            }

            let orders = strategy.on_tick(&mut ctx);
            submit_orders_dry(
                &mut ctx,
                strategy.as_mut(),
                orders,
                &mut outcome,
                db,
                &strategy_name,
            )
            .await;

            let fills = ctx.drain_fills();
            for fill in fills {
                // 034: 单笔成交完整处置 (含 on_fill 返回订单的立即提交与递归派发)。
                handle_dry_fill(
                    &mut ctx,
                    strategy.as_mut(),
                    fill,
                    0,
                    &mut outcome,
                    db,
                    notifier.as_ref(),
                    &strategy_name,
                    &pair,
                )
                .await;
            }
        }

        // 停机清理: 策略实现了 on_stop 才会调 (是否实现由脚本决定, 引擎不虚构清理行为)
        outcome.on_stop_implemented = strategy.has_on_stop();
        strategy.on_stop(&mut ctx);
        // 030 断点续接: 停机时持久化最终状态(不清仓, 状态留给下次启动)。
        if let Some(db) = db {
            for (k, v) in strategy.state_snapshot() {
                if let Err(e) = db.strategy_state_set(&strategy_name, &k, &v).await {
                    outcome.persist_errors += 1;
                    tracing::error!(target: "engine", key = %k, "策略状态落库失败: {e}");
                }
            }
        }

        // 清理回调产生的成交同样落库 + 回调 (与主循环一致)
        for fill in ctx.drain_fills() {
            outcome.fills += 1;
            if let Some(db) = db {
                if let Err(e) = db.insert_fill(&strategy_name, &fill).await {
                    outcome.persist_errors += 1;
                    let msg = e.to_string();
                    tracing::error!(target: "engine", name = %strategy_name, "清理成交落库失败: {msg}");
                    outcome.last_error = Some(format!("清理成交落库失败: {msg}"));
                }
                // 026 时点②: 订单状态 + PnL 快照 (与成交同批落库)
                persist_fill_facts(db, &mut outcome, &strategy_name, &fill, ctx.pnl(), mode).await;
            }
            if let Some(n) = &notifier {
                n.notify(NotifyEvent::Fill {
                    pair: fill.pair.clone(),
                    side: format!("{:?}", fill.side),
                    price: fill.fill_price,
                    size: fill.fill_size,
                    fee: fill.fee,
                });
            }
            strategy.on_fill(&mut ctx, fill);
            // 026 时点④: 清理后的持仓事实同样是虚拟持仓
            if let Some(db) = db {
                persist_position(
                    db,
                    &mut outcome,
                    &strategy_name,
                    &pair,
                    ctx.position(&pair).as_slice(),
                    mode,
                )
                .await;
            }
        }

        if outcome.stop_reason.is_none() {
            outcome.stop_reason = Some(StopReason::StreamEnded);
        }

        tracing::info!(
            target: "engine", name = %strategy_name,
            reason = %outcome.stop_reason.map(|r| r.to_string()).unwrap_or_default(),
            ticks = outcome.ticks, fills = outcome.fills, errors = outcome.order_errors,
            "dry run stopped"
        );
        Ok(outcome)
    }

    /// 运行实盘 (011): 真实账户事实源 + 真实下单/成交 + 停机清理。
    ///
    /// 与 `run_dry_run` 并列 (plan D1): 账户事实源与停机清理语义不同, 不合并到同一条循环。
    /// - `close_all`: 停机清理时是否市价平掉策略持仓;
    /// - 用户数据流断线 = **异常停机** (plan P1; 断线后成交无法回灌, 静默继续会误判仓位);
    /// - 用户流事件只回写成交, **不驱动 tick** (plan P2, 与 Dry Run 的 tick 语义一致)。
    pub async fn run_live(
        &self,
        config: StrategyConfig,
        exchange: Arc<dyn Exchange>,
        db: Option<&Database>,
        stop: Option<StopSignal>,
        close_all: bool,
        // 运行模式: 日志标签(`label()`)必须如实标注, 不得把 demo 说成实盘; 落库口径走 `as_str()`
        mode: RunMode,
    ) -> CoreResult<RunOutcome> {
        let pair = config.get_str("pair").unwrap_or("ETHUSDT").to_string();
        let strategy_name = config.name.clone();

        // ① 启动装配: 交易所过滤器 (下单参数对齐依据) —— 拉不到即拒绝启动, 不做无过滤器的盲下单
        let markets = exchange.get_markets().await.map_err(|e| {
            CoreError::Exchange(format!("实盘启动失败: 拉取交易所过滤器 (exchangeInfo) 失败: {e}"))
        })?;
        let market =
            markets.iter().find(|m| m.symbol.eq_ignore_ascii_case(&pair)).cloned().ok_or_else(
                || {
                    CoreError::InvalidArgument(format!(
                        "实盘启动失败: 交易所列表未含交易对 {pair} (无法确定下单过滤器)"
                    ))
                },
            )?;

        let rt = tokio::runtime::Handle::current();
        let mut ctx = LiveContext::new(exchange.clone(), config.clone(), rt);
        // 预装 K 线历史见 on_init 之后 —— 须先跑声明阶段收集 need_klines。

        ctx.set_markets(&markets);
        let prefix = ctx.order_prefix().to_string();

        // 市场分派 (012): 合约需定向持仓与合约语义的平仓参数
        let is_futures = config.market.eq_ignore_ascii_case("futures");
        let hedge = is_futures && config.position_mode.eq_ignore_ascii_case("hedge");
        // 强平距离告警阈值 (014 FR-005): params `liq_warn_pct`, 默认 15% (只提示不动作)
        let liq_warn_threshold = config.get_f64("liq_warn_pct").unwrap_or(15.0) / 100.0;
        // 出站通知 (003): 未配 `params.notify_webhook` → None (完全不打通道, 保守默认)
        let notifier = Notifier::from_config(&config);
        if notifier.is_some() {
            tracing::info!(target: "notify", name = %strategy_name, "通知已启用 (成交/接近强平/停机残留)");
        }

        // ② 账户快照 (真实事实源; 不生成虚拟资金)
        //    现货: base/quote 余额 + base 可用余额包装为多头持仓
        //    合约: 报价资产可用余额 + 定向持仓(one-way 一条 / hedge 两侧)
        // 026: 结果台账提前声明 —— 账户快照 (时点④ 起始持仓) 也要落库
        let mut outcome = RunOutcome::default();
        let base_asset = market.base_asset.clone();
        let quote_asset = market.quote_asset.clone();
        let quote_free = match exchange.get_balance(&quote_asset).await {
            Ok(b) => {
                ctx.update_balance(&quote_asset, b.clone());
                Some(b.free)
            }
            Err(e) => {
                tracing::warn!(target: "engine", name = %strategy_name, "启动读取 {quote_asset} 余额失败: {e}");
                None
            }
        };
        let position_desc = if is_futures {
            match exchange.get_positions_directional(&pair).await {
                Ok(positions) => {
                    let desc = describe_positions(&positions);
                    warn_near_liquidation(
                        &positions,
                        &strategy_name,
                        &pair,
                        liq_warn_threshold,
                        notifier.as_ref(),
                    );
                    ctx.set_positions(&pair, &positions);
                    // 026 时点④: 启动快照即当前真实持仓 (查询失败分支不落库 —— 不把"查不到"写成"已平仓")
                    if let Some(db) = db {
                        persist_position(db, &mut outcome, &strategy_name, &pair, &positions, mode)
                            .await;
                    }
                    desc
                }
                Err(e) => {
                    tracing::warn!(target: "engine", name = %strategy_name, "启动读取合约持仓失败: {e}");
                    "查询失败".to_string()
                }
            }
        } else {
            match exchange.get_balance(&base_asset).await {
                Ok(b) => {
                    ctx.update_balance(&base_asset, b.clone());
                    let positions: Vec<Position> =
                        spot_position_of(&exchange, &market, &pair).await.into_iter().collect();
                    let desc = describe_positions(&positions);
                    ctx.set_positions(&pair, &positions);
                    // 026 时点④: 现货起始持仓 (空 = 确实无仓)
                    if let Some(db) = db {
                        persist_position(db, &mut outcome, &strategy_name, &pair, &positions, mode)
                            .await;
                    }
                    desc
                }
                Err(e) => {
                    tracing::warn!(target: "engine", name = %strategy_name, "启动读取 {base_asset} 余额失败: {e}");
                    "查询失败".to_string()
                }
            }
        };
        tracing::info!(
            target: "engine", name = %strategy_name, pair = %pair, market = %config.market,
            quote = %format_args!("{quote_asset} {}", quote_free.map(|d| d.to_string()).unwrap_or_else(|| "?".into())),
            position = %position_desc,
            "实盘账户快照"
        );
        // 启动期挂单观测 (残留提示; 不在启动时擅自撤单)
        match exchange.get_open_orders(&pair).await {
            Ok(orders) if !orders.is_empty() => {
                let mine = orders.iter().filter(|o| is_owned(&o.client_order_id, &prefix)).count();
                tracing::warn!(
                    target: "engine", name = %strategy_name,
                    total = orders.len(), owned = mine,
                    "启动时该交易对已有挂单 (本实例归属 {mine} 笔, 停机时按前缀处理)"
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(target: "engine", name = %strategy_name, "启动查询挂单失败: {e}")
            }
        }

        // ③ 策略与双流 (行情 + 用户数据流)
        let mut strategy = load_strategy(&config)?;
        // 030 断点续接: 把上次会话保存的策略状态注回 (无记录 = 首次运行)。
        if let Some(db) = db {
            match db.strategy_state_all(&strategy_name).await {
                Ok(items) if !items.is_empty() => {
                    tracing::info!(target: "engine", name = %strategy_name, keys = items.len(), "注入已保存的策略状态");
                    strategy.state_restore(items);
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(target: "engine", name = %strategy_name, "读策略状态失败: {e}");
                }
            }
        }
        strategy.on_init(&mut ctx);

        // 按策略声明 (need_klines) 预装 K 线历史 —— 不读任何策略参数名。
        {
            let mut by_tf: std::collections::HashMap<String, (bool, u32)> =
                std::collections::HashMap::new();
            for d in ctx.declarations() {
                let e = by_tf.entry(d.tf.clone()).or_insert((false, 0));
                e.0 |= d.role == "primary";
                e.1 = e.1.max(d.min_bars);
            }
            for (tf, (is_primary, need)) in by_tf {
                match exchange.get_klines(&pair, &tf, need).await {
                    Ok(bars) if !bars.is_empty() => {
                        let tf_ms = ricow_strategy::tf_ms_of(&tf).unwrap_or(3_600_000) as i64;
                        let complete = ricow_strategy::resample_complete(&bars, tf_ms);
                        tracing::info!(target: "engine", tf = %tf, n = complete.len(), "预装 K 线历史");
                        ctx.set_tf_klines(&pair, &tf, complete);
                        if is_primary {
                            ctx.update_klines(&pair, bars);
                        }
                    }
                    Ok(_) => tracing::warn!(target: "engine", tf = %tf, "K 线历史为空"),
                    Err(e) => tracing::warn!(target: "engine", tf = %tf, "取 K 线历史失败: {e}"),
                }
            }
        }

        let mut quote_stream = market::subscribe_orderbook(&exchange, &pair).await?;
        let mut user_stream = exchange
            .subscribe_user_events()
            .await
            .map_err(|e| CoreError::Exchange(format!("实盘启动失败: 订阅用户数据流失败: {e}")))?;
        // 资金费补拉 (014 FR-003): 启动先补齐历史, 运行期再按 30 分钟增量拉取
        if is_futures {
            if let Some(db) = db {
                let n = poll_funding_income(&exchange, db, &strategy_name).await;
                if n > 0 {
                    tracing::info!(target: "engine", name = %strategy_name, inserted = n, "启动补拉资金费流水");
                }
            }
        }
        let mut funding_tick = tokio::time::interval(Duration::from_secs(FUNDING_POLL_SECS));
        funding_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 035: 行情静默期兜底心跳 (首拍即触发, 由 orderbook 为空守卫拦下, 见 Heartbeat 分支)
        let mut heartbeat_tick = tokio::time::interval(Duration::from_secs(QUOTE_HEARTBEAT_SECS));
        heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tracing::info!(target: "engine", name = %strategy_name, pair = %pair, mode = %mode.label(), "live run started");

        let mut stop_rx = stop;
        // 停机时是否平仓: 启动参数与停机指令二者取或 (`start --live` 后 `stop --close-all` 也生效)
        let mut close_all_at_stop = close_all;

        loop {
            let ev = match stop_rx.as_mut() {
                Some(rx) => tokio::select! {
                    biased;
                    res = rx.changed() => LiveEvent::Stop(match res {
                        Ok(()) => *rx.borrow(),
                        // 发送端已关闭 = 管理器进程消失 (按无平仓意图处理)
                        Err(_) => Some(StopRequest {
                            reason: Some(StopReason::ManagerGone),
                            close_all: false,
                        }),
                    }),
                    upd = quote_stream.next() => LiveEvent::Quote(upd),
                    ev = user_stream.next() => LiveEvent::User(ev),
                    _ = funding_tick.tick() => LiveEvent::Funding,
                    _ = heartbeat_tick.tick() => LiveEvent::Heartbeat,
                },
                None => tokio::select! {
                    upd = quote_stream.next() => LiveEvent::Quote(upd),
                    ev = user_stream.next() => LiveEvent::User(ev),
                    _ = funding_tick.tick() => LiveEvent::Funding,
                    _ = heartbeat_tick.tick() => LiveEvent::Heartbeat,
                },
            };

            match ev {
                LiveEvent::Stop(req) => {
                    match req {
                        Some(r) => {
                            close_all_at_stop |= r.close_all;
                            outcome.stop_reason = Some(r.reason.unwrap_or(StopReason::Requested));
                        }
                        None => outcome.stop_reason = Some(StopReason::Requested),
                    }
                    break;
                }
                LiveEvent::Funding => {
                    if is_futures {
                        if let Some(db) = db {
                            let n = poll_funding_income(&exchange, db, &strategy_name).await;
                            if n > 0 {
                                tracing::info!(target: "engine", name = %strategy_name, inserted = n, "资金费流水增量入库");
                            }
                        }
                    }
                }
                LiveEvent::Quote(None) => {
                    outcome.stop_reason = Some(StopReason::StreamEnded);
                    outcome.last_error = Some("行情流中断 (WebSocket 断开)".into());
                    break;
                }
                LiveEvent::Quote(Some(update)) => {
                    let ob = market::to_orderbook(update);
                    ctx.update_orderbook(&pair, ob);
                    outcome.ticks += 1;
                    // 030: 用最新价刷新"当前未收盘"K 线 —— 这是实盘侧 ctx:klines 从空到有的来源。
                    if let Some(px) = ctx.price(&pair) {
                        ctx.tick_kline(&pair, px, chrono::Utc::now().timestamp_millis());
                    }

                    // 035: 决策出口与心跳共用 (FR-002)
                    live_decision_tick(
                        &mut ctx,
                        &mut *strategy,
                        &mut outcome,
                        db,
                        &strategy_name,
                        mode,
                        &exchange,
                        &market,
                        &pair,
                        is_futures,
                        liq_warn_threshold,
                        notifier.as_ref(),
                    )
                    .await;
                }
                LiveEvent::Heartbeat => {
                    // 035 FR-003: 静默期兜底 —— 用最后一次 orderbook 决策, 不虚构行情事实。
                    // 启动后从未收到 quote (orderbook 为空) 时跳过; 不 tick_kline、不计 ticks。
                    if ctx.price(&pair).is_none() {
                        continue;
                    }
                    tracing::debug!(target: "engine", name = %strategy_name, "行情静默心跳触发决策");
                    live_decision_tick(
                        &mut ctx,
                        &mut *strategy,
                        &mut outcome,
                        db,
                        &strategy_name,
                        mode,
                        &exchange,
                        &market,
                        &pair,
                        is_futures,
                        liq_warn_threshold,
                        notifier.as_ref(),
                    )
                    .await;
                }
                LiveEvent::User(None) => {
                    // 用户数据流断线: 成交无法回灌 → 异常停机 (plan P1), 不静默继续
                    outcome.stop_reason = Some(StopReason::StreamEnded);
                    outcome.last_error = Some("用户数据流中断: 成交无法回灌, 已按异常停机".into());
                    break;
                }
                LiveEvent::User(Some(ev)) => match ev {
                    UserEvent::Fill(fill) => {
                        if !is_owned(&fill.client_order_id, &prefix) {
                            // 用户流是全账户的: 非本实例成交不参与本策略 PnL/落库
                            tracing::debug!(
                                target: "engine", name = %strategy_name,
                                "忽略非本实例成交: {}", fill.client_order_id
                            );
                            continue;
                        }
                        ctx.record_fill(&fill);
                        outcome.fills += 1;
                        if let Some(db) = db {
                            if let Err(e) = db.insert_fill(&strategy_name, &fill).await {
                                outcome.persist_errors += 1;
                                let msg = e.to_string();
                                tracing::error!(target: "engine", name = %strategy_name, "成交落库失败: {msg}");
                                outcome.last_error = Some(format!("成交落库失败: {msg}"));
                            }
                            // 026 时点②: 订单状态 + PnL 快照 (与成交同批落库)
                            persist_fill_facts(
                                db,
                                &mut outcome,
                                &strategy_name,
                                &fill,
                                ctx.pnl(),
                                mode,
                            )
                            .await;
                        }
                        if let Some(n) = &notifier {
                            n.notify(NotifyEvent::Fill {
                                pair: fill.pair.clone(),
                                side: format!("{:?}", fill.side),
                                price: fill.fill_price,
                                size: fill.fill_size,
                                fee: fill.fee,
                            });
                        }
                        // 审计 #4: 策略在成交回调里判断"是否已清仓", 而持仓刷新原本在本行之后,
                        // 回调读到的是**成交前**的旧仓位(清仓那笔判不出 -> finished 永不置位)。
                        // 回调前先刷一次, 使"策略看到的持仓"与本次成交口径一致; 失败不阻塞回调。
                        let _ = refresh_positions(
                            &exchange,
                            &market,
                            &pair,
                            &ctx,
                            is_futures,
                            &strategy_name,
                            liq_warn_threshold,
                            notifier.as_ref(),
                        )
                        .await;
                        // 034 事件驱动: on_fill 返回的订单立即提交 (实盘新成交经用户流
                        // 再次触发 on_fill, 天然闭环, 无需本地递归)。
                        for req in strategy.on_fill(&mut ctx, fill) {
                            outcome.orders_submitted += 1;
                            match ctx.place_order(req) {
                                Ok(ack) if ack.status == OrderStatus::Rejected => {
                                    outcome.rejections += 1;
                                    strategy.on_order_update(&mut ctx, ack.to_update(Utc::now()));
                                }
                                Ok(ack) => {
                                    if ack.status == OrderStatus::Cancelled {
                                        strategy
                                            .on_order_update(&mut ctx, ack.to_update(Utc::now()));
                                    }
                                    if let Some(db) = db {
                                        persist_order_ack(
                                            db,
                                            &mut outcome,
                                            &strategy_name,
                                            &ack,
                                            mode,
                                        )
                                        .await;
                                    }
                                }
                                Err(e) => {
                                    outcome.order_errors += 1;
                                    let msg = e.to_string();
                                    tracing::error!(target: "engine", name = %strategy_name, "下单失败: {msg}");
                                    outcome.last_error = Some(format!("下单失败: {msg}"));
                                }
                            }
                        }
                        // 030 断点续接: 成交后立即持久化策略状态(关机/中止后重启可继续)。
                        if let Some(db) = db {
                            for (k, v) in strategy.state_snapshot() {
                                if let Err(e) = db.strategy_state_set(&strategy_name, &k, &v).await
                                {
                                    outcome.persist_errors += 1;
                                    tracing::error!(target: "engine", key = %k, "策略状态落库失败: {e}");
                                }
                            }
                        }
                        // 成交后刷新持仓 (P3: 不做高频轮询, 只在成交后刷; 覆盖式避免陈旧仓位)
                        let refreshed = refresh_positions(
                            &exchange,
                            &market,
                            &pair,
                            &ctx,
                            is_futures,
                            &strategy_name,
                            liq_warn_threshold,
                            notifier.as_ref(),
                        )
                        .await;
                        // 026 时点④: 只落**查到的事实**; None = 查询失败 → 不落库 (不拿陈旧快照冒充现状)
                        if let (Some(db), Some(positions)) = (db, refreshed) {
                            persist_position(
                                db,
                                &mut outcome,
                                &strategy_name,
                                &pair,
                                &positions,
                                mode,
                            )
                            .await;
                        }
                    }
                    // 026 时点③: 撤单/过期/部分成交此前完全不可见 —— 交易所回报的订单状态变化必须落到本地库
                    UserEvent::Order(upd) => {
                        if !is_owned(&upd.client_order_id, &prefix) {
                            continue;
                        }
                        if let Some(db) = db {
                            persist_order_update(db, &mut outcome, &strategy_name, &upd, mode)
                                .await;
                        }
                    }
                },
            }
        }

        // 停机前再拉一次资金费 (014 FR-003): 覆盖停机瞬间前的最后一笔结算
        if is_futures {
            if let Some(db) = db {
                let n = poll_funding_income(&exchange, db, &strategy_name).await;
                if n > 0 {
                    tracing::info!(target: "engine", name = %strategy_name, inserted = n, "停机前补拉资金费流水");
                }
            }
        }
        // ④ 停机清理 (D5 顺序): 停消费 → on_stop → 撤单兜底 → 可选平仓 → 残留复查 → 如实输出; 幂等
        outcome.on_stop_implemented = strategy.has_on_stop();
        strategy.on_stop(&mut ctx);
        // 030 断点续接: 停机时持久化最终状态(不清仓, 状态留给下次启动)。
        if let Some(db) = db {
            for (k, v) in strategy.state_snapshot() {
                if let Err(e) = db.strategy_state_set(&strategy_name, &k, &v).await {
                    outcome.persist_errors += 1;
                    tracing::error!(target: "engine", key = %k, "策略状态落库失败: {e}");
                }
            }
        }
        for fill in ctx.drain_fills() {
            outcome.fills += 1;
            if let Some(db) = db {
                if let Err(e) = db.insert_fill(&strategy_name, &fill).await {
                    outcome.persist_errors += 1;
                    let msg = e.to_string();
                    tracing::error!(target: "engine", name = %strategy_name, "清理成交落库失败: {msg}");
                    outcome.last_error = Some(format!("清理成交落库失败: {msg}"));
                }
                // 026 时点②: 订单状态 + PnL 快照 (与成交同批落库)
                persist_fill_facts(db, &mut outcome, &strategy_name, &fill, ctx.pnl(), mode).await;
            }
            strategy.on_fill(&mut ctx, fill);
        }

        let mut gate = OnceGate::default();
        if gate.enter() {
            let mut cleanup = CleanupOutcome::default();
            // 平仓单号前缀尽量短: 交易所 clientOrderId ≤ 36 字符 (毫秒时间戳 + 策略名前缀会超限)
            let close_cid = format!("{prefix}c{}", chrono::Utc::now().timestamp() % 1_000_000);
            let orders = match exchange.get_open_orders(&pair).await {
                Ok(o) => o,
                Err(e) => {
                    // 查询失败如实记录; 残留判定随之失效 (绝不声称"已清干净")
                    cleanup.cancel_failed.push(("<查询挂单失败>".into(), e.to_string()));
                    Vec::new()
                }
            };
            // 持仓来源按市场分支 (012 回归修复): 现货的"持仓"= base 余额包装单条(spot_position_of,
            // 与成交后刷新同一来源), **不能**用 `get_positions_directional` —— 现货实现对该方法恒返回空
            // (BnSpotExchange::get_position 对现货返回 None), 会导致 `stop --close-all` 静默不平仓。
            let positions = if is_futures {
                match exchange.get_positions_directional(&pair).await {
                    Ok(p) => p,
                    Err(e) => {
                        cleanup.close_notes.push(format!("持仓查询失败: {e}"));
                        Vec::new()
                    }
                }
            } else {
                spot_position_of(&exchange, &market, &pair).await.into_iter().collect()
            };
            let plan = plan_cleanup(
                &orders,
                &prefix,
                &positions,
                &pair,
                close_all_at_stop,
                &market,
                &close_cid,
                hedge,
            );
            cleanup.close_notes = plan.close_notes.clone();
            for o in &plan.foreign {
                tracing::warn!(
                    target: "engine", name = %strategy_name,
                    "非本实例挂单未撤 (只上报): {} size={} side={}",
                    o.client_order_id, o.size, o.side
                );
            }
            for o in &plan.cancels {
                // 单个撤单失败不中断其余 (P4)
                let res = exchange
                    .cancel_order(&pair, &o.client_order_id)
                    .await
                    .map_err(|e| e.to_string());
                cleanup.record_cancel(&o.client_order_id, res);
            }
            for req in plan.closes {
                let cid = req.client_order_id.clone();
                match exchange.place_order(req).await {
                    Ok(ack) => {
                        // 026 时点①: 兜底平仓同样是本实例的委托, 委托价/委托量在提交时落库。
                        // 拒单 ack 的 `exchange_order_id` 是空串(会撞主键) → 与非拒单同判据, 拒单不产生订单行
                        if ack.status != OrderStatus::Rejected {
                            if let Some(db) = db {
                                persist_order_ack(db, &mut outcome, &strategy_name, &ack, mode)
                                    .await;
                            }
                        }
                        cleanup.close_done.push(ack.client_order_id)
                    }
                    Err(e) => cleanup.close_error.push((cid, e.to_string())),
                }
            }
            // 复查残留 (如实, 不修饰)
            match exchange.get_open_orders(&pair).await {
                Ok(after) => cleanup.residual = residual_owned(&after, &prefix),
                Err(e) => cleanup.cancel_failed.push(("<复查挂单失败>".into(), e.to_string())),
            }
            // 残留复查同样按市场分支 (否则现货永远报"残留 0", 与真相反)
            let residual_positions = if is_futures {
                exchange.get_positions_directional(&pair).await.unwrap_or_default()
            } else {
                spot_position_of(&exchange, &market, &pair).await.into_iter().collect()
            };
            cleanup.residual_position = match Ok::<_, ricow_core::CoreError>(residual_positions) {
                Ok(ps) => Some(
                    ps.iter()
                        .map(|p| match p.side {
                            OrderSide::Buy => p.size,
                            OrderSide::Sell => -p.size,
                        })
                        .sum(),
                ),
                Err(_) => None,
            };
            let close_was_sent = !cleanup.close_done.is_empty();
            // 003: 停机残留 → 出站通知 (用户不在终端前时, 这是唯一能看到的告警)
            if let Some(n) = &notifier {
                let residual_pos = cleanup.residual_position.unwrap_or(Decimal::ZERO);
                if !cleanup.residual.is_empty() || residual_pos != Decimal::ZERO {
                    n.notify(NotifyEvent::Residual {
                        orders: cleanup.residual.len(),
                        position: residual_pos,
                    });
                }
            }
            outcome.cleanup = Some(cleanup);

            // ⑤ 兜底平仓的成交只能经用户流送达: 短暂吸干并回写/落库 (否则报表漏记平仓成交)
            if close_was_sent {
                let drained = drain_user_events(
                    &mut user_stream,
                    &mut ctx,
                    strategy.as_mut(),
                    db,
                    &strategy_name,
                    &prefix,
                    &mut outcome,
                    CLEANUP_DRAIN_WINDOW,
                    notifier.as_ref(),
                    mode,
                )
                .await;
                tracing::info!(
                    target: "engine", name = %strategy_name, window_s = CLEANUP_DRAIN_WINDOW.as_secs(),
                    drained, "停机清理后用户流吸干完成"
                );
            }
        }

        if outcome.stop_reason.is_none() {
            outcome.stop_reason = Some(StopReason::StreamEnded);
        }

        let (canceled, cancel_failed, residual, residual_pos) = outcome
            .cleanup
            .as_ref()
            .map(|c| {
                (
                    c.canceled.len(),
                    c.cancel_failed.len(),
                    c.residual.len(),
                    c.residual_position.unwrap_or(Decimal::ZERO),
                )
            })
            .unwrap_or((0, 0, 0, Decimal::ZERO));
        tracing::info!(
            target: "engine", name = %strategy_name,
            reason = %outcome.stop_reason.map(|r| r.to_string()).unwrap_or_default(),
            ticks = outcome.ticks, fills = outcome.fills, errors = outcome.order_errors,
            canceled = canceled, cancel_failed = cancel_failed,
            residual_orders = residual, residual_position = %residual_pos,
            "live run stopped"
        );
        Ok(outcome)
    }

    /// 运行回测: 本地 K 线库数据 → 回测报告。
    pub fn backtest(
        &self,
        config: StrategyConfig,
        initial_balance: Balance,
        klines: &[Kline],
        close_at_end: bool,
    ) -> CoreResult<ricow_strategy::BacktestReport> {
        // 杠杆校验 (方案 A): params 池为三层合并最终源 (CLI 写回; engine/AI 路径缺省 1x/上限 10)。
        // 纯 TOML [backtest] 不经 params 池的直调场景由 CLI 侧校验兜底。
        let lev = config.get_f64("leverage").unwrap_or(1.0);
        let maxl = config.get_f64("max_leverage").unwrap_or(10.0);
        ricow_strategy::BacktestParams::validate_leverage(lev, maxl)?;
        let mut strategy = load_strategy(&config)?;
        Ok(run_backtest(config, initial_balance, klines, strategy.as_mut(), close_at_end))
    }

    /// AI 建策略闭环 (三入口复用): 门禁 → 拉 K 线 → 回测 → 两步确认 preview。
    ///
    /// `params` 为策略配置参数 (config_xxx 读取)。返回 (回测报告, preview_id),
    /// 用户批准后携 token 调 `execute_strategy` 部署。
    pub async fn create_strategy_preview(
        &self,
        db: &Database,
        exchange: Arc<dyn Exchange>,
        name: &str,
        code: &str,
        pair: &str,
        params: HashMap<String, ConfigValue>,
    ) -> CoreResult<(ricow_strategy::BacktestReport, String)> {
        let config =
            crate::create_strategy(name, code, pair, params).map_err(CoreError::InvalidArgument)?;

        // 沙箱回测: 90 天 1h K 线。
        let klines = exchange
            .get_klines(pair, "1h", 90 * 24)
            .await
            .map_err(|e| CoreError::Exchange(e.to_string()))?;
        if klines.is_empty() {
            return Err(CoreError::InvalidArgument(format!("no klines for {pair}")));
        }

        let initial_cash = Decimal::from_f64_retain(
            ricow_strategy::BacktestParams::resolve(
                &config,
                &ricow_strategy::BacktestToml::default(),
            )
            .initial_cash,
        )
        .ok_or_else(|| CoreError::InvalidArgument("initial_cash 非法".into()))?;

        self.backtest_and_preview(db, config, initial_cash, &klines).await
    }

    /// 回测 + 生成两步确认 preview (纯逻辑, 与数据源解耦, 可单元测试)。
    ///
    /// `initial_cash` 为回测本金 (quote), 由调用方按**唯一权威** `BacktestParams::resolve`
    /// (三层合并: 内置默认 < TOML `[backtest]` < CLI/显式覆盖) 解析后传入 —— 引擎不自行取值,
    /// 免得预览报告表头与本金额各算一套 (FR-016 同口径)。
    pub async fn backtest_and_preview(
        &self,
        db: &Database,
        config: StrategyConfig,
        initial_cash: Decimal,
        klines: &[Kline],
    ) -> CoreResult<(ricow_strategy::BacktestReport, String)> {
        // 计价资产: 全项目口径 USDT (bStocks 现货 quote 亦为 USDT, 见 specs/backtest.md §十一) ——
        // 原先硬编码 "USDC" 会让报告打错币种。
        let balance = Balance { asset: "USDT".into(), free: initial_cash, locked: Decimal::ZERO };
        let report = self.backtest(config.clone(), balance, klines, false)?;

        let toml_str = config.to_toml().map_err(|e| CoreError::Parse(e.to_string()))?;
        let preview_id = create_preview(db, "strategy", &toml_str).await?;
        Ok((report, preview_id))
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_core::Kline;
    use rust_decimal_macros::dec;

    fn make_klines(n: u32) -> Vec<Kline> {
        (0..n)
            .map(|i| Kline {
                open_time: chrono::DateTime::from_timestamp(i as i64 * 3600, 0).unwrap(),
                open: dec!(3000),
                high: dec!(3010),
                low: dec!(2990),
                close: Decimal::from(3000 + (i % 10)),
                volume: dec!(100),
                close_time: chrono::DateTime::from_timestamp(i as i64 * 3600 + 3599, 0).unwrap(),
            })
            .collect()
    }

    fn lua_config(name: &str, pair: &str) -> StrategyConfig {
        crate::create_strategy(name, "function on_tick(ctx) return {} end", pair, HashMap::new())
            .unwrap()
    }

    #[tokio::test]
    async fn test_backtest_and_preview_flow() {
        let db = Database::open_in_memory().await.expect("open in-memory db");
        let klines = make_klines(24);

        let (report, preview_id) = Engine::new()
            .backtest_and_preview(&db, lua_config("t-grid", "ETH"), dec!(100_000), &klines)
            .await
            .expect("回测+preview 应成功");

        assert_eq!(report.total_bars, 24);
        assert_eq!(report.total_trades, 0, "空策略不应有成交");
        assert!(!preview_id.is_empty());

        // preview payload 可批准消费并还原为策略配置。
        let token = crate::approve(&db, &preview_id).await.unwrap();
        let payload = crate::consume(&db, &preview_id, &token).await.unwrap();
        let restored = StrategyConfig::from_toml(&payload).unwrap();
        assert_eq!(restored.name, "t-grid");
        assert_eq!(restored.strategy_type, "lua");
    }

    /// FR-016 同口径回归: 回测本金必须**用调用方传入值**, 不得再硬编码 100_000
    /// (否则 `--param cash=<非默认>` 时预览报告表头与实喂本金各说一套)。
    #[tokio::test]
    async fn test_backtest_and_preview_uses_given_initial_cash() {
        let db = Database::open_in_memory().await.expect("open in-memory db");
        let klines = make_klines(24);

        let (report, _) = Engine::new()
            .backtest_and_preview(&db, lua_config("t-cash", "ETH"), dec!(50_000), &klines)
            .await
            .expect("回测+preview 应成功");

        // 空策略无成交无费用 → 期末现金 = 建仓后现金 = 传入本金。
        assert_eq!(report.final_cash, dec!(50_000), "期末现金须等于传入本金");
        assert_eq!(report.base_cash, dec!(50_000), "现金变化基准须等于传入本金");
    }

    // ---- 026 T015: 四时点落库 ----

    fn sample_ack() -> OrderAck {
        OrderAck {
            exchange_order_id: "EX-1".into(),
            client_order_id: "cid-1".into(),
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            price: dec!(3000),
            size: dec!(2),
            filled_size: Decimal::ZERO,
            status: OrderStatus::Open,
        }
    }

    fn sample_fill() -> OrderFill {
        OrderFill {
            trade_id: Some("T-1".into()),
            exchange_order_id: "EX-1".into(),
            client_order_id: "cid-1".into(),
            pair: "ETHUSDT".into(),
            side: OrderSide::Buy,
            fill_price: dec!(3001),
            fill_size: dec!(2),
            fee: dec!(1.2),
            timestamp: Utc::now(),
            position_side: None,
        }
    }

    fn sample_update() -> OrderUpdate {
        OrderUpdate {
            exchange_order_id: "EX-1".into(),
            client_order_id: "cid-1".into(),
            pair: "ETHUSDT".into(),
            status: OrderStatus::Cancelled,
            filled_size: dec!(0.5),
            remaining_size: dec!(1.5),
            avg_price: Some(dec!(3000)),
            timestamp: Utc::now(),
        }
    }

    fn sample_position(side: OrderSide, size: Decimal, entry: Decimal) -> Position {
        Position {
            pair: "ETHUSDT".into(),
            side,
            size,
            entry_price: entry,
            mark_price: entry,
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: None,
        }
    }

    /// 落库口径 = 稳定短串 (`dry_run`/`demo`/`live`), 与界面文案解耦 (D5)。
    #[test]
    fn test_run_mode_db_tag_is_stable_short_string() {
        assert_eq!(RunMode::DryRun.as_str(), "dry_run");
        assert_eq!(RunMode::Demo.as_str(), "demo");
        assert_eq!(RunMode::Live.as_str(), "live");
        // 面向用户的标签必须如实 (绝不把 demo 说成实盘)
        assert_eq!(RunMode::Demo.label(), "测试网模拟盘(demo)");
        assert_eq!(RunMode::Live.label(), "实盘");
    }

    /// 时点①: 委托价/委托量只有此刻拿得到 → 原样落库 (P1)。
    #[test]
    fn test_order_row_of_ack_keeps_delegation_price_and_size() {
        let rec = order_row_of_ack("s1", &sample_ack(), RunMode::Demo, 1000);
        assert_eq!(rec.price, dec!(3000), "price = 委托价");
        assert_eq!(rec.size, dec!(2), "size = 委托量");
        assert_eq!(rec.side, "buy");
        assert_eq!(rec.status, "open");
        assert_eq!(rec.mode, "demo");
    }

    /// 时点③: `OrderUpdate` 不带方向/委托价 → `side` 留空(未知), `size` = 该单总量 (P2)。
    #[test]
    fn test_order_row_of_update_is_honest_about_missing_fields() {
        let rec = order_row_of_update("s1", &sample_update(), RunMode::Live, 1000);
        assert_eq!(rec.side, "", "方向未知 → 留空, 不编造");
        assert_eq!(rec.size, dec!(2), "size = filled + remaining");
        assert_eq!(rec.filled_size, dec!(0.5));
        assert_eq!(rec.status, "cancelled");
        assert_eq!(rec.mode, "live");
    }

    /// 时点④: 净仓 = 多 − 空; 空切片 = 已平 → 落 `size = 0` (不是不落库)。
    #[test]
    fn test_position_row_of_nets_long_minus_short() {
        let long = sample_position(OrderSide::Buy, dec!(3), dec!(3000));
        let short = sample_position(OrderSide::Sell, dec!(1), dec!(3100));
        let rec = position_row_of("s1", "ETHUSDT", &[long, short], RunMode::Demo, 1000);
        assert_eq!(rec.size, dec!(2), "净仓 = 多头 3 − 空头 1");
        assert_eq!(rec.entry_price, dec!(3000), "取首个非零成本价");
        assert_eq!(rec.mode, "demo");

        let flat = position_row_of("s1", "ETHUSDT", &[], RunMode::Demo, 1000);
        assert_eq!(flat.size, Decimal::ZERO, "空切片 = 确实无仓");
    }

    /// SC-003 / FR-008: 四时点依次落库后, 四表行数与数值符合预期; PnL 快照与 `PnlTracker` 同源。
    #[tokio::test]
    async fn test_four_timepoints_persist_expected_rows() {
        let db = Database::open_in_memory().await.expect("open in-memory db");
        let mut outcome = RunOutcome::default();
        let name = "t-026";

        // 时点① 下单提交
        persist_order_ack(&db, &mut outcome, name, &sample_ack(), RunMode::Demo).await;
        let orders = db.recent_orders(Some(name), 10).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].price, dec!(3000));
        assert_eq!(orders[0].size, dec!(2));
        assert_eq!(orders[0].status, "open");

        // 时点② 成交回报 (与既有 insert_fill 同批)
        let fill = sample_fill();
        db.insert_fill(name, &fill).await.unwrap();
        let mut pnl = PnlTracker::default();
        pnl.record_fill(&fill);
        pnl.record_pnl(dec!(12.5));
        persist_fill_facts(&db, &mut outcome, name, &fill, &pnl, RunMode::Demo).await;

        assert_eq!(db.fill_count().await.unwrap(), 1, "fills 表一行");
        let orders = db.recent_orders(Some(name), 10).await.unwrap();
        assert_eq!(orders.len(), 1, "一单一行 (upsert, 不是流水)");
        assert_eq!(orders[0].status, "filled");
        assert_eq!(orders[0].filled_size, dec!(2));

        let snaps = db.recent_pnl_snapshots(Some(name), 10).await.unwrap();
        assert_eq!(snaps.len(), 1, "每笔成交后一条快照");
        assert_eq!(snaps[0].realized_pnl, pnl.realized_pnl(), "与 PnlTracker 同源 (不重算)");
        assert_eq!(snaps[0].fees, pnl.total_fees());
        assert_eq!(snaps[0].net_pnl, pnl.net_pnl());
        assert_eq!(snaps[0].trade_count, pnl.trade_count() as i64);

        let with_mode = db.recent_fills_with_mode(Some(name), 10).await.unwrap();
        assert_eq!(with_mode.len(), 1);
        assert_eq!(with_mode[0].mode.as_deref(), Some("demo"), "mode 由 orders 关联带出");

        // 时点③ 订单状态变化 (撤单): 覆盖状态, 但**不得**抹掉已知方向, 也不得改写委托价
        persist_order_update(&db, &mut outcome, name, &sample_update(), RunMode::Demo).await;
        let orders = db.recent_orders(Some(name), 10).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].status, "cancelled");
        assert_eq!(orders[0].filled_size, dec!(0.5));
        assert_eq!(orders[0].side, "buy", "未知方向不得覆盖已知方向");
        assert_eq!(orders[0].price, dec!(3000), "委托价一经提交不再改写 (P1)");

        // 时点④ 持仓变化
        let long = sample_position(OrderSide::Buy, dec!(2), dec!(3000));
        persist_position(&db, &mut outcome, name, "ETHUSDT", &[long], RunMode::Demo).await;
        let positions = db.current_positions(Some(name)).await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].size, dec!(2));
        assert_eq!(positions[0].entry_price, dec!(3000));

        // 平仓 → size 归零 (仍是"已平", 不是"不落库")
        persist_position(&db, &mut outcome, name, "ETHUSDT", &[], RunMode::Demo).await;
        let positions = db.current_positions(Some(name)).await.unwrap();
        assert_eq!(positions.len(), 1, "已平也留一行 (面板才能显示「已无持仓」)");
        assert_eq!(positions[0].size, Decimal::ZERO);

        assert_eq!(outcome.persist_errors, 0, "正常路径无落库失败");
    }

    /// FR-004 / SC-003: 写库失败**只**计数 + 记 `last_error`, 不上抛、不改变交易台账。
    #[tokio::test]
    async fn test_persist_failures_do_not_break_trading_flow() {
        let db = Database::open_in_memory().await.expect("open in-memory db");
        db.close_pool_for_test().await; // 注入写库错误: 连接池已关闭 → 任何读写都失败

        // 主流程台账: 写库失败不得改动它
        let mut outcome = RunOutcome { fills: 3, ..RunOutcome::default() };
        let pnl = PnlTracker::default();

        persist_order_ack(&db, &mut outcome, "t-026", &sample_ack(), RunMode::Demo).await;
        persist_fill_facts(&db, &mut outcome, "t-026", &sample_fill(), &pnl, RunMode::Demo).await;
        persist_order_update(&db, &mut outcome, "t-026", &sample_update(), RunMode::Demo).await;
        persist_position(&db, &mut outcome, "t-026", "ETHUSDT", &[], RunMode::Demo).await;

        // 时点① 1 次 + 时点② 订单与快照各 1 次 + 时点③ 1 次 + 时点④ 1 次
        assert_eq!(outcome.persist_errors, 5, "每次失败都如实计数");
        assert!(
            outcome.last_error.as_deref().unwrap_or("").contains("落库失败"),
            "如实记录最近一次失败原因: {:?}",
            outcome.last_error
        );
        assert_eq!(outcome.fills, 3, "写库失败不改变交易台账");
        assert_eq!(outcome.order_errors, 0, "落库失败不计入下单失败");
    }

    // ---- 035 T004: 实盘静默期兜底心跳 (共用决策出口 live_decision_tick) ----

    /// 最小测试交易所 (单元测试专用, 不触网): 记录提交的订单并返回 Open ack; 其余不可用。
    struct RecordingExchange(std::sync::Mutex<Vec<OrderRequest>>);

    #[async_trait::async_trait]
    impl Exchange for RecordingExchange {
        fn name(&self) -> &'static str {
            "recording"
        }
        async fn get_markets(&self) -> CoreResult<Vec<Market>> {
            Err(CoreError::InvalidArgument("recording".into()))
        }
        async fn get_klines(&self, _: &str, _: &str, _: u32) -> CoreResult<Vec<Kline>> {
            Err(CoreError::InvalidArgument("recording".into()))
        }
        async fn get_orderbook(&self, _: &str, _: u32) -> CoreResult<ricow_core::OrderBook> {
            Err(CoreError::InvalidArgument("recording".into()))
        }
        async fn place_order(&self, req: OrderRequest) -> CoreResult<OrderAck> {
            self.0.lock().unwrap().push(req);
            Ok(OrderAck {
                exchange_order_id: "EX-HB".into(),
                client_order_id: "hb-cid".into(),
                pair: "ETHUSDT".into(),
                side: OrderSide::Buy,
                price: dec!(2999),
                size: dec!(0.01),
                filled_size: Decimal::ZERO,
                status: OrderStatus::Open,
            })
        }
        async fn cancel_order(&self, _: &str, _: &str) -> CoreResult<()> {
            Err(CoreError::InvalidArgument("recording".into()))
        }
        async fn get_open_orders(&self, _: &str) -> CoreResult<Vec<ricow_core::OrderInfo>> {
            Ok(Vec::new())
        }
        async fn get_balance(&self, _: &str) -> CoreResult<Balance> {
            Err(CoreError::InvalidArgument("recording".into()))
        }
        async fn get_position(&self, _: &str) -> CoreResult<Option<Position>> {
            Ok(None)
        }
        async fn subscribe_orderbook(
            &self,
            _: &str,
        ) -> CoreResult<Pin<Box<dyn Stream<Item = ricow_core::OrderBookUpdate> + Send>>> {
            // 恒静默: 模拟"流活着但无行情更新" (035 待验证的场景)
            Ok(Box::pin(futures::stream::pending()))
        }
        async fn subscribe_user_events(
            &self,
        ) -> CoreResult<Pin<Box<dyn Stream<Item = UserEvent> + Send>>> {
            Ok(Box::pin(futures::stream::pending()))
        }
    }

    /// 035 FR-002: 心跳与 Quote 共用决策出口 —— on_tick 产单被真实提交 (计数 + 交易所收到)。
    /// multi_thread: LiveContext.place_order 走 block_in_place 桥接 async。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_live_decision_tick_calls_on_tick_and_submits() {
        let config = crate::create_strategy(
            "t-hb",
            r#"
            count = 0
            function on_tick(ctx)
                if count == 0 then
                    count = 1
                    return { { pair = "ETHUSDT", side = "buy", size = 0.01, order_type = "limit", price = 2999 } }
                end
                return {}
            end
        "#,
            "ETHUSDT",
            HashMap::new(),
        )
        .unwrap();
        let ex = Arc::new(RecordingExchange(std::sync::Mutex::default()));
        let market = Market {
            symbol: "ETHUSDT".into(),
            base_asset: "ETH".into(),
            quote_asset: "USDT".into(),
            is_perpetual: false,
            min_size: dec!(0.001),
            tick_size: dec!(0.01),
            step_size: None,
            min_notional: None,
            max_leverage: None,
            margin_mode: None,
            is_delisted: false,
        };
        let mut ctx =
            LiveContext::new(ex.clone(), config.clone(), tokio::runtime::Handle::current());
        ctx.set_markets(std::slice::from_ref(&market));
        // 035 FR-003 前置: 已有行情 (心跳只在收到过 quote 后才决策)
        ctx.update_orderbook(
            "ETHUSDT",
            ricow_core::OrderBook::new_sorted(
                vec![ricow_core::PriceLevel { price: dec!(2999.5), size: dec!(10) }],
                vec![ricow_core::PriceLevel { price: dec!(3000.5), size: dec!(10) }],
            ),
        );
        let mut strategy = load_strategy(&config).unwrap();
        let mut outcome = RunOutcome::default();

        live_decision_tick(
            &mut ctx,
            &mut *strategy,
            &mut outcome,
            None,
            "t-hb",
            RunMode::Demo,
            &(ex.clone() as Arc<dyn Exchange>),
            &market,
            "ETHUSDT",
            false,
            0.15,
            None,
        )
        .await;

        assert_eq!(outcome.orders_submitted, 1, "on_tick 产单经共用出口提交");
        assert_eq!(outcome.rejections, 0);
        assert_eq!(
            ex.0.lock().unwrap().len(),
            1,
            "交易所必须真实收到订单 (RecordingExchange 记录)"
        );
        assert_eq!(outcome.ticks, 0, "共用出口不制造行情事实 (FR-003)");
    }
}
