//! 运行上下文: [`Context`] trait + 实盘 [`LiveContext`] + 模拟盘 [`DryRunContext`]。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ricow_core::{
    parse_pair, Balance, CoreError, CoreResult, Exchange, Kline, Market, OrderAck, OrderBook,
    OrderFill, OrderRequest, OrderSide, OrderStatus, OrderType, Position,
};
use rust_decimal::Decimal;

use crate::align::{ownership_prefix, prepare_live_order};
use crate::config::StrategyConfig;
use crate::fee::FeeModel;
use crate::order_guard::OrderGuard;
use crate::pnl::PnlTracker;

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

            let old_side = entry.side;
            match req.side {
                OrderSide::Buy => {
                    if old_side == OrderSide::Sell && entry.size > Decimal::ZERO {
                        let close_size = req.size.min(entry.size);
                        if close_size > Decimal::ZERO && entry.entry_price > Decimal::ZERO {
                            let realized = (entry.entry_price - fill_price) * close_size;
                            self.pnl.record_pnl(realized);
                        }
                        entry.size -= close_size;
                        let remaining = req.size - close_size;
                        if remaining > Decimal::ZERO {
                            entry.side = OrderSide::Buy;
                            entry.entry_price = fill_price;
                            entry.size = remaining;
                        } else if entry.size == Decimal::ZERO {
                            entry.entry_price = Decimal::ZERO;
                            entry.side = OrderSide::Buy;
                        }
                    } else {
                        let old_notional = entry.entry_price * entry.size;
                        entry.size += req.size;
                        if entry.size != Decimal::ZERO {
                            entry.entry_price = (old_notional + fill_price * req.size) / entry.size;
                        }
                    }
                }
                OrderSide::Sell => {
                    if old_side == OrderSide::Buy && entry.size > Decimal::ZERO {
                        let close_size = req.size.min(entry.size);
                        if close_size > Decimal::ZERO && entry.entry_price > Decimal::ZERO {
                            let realized = (fill_price - entry.entry_price) * close_size;
                            self.pnl.record_pnl(realized);
                        }
                        entry.size -= close_size;
                        let remaining = req.size - close_size;
                        if remaining > Decimal::ZERO {
                            entry.side = OrderSide::Sell;
                            entry.entry_price = fill_price;
                            entry.size = remaining;
                        } else if entry.size == Decimal::ZERO {
                            entry.entry_price = Decimal::ZERO;
                            entry.side = OrderSide::Sell;
                        }
                    } else {
                        let old_notional = entry.entry_price * entry.size;
                        entry.size += req.size;
                        if entry.size != Decimal::ZERO {
                            entry.entry_price = (old_notional + fill_price * req.size) / entry.size;
                        }
                    }
                }
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
}
