//! 运行上下文: [`Context`] trait + 实盘 [`LiveContext`] + 模拟盘 [`DryRunContext`]。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use ricow_core::{
    parse_pair, Balance, CoreError, CoreResult, Exchange, Kline, Market, OrderAck, OrderAction,
    OrderBook, OrderFill, OrderRequest, OrderSide, OrderStatus, OrderType, Position,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::align::{ownership_prefix, prepare_live_order};
use crate::backtest::isolated_liq_price;
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
    /// 逐仓钱包余额合计 (合约; 无钱包概念端恒 0)。
    ///
    /// 合约权益口径必须含逐仓钱包: hedge 逐仓模式下开仓保证金从现金划入钱包,
    /// 漏加钱包会把"锁定保证金"当成消失的权益 → 权益/WARN 虚低 (032 复审)。
    /// 默认 0 = 该端无钱包概念 (现货/DryRun/Live); BacktestContext 覆写返回钱包合计。
    fn wallets_total(&self) -> Decimal {
        Decimal::ZERO
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
    /// 实盘订单号单调计数器 (032): Lua 产出的下单请求 client_order_id 为空, 若原样注入归属前缀,
    /// 则**同一 tick 批量挂单**(如网格重挂的买+卖两张)会拿到完全相同的 clientOrderId → 交易所拒
    /// `ClientOrderId is duplicated`。下单前给空 cid 打 `{毫秒}{seq}` 唯一戳, 保证批量内/跨重启都不撞。
    order_seq: u64,
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
            order_seq: 0,
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
        // 032: 空订单号打唯一戳 (毫秒 + 单调序号), 避免同 tick 批量挂单撞 clientOrderId。
        // 归属前缀稍后由 prepare_live_order 注入, 故此处只保证"批量内互不相同"。
        let mut req = req;
        if req.client_order_id.is_empty() {
            self.order_seq = self.order_seq.wrapping_add(1);
            req.client_order_id =
                format!("{}{}", chrono::Utc::now().timestamp_millis(), self.order_seq);
        }
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
        if let Some(existing) = self.declarations.iter_mut().find(|d| d.role == role && d.tf == tf)
        {
            existing.min_bars = existing.min_bars.max(min_bars);
            return;
        }
        self.declarations.push(Declaration {
            role: role.to_string(),
            tf: tf.to_string(),
            min_bars,
        });
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
        let Some(tf) = self.declarations.iter().find(|d| d.role == "primary").map(|d| d.tf.clone())
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
    /// 合约逐侧保证金钱包 (032 DryRun hedge): 键 = 方向仓键 (`{pair}|{side_tag}`), 值 = 冻结名义/杠杆。
    /// 现货/one-way 不用 (净仓模型)。开仓冻结、平仓回笼、两侧全平转回现金。
    wallets: HashMap<String, Decimal>,
    /// 已 WARN 过爆仓价穿越的方向仓键 (防 update_orderbook 每次刷盘口都刷屏)。
    liq_warned: std::collections::HashSet<String>,
    /// 合约杠杆 (DryRun 显示/保证金口径; 来自 config.backtest.leverage → params.leverage → 1.0)。
    leverage: Decimal,
    /// 维持保证金率 (比率; 来自 params.mmr_pct/100, 装配层注入 tier1 查表值, 审核 M5)。
    mmr: Decimal,
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
        let is_futures = config.market.eq_ignore_ascii_case("futures");
        // 合约参数 (032): 杠杆 = params.leverage (装配层由 [backtest].leverage 回写, 缺省 1.0);
        // MMR = params.mmr_pct/100 (装配层注入 tier1 查表值, 与回测同源, 审核 M5; 缺省 1.0% 保守)。
        let leverage = config
            .get_f64("leverage")
            .and_then(Decimal::from_f64_retain)
            .filter(|d| *d >= Decimal::ONE)
            .unwrap_or(Decimal::ONE);
        let mmr = config
            .get_f64("mmr_pct")
            .and_then(Decimal::from_f64_retain)
            .map(|d| d / dec!(100))
            .filter(|d| *d > Decimal::ZERO)
            .unwrap_or(dec!(0.01));
        // 合约费率 (USDT-M 默认 maker 2 / taker 5 bps; params 可覆盖)。
        let fee_model = if is_futures {
            let maker = config.get_f64("fee_maker_bps").unwrap_or(2.0);
            let taker = config.get_f64("fee_taker_bps").unwrap_or(5.0);
            FeeModel::new(
                Decimal::from_f64_retain(maker).unwrap_or(dec!(2)),
                Decimal::from_f64_retain(taker).unwrap_or(dec!(5)),
            )
        } else {
            FeeModel::default()
        };
        Self {
            exchanges,
            default_exchange: default_exchange.into(),
            guard: RefCell::new(OrderGuard::new()),
            config,
            pnl: PnlTracker::default(),
            fee_model,
            quote_asset,
            orderbook_cache: RwLock::new(HashMap::new()),
            virtual_positions: RwLock::new(HashMap::new()),
            virtual_balance: RwLock::new(balance_map),
            wallets: HashMap::new(),
            liq_warned: std::collections::HashSet::new(),
            leverage,
            mmr,
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

    /// 是否合约市场 (032 DryRun hedge)。
    fn is_futures(&self) -> bool {
        self.config.market.eq_ignore_ascii_case("futures")
    }

    /// 是否双向持仓模式 (hedge)。one-way 保持裸键 (单净仓, 与旧行为一致)。
    fn is_hedge(&self) -> bool {
        self.is_futures() && self.config.position_mode.eq_ignore_ascii_case("hedge")
    }

    /// 方向仓键 (hedge): `{pair}|{side_tag}`; 现货/one-way = 裸 pair 键。
    fn pos_key(&self, pair: &str, side: OrderSide) -> String {
        if self.is_hedge() {
            format!("{}|{}", self.resolve_key(pair), side_tag(side))
        } else {
            self.resolve_key(pair)
        }
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
    /// hedge (032): 按 `position_side` 指定的**该侧**封顶 (平多只减多仓); 无定向回落净仓语义。
    fn reduce_only_size(&self, req: &OrderRequest) -> Option<Decimal> {
        if self.is_hedge() {
            let target = match req.position_side.as_deref() {
                Some("long") => Some(OrderSide::Buy),
                Some("short") => Some(OrderSide::Sell),
                // 无定向 reduce_only: 平反向侧 (buy 单平空 / sell 单平多)。
                _ => Some(match req.side {
                    OrderSide::Buy => OrderSide::Sell,
                    OrderSide::Sell => OrderSide::Buy,
                }),
            };
            let avail = self.side_size(&req.pair, target?);
            if avail <= Decimal::ZERO {
                return None;
            }
            return Some(req.size.min(avail));
        }
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
        let is_maker = matches!(req.order_type, OrderType::Limit);
        let fee = self.fee_model.calc_fee(fill_price, req.size, is_maker);
        if self.is_hedge() {
            // 合约 hedge (032): quote 保证金 + 按侧仓, 与回测 K3 钱包模型同构。
            self.apply_hedge_fill(req, fill_price, fee);
        } else {
            self.apply_position_change(req, fill_price);
            self.apply_virtual_balance_change(req, fill_price);
        }

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
            // hedge: 透传请求方向仓 (on_fill 路由依据); 现货/one-way 为 None (032)。
            position_side: req.position_side.clone(),
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

    // ============================ 032: DryRun 合约 hedge ============================

    /// 单方向仓数量 (hedge 按侧键; 无仓 = 0)。
    fn side_size(&self, pair: &str, side: OrderSide) -> Decimal {
        let key = self.pos_key(pair, side);
        self.virtual_positions
            .read()
            .ok()
            .and_then(|c| c.get(&key).map(|p| p.size))
            .unwrap_or(Decimal::ZERO)
    }

    /// 成交效果拆分 (与回测 fill_effect 同语义): (平仓方向, 开仓方向)。
    /// position_side=long/short 显式定向; 无定向 = one-way 净仓语义 (先平反向再开同向)。
    fn hedge_effect(&self, req: &OrderRequest) -> (Option<OrderSide>, Option<OrderSide>) {
        match (req.position_side.as_deref(), req.side) {
            (Some("long"), OrderSide::Buy) => (None, Some(OrderSide::Buy)),
            (Some("long"), OrderSide::Sell) => (Some(OrderSide::Buy), None),
            (Some("short"), OrderSide::Sell) => (None, Some(OrderSide::Sell)),
            (Some("short"), OrderSide::Buy) => (Some(OrderSide::Sell), None),
            (_, OrderSide::Buy) => (Some(OrderSide::Sell), Some(OrderSide::Buy)),
            (_, OrderSide::Sell) => (Some(OrderSide::Buy), Some(OrderSide::Sell)),
        }
    }

    /// 刷新某方向仓的 unrealized (mark 口径) 与爆仓价穿越告警。
    /// 自由函数: 只借用 `warned` 字段, 与 `virtual_positions` 写锁不冲突 (032)。
    fn refresh_side_mark(
        key: &str,
        p: &mut Position,
        warned: &mut std::collections::HashSet<String>,
    ) {
        p.unrealized_pnl = match p.side {
            OrderSide::Buy => (p.mark_price - p.entry_price) * p.size,
            OrderSide::Sell => (p.entry_price - p.mark_price) * p.size,
        };
        // 穿越爆仓价 (显示口径): 只 WARN 不平仓 (DryRun 不模拟强平, spec 范围外)。
        if let Some(liq) = p.liquidation_price {
            let crossed = match p.side {
                OrderSide::Buy => p.mark_price <= liq,
                OrderSide::Sell => p.mark_price >= liq,
            };
            if crossed && warned.insert(key.to_string()) {
                tracing::warn!(
                    target: "strategy.dryrun",
                    position = %key,
                    mark = %p.mark_price,
                    liq = %liq,
                    "标记价穿越爆仓价 (DryRun 不模拟强平, 仅告警)"
                );
            } else if !crossed {
                warned.remove(key);
            }
        }
    }

    /// hedge 成交记账 (032): quote 保证金 + 按侧独立仓, 与回测 K3 钱包模型同构 (无 LIFO 批次, 加权均价)。
    fn apply_hedge_fill(&mut self, req: &OrderRequest, fill_price: Decimal, fee: Decimal) {
        let (close_side, open_side) = self.hedge_effect(req);
        let total_notional = req.size * fill_price;
        let mut remain = req.size;
        let mut close_fee = Decimal::ZERO;
        if let Some(cs) = close_side {
            let avail = self.side_size(&req.pair, cs);
            let sz = remain.min(avail);
            if sz > Decimal::ZERO {
                remain -= sz;
                close_fee = fee * (sz * fill_price) / total_notional;
                self.hedge_close(&req.pair, cs, sz, fill_price, close_fee);
            }
        }
        if !req.reduce_only {
            if let Some(os) = open_side {
                if remain > Decimal::ZERO {
                    let open_fee = fee - close_fee;
                    self.hedge_open(&req.pair, os, remain, fill_price, open_fee);
                }
            }
        }
        // locked 汇总 = Σ wallets (balance(quote) 口径: free=可用现金, locked=冻结保证金)。
        let total_wallet: Decimal = self.wallets.values().sum();
        let quote = self.quote_asset.clone();
        if let Ok(mut bal) = self.virtual_balance.write() {
            if let Some(q) = bal.get_mut(&quote) {
                q.locked = total_wallet;
            }
        }
    }

    /// hedge 开/加仓: 现金划入该侧钱包 (M = 名义/杠杆), 加权均价, 公式爆仓价。
    fn hedge_open(
        &mut self,
        pair: &str,
        side: OrderSide,
        size: Decimal,
        fill_price: Decimal,
        fee: Decimal,
    ) {
        let margin = size * fill_price / self.leverage;
        let quote = self.quote_asset.clone();
        let sufficient =
            self.virtual_balance.read().ok().and_then(|b| b.get(&quote).map(|q| q.free >= margin))
                == Some(true);
        if !sufficient {
            tracing::warn!(
                target: "strategy.dryrun",
                pair = %pair, "保证金不足 (free < 名义/杠杆), 开仓跳过 (DryRun 资金校验)"
            );
            return;
        }
        {
            let Ok(mut bal) = self.virtual_balance.write() else { return };
            if let Some(q) = bal.get_mut(&quote) {
                q.free -= margin;
            }
        }
        let key = self.pos_key(pair, side);
        let wallet = self.wallets.entry(key.clone()).or_insert(Decimal::ZERO);
        *wallet += margin - fee;
        let lev = self.leverage;
        let mmr = self.mmr;
        let mut guard = self.virtual_positions.write().ok();
        if let Some(cache) = guard.as_mut() {
            let e = cache.entry(key.clone()).or_insert_with(|| Position {
                pair: pair.to_string(),
                side,
                size: Decimal::ZERO,
                entry_price: Decimal::ZERO,
                mark_price: fill_price,
                liquidation_price: None,
                unrealized_pnl: Decimal::ZERO,
                leverage: Some(lev),
            });
            let old_notional = e.entry_price * e.size;
            e.size += size;
            e.entry_price = (old_notional + fill_price * size) / e.size;
            e.mark_price = fill_price;
            e.liquidation_price = isolated_liq_price(e.entry_price, side, lev, mmr);
            Self::refresh_side_mark(&key, e, &mut self.liq_warned);
        }
    }

    /// hedge 平仓: 已实现盈亏与平仓费走该侧钱包; 归零删键; 两侧全平 → 钱包转回现金。
    fn hedge_close(
        &mut self,
        pair: &str,
        side: OrderSide,
        size: Decimal,
        fill_price: Decimal,
        fee: Decimal,
    ) {
        let key = self.pos_key(pair, side);
        let realized = {
            let Ok(cache) = self.virtual_positions.read() else { return };
            match cache.get(&key) {
                Some(p) => {
                    let sz = size.min(p.size);
                    match p.side {
                        OrderSide::Buy => (fill_price - p.entry_price) * sz,
                        OrderSide::Sell => (p.entry_price - fill_price) * sz,
                    }
                }
                None => Decimal::ZERO,
            }
        };
        if realized != Decimal::ZERO {
            self.pnl.record_pnl(realized);
        }
        let wallet = self.wallets.entry(key.clone()).or_insert(Decimal::ZERO);
        *wallet += realized - fee;
        let lev = self.leverage;
        let mmr = self.mmr;
        let mut sweep_sides: Vec<OrderSide> = Vec::new();
        {
            let Ok(mut cache) = self.virtual_positions.write() else { return };
            if let Some(p) = cache.get_mut(&key) {
                p.size = (p.size - size).max(Decimal::ZERO);
                p.mark_price = fill_price;
                if p.size <= Decimal::ZERO {
                    p.entry_price = Decimal::ZERO;
                    p.unrealized_pnl = Decimal::ZERO;
                    p.liquidation_price = None;
                    cache.remove(&key);
                    self.liq_warned.remove(&key);
                    sweep_sides = vec![OrderSide::Buy, OrderSide::Sell];
                } else {
                    p.liquidation_price = isolated_liq_price(p.entry_price, side, lev, mmr);
                    Self::refresh_side_mark(&key, p, &mut self.liq_warned);
                }
            }
        }
        // 该对两侧全平 → 各侧钱包余额转回现金 (K3 同构)。
        if !sweep_sides.is_empty() {
            let flat = self.side_size(pair, OrderSide::Buy).is_zero()
                && self.side_size(pair, OrderSide::Sell).is_zero();
            if flat {
                let quote = self.quote_asset.clone();
                for s in [OrderSide::Buy, OrderSide::Sell] {
                    let k = self.pos_key(pair, s);
                    if let Some(w) = self.wallets.remove(&k) {
                        if let Ok(mut bal) = self.virtual_balance.write() {
                            if let Some(q) = bal.get_mut(&quote) {
                                q.free += w;
                            }
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
        // hedge (032): 两侧独立键 → 合并净仓 (net 0 → None); 现货/one-way = 裸键单仓。
        let base = self.resolve_key(pair);
        let cache = self.virtual_positions.read().ok()?;
        if self.is_hedge() {
            let long = cache.get(&format!("{base}|buy")).cloned();
            let short = cache.get(&format!("{base}|sell")).cloned();
            return net_position(long, short);
        }
        cache.get(&base).cloned()
    }

    fn position_directional(&self, pair: &str, side: OrderSide) -> Option<Position> {
        // hedge: 按侧键返回该方向仓 (size>0); 现货/one-way 回落裸键 (仅 Buy = 净仓)。
        let key = self.pos_key(pair, side);
        let p = self.virtual_positions.read().ok()?.get(&key).cloned()?;
        if p.size > Decimal::ZERO {
            Some(p)
        } else {
            None
        }
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
        // 合约 (032): 盘口更新 → 刷该对方向仓 mark/unrealized + 穿越告警 (DryRun 无 bar 收盘)。
        if self.is_futures() {
            if let Some(px) = self.price(pair) {
                let base = self.resolve_key(pair);
                let sides: Vec<String> = if self.is_hedge() {
                    vec![format!("{base}|buy"), format!("{base}|sell")]
                } else {
                    vec![base]
                };
                if let Ok(mut cache) = self.virtual_positions.write() {
                    for k in sides {
                        if let Some(p) = cache.get_mut(&k) {
                            if p.size > Decimal::ZERO {
                                p.mark_price = px;
                                Self::refresh_side_mark(&k, p, &mut self.liq_warned);
                            }
                        }
                    }
                }
            }
        }
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
        if let Some(existing) = self.declarations.iter_mut().find(|d| d.role == role && d.tf == tf)
        {
            existing.min_bars = existing.min_bars.max(min_bars);
            return;
        }
        self.declarations.push(Declaration {
            role: role.to_string(),
            tf: tf.to_string(),
            min_bars,
        });
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
        let Some(tf) = self.declarations.iter().find(|d| d.role == "primary").map(|d| d.tf.clone())
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

    // ==================== 032 T009: DryRun 合约 hedge ====================

    use crate::config::ConfigValue;
    use ricow_core::{OrderBookUpdate, OrderInfo, UserEvent};

    /// 最小 mock 交易所 (DryRun 虚拟撮合不触网): 仅满足 trait, 交易方法一律报错。
    struct NoopExchange;

    #[async_trait::async_trait]
    impl Exchange for NoopExchange {
        fn name(&self) -> &'static str {
            "noop"
        }
        async fn get_markets(&self) -> CoreResult<Vec<Market>> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn get_klines(&self, _: &str, _: &str, _: u32) -> CoreResult<Vec<Kline>> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn get_orderbook(&self, _: &str, _: u32) -> CoreResult<OrderBook> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn place_order(&self, _: OrderRequest) -> CoreResult<OrderAck> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn cancel_order(&self, _: &str, _: &str) -> CoreResult<()> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn get_open_orders(&self, _: &str) -> CoreResult<Vec<OrderInfo>> {
            Ok(Vec::new())
        }
        async fn get_balance(&self, _: &str) -> CoreResult<Balance> {
            Err(CoreError::InvalidArgument("noop".into()))
        }
        async fn get_position(&self, _: &str) -> CoreResult<Option<Position>> {
            Ok(None)
        }
        async fn subscribe_orderbook(
            &self,
            _: &str,
        ) -> CoreResult<std::pin::Pin<Box<dyn futures::Stream<Item = OrderBookUpdate> + Send>>>
        {
            Ok(Box::pin(futures::stream::empty()))
        }
        async fn subscribe_user_events(
            &self,
        ) -> CoreResult<std::pin::Pin<Box<dyn futures::Stream<Item = UserEvent> + Send>>> {
            Ok(Box::pin(futures::stream::empty()))
        }
    }

    fn ob(bid: Decimal, ask: Decimal) -> OrderBook {
        OrderBook::new_sorted(
            vec![ricow_core::PriceLevel { price: bid, size: dec!(10) }],
            vec![ricow_core::PriceLevel { price: ask, size: dec!(10) }],
        )
    }

    fn dryrun_cfg(market: &str, mode: &str) -> StrategyConfig {
        StrategyConfig {
            name: "t".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params: HashMap::new(),
            dry_run_started_at: None,
            live_enabled: false,
            market: market.into(),
            position_mode: mode.into(),
            backtest: None,
        }
    }

    fn dryrun_futures() -> DryRunContext {
        let mut cfg = dryrun_cfg("futures", "hedge");
        cfg.params.insert("leverage".into(), ConfigValue::Float(2.0));
        cfg.params.insert("mmr_pct".into(), ConfigValue::Float(0.5));
        DryRunContext::new(
            Arc::new(NoopExchange),
            cfg,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        )
    }

    /// 开双仓并存: 按侧独立仓 + 公式爆仓价 + quote 保证金冻结 (free−=名义/杠杆, locked=Σ钱包)。
    #[test]
    fn test_dryrun_futures_hedge_open_both_sides() {
        let mut ctx = dryrun_futures();
        ctx.update_orderbook("ETHUSDT", ob(dec!(3000), dec!(3001)));

        let mut buy = OrderRequest::new_market("ETHUSDT", OrderSide::Buy, dec!(1));
        buy.position_side = Some("long".into());
        assert_eq!(ctx.place_order(buy).unwrap().status, OrderStatus::Filled);
        let mut sell = OrderRequest::new_market("ETHUSDT", OrderSide::Sell, dec!(1));
        sell.position_side = Some("short".into());
        assert_eq!(ctx.place_order(sell).unwrap().status, OrderStatus::Filled);

        // 两侧独立可见。
        let long = ctx.position_directional("ETHUSDT", OrderSide::Buy).expect("多头仓");
        let short = ctx.position_directional("ETHUSDT", OrderSide::Sell).expect("空头仓");
        assert_eq!(long.size, dec!(1));
        assert_eq!(long.entry_price, dec!(3001), "市价买按 ask 成交");
        assert_eq!(long.liquidation_price, Some(dec!(1515.505)), "3001×(1−1/2+0.005)");
        assert_eq!(short.size, dec!(1));
        assert_eq!(short.entry_price, dec!(3000));
        assert_eq!(short.liquidation_price, Some(dec!(4485)), "3000×(1+1/2−0.005)");

        // 净仓 = 0 → None; 保证金: free = 100000 − (3001+3000)/2; locked = Σ钱包 (扣 taker 5bps)。
        assert!(ctx.position("ETHUSDT").is_none(), "hedge 等量对冲 → 净仓 None");
        let bal = ctx.virtual_balance("USDT").expect("USDT 余额");
        assert_eq!(bal.free, dec!(96999.5));
        assert_eq!(bal.locked, dec!(2997.4995), "钱包 = Σ(名义/2 − 开仓费)");
    }

    /// reduce_only 按侧封顶: 平空单只减空仓, 多仓不动。
    #[test]
    fn test_dryrun_futures_hedge_reduce_only_per_side() {
        let mut ctx = dryrun_futures();
        ctx.update_orderbook("ETHUSDT", ob(dec!(3000), dec!(3001)));
        let mut buy = OrderRequest::new_market("ETHUSDT", OrderSide::Buy, dec!(1));
        buy.position_side = Some("long".into());
        let _ = ctx.place_order(buy);
        let mut sell = OrderRequest::new_market("ETHUSDT", OrderSide::Sell, dec!(1));
        sell.position_side = Some("short".into());
        let _ = ctx.place_order(sell);

        // reduce_only 买 2 定向平空 → 封顶 1 (空仓只有 1), 多仓不动。
        let mut close_short = OrderRequest::new_market("ETHUSDT", OrderSide::Buy, dec!(2));
        close_short.position_side = Some("short".into());
        close_short.reduce_only = true;
        let ack = ctx.place_order(close_short).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        assert_eq!(ack.size, dec!(1), "按侧封顶");
        assert!(ctx.position_directional("ETHUSDT", OrderSide::Sell).is_none(), "空仓已平");
        assert_eq!(ctx.position_directional("ETHUSDT", OrderSide::Buy).unwrap().size, dec!(1));
    }

    /// 穿越爆仓价: 只 WARN 不平仓 (DryRun 不模拟强平), 标记价按盘口刷新。
    #[test]
    fn test_dryrun_futures_hedge_liq_cross_warns_only() {
        let mut ctx = dryrun_futures();
        ctx.update_orderbook("ETHUSDT", ob(dec!(3000), dec!(3001)));
        let mut buy = OrderRequest::new_market("ETHUSDT", OrderSide::Buy, dec!(1));
        buy.position_side = Some("long".into());
        let _ = ctx.place_order(buy);

        // 价格砸穿爆仓价 1515.505 → 只告警, 仓位保留。
        ctx.update_orderbook("ETHUSDT", ob(dec!(1500), dec!(1501)));
        let long = ctx.position_directional("ETHUSDT", OrderSide::Buy).expect("不强制平仓");
        assert_eq!(long.size, dec!(1));
        assert_eq!(long.mark_price, dec!(1500.5), "mark = 盘口中价");
        assert_eq!(long.unrealized_pnl, dec!(-1500.5));
        let key = ctx.pos_key("ETHUSDT", OrderSide::Buy);
        assert!(ctx.liq_warned.contains(&key), "穿越应记入告警集");
    }

    /// 现货路径回归: 裸键净仓模型不变 (032 不影响现货)。
    #[test]
    fn test_dryrun_spot_path_unchanged() {
        let mut cfg = dryrun_cfg("spot", "one-way");
        cfg.params.insert("pair".into(), ConfigValue::String("ETHUSDT".into()));
        let mut ctx = DryRunContext::new(
            Arc::new(NoopExchange),
            cfg,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.update_orderbook("ETHUSDT", ob(dec!(3000), dec!(3001)));
        let ack =
            ctx.place_order(OrderRequest::new_market("ETHUSDT", OrderSide::Buy, dec!(1))).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        let p = ctx.position("ETHUSDT").expect("现货净仓");
        assert_eq!(p.size, dec!(1));
        assert_eq!(p.liquidation_price, None, "现货无爆仓价");
        assert_eq!(
            ctx.virtual_balance("ETHUSDT").unwrap().free,
            dec!(1),
            "base 资产键 = 完整 pair (parse_pair 仅拆 ':')"
        );
        assert_eq!(ctx.virtual_balance("USDT").unwrap().free, dec!(96999), "100000−3001");
    }
}
