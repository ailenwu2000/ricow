//! 运行上下文: [`Context`] trait + 实盘 [`LiveContext`] + 模拟盘 [`DryRunContext`]。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ricow_core::{
    parse_pair, Balance, CoreError, CoreResult, Exchange, Kline, Market, OrderAck, OrderAction,
    OrderBook, OrderFill, OrderRequest, OrderSide, OrderStatus, OrderType, Position,
};
use rust_decimal::Decimal;

use crate::align::{ownership_prefix, prepare_live_order};
use crate::config::StrategyConfig;
use crate::fee::FeeModel;
use crate::multiframe::{tf_key, TfCache};
use crate::order_guard::OrderGuard;
use crate::pnl::PnlTracker;

/// 策略的数据需求声明 (由 `need_klines` 写入, 引擎装配阶段读取)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// "primary"(主时钟, 驱动逐 bar 推进, 至多一个) | "aux"(辅助周期)。
    pub role: String,
    /// 周期标签, 如 "15m"/"1h"/"1d"。
    pub tf: String,
    /// 该 tf 所需最少已收盘根数(单位 = 该 tf 自己的根, 不是主时钟根)。
    pub min_bars: u32,
}

/// 策略可见的上下文接口。
///
/// pair 参数支持 `"exchange:pair"` 前缀 (如 `"binance:ETH"`), 无前缀路由到默认交易所。
pub trait Context: Send {
    fn price(&self, pair: &str) -> Option<Decimal>;
    fn orderbook(&self, pair: &str) -> Option<OrderBook>;
    /// 净持仓 (D9): 现货恒 Buy; 合约 one-way 单侧; hedge 模式合并多空后取净 (net 0 → None)。
    fn position(&self, pair: &str) -> Option<Position>;
    /// 带方向持仓查询 (D9/hedge): 返回 (pair, side) 方向仓 (size>0)。
    /// 默认实现 = 不支持 (None); BacktestContext 实现方向仓查询, 实盘后续接入。
    fn position_directional(&self, _pair: &str, _side: OrderSide) -> Option<Position> {
        None
    }
    fn balance(&self, asset: &str) -> Option<Decimal>;
    fn place_order(&mut self, req: OrderRequest) -> CoreResult<OrderAck>;
    fn cancel_order(&mut self, pair: &str, order_id: &str) -> CoreResult<()>;
    fn update_orderbook(&mut self, pair: &str, ob: OrderBook);
    fn record_fill(&mut self, _fill: &OrderFill) {}
    /// 取走上下文内部产生的成交事件 (DryRun 虚拟撮合)。
    fn drain_fills(&mut self) -> Vec<OrderFill> {
        vec![]
    }
    fn log(&self, msg: &str);
    fn pnl(&self) -> &PnlTracker;
    fn config(&self) -> &StrategyConfig;
    fn set_config(&mut self, config: StrategyConfig);
    /// 已收盘 K 线历史 (不含当前未收盘 bar)。默认无通道。
    /// 返回拷贝: 实现方可能持有锁 (DryRun/Live 的 RwLock 缓存), 无法返回内部引用;
    /// 策略 tick 频率低 (分钟/小时级), 拷贝开销可忽略。
    fn klines(&self, _pair: &str) -> Option<Vec<Kline>> {
        None
    }
    /// 当前 tick 时间 (UTC, 无前视: 回测 = 本 tick 已开盘 bar 的 open_time; 实盘 = 当前时刻)。
    /// 默认无通道 (返回 None) → Lua `ctx:now()` 返回 nil, 策略需按 `if t then` 判断。
    /// 用途: 盘中策略按"每日固定时刻"下单 (如美股开盘 1 小时后), 其余 tick 只估值。
    fn now_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }
    /// 声明数据需求(策略 → 引擎): 记录 `(role, tf, min_bars)` 到登记容器, 引擎装配阶段读取。
    /// role = "primary"(主时钟, 至多一个) | "aux"(辅助周期); tf = 周期标签;
    /// min_bars = 该 tf 所需最少已收盘根数(单位 = 该 tf 自己的根)。
    /// 同一 (role, tf) 重复声明取较大 min_bars; 同 role 多个 primary → 报错。
    fn need_klines(&mut self, role: &str, tf: &str, min_bars: u32);

    /// 已声明的数据需求(引擎装配阶段读取)。
    fn declarations(&self) -> Vec<Declaration>;

    /// 高周期(第二序列)K 线: 由 [`Context::set_tf_klines`] 预装, 返回**当前已收盘**的可见前缀
    /// (无前视, 未收盘桶剔除)。未预装 / 无已收盘桶 → None。策略据此显式算 ATR/EMA/close。
    fn tf_klines(&self, pair: &str, tf: &str) -> Option<Vec<Kline>>;

    /// 高周期序列缓存的共享引用 (键 = `pair|tf`)。
    ///
    /// Lua 快照 ([`crate::lua::LuaCtxData`]) 须为 `'static` (mlua UserData), 无法持有 `&dyn Context`,
    /// 故指标 (atr_tf/ema_tf/close_tf) 改为由快照直接调 [`TfCache`] 的缓存 + 尾窗方法 —— 每根
    /// 高周期 bar 只算一次, 且尾窗裁剪避免对全量可见前缀做 O(n²) 重算 (2026-09-25 修回测性能)。
    /// 未预装 → None。默认无通道。
    fn tf_cache_ref(&self, _pair: &str, _tf: &str) -> Option<Arc<TfCache>> {
        None
    }

    /// 预装高周期 K 线 (第二序列)。`tf` = 周期标签(如 `"1h"`); 装配层须先用
    /// [`crate::resample_complete`] 剔除不完整/缺口桶再装入。同一 `pair` 可装多套
    /// (如 4h ATR + 日线趋势判据), 键 = `pair|tf`。默认 no-op。
    fn set_tf_klines(&mut self, _pair: &str, _tf: &str, _bars: Vec<Kline>) {}

    /// 用一笔最新价更新**当前未收盘**主时钟 K 线(实盘/demo 专用)。
    ///
    /// 实盘没有回测那样"逐根推进"的 bar 流, 因此由引擎在每次 tick 时用最新价刷新最后一根:
    /// high/low/close 随价格更新; 跨桶(新的主时钟 K 线开盘)时自动追加新 bar 并裁剪窗口长度。
    /// 默认 no-op —— 回测上下文由 `step_bar` 推进, 不需要它。
    fn tick_kline(&self, _pair: &str, _price: Decimal, _now_ms: i64) {}
}

// ---- LiveContext ----

/// 实盘上下文。`place_order`/`cancel_order` 以阻塞方式桥接 async Exchange。
pub struct LiveContext {
    exchanges: HashMap<String, Arc<dyn Exchange>>,
    default_exchange: String,
    config: StrategyConfig,
    pnl: PnlTracker,
    /// 下单工程护栏 (019-R5): 固定 100 单/秒, 与回测/Dry Run 同一实现; 平台不做投资风控。
    guard: RefCell<OrderGuard>,
    /// 订单号归属前缀 `<策略名>-` (011 D6): 下单前注入, 停机撤单据此只撤本实例的单。
    order_prefix: String,
    /// 交易所过滤器缓存 (启动期 `get_markets` 注入): 下单参数对齐的依据; 缺失则不干预参数。
    markets: RwLock<HashMap<String, Market>>,
    orderbook_cache: RwLock<HashMap<String, OrderBook>>,
    position_cache: RwLock<HashMap<String, Position>>,
    balance_cache: RwLock<HashMap<String, Balance>>,
    klines_cache: RwLock<HashMap<String, Vec<Kline>>>,
    /// 高周期序列缓存 (023 香农 ETF 指数增加策略; 2026-09-18 扩为多套): 键 = `pair|tf`。
    /// 由 `set_tf_klines` 预装(装配层已剔除不完整桶); `tf_klines` 按当前时刻取可见前缀。
    tf_cache: RwLock<HashMap<String, Arc<TfCache>>>,
    /// 策略数据需求声明 (need_klines 写入, 引擎装配阶段读取)。
    declarations: Vec<Declaration>,
    rt: tokio::runtime::Handle,
}

impl LiveContext {
    pub fn new(
        exchange: Arc<dyn Exchange>,
        config: StrategyConfig,
        rt: tokio::runtime::Handle,
    ) -> Self {
        let mut exchanges = HashMap::new();
        exchanges.insert("bn".to_string(), exchange);
        Self::new_multi(exchanges, "bn", config, rt)
    }

    pub fn new_multi(
        exchanges: HashMap<String, Arc<dyn Exchange>>,
        default_exchange: impl Into<String>,
        config: StrategyConfig,
        rt: tokio::runtime::Handle,
    ) -> Self {
        let order_prefix = ownership_prefix(&config.name);
        Self {
            exchanges,
            default_exchange: default_exchange.into(),
            order_prefix,
            guard: RefCell::new(OrderGuard::new()),
            config,
            pnl: PnlTracker::default(),
            markets: RwLock::new(HashMap::new()),
            orderbook_cache: RwLock::new(HashMap::new()),
            position_cache: RwLock::new(HashMap::new()),
            balance_cache: RwLock::new(HashMap::new()),
            klines_cache: RwLock::new(HashMap::new()),
            tf_cache: RwLock::new(HashMap::new()),
            declarations: Vec::new(),
            rt,
        }
    }

    /// 本实例订单号归属前缀 (`<策略名>-`)。
    pub fn order_prefix(&self) -> &str {
        &self.order_prefix
    }

    /// 注入交易所过滤器 (启动装配: `get_markets` 结果) —— 下单参数对齐的依据。
    ///
    /// 未注入的 pair 不干预下单参数 (交交易所判定), 与 plan P8 的降级口径一致。
    pub fn set_markets(&self, markets: &[Market]) {
        if let Ok(mut cache) = self.markets.write() {
            for m in markets {
                cache.insert(m.symbol.to_uppercase(), m.clone());
            }
        }
    }

    /// 取某 pair 的过滤器 (按 symbol 大写匹配; 未缓存 → None)。
    pub fn market_of(&self, pair_base: &str) -> Option<Market> {
        self.markets.read().ok()?.get(&pair_base.to_uppercase()).cloned()
    }

    /// 工程护栏前置检查 (019-R5): 超频 → 返回 `Rejected` ack (不抛错, 策略循环不中断), 并记 warn 日志。
    fn guard_reject(&mut self, req: &OrderRequest) -> Option<OrderAck> {
        let verdict = self.guard.borrow_mut().check(self.now_utc());
        match verdict {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!(
                    target: "order_guard",
                    name = %self.config.name,
                    pair = %req.pair,
                    side = ?req.side,
                    "工程护栏拒单(下单频率超限): {e}"
                );
                Some(crate::order_guard::rejected_ack(req))
            }
        }
    }

    fn resolve_key(&self, pair: &str) -> String {
        let (prefix, base) = parse_pair(pair);
        if prefix.is_empty() {
            format!("{}:{}", self.default_exchange, base)
        } else {
            pair.to_string()
        }
    }

    fn resolve_exchange(&self, prefix: &str) -> CoreResult<Arc<dyn Exchange>> {
        let key = if prefix.is_empty() { &self.default_exchange } else { prefix };
        self.exchanges.get(key).cloned().ok_or_else(|| {
            CoreError::InvalidArgument(format!("exchange prefix '{key}' not configured"))
        })
    }

    /// 写入**单条定向**持仓 (现货恒 Buy; 合约 one-way 按净仓方向; hedge 两侧各调一次用 `set_positions`)。
    pub fn update_position(&self, pair: &str, pos: Position) {
        let key = self.resolve_key(pair);
        let tag = side_tag(pos.side);
        if let Ok(mut cache) = self.position_cache.write() {
            cache.insert(format!("{key}|{tag}"), pos);
        }
    }

    /// **覆盖式**写入该 pair 的定向持仓 (先清空两侧再写入; 空数组 = 平仓后清仓)。
    ///
    /// 引擎装配/停机复核用: 避免"上一次的仓还在缓存里"导致的假持仓。
    pub fn set_positions(&self, pair: &str, positions: &[Position]) {
        let key = self.resolve_key(pair);
        if let Ok(mut cache) = self.position_cache.write() {
            cache.remove(&format!("{key}|buy"));
            cache.remove(&format!("{key}|sell"));
            for p in positions {
                cache.insert(format!("{key}|{}", side_tag(p.side)), p.clone());
            }
        }
    }

    /// 更新已收盘 K 线历史缓存 (由实盘启动侧/数据层填充)。
    pub fn update_klines(&self, pair: &str, klines: Vec<Kline>) {
        if let Ok(mut cache) = self.klines_cache.write() {
            cache.insert(pair.to_string(), klines);
        }
    }

    pub fn update_balance(&self, asset: &str, bal: Balance) {
        if let Ok(mut cache) = self.balance_cache.write() {
            cache.insert(asset.to_string(), bal);
        }
    }

    /// 撤销本实例(`<策略名>-` 前缀归属)在某 pair 上的全部挂单 (023 撤单指令用)。
    ///
    /// 只撤自己的单: 同一账户上可能还有别的策略或手工单, 归属前缀是唯一判据。
    /// 单张撤单失败不中断其余撤单, 但计入 failed 并告警 (不静默)。
    pub fn cancel_owned_orders(&self, pair: &str) -> CoreResult<OrderAck> {
        let (prefix, base) = parse_pair(pair);
        let exchange = self.resolve_exchange(prefix)?;
        let base = base.to_string();
        let owned = self.order_prefix.clone();
        self.run_async(async move {
            let orders = exchange.get_open_orders(&base).await?;
            let mut cancelled = 0usize;
            let mut failed = 0usize;
            for o in orders {
                if !o.client_order_id.starts_with(&owned) {
                    continue;
                }
                match exchange.cancel_order(&base, &o.exchange_order_id).await {
                    Ok(()) => cancelled += 1,
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(
                            target: "strategy.live", pair = %base, order = %o.exchange_order_id,
                            "撤单失败: {e}"
                        );
                    }
                }
            }
            tracing::info!(target: "strategy.live", pair = %base, cancelled, failed, "撤单指令执行");
            Ok(OrderAck {
                exchange_order_id: String::new(),
                client_order_id: String::new(),
                pair: base,
                side: OrderSide::Buy,
                price: Decimal::ZERO,
                size: Decimal::ZERO,
                filled_size: Decimal::ZERO,
                status: OrderStatus::Cancelled,
            })
        })
    }

    /// 从交易所刷新持仓 (覆盖式: 交易所返回的定向持仓为准, 空则清仓)。
    pub async fn refresh_position(&self, pair: &str) -> CoreResult<()> {
        let (prefix, base) = parse_pair(pair);
        let exchange = self.resolve_exchange(prefix)?;
        let positions = exchange.get_positions_directional(base).await?;
        self.set_positions(pair, &positions);
        Ok(())
    }

    pub async fn refresh_balance(&self, asset: &str) -> CoreResult<()> {
        let exchange = self.resolve_exchange("")?;
        let bal = exchange.get_balance(asset).await?;
        self.update_balance(asset, bal);
        Ok(())
    }

    fn run_async<F: std::future::Future>(&self, fut: F) -> F::Output {
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.rt.block_on(fut))
        } else {
            self.rt.block_on(fut)
        }
    }
}

/// 定向持仓缓存键的方向标签。
fn side_tag(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

/// 定向持仓 → **净仓** (纯函数, 便于单测): 多头 − 空头; 净 0 → None。
///
/// 注: 净仓的 mark/unrealized 取占优方向的取值 (合约逐仓下两侧独立, 净仓是派生口径);
/// 精确的双向敞口请用 `position_directional`。
fn net_position(long: Option<Position>, short: Option<Position>) -> Option<Position> {
    match (long, short) {
        (None, None) => None,
        (Some(p), None) | (None, Some(p)) => Some(p),
        (Some(l), Some(s)) => {
            let net = l.size - s.size;
            if net > Decimal::ZERO {
                Some(Position { size: net, ..l })
            } else if net < Decimal::ZERO {
                Some(Position { size: -net, ..s })
            } else {
                None
            }
        }
    }
}

/// 把一笔成交应用到 Dry Run 的**净仓**记录上 (纯函数, 便于单测), 返回该笔成交的**已实现盈亏**。
///
/// 净仓不变式: `size > 0` 时 `side` = 当前持仓方向; `size == 0` 时无方向
/// (下游 `lua::position_side_label` 按 `size <= 0` 报 `none`)。
///
/// 分支语义:
/// - 成交方向与持仓方向相反且持仓非空 → 平仓(可部分); 有剩余量则为**反手**, 按新方向建仓;
/// - 否则(同向加仓 / 当前无持仓) → 加权平均开仓价累加。
fn apply_fill_to_net_position(
    entry: &mut Position,
    side: OrderSide,
    size: Decimal,
    fill_price: Decimal,
) -> Decimal {
    let mut realized = Decimal::ZERO;
    let old_side = entry.side;
    match side {
        OrderSide::Buy => {
            if old_side == OrderSide::Sell && entry.size > Decimal::ZERO {
                let close_size = size.min(entry.size);
                if close_size > Decimal::ZERO && entry.entry_price > Decimal::ZERO {
                    realized = (entry.entry_price - fill_price) * close_size;
                }
                entry.size -= close_size;
                let remaining = size - close_size;
                if remaining > Decimal::ZERO {
                    entry.side = OrderSide::Buy;
                    entry.entry_price = fill_price;
                    entry.size = remaining;
                } else if entry.size == Decimal::ZERO {
                    entry.entry_price = Decimal::ZERO;
                    entry.side = OrderSide::Buy;
                }
            } else {
                // 净仓不变式: 平仓归零后残留的 side(= 平仓方向)不得污染本次开仓方向。
                entry.side = OrderSide::Buy;
                let old_notional = entry.entry_price * entry.size;
                entry.size += size;
                if entry.size != Decimal::ZERO {
                    entry.entry_price = (old_notional + fill_price * size) / entry.size;
                }
            }
        }
        OrderSide::Sell => {
            if old_side == OrderSide::Buy && entry.size > Decimal::ZERO {
                let close_size = size.min(entry.size);
                if close_size > Decimal::ZERO && entry.entry_price > Decimal::ZERO {
                    realized = (fill_price - entry.entry_price) * close_size;
                }
                entry.size -= close_size;
                let remaining = size - close_size;
                if remaining > Decimal::ZERO {
                    entry.side = OrderSide::Sell;
                    entry.entry_price = fill_price;
                    entry.size = remaining;
                } else if entry.size == Decimal::ZERO {
                    entry.entry_price = Decimal::ZERO;
                    entry.side = OrderSide::Sell;
                }
            } else {
                // 净仓不变式: 平仓归零后残留的 side(= 平仓方向)不得污染本次开仓方向。
                entry.side = OrderSide::Sell;
                let old_notional = entry.entry_price * entry.size;
                entry.size += size;
                if entry.size != Decimal::ZERO {
                    entry.entry_price = (old_notional + fill_price * size) / entry.size;
                }
            }
        }
    }
    realized
}

impl Context for LiveContext {
    fn price(&self, pair: &str) -> Option<Decimal> {
        let key = self.resolve_key(pair);
        self.orderbook_cache.read().ok()?.get(&key)?.mid_price()
    }

    fn orderbook(&self, pair: &str) -> Option<OrderBook> {
        let key = self.resolve_key(pair);
        self.orderbook_cache.read().ok()?.get(&key).cloned()
    }

    fn position(&self, pair: &str) -> Option<Position> {
        let key = self.resolve_key(pair);
        let cache = self.position_cache.read().ok()?;
        let long = cache.get(&format!("{key}|buy")).cloned();
        let short = cache.get(&format!("{key}|sell")).cloned();
        net_position(long, short)
    }

    fn balance(&self, asset: &str) -> Option<Decimal> {
        self.balance_cache.read().ok()?.get(asset).map(|b| b.total())
    }

    fn update_orderbook(&mut self, pair: &str, ob: OrderBook) {
        let key = self.resolve_key(pair);
        if let Ok(mut cache) = self.orderbook_cache.write() {
            cache.insert(key, ob);
        }
    }

    fn record_fill(&mut self, fill: &OrderFill) {
        self.pnl.record_fill(fill);
    }

    /// 定向持仓查询 (009/012): 现货恒 Buy; 合约 one-way 按净仓方向; hedge 两侧独立可见。
    fn position_directional(&self, pair: &str, side: OrderSide) -> Option<Position> {
        let key = self.resolve_key(pair);
        self.position_cache.read().ok()?.get(&format!("{key}|{}", side_tag(side))).cloned()
    }

    fn place_order(&mut self, req: OrderRequest) -> CoreResult<OrderAck> {
        // 撤单指令 (023): 必须在**对齐层与风控之前**短路 —— size = 0 会被 `align` 判
        // "对齐后数量为 0"直接拒单, 撤单永远执行不到; 撤单不占限频、不计 rejected_count。
        if req.action == OrderAction::CancelPending {
            return self.cancel_owned_orders(&req.pair);
        }
        let (prefix, base) = parse_pair(&req.pair);
        let (prefix, base) = (prefix.to_string(), base.to_string());
        // 拒单 ack 模板 (对齐失败时返回, 与风控拒单同形: 策略循环不中断)
        let rejected_ack = OrderAck {
            exchange_order_id: String::new(),
            client_order_id: req.client_order_id.clone(),
            pair: req.pair.clone(),
            side: req.side,
            price: req.price.unwrap_or(Decimal::ZERO),
            size: req.size,
            filled_size: Decimal::ZERO,
            status: OrderStatus::Rejected,
        };
        // ① 实盘下单前处理: 注入归属前缀 + 按已缓存过滤器对齐 (无缓存则不干预)
        let market = self.market_of(&base);
        let req = match prepare_live_order(req, market.as_ref(), &self.order_prefix) {
            Ok((r, adjustments)) => {
                for a in &adjustments {
                    tracing::warn!(
                        target: "align", name = %self.config.name, pair = %r.pair,
                        "下单参数已对齐: {a}"
                    );
                }
                r
            }
            Err(e) => {
                // 数量/名义不足交易所最小值: 以 Rejected ack 返回 (原因入日志, 不静默丢弃)
                tracing::warn!(
                    target: "align", name = %self.config.name, pair = %base,
                    "对齐拒单: {e}"
                );
                return Ok(rejected_ack);
            }
        };
        // ② 工程护栏 (019-R5): 对齐后再校验 (看到的是实际下单量)
        if let Some(rejected) = self.guard_reject(&req) {
            return Ok(rejected);
        }
        let exchange = self.resolve_exchange(&prefix)?;
        self.run_async(async move { exchange.place_order(req).await })
    }

    fn cancel_order(&mut self, pair: &str, order_id: &str) -> CoreResult<()> {
        let (prefix, base) = parse_pair(pair);
        let exchange = self.resolve_exchange(prefix)?;
        let base = base.to_string();
        let order_id = order_id.to_string();
        self.run_async(async move { exchange.cancel_order(&base, &order_id).await })
    }

    fn log(&self, msg: &str) {
        tracing::info!(target: "strategy", name = %self.config.name, "{msg}");
    }

    fn pnl(&self) -> &PnlTracker {
        &self.pnl
    }

    fn config(&self) -> &StrategyConfig {
        &self.config
    }

    fn set_config(&mut self, config: StrategyConfig) {
        self.config = config;
    }

    fn klines(&self, pair: &str) -> Option<Vec<Kline>> {
        self.klines_cache.read().ok()?.get(pair).cloned()
    }

    fn need_klines(&mut self, role: &str, tf: &str, min_bars: u32) {
        if role == "primary" && self.declarations.iter().any(|d| d.role == "primary") {
            tracing::error!(target: "context", tf, "主时钟(primary)重复声明: 至多一个 primary 序列");
            return;
        }
        if let Some(existing) = self
            .declarations
            .iter_mut()
            .find(|d| d.role == role && d.tf == tf)
        {
            existing.min_bars = existing.min_bars.max(min_bars);
            return;
        }
        self.declarations.push(Declaration { role: role.to_string(), tf: tf.to_string(), min_bars });
    }

    fn declarations(&self) -> Vec<Declaration> {
        self.declarations.clone()
    }

    fn tf_klines(&self, pair: &str, tf: &str) -> Option<Vec<Kline>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.tf_cache.read().ok()?.get(&tf_key(pair, tf)).map(|c| c.visible(now_ms).to_vec())
    }

    fn tf_cache_ref(&self, pair: &str, tf: &str) -> Option<Arc<TfCache>> {
        self.tf_cache.read().ok()?.get(&tf_key(pair, tf)).cloned()
    }

    /// 实盘/demo: 用最新价维护"当前未收盘"主时钟 K 线(周期 = 策略声明的 primary)。
    /// 这是实盘侧 `ctx:klines` 能从空到有的关键 —— 此前实盘上下文的 klines_cache 无人填充,
    /// 导致策略在第一个守卫(K 线)就 return, 永不动作(2026-09-24 demo 实测定位)。
    fn tick_kline(&self, pair: &str, price: Decimal, now_ms: i64) {
        // 主时钟周期 = 策略声明的 primary; 未声明 → no-op(如实暴露, 不猜默认)。
        let Some(tf) = self
            .declarations
            .iter()
            .find(|d| d.role == "primary")
            .map(|d| d.tf.clone())
        else {
            tracing::warn!(target: "context", "策略未声明 primary 主时钟, tick_kline 跳过");
            return;
        };
        let Some(step) = crate::tf_ms_of(&tf) else {
            return;
        };
        let bucket_ms = now_ms / step * step;
        let mut opened_new_bar = false;
        {
        let Ok(mut cache) = self.klines_cache.write() else {
            return;
        };
        let v = cache.entry(pair.to_string()).or_default();
        let cur = v.last().map(|k| k.open_time.timestamp_millis());
        match cur {
            // 同一根未收盘 bar: 更新 high/low/close
            Some(b) if b == bucket_ms => {
                if let Some(last) = v.last_mut() {
                    if price > last.high {
                        last.high = price;
                    }
                    if price < last.low {
                        last.low = price;
                    }
                    last.close = price;
                }
            }
            // 乱序/过期的 tick: 忽略
            Some(b) if b > bucket_ms => {}
            // 跨桶: 开新 bar(open = 该 tick 的价格), 并裁剪窗口
            _ => {
                let open_time = chrono::DateTime::from_timestamp_millis(bucket_ms)
                    .unwrap_or_else(chrono::Utc::now);
                let close_time = chrono::DateTime::from_timestamp_millis(bucket_ms + step - 1)
                    .unwrap_or_else(chrono::Utc::now);
                v.push(Kline {
                    open_time,
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume: Decimal::ZERO,
                    close_time,
                });
                let max_keep = 1500usize;
                if v.len() > max_keep {
                    let drop_n = v.len() - max_keep;
                    v.drain(0..drop_n);
                }
                opened_new_bar = true;
            }
        }
        }
        // 跨桶 = 上一根主时钟 K 线已收盘 -> 重采样刷新高周期缓存。
        // 不刷新的话 atr_tf / close_tf / ema_tf 会一直用启动时那份序列(长跑后失真)。
        // 必须在**释放 klines_cache 写锁之后**调用, 否则与本函数的读锁互等(死锁)。
        if opened_new_bar {
            self.refresh_tf_cache(pair);
        }
    }


    fn set_tf_klines(&mut self, pair: &str, tf: &str, bars: Vec<Kline>) {
        let Some(tf_ms) = crate::multiframe::tf_ms_of(tf) else {
            tracing::warn!(target: "multiframe", tf, "未知高周期标签, 忽略预装");
            return;
        };
        if let Ok(mut c) = self.tf_cache.write() {
            c.insert(tf_key(pair, tf), Arc::new(TfCache::new(tf_ms, bars)));
        }
    }

    /// 实盘当前时刻 (真实 UTC): 与风控窗口/结算周期、`ctx:now()` 同源。
    fn now_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(chrono::Utc::now())
    }
}

// ---- DryRunContext ----

/// 模拟盘上下文 — 虚拟撮合。
///
/// 撮合规则:
/// - 市价单按对手方最优价成交;
/// - 限价单与对手方最优价交叉时按对手价成交, 否则挂入 pending;
/// - pending 在每次 `update_orderbook` 时 FIFO 重新撮合;
/// - reduce_only 仅减少反向持仓。
pub struct DryRunContext {
    exchanges: HashMap<String, Arc<dyn Exchange>>,
    default_exchange: String,
    config: StrategyConfig,
    pnl: PnlTracker,
    fee_model: FeeModel,
    quote_asset: String,
    /// 下单工程护栏 (019-R5): 固定 100 单/秒, 与回测/实盘同一实现; 平台不做投资风控。
    guard: RefCell<OrderGuard>,
    orderbook_cache: RwLock<HashMap<String, OrderBook>>,
    virtual_positions: RwLock<HashMap<String, Position>>,
    virtual_balance: RwLock<HashMap<String, Balance>>,
    klines_cache: RwLock<HashMap<String, Vec<Kline>>>,
    /// 高周期序列缓存: 键 = `pair|tf`。由 `set_tf_klines` 预装(装配层已剔除不完整桶);
    /// 策略通过 `tf_klines(pair, tf)` 按声明读取, 可见前缀受当前时刻裁剪(无前视)。
    tf_cache: RwLock<HashMap<String, Arc<TfCache>>>,
    /// 策略数据需求声明 (need_klines 写入, 引擎装配阶段读取)。
    declarations: Vec<Declaration>,
    pending_orders: Vec<(String, OrderRequest)>,
    fill_queue: Vec<OrderFill>,
}

impl DryRunContext {
    pub fn new(
        exchange: Arc<dyn Exchange>,
        config: StrategyConfig,
        initial_balance: Balance,
    ) -> Self {
        let mut exchanges = HashMap::new();
        exchanges.insert("bn".to_string(), exchange);
        Self::new_multi(exchanges, "bn", config, initial_balance)
    }

    pub fn new_multi(
        exchanges: HashMap<String, Arc<dyn Exchange>>,
        default_exchange: impl Into<String>,
        config: StrategyConfig,
        initial_balance: Balance,
    ) -> Self {
        let quote_asset = initial_balance.asset.clone();
        let mut balance_map = HashMap::new();
        balance_map.insert(initial_balance.asset.clone(), initial_balance);
        Self {
            exchanges,
            default_exchange: default_exchange.into(),
            guard: RefCell::new(OrderGuard::new()),
            config,
            pnl: PnlTracker::default(),
            fee_model: FeeModel::default(),
            quote_asset,
            orderbook_cache: RwLock::new(HashMap::new()),
            virtual_positions: RwLock::new(HashMap::new()),
            virtual_balance: RwLock::new(balance_map),
            klines_cache: RwLock::new(HashMap::new()),
            tf_cache: RwLock::new(HashMap::new()),
            declarations: Vec::new(),
            pending_orders: Vec::new(),
            fill_queue: Vec::new(),
        }
    }

    /// 工程护栏前置检查 (019-R5): 超频 → 返回 `Rejected` ack (不抛错, 策略循环不中断), 并记 warn 日志。
    fn guard_reject(&mut self, req: &OrderRequest) -> Option<OrderAck> {
        let verdict = self.guard.borrow_mut().check(self.now_utc());
        match verdict {
            Ok(()) => None,
            Err(e) => {
                tracing::warn!(
                    target: "order_guard",
                    name = %self.config.name,
                    pair = %req.pair,
                    side = ?req.side,
                    "工程护栏拒单(下单频率超限): {e}"
                );
                Some(crate::order_guard::rejected_ack(req))
            }
        }
    }

    fn resolve_key(&self, pair: &str) -> String {
        let (prefix, base) = parse_pair(pair);
        if prefix.is_empty() {
            format!("{}:{}", self.default_exchange, base)
        } else {
            pair.to_string()
        }
    }

    fn resolve_exchange(&self, prefix: &str) -> CoreResult<Arc<dyn Exchange>> {
        let key = if prefix.is_empty() { &self.default_exchange } else { prefix };
        self.exchanges.get(key).cloned().ok_or_else(|| {
            CoreError::InvalidArgument(format!("exchange prefix '{key}' not configured"))
        })
    }

    /// 从交易所拉取历史 K 线。
    pub async fn fetch_klines(
        &self,
        pair: &str,
        interval: &str,
        limit: u32,
    ) -> CoreResult<Vec<ricow_core::Kline>> {
        let (prefix, base) = parse_pair(pair);
        let exchange = self.resolve_exchange(prefix)?;
        exchange.get_klines(base, interval, limit).await
    }

    pub fn virtual_position(&self, pair: &str) -> Option<Position> {
        let key = self.resolve_key(pair);
        self.virtual_positions.read().ok()?.get(&key).cloned()
    }

    pub fn virtual_balance(&self, asset: &str) -> Option<Balance> {
        self.virtual_balance.read().ok()?.get(asset).cloned()
    }

    pub fn pending_order_count(&self) -> usize {
        self.pending_orders.len()
    }

    /// 更新已收盘 K 线历史缓存 (由数据层/启动侧填充)。
    pub fn update_klines(&self, pair: &str, klines: Vec<Kline>) {
        if let Ok(mut cache) = self.klines_cache.write() {
            cache.insert(pair.to_string(), klines);
        }
    }

    /// 尝试撮合: 可成交返回 Some(成交价), 否则 None。
    fn try_match(&self, req: &OrderRequest) -> Option<Decimal> {
        let ob = self.orderbook(&req.pair)?;
        let opposite = match req.side {
            OrderSide::Buy => ob.best_ask()?.price,
            OrderSide::Sell => ob.best_bid()?.price,
        };
        match req.order_type {
            // 市价单按对手方最优价成交 (盘口实时价)。与回测语义对齐:
            // 回测无盘口, 以 bar.open ± slippage_bps 近似 (backtest.rs try_match_ohlc),
            // 实盘等价: 对手价 ≈ 实时价 ± 滑点。
            OrderType::Market => Some(opposite),
            OrderType::Limit => {
                let limit = req.price?;
                let crossed = match req.side {
                    OrderSide::Buy => limit >= opposite,
                    OrderSide::Sell => limit <= opposite,
                };
                if crossed {
                    Some(opposite)
                } else {
                    None
                }
            }
        }
    }

    /// reduce_only 订单的有效数量 (仅减少反向持仓)。
    fn reduce_only_size(&self, req: &OrderRequest) -> Option<Decimal> {
        let pos = self.position(&req.pair)?;
        if pos.side == req.side || pos.size <= Decimal::ZERO {
            return None;
        }
        Some(req.size.min(pos.size))
    }

    /// FIFO 重新撮合 pending 订单。
    fn match_pending(&mut self, pair: &str) {
        let resolved_pair = self.resolve_key(pair);
        let matched: Vec<(String, OrderRequest, Decimal)> = self
            .pending_orders
            .iter()
            .filter(|(_, req)| self.resolve_key(&req.pair) == resolved_pair)
            .filter_map(|(id, req)| self.try_match(req).map(|px| (id.clone(), req.clone(), px)))
            .collect();

        for (id, mut req, fill_price) in matched {
            self.pending_orders.retain(|(pid, _)| *pid != id);
            if req.reduce_only {
                match self.reduce_only_size(&req) {
                    Some(sz) => req.size = sz,
                    None => continue,
                }
            }
            self.execute_fill(&id, &req, fill_price);
        }
    }

    /// 执行成交: 更新虚拟持仓 + 余额, 记录 fill 到 PnL。
    fn execute_fill(&mut self, order_id: &str, req: &OrderRequest, fill_price: Decimal) {
        self.apply_position_change(req, fill_price);
        self.apply_virtual_balance_change(req, fill_price);

        let is_maker = matches!(req.order_type, OrderType::Limit);
        let fee = self.fee_model.calc_fee(fill_price, req.size, is_maker);
        let fill = OrderFill {
            trade_id: Some(uuid::Uuid::new_v4().to_string()),
            exchange_order_id: order_id.to_string(),
            client_order_id: req.client_order_id.clone(),
            pair: req.pair.clone(),
            side: req.side,
            fill_price,
            fill_size: req.size,
            fee,
            timestamp: chrono::Utc::now(),
        };
        self.pnl.record_fill(&fill);
        self.fill_queue.push(fill);
    }

    fn apply_position_change(&mut self, req: &OrderRequest, fill_price: Decimal) {
        if let Ok(mut pos_cache) = self.virtual_positions.write() {
            let key = self.resolve_key(&req.pair);
            let entry = pos_cache.entry(key.clone()).or_insert_with(|| Position {
                pair: key,
                side: req.side,
                size: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                mark_price: fill_price,
                liquidation_price: None,
                unrealized_pnl: Decimal::ZERO,
                leverage: None,
            });

            let realized = apply_fill_to_net_position(entry, req.side, req.size, fill_price);
            if realized != Decimal::ZERO {
                self.pnl.record_pnl(realized);
            }
            entry.mark_price = fill_price;
        }
    }

    fn apply_virtual_balance_change(&self, req: &OrderRequest, price: Decimal) {
        if let Ok(mut bal_cache) = self.virtual_balance.write() {
            let base = parse_pair(&req.pair).1.to_string();
            let quote = self.quote_asset.clone();

            bal_cache.entry(base.clone()).or_insert_with(|| Balance {
                asset: base.clone(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            bal_cache.entry(quote.clone()).or_insert_with(|| Balance {
                asset: quote.clone(),
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });

            match req.side {
                OrderSide::Buy => {
                    let notional = req.size * price;
                    let sufficient =
                        bal_cache.get(&quote).map(|q| q.free >= notional).unwrap_or(false);
                    if sufficient {
                        if let Some(q) = bal_cache.get_mut(&quote) {
                            q.free -= notional;
                        }
                        if let Some(b) = bal_cache.get_mut(&base) {
                            b.free += req.size;
                        }
                    }
                }
                OrderSide::Sell => {
                    let sufficient =
                        bal_cache.get(&base).map(|b| b.free >= req.size).unwrap_or(false);
                    if sufficient {
                        let notional = req.size * price;
                        if let Some(b) = bal_cache.get_mut(&base) {
                            b.free -= req.size;
                        }
                        if let Some(q) = bal_cache.get_mut(&quote) {
                            q.free += notional;
                        }
                    }
                }
            }
        }
    }
}

impl Context for DryRunContext {
    fn price(&self, pair: &str) -> Option<Decimal> {
        let key = self.resolve_key(pair);
        self.orderbook_cache.read().ok()?.get(&key)?.mid_price()
    }

    fn orderbook(&self, pair: &str) -> Option<OrderBook> {
        let key = self.resolve_key(pair);
        self.orderbook_cache.read().ok()?.get(&key).cloned()
    }

    fn position(&self, pair: &str) -> Option<Position> {
        let key = self.resolve_key(pair);
        self.virtual_positions.read().ok()?.get(&key).cloned()
    }

    fn balance(&self, asset: &str) -> Option<Decimal> {
        self.virtual_balance.read().ok()?.get(asset).map(|b| b.total())
    }

    fn update_orderbook(&mut self, pair: &str, ob: OrderBook) {
        let key = self.resolve_key(pair);
        if let Ok(mut cache) = self.orderbook_cache.write() {
            cache.insert(key, ob);
        }
        self.match_pending(pair);
    }

    fn record_fill(&mut self, fill: &OrderFill) {
        self.pnl.record_fill(fill);
    }

    fn drain_fills(&mut self) -> Vec<OrderFill> {
        std::mem::take(&mut self.fill_queue)
    }

    fn place_order(&mut self, req: OrderRequest) -> CoreResult<OrderAck> {
        // 撤单指令 (023): 在风控之前短路(size = 0 会被判无效); 清掉本策略挂单。
        if req.action == OrderAction::CancelPending {
            // 注意: `pending_orders` 的元素是 `(订单号, 请求)` —— 按**请求里的 pair** 过滤
            // (先收集订单号再 retain, 避免闭包里再借 self)。
            let key = self.resolve_key(&req.pair);
            let ids: Vec<String> = self
                .pending_orders
                .iter()
                .filter(|(_, o)| self.resolve_key(&o.pair) == key)
                .map(|(id, _)| id.clone())
                .collect();
            let before = self.pending_orders.len();
            self.pending_orders.retain(|(id, _)| !ids.contains(id));
            let cancelled = before - self.pending_orders.len();
            tracing::info!(target: "strategy.dryrun", pair = %key, cancelled, "撤单指令: 清挂单");
            return Ok(OrderAck {
                exchange_order_id: String::new(),
                client_order_id: String::new(),
                pair: req.pair,
                side: req.side,
                price: Decimal::ZERO,
                size: Decimal::ZERO,
                filled_size: Decimal::ZERO,
                status: OrderStatus::Cancelled,
            });
        }
        if let Some(rejected) = self.guard_reject(&req) {
            return Ok(rejected);
        }
        let exchange_order_id = uuid::Uuid::new_v4().to_string();
        let mut req = req;

        if req.reduce_only {
            match self.reduce_only_size(&req) {
                Some(sz) => req.size = sz,
                None => {
                    return Ok(OrderAck {
                        exchange_order_id,
                        client_order_id: req.client_order_id.clone(),
                        pair: req.pair.clone(),
                        side: req.side,
                        price: req.price.unwrap_or(Decimal::ZERO),
                        size: req.size,
                        filled_size: Decimal::ZERO,
                        status: OrderStatus::Rejected,
                    });
                }
            }
        }

        let ack = match self.try_match(&req) {
            Some(fill_price) => {
                self.execute_fill(&exchange_order_id, &req, fill_price);
                OrderAck {
                    exchange_order_id: exchange_order_id.clone(),
                    client_order_id: req.client_order_id.clone(),
                    pair: req.pair.clone(),
                    side: req.side,
                    price: fill_price,
                    size: req.size,
                    filled_size: req.size,
                    status: OrderStatus::Filled,
                }
            }
            None => {
                let ack = OrderAck {
                    exchange_order_id: exchange_order_id.clone(),
                    client_order_id: req.client_order_id.clone(),
                    pair: req.pair.clone(),
                    side: req.side,
                    price: req.price.unwrap_or(Decimal::ZERO),
                    size: req.size,
                    filled_size: Decimal::ZERO,
                    status: OrderStatus::Open,
                };
                self.pending_orders.push((exchange_order_id, req));
                ack
            }
        };

        tracing::info!(
            target: "strategy.dryrun",
            name = %self.config.name,
            pair = %ack.pair,
            side = ?ack.side,
            price = %ack.price,
            status = ?ack.status,
            "dry run order placed"
        );
        Ok(ack)
    }

    fn cancel_order(&mut self, _pair: &str, order_id: &str) -> CoreResult<()> {
        let before = self.pending_orders.len();
        self.pending_orders.retain(|(id, _)| id != order_id);
        if self.pending_orders.len() < before {
            Ok(())
        } else {
            Err(CoreError::OrderNotFound(order_id.to_string()))
        }
    }

    fn log(&self, msg: &str) {
        tracing::info!(target: "strategy.dryrun", name = %self.config.name, "{msg}");
    }

    fn pnl(&self) -> &PnlTracker {
        &self.pnl
    }

    fn config(&self) -> &StrategyConfig {
        &self.config
    }

    fn set_config(&mut self, config: StrategyConfig) {
        self.config = config;
    }

    fn klines(&self, pair: &str) -> Option<Vec<Kline>> {
        self.klines_cache.read().ok()?.get(pair).cloned()
    }

    fn need_klines(&mut self, role: &str, tf: &str, min_bars: u32) {
        if role == "primary" && self.declarations.iter().any(|d| d.role == "primary") {
            tracing::error!(target: "context", tf, "主时钟(primary)重复声明: 至多一个 primary 序列");
            return;
        }
        if let Some(existing) = self
            .declarations
            .iter_mut()
            .find(|d| d.role == role && d.tf == tf)
        {
            existing.min_bars = existing.min_bars.max(min_bars);
            return;
        }
        self.declarations.push(Declaration { role: role.to_string(), tf: tf.to_string(), min_bars });
    }

    fn declarations(&self) -> Vec<Declaration> {
        self.declarations.clone()
    }

    fn tf_klines(&self, pair: &str, tf: &str) -> Option<Vec<Kline>> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.tf_cache.read().ok()?.get(&tf_key(pair, tf)).map(|c| c.visible(now_ms).to_vec())
    }

    fn tf_cache_ref(&self, pair: &str, tf: &str) -> Option<Arc<TfCache>> {
        self.tf_cache.read().ok()?.get(&tf_key(pair, tf)).cloned()
    }

    /// 实盘/demo: 用最新价维护"当前未收盘"主时钟 K 线(周期 = 策略声明的 primary)。
    /// 这是实盘侧 `ctx:klines` 能从空到有的关键 —— 此前实盘上下文的 klines_cache 无人填充,
    /// 导致策略在第一个守卫(K 线)就 return, 永不动作(2026-09-24 demo 实测定位)。
    fn tick_kline(&self, pair: &str, price: Decimal, now_ms: i64) {
        // 主时钟周期 = 策略声明的 primary; 未声明 → no-op(如实暴露, 不猜默认)。
        let Some(tf) = self
            .declarations
            .iter()
            .find(|d| d.role == "primary")
            .map(|d| d.tf.clone())
        else {
            tracing::warn!(target: "context", "策略未声明 primary 主时钟, tick_kline 跳过");
            return;
        };
        let Some(step) = crate::tf_ms_of(&tf) else {
            return;
        };
        let bucket_ms = now_ms / step * step;
        let mut opened_new_bar = false;
        {
        let Ok(mut cache) = self.klines_cache.write() else {
            return;
        };
        let v = cache.entry(pair.to_string()).or_default();
        let cur = v.last().map(|k| k.open_time.timestamp_millis());
        match cur {
            // 同一根未收盘 bar: 更新 high/low/close
            Some(b) if b == bucket_ms => {
                if let Some(last) = v.last_mut() {
                    if price > last.high {
                        last.high = price;
                    }
                    if price < last.low {
                        last.low = price;
                    }
                    last.close = price;
                }
            }
            // 乱序/过期的 tick: 忽略
            Some(b) if b > bucket_ms => {}
            // 跨桶: 开新 bar(open = 该 tick 的价格), 并裁剪窗口
            _ => {
                let open_time = chrono::DateTime::from_timestamp_millis(bucket_ms)
                    .unwrap_or_else(chrono::Utc::now);
                let close_time = chrono::DateTime::from_timestamp_millis(bucket_ms + step - 1)
                    .unwrap_or_else(chrono::Utc::now);
                v.push(Kline {
                    open_time,
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume: Decimal::ZERO,
                    close_time,
                });
                let max_keep = 1500usize;
                if v.len() > max_keep {
                    let drop_n = v.len() - max_keep;
                    v.drain(0..drop_n);
                }
                opened_new_bar = true;
            }
        }
        }
        // 跨桶 = 上一根主时钟 K 线已收盘 -> 重采样刷新高周期缓存。
        // 不刷新的话 atr_tf / close_tf / ema_tf 会一直用启动时那份序列(长跑后失真)。
        // 必须在**释放 klines_cache 写锁之后**调用, 否则与本函数的读锁互等(死锁)。
        if opened_new_bar {
            self.refresh_tf_cache(pair);
        }
    }


    fn set_tf_klines(&mut self, pair: &str, tf: &str, bars: Vec<Kline>) {
        let Some(tf_ms) = crate::multiframe::tf_ms_of(tf) else {
            tracing::warn!(target: "multiframe", tf, "未知高周期标签, 忽略预装");
            return;
        };
        if let Ok(mut c) = self.tf_cache.write() {
            c.insert(tf_key(pair, tf), Arc::new(TfCache::new(tf_ms, bars)));
        }
    }

    /// Dry Run 当前时刻 (真实 UTC, 与实盘同源): 风控窗口/结算周期与 `ctx:now()` 用。
    fn now_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(chrono::Utc::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn pos(side: OrderSide, size: Decimal) -> Position {
        Position {
            pair: "ETHUSDT".into(),
            side,
            size,
            entry_price: dec!(2500),
            mark_price: dec!(2500),
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: None,
        }
    }

    #[test]
    fn test_net_position_aggregates_both_sides() {
        assert!(net_position(None, None).is_none());
        let long_only = net_position(Some(pos(OrderSide::Buy, dec!(0.5))), None).unwrap();
        assert_eq!(long_only.side, OrderSide::Buy);
        assert_eq!(long_only.size, dec!(0.5));

        // hedge: 多 0.5 空 0.2 → 净多 0.3
        let net = net_position(
            Some(pos(OrderSide::Buy, dec!(0.5))),
            Some(pos(OrderSide::Sell, dec!(0.2))),
        )
        .unwrap();
        assert_eq!(net.side, OrderSide::Buy);
        assert_eq!(net.size, dec!(0.3));

        // 空占优 → 净空
        let net_short = net_position(
            Some(pos(OrderSide::Buy, dec!(0.1))),
            Some(pos(OrderSide::Sell, dec!(0.4))),
        )
        .unwrap();
        assert_eq!(net_short.side, OrderSide::Sell);
        assert_eq!(net_short.size, dec!(0.3));

        // 两侧相抵 → 无净仓
        assert!(net_position(
            Some(pos(OrderSide::Buy, dec!(0.2))),
            Some(pos(OrderSide::Sell, dec!(0.2)))
        )
        .is_none());
    }

    #[test]
    fn test_side_tag_is_distinct() {
        assert_ne!(side_tag(OrderSide::Buy), side_tag(OrderSide::Sell));
        assert_eq!(side_tag(OrderSide::Buy), "buy");
        assert_eq!(side_tag(OrderSide::Sell), "sell");
    }

    // ---- 净仓方向记账 (024 F5): 平仓后残留 side 不得污染下一次开仓 ----

    /// FR-007①: 平多归零后反向卖出 → 必须建空仓。
    #[test]
    fn test_open_short_after_closing_long() {
        let mut p = pos(OrderSide::Buy, dec!(1));
        let realized = apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(1), dec!(2600));
        assert_eq!(realized, dec!(100));
        assert_eq!(p.size, Decimal::ZERO);
        assert_eq!(p.entry_price, Decimal::ZERO);

        apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(0.4), dec!(2550));
        assert_eq!(p.side, OrderSide::Sell);
        assert_eq!(p.size, dec!(0.4));
        assert_eq!(p.entry_price, dec!(2550));
    }

    /// FR-007②: 平空归零后反向买入 → 必须建多仓。
    #[test]
    fn test_open_long_after_closing_short() {
        let mut p = pos(OrderSide::Sell, dec!(1));
        let realized = apply_fill_to_net_position(&mut p, OrderSide::Buy, dec!(1), dec!(2400));
        assert_eq!(realized, dec!(100));
        assert_eq!(p.size, Decimal::ZERO);
        assert_eq!(p.entry_price, Decimal::ZERO);

        apply_fill_to_net_position(&mut p, OrderSide::Buy, dec!(0.4), dec!(2450));
        assert_eq!(p.side, OrderSide::Buy);
        assert_eq!(p.size, dec!(0.4));
        assert_eq!(p.entry_price, dec!(2450));
    }

    /// FR-007③: 平多归零后同向再开多 → 方向必须翻回 Buy 且开仓价重算(修复前为伪空头)。
    #[test]
    fn test_reopen_long_after_closing_long() {
        let mut p = pos(OrderSide::Buy, dec!(1));
        let realized = apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(1), dec!(2400));
        assert_eq!(realized, dec!(-100));
        assert_eq!(p.size, Decimal::ZERO);

        apply_fill_to_net_position(&mut p, OrderSide::Buy, dec!(0.5), dec!(2450));
        assert_eq!(p.side, OrderSide::Buy);
        assert_eq!(p.size, dec!(0.5));
        assert_eq!(p.entry_price, dec!(2450));
    }

    /// FR-007③ 对称面: 平空归零后同向再开空。
    #[test]
    fn test_reopen_short_after_closing_short() {
        let mut p = pos(OrderSide::Sell, dec!(1));
        apply_fill_to_net_position(&mut p, OrderSide::Buy, dec!(1), dec!(2600));
        assert_eq!(p.size, Decimal::ZERO);

        apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(0.5), dec!(2550));
        assert_eq!(p.side, OrderSide::Sell);
        assert_eq!(p.size, dec!(0.5));
        assert_eq!(p.entry_price, dec!(2550));
    }

    /// FR-007④: 部分平仓保留剩余方向与开仓价, 且记已实现盈亏。
    #[test]
    fn test_partial_close_keeps_side_and_entry() {
        let mut p = pos(OrderSide::Buy, dec!(2));
        let realized = apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(0.5), dec!(2600));
        assert_eq!(realized, dec!(50));
        assert_eq!(p.side, OrderSide::Buy);
        assert_eq!(p.size, dec!(1.5));
        assert_eq!(p.entry_price, dec!(2500));

        let mut s = pos(OrderSide::Sell, dec!(2));
        let realized = apply_fill_to_net_position(&mut s, OrderSide::Buy, dec!(0.5), dec!(2400));
        assert_eq!(realized, dec!(50));
        assert_eq!(s.side, OrderSide::Sell);
        assert_eq!(s.size, dec!(1.5));
        assert_eq!(s.entry_price, dec!(2500));
    }

    /// FR-007⑤: 反手(成交量 > 当前持仓量) → 余量按新方向建仓, 开仓价取新成交价。
    #[test]
    fn test_reversal_larger_than_position() {
        let mut p = pos(OrderSide::Buy, dec!(1));
        let realized = apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(1.5), dec!(2600));
        assert_eq!(realized, dec!(100));
        assert_eq!(p.side, OrderSide::Sell);
        assert_eq!(p.size, dec!(0.5));
        assert_eq!(p.entry_price, dec!(2600));

        let mut s = pos(OrderSide::Sell, dec!(1));
        let realized = apply_fill_to_net_position(&mut s, OrderSide::Buy, dec!(1.5), dec!(2400));
        assert_eq!(realized, dec!(100));
        assert_eq!(s.side, OrderSide::Buy);
        assert_eq!(s.size, dec!(0.5));
        assert_eq!(s.entry_price, dec!(2400));
    }

    /// 023 §七 现象 A 回归: 反复"平多 → 同向再开多"不得让 side 停在 Sell、也不得让 size 单调放大。
    #[test]
    fn test_repeated_open_close_never_leaves_stale_side() {
        let mut p = pos(OrderSide::Buy, dec!(0.27));
        for _ in 0..5 {
            apply_fill_to_net_position(&mut p, OrderSide::Sell, dec!(0.27), dec!(2500));
            assert_eq!(p.size, Decimal::ZERO);

            apply_fill_to_net_position(&mut p, OrderSide::Buy, dec!(0.27), dec!(2500));
            assert_eq!(p.side, OrderSide::Buy);
            assert_eq!(p.size, dec!(0.27));
        }
    }
}

impl LiveContext {
    /// 用主时钟 K 线刷新高周期缓存(策略声明的全部 tf, 含 primary 与 aux)。
    fn refresh_tf_cache(&self, pair: &str) {
        let bars = {
            let Ok(kc) = self.klines_cache.read() else {
                return;
            };
            match kc.get(pair) {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if bars.is_empty() {
            return;
        }
        // 目标周期 = 策略声明的全部 tf, 去重 —— 不读任何策略参数名。
        let mut targets: Vec<String> = Vec::new();
        for d in &self.declarations {
            if !targets.iter().any(|t| t == &d.tf) {
                targets.push(d.tf.clone());
            }
        }
        if let Ok(mut tfc) = self.tf_cache.write() {
            for tf in targets {
                let Some(tf_ms) = crate::multiframe::tf_ms_of(&tf) else {
                    continue;
                };
                let resampled = crate::multiframe::resample_complete(&bars, tf_ms);
                tfc.insert(tf_key(pair, &tf), Arc::new(TfCache::new(tf_ms, resampled)));
            }
        }
    }
}

impl DryRunContext {
    /// 用主时钟 K 线刷新高周期缓存(策略声明的全部 tf, 含 primary 与 aux)。
    fn refresh_tf_cache(&self, pair: &str) {
        let bars = {
            let Ok(kc) = self.klines_cache.read() else {
                return;
            };
            match kc.get(pair) {
                Some(b) => b.clone(),
                None => return,
            }
        };
        if bars.is_empty() {
            return;
        }
        // 目标周期 = 策略声明的全部 tf, 去重 —— 不读任何策略参数名。
        let mut targets: Vec<String> = Vec::new();
        for d in &self.declarations {
            if !targets.iter().any(|t| t == &d.tf) {
                targets.push(d.tf.clone());
            }
        }
        if let Ok(mut tfc) = self.tf_cache.write() {
            for tf in targets {
                let Some(tf_ms) = crate::multiframe::tf_ms_of(&tf) else {
                    continue;
                };
                let resampled = crate::multiframe::resample_complete(&bars, tf_ms);
                tfc.insert(tf_key(pair, &tf), Arc::new(TfCache::new(tf_ms, resampled)));
            }
        }
    }
}
