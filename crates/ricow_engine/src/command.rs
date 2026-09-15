//! 命令分发: 组合 loader / market / backtest / keys 完成引擎动作。

use std::collections::HashMap;
use std::sync::Arc;

use std::pin::Pin;
use std::time::Duration;

use chrono::Utc;
use futures::{Stream, StreamExt};
use ricow_core::{
    Balance, CoreError, CoreResult, Exchange, Kline, Market, OrderSide, OrderStatus, Position,
    UserEvent,
};
use ricow_strategy::{
    is_owned, ConfigValue, Context, Database, DryRunContext, LiveContext, Strategy, StrategyConfig,
};
use rust_decimal::Decimal;

use crate::backtest_runner::run_backtest;
use crate::confirm::create_preview;
use crate::live::{plan_cleanup, residual_owned, CleanupOutcome, OnceGate};
use crate::loader::load_strategy;
use crate::market;
use crate::notify::{NotifyEvent, Notifier};

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

/// 停机清理后吸干用户流的窗口: 兜底平仓的成交只能靠用户流送达 (少了它会漏记成交)。
const CLEANUP_DRAIN_WINDOW: Duration = Duration::from_secs(5);

/// 资金费补拉回溯窗口 (014 D3): 7 天 —— 资金费 8h 一条, 足够覆盖停机时长。
const FUNDING_LOOKBACK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 资金费增量拉取间隔 (014 D4): 30 分钟 —— 资金费 8h 结算一次, 该粒度足够且无变现频压力。
const FUNDING_POLL_SECS: u64 = 1800;

/// 实盘双流事件 (行情 / 用户数据流 / 停机信号 / 资金费对账)。
enum LiveEvent {
    Stop(Option<StopRequest>),
    Quote(Option<ricow_core::OrderBookUpdate>),
    User(Option<UserEvent>),
    /// 资金费增量拉取 (014 FR-003): 不驱动策略 tick, 只对账落库。
    Funding,
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
            Err(e) => tracing::warn!(target: "engine", name = %strategy_name, "资金费落库失败: {e}"),
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
            Ok(Some(_)) => {}
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

/// 成交后刷新持仓: 合约走定向持仓覆盖; 现货走 base 可用余额包装 (空余额 = 清仓, 不留陈旧仓)。
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
) {
    if is_futures {
        match exchange.get_positions_directional(pair).await {
            Ok(ps) => {
                warn_near_liquidation(&ps, strategy_name, pair, liq_warn_threshold, notifier);
                ctx.set_positions(pair, &ps)
            }
            Err(e) => {
                tracing::warn!(target: "engine", pair = %pair, "成交后刷新合约持仓失败: {e}")
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
    /// 成交落库失败数
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

        let mut ctx = DryRunContext::new(exchange.clone(), config.clone(), initial_balance);
        let mut strategy = load_strategy(&config)?;
        strategy.on_init(&mut ctx);

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

            let orders = strategy.on_tick(&mut ctx);
            for req in orders {
                outcome.orders_submitted += 1;
                match ctx.place_order(req) {
                    Ok(ack) if ack.status == OrderStatus::Rejected => {
                        // 风控/参数对齐拒单: 与"下单失败"区分计数 (策略循环不中断)
                        outcome.rejections += 1;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // 下单失败如实记录并计数 (不再静默吞掉)
                        outcome.order_errors += 1;
                        let msg = e.to_string();
                        tracing::error!(target: "engine", name = %strategy_name, "下单失败: {msg}");
                        outcome.last_error = Some(format!("下单失败: {msg}"));
                    }
                }
            }

            let fills = ctx.drain_fills();
            outcome.fills += fills.len() as u64;
            for fill in fills {
                if let Some(db) = db {
                    if let Err(e) = db.insert_fill(&strategy_name, &fill).await {
                        outcome.persist_errors += 1;
                        let msg = e.to_string();
                        tracing::error!(target: "engine", name = %strategy_name, "成交落库失败: {msg}");
                        outcome.last_error = Some(format!("成交落库失败: {msg}"));
                    }
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
            }
        }

        // 停机清理: 策略实现了 on_stop 才会调 (是否实现由脚本决定, 引擎不虚构清理行为)
        outcome.on_stop_implemented = strategy.has_on_stop();
        strategy.on_stop(&mut ctx);

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
        // 运行模式标签(如 "实盘" / "测试网模拟盘(demo)"): 日志必须如实标注, 不得把 demo 说成实盘
        mode_label: &str,
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
        strategy.on_init(&mut ctx);
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
        tracing::info!(target: "engine", name = %strategy_name, pair = %pair, mode = %mode_label, "live run started");

        let mut outcome = RunOutcome::default();
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
                },
                None => tokio::select! {
                    upd = quote_stream.next() => LiveEvent::Quote(upd),
                    ev = user_stream.next() => LiveEvent::User(ev),
                    _ = funding_tick.tick() => LiveEvent::Funding,
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
                    let orders = strategy.on_tick(&mut ctx);
                    let mut any_filled = false;
                    for req in orders {
                        outcome.orders_submitted += 1;
                        match ctx.place_order(req) {
                            Ok(ack) if ack.status == OrderStatus::Rejected => {
                                outcome.rejections += 1;
                            }
                            Ok(ack) => {
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
                        refresh_positions(
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
                    }
                }
                LiveEvent::User(None) => {
                    // 用户数据流断线: 成交无法回灌 → 异常停机 (plan P1), 不静默继续
                    outcome.stop_reason = Some(StopReason::StreamEnded);
                    outcome.last_error = Some("用户数据流中断: 成交无法回灌, 已按异常停机".into());
                    break;
                }
                LiveEvent::User(Some(ev)) => {
                    if let UserEvent::Fill(fill) = ev {
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
                        // 成交后刷新持仓 (P3: 不做高频轮询, 只在成交后刷; 覆盖式避免陈旧仓位)
                        refresh_positions(
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
                    }
                }
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
        for fill in ctx.drain_fills() {
            outcome.fills += 1;
            if let Some(db) = db {
                if let Err(e) = db.insert_fill(&strategy_name, &fill).await {
                    outcome.persist_errors += 1;
                    let msg = e.to_string();
                    tracing::error!(target: "engine", name = %strategy_name, "清理成交落库失败: {msg}");
                    outcome.last_error = Some(format!("清理成交落库失败: {msg}"));
                }
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
                    Ok(ack) => cleanup.close_done.push(ack.client_order_id),
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
    ) -> CoreResult<ricow_strategy::BacktestReport> {
        // 杠杆校验 (方案 A): params 池为三层合并最终源 (CLI 写回; engine/AI 路径缺省 1x/上限 10)。
        // 纯 TOML [backtest] 不经 params 池的直调场景由 CLI 侧校验兜底。
        let lev = config.get_f64("leverage").unwrap_or(1.0);
        let maxl = config.get_f64("max_leverage").unwrap_or(10.0);
        ricow_strategy::BacktestParams::validate_leverage(lev, maxl)?;
        let mut strategy = load_strategy(&config)?;
        Ok(run_backtest(config, initial_balance, klines, strategy.as_mut()))
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

        self.backtest_and_preview(db, config, &klines).await
    }

    /// 回测 + 生成两步确认 preview (纯逻辑, 与数据源解耦, 可单元测试)。
    pub async fn backtest_and_preview(
        &self,
        db: &Database,
        config: StrategyConfig,
        klines: &[Kline],
    ) -> CoreResult<(ricow_strategy::BacktestReport, String)> {
        // 计价资产: 全项目口径 USDT (bStocks 现货 quote 亦为 USDT, 见 specs/backtest.md §十一) —— 
        // 原先硬编码 "USDC" 会让报告打错币种。
        let balance =
            Balance { asset: "USDT".into(), free: Decimal::from(100_000), locked: Decimal::ZERO };
        let report = self.backtest(config.clone(), balance, klines)?;

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
            .backtest_and_preview(&db, lua_config("t-grid", "ETH"), &klines)
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
}
