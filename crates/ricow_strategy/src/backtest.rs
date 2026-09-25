//! 回测引擎: 历史 K 线逐根驱动, OHLC 合成撮合。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use ricow_core::{
    parse_pair, Balance, CoreResult, Kline, OrderAck, OrderAction, OrderBook, OrderFill,
    OrderRequest, OrderSide, OrderStatus, OrderType, Position,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::config::{BacktestParams, BacktestToml, ConfigValue, StrategyConfig};
use crate::context::{Context, Declaration};
use crate::fee::FeeModel;
use crate::multiframe::{tf_key, TfCache};
use crate::order_guard::OrderGuard;
use crate::pnl::PnlTracker;

/// hedge 模式按侧明细 (013 FR-006) —— 与交易所账单/真实清算事件对照用。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HedgeSides {
    /// LONG 侧强平次数。
    pub liquidation_count_long: u64,
    /// SHORT 侧强平次数。
    pub liquidation_count_short: u64,
    /// 收盘时 LONG 侧逐仓钱包余额 (Σ 该侧各交易对)。
    pub wallet_long: Decimal,
    /// 收盘时 SHORT 侧逐仓钱包余额 (Σ 该侧各交易对)。
    pub wallet_short: Decimal,
}

/// 回测报告。
#[derive(Debug, Clone, Default)]
pub struct BacktestReport {
    pub total_bars: usize,
    pub total_trades: u64,
    pub realized_pnl: Decimal,
    pub total_fees: Decimal,
    pub net_pnl: Decimal,
    pub win_rate: f64,
    pub max_drawdown: Decimal,
    pub fee_ratio: Decimal,
    pub fills: Vec<OrderFill>,
    /// 标的涨跌幅 (期初 open → 期末 close), %。
    pub price_change_pct: f64,
    /// 持仓币数变化 (基准 = 建仓后持仓; 期初 0 仓为 None), %。
    pub coin_change_pct: Option<f64>,
    /// 现金 (USDT) 变化 (基准 = 初始余额), %。
    pub cash_change_pct: f64,
    /// 总价值变化 (币市值+现金, 含未实现; 基准 = 初始余额), %。
    pub equity_change_pct: f64,
    /// 期末绝对值 (供展示, 防百分比误导)。
    pub final_price: Decimal,
    pub final_pos_size: Decimal,
    pub final_cash: Decimal,
    pub final_equity: Decimal,
    pub base_coin_size: Decimal,
    /// 建仓后现金 (现金变化基准)。
    pub base_cash: Decimal,
    /// 年化收益率 (几何, 比率; 数据不足 None)。
    pub annual_return: Option<f64>,
    /// 年化波动率 (比率)。
    pub annual_volatility: Option<f64>,
    /// 夏普比率 (rf = 参数 `risk_free_rate`, 默认 3.8% 年化)。
    pub sharpe: Option<f64>,
    /// 索提诺比率 (下行偏差以 rf 为最小可接受收益)。
    pub sortino: Option<f64>,
    /// Calmar 比率 (年化收益 / 最大回撤)。
    pub calmar: Option<f64>,
    /// 盈亏比 (总盈利 / 总亏损)。
    pub profit_factor: Option<f64>,
    /// 平均盈利 / 平均亏损 (quote 计价, 无交易为 None)。
    pub avg_win: Option<f64>,
    pub avg_loss: Option<f64>,
    /// 本次回测使用的无风险利率 (年化 %)。
    pub risk_free_rate: f64,
    /// 资金不足/无仓可平被拒的订单数 (市价丢弃; Bug A 修复后如实显示)。
    pub rejected_count: u64,
    // ---- 合约字段 (现货回测为 None; 见 specs/backtest.md §五.8) ----
    /// 杠杆 L (逐仓)。
    pub leverage: Option<f64>,
    /// 资金费净额 (对冲后净额)。
    pub funding_net: Option<Decimal>,
    /// 强平次数。
    pub liquidation_count: Option<u64>,
    /// 持仓模式 ("one-way"/"hedge")。
    pub position_mode: Option<String>,
    /// hedge 按侧明细 (强平次数/收盘钱包余额); one-way 与现货为 None。
    pub hedge_sides: Option<HedgeSides>,
    /// 期末名义敞口 (合约: Σ|size|×期末价; 现货 None)。
    pub final_notional: Option<Decimal>,
    /// 名义敞口变化 % (合约, 基准 = 建仓后; 现货 None)。
    pub nominal_exposure_pct: Option<f64>,
    // ---- 基准对照 (023 T8): 以**首次成交价**为买入价、同时点同本金起算 ----
    /// 首次成交价 (基准的买入价); 窗口内从未成交 → None。
    pub benchmark_entry_price: Option<Decimal>,
    /// 建仓时刻的策略权益 (apples-to-apples 起算点)。
    pub entry_equity: Option<Decimal>,
    /// 策略自建仓时点起的收益 % (= 期末权益 / 建仓权益 − 1)。
    pub strategy_return_since_entry_pct: Option<f64>,
    /// 满仓持有收益 % (同本金全额买入持有到期末); 只用于对照, 不是策略目标。
    pub benchmark_return_pct: Option<f64>,
    /// 满仓持有最大回撤 (比率)。
    pub benchmark_max_drawdown: Option<Decimal>,
    /// 满仓持有年化波动率 (比率)。
    pub benchmark_annual_volatility: Option<f64>,
    // ---- 组合回测口径 (M2; 单标的: equity_curve/turnover_ratio 亦填, holdings_snapshots 为空) ----
    /// 净值曲线 (quote; 起点 = 初始现金, 逐 bar/tick 收盘估值 + 末根补估)。
    pub equity_curve: Vec<Decimal>,
    /// 换手率 (比率, 1.0 = 100%): 单边成交额 / 平均权益 (平均权益 = 净值曲线均值)。
    pub turnover_ratio: f64,
    /// 组合持仓快照序列: 每收盘估值点的 (pair, size>0); 单标的路径为空。
    pub holdings_snapshots: Vec<Vec<(String, Decimal)>>,
}

/// 组合信号模式 ctx:klines 返回段的最大长度 (007 v2, R1 十年回测性能前提)。
///
/// 打分所需最近根数: IBD RS ROC252 / EMA200(t-20) / 52w 高窗口 ≈ 253 根下界;
/// 400 根留足递推与趋势确认余量。十年 (≈2514 根) 信号线每 tick 全量克隆会卡死,
/// 超长段只保留最近 SIGNAL_TAIL 根 (升序) — 无前视不变 (截断仍按全局 tick 时间)。
pub const SIGNAL_TAIL: usize = 400;

/// 单标的路径 `ctx:klines` 的尾窗长度 (023 性能修复, 2026-09-17): 只回最近这么多根
/// 已收盘 bar。此前每 tick 克隆**全部**已收盘 bar —— 1m × 79 天 ≈ 11.4 万根 × 11.4 万
/// 次 tick = 10^9 级拷贝, 1m 回测根本跑不动。冻结为 100 根同时把"指标输入上限 =
/// 100 根"固定成契约 (写入 specs/lua-api.md); 需要更长窗口的指标不走主序列。
/// 组合信号模式 (full_klines) 不受影响, 仍用 SIGNAL_TAIL = 400。
// 注: ctx:klines 不再截断 —— 023 引入的 100 根尾窗无设计依据(2026-09-24 用户确认从未
// 提出), 已撤销, 回测与实盘路径行为一致。常量仅保留以标记该历史行为。
#[allow(dead_code)]
const KLINES_TAIL: usize = 100;

/// 该 bar 覆盖时段 `[open, close)` 内的 8h 资金费结算点个数 (UTC 00/08/16)。
///
/// 013 FR-004 修正 L1: 旧实现按 `bar.open_time.hour() ∈ {0,8,16}` 判定 —— 1m/5m 的 00:00–00:59 段 bar
/// **全部命中**(每日超结数十次), 1d 每日仅命中 1 次(欠结 2/3)。按"覆盖时段包含结算点"判定则与 interval 无关:
/// 1m/5m/15m/1h/4h 恰 1 次、1d 恰 3 次, 且结算点不重不漏(前一 bar 的 `close` == 后一 bar 的 `open` 时只算后一根)。
pub fn funding_settlements_in_bar(open_ms: i64, close_ms: i64) -> u32 {
    const STEP_MS: i64 = 8 * 60 * 60 * 1000;
    if close_ms <= open_ms {
        return 0;
    }
    // 第一个 ≥ open 的 8h 边界 (ceil), 半开区间 [open, close)
    let first = ((open_ms + STEP_MS - 1) / STEP_MS) * STEP_MS;
    if first >= close_ms {
        0
    } else {
        ((close_ms - 1 - first) / STEP_MS + 1) as u32
    }
}

/// 回测上下文: OHLC 合成撮合。
///
/// 撮合规则 (回测无盘口):
/// - 限价买单: bar.low ≤ 挂单价 → 成交 @ 挂单价 (不假设更优)
/// - 限价卖单: bar.high ≥ 挂单价 → 成交 @ 挂单价
/// - 市价单: 按 bar.open ± 滑点成交 (近似; 滑点由 config 参数 `slippage_bps` 控制, 默认 2)
pub struct BacktestContext {
    config: StrategyConfig,
    pnl: PnlTracker,
    fee_model: FeeModel,
    quote_asset: String,
    /// 仓位表, 键 = (规范化 pair, 方向) (K6)。现货恒 (pair, Buy); 合约 one-way 下同对
    /// 至多一侧有仓 (撮合层先平后开), hedge 下多空并存。
    virtual_positions: HashMap<(String, OrderSide), Position>,
    /// 开仓批次队列 per (pair, 开仓方向) (尾部 = 最近开仓), LIFO 配对计算已实现盈亏。
    /// 现货只用 (pair, Buy) (开多批次); 合约每方向独立队列 (K3 "LIFO 批次 per side")。
    position_lots: HashMap<(String, OrderSide), Vec<(Decimal, Decimal)>>,
    /// 现货资金簿 (quote + base 余额表, 行为不变); 合约模式不用 (见 futures_cash)。
    virtual_balance: HashMap<String, Balance>,
    /// 合约账户模型 (K3, 逐仓钱包转账; 现货模式恒 ZERO):
    /// 现金 (未入钱包的 quote) + 每交易对一个逐仓钱包 + 账户级资金费净额 + 强平计数。
    futures_cash: Decimal,
    wallet: HashMap<String, Decimal>,
    funding_net: Decimal,
    liquidation_count: u64,
    /// 按侧强平次数 (013 FR-006; hedge 报告用)。
    liquidation_long: u64,
    liquidation_short: u64,
    /// 合约参数 (resolve 后的全量有效值, 见 specs/backtest.md §三)。
    leverage: Decimal,
    /// 维持保证金率 (比率: mmr_pct / 100)。
    mmr: Decimal,
    /// 资金费率 / 8h (0 = 关闭)。
    funding_rate_8h: Decimal,
    /// 资金不足/无仓可平而被拒的订单数 (市价单丢弃; 报告如实显示)。
    rejected_count: u64,
    /// 下单工程护栏 (019-R5): 固定 100 单/秒, 与 Dry Run / 实盘同一实现; 平台不做投资风控。
    guard: RefCell<OrderGuard>,
    pending_orders: Vec<(String, OrderRequest)>,
    fill_queue: Vec<OrderFill>,
    current_bar: Option<Kline>,
    closed_klines: Vec<Kline>,
    /// 尾 bar 补结算幂等标志 (013 FR-005, 修 L2)。
    finalized: bool,
    /// 组合模式 (多标的轮动回测, M2): 当前 tick 各 pair 的 bar (键 = resolve_key(pair))。
    /// 单标的路径恒空; 撮合/估值按 pair 路由时回落 current_bar。
    portfolio_bars: HashMap<String, Kline>,
    /// 组合模式: 各 pair 已收盘序列 (键 = resolve_key(pair)), ctx:klines(pair) 按 pair 路由。
    portfolio_klines: HashMap<String, Vec<Kline>>,
    /// 组合模式: 最近收盘价 (键 = resolve_key(pair), 期末估值/组合权益用)。
    portfolio_prices: HashMap<String, Decimal>,
    /// 组合模式 (M2): 逐 tick 收盘估值点的持仓快照 (pair, size>0, 键 = resolve_key(pair))。
    /// 与 equity_curve 每 tick 收盘点同点记录 (曲线初始点与末根补估点无快照); 单标的路径恒空。
    holdings_snapshots: Vec<Vec<(String, Decimal)>>,
    /// 组合信号模式 (2026-09-09 bs_momentum Lua 化): 美股信号日线 (键 = resolve_key(pair),
    /// 全量预装, 不随 tick 推进)。组合模式下 ctx:klines(pair) 返回该容器按全局 tick 时间
    /// 截断的已收盘段 (脚本永不见未来, 无前视); 容器空 = 信号通道关闭, 回落成交轨
    /// portfolio_klines/closed_klines (单标的与纯成交轨组合行为零改动)。
    signal_klines: HashMap<String, Vec<Kline>>,
    /// 首次成交价/时刻/当时权益 (023 T8): 满仓持有基准的买入价与 apples-to-apples 起算点。
    /// **无条件记录**(首笔可能是卖出 —— 现货先有持仓时; 用 `if size>0` 之类的守卫会漏)。
    first_fill_price: Option<Decimal>,
    first_fill_time: Option<chrono::DateTime<chrono::Utc>>,
    entry_equity: Option<Decimal>,
    /// 高周期序列 (第二序列, 023; 2026-09-18 扩为多套): 键 = `resolve_key(pair)|tf`,
    /// 装配层预装(已剔除不完整桶)。`atr_tf`/`ema_tf`/`close_tf` 按当前 tick 时间取可见前缀
    /// 计算, 每根高周期 bar 只算一次(TfCache 内部缓存)。
    tf_caches: HashMap<String, Arc<TfCache>>,
    /// 策略数据需求声明 (need_klines 写入, 回测两阶段的声明入口读取)。
    declarations: Vec<Declaration>,
    default_exchange: String,
    slippage_bps: u32,
    initial_equity: Decimal,
    equity_curve: Vec<Decimal>,
    turnover: Decimal,
    total_bars: usize,
    /// 首根 bar 的 open (价格涨跌基准)。
    first_open: Option<Decimal>,
    /// 首次出现持仓时的 size (币变化基准 = 建仓后)。
    first_pos_size: Option<Decimal>,
    /// 首次建仓时的名义敞口 (合约 nominal 变化基准)。
    first_notional: Option<Decimal>,
    /// 建仓完成时的现金余额 (现金变化基准 = 建仓后, 而非期初投入)。
    first_cash_after_build: Option<Decimal>,
}

/// 拆单结果: `(平仓动作, 开仓动作)`, 每项为 `(方向, 数量)`(仅为表达 `split_fill` 的返回类型)。
type FillSplit = (Option<(OrderSide, Decimal)>, Option<(OrderSide, Decimal)>);

impl BacktestContext {
    pub fn new(mut config: StrategyConfig, initial_balance: Balance) -> Self {
        let quote_asset = initial_balance.asset.clone();
        let is_futures = config.market == "futures";
        // 回测参数三层解析 (内置默认 + 策略 TOML [backtest]); params 池同名 key 再覆盖
        // (旧 slippage_bps 透传路径 + CLI 单次回测覆盖写回, 见 ricow backtest.rs)。
        let mut p = BacktestParams::resolve(&config, &BacktestToml::default());
        if let Some(v) = config.get_f64("slippage_bps") {
            p.slippage_bps = v;
        }
        if let Some(v) = config.get_f64("fee_maker_bps") {
            p.fee_maker_bps = v;
        }
        if let Some(v) = config.get_f64("fee_taker_bps") {
            p.fee_taker_bps = v;
        }
        if let Some(v) = config.get_f64("leverage") {
            p.leverage = v;
        }
        if let Some(v) = config.get_f64("mmr_pct") {
            p.mmr_pct = v;
        }
        if let Some(v) = config.get_f64("funding_rate_8h") {
            p.funding_rate_8h = v;
        }
        // 统一覆盖回写 (Y7, 2026-09-09 bs_momentum Lua 化): resolve 出最终值后写回
        // config.params — 策略 (Lua ctx:config_f64 / Rust 通用) 读到的费率/滑点/杠杆/
        // MMR/资金费与撮合层 FeeModel/账户模型完全同源一致 (未显式配置的默认 10bps 等
        // 也可读), 调仓预算按"净回笼口径"闭合的前提; 值 = resolve 后同源, 覆盖无害。
        for (key, v) in [
            ("fee_maker_bps", p.fee_maker_bps),
            ("fee_taker_bps", p.fee_taker_bps),
            ("slippage_bps", p.slippage_bps),
            ("leverage", p.leverage),
            ("mmr_pct", p.mmr_pct),
            ("funding_rate_8h", p.funding_rate_8h),
        ] {
            config.params.insert(key.into(), ConfigValue::Float(v));
        }
        let fee_model = FeeModel::new(
            Decimal::from_f64_retain(p.fee_maker_bps).unwrap_or(Decimal::ZERO),
            Decimal::from_f64_retain(p.fee_taker_bps).unwrap_or(Decimal::ZERO),
        );
        let slippage_bps = p.slippage_bps.round().max(0.0) as u32;
        let initial_cash = initial_balance.free;
        let mut balance_map = HashMap::new();
        if !is_futures {
            balance_map.insert(initial_balance.asset.clone(), initial_balance);
        }
        // 工程护栏: 固定 100 单/秒, 不接受配置。
        let guard = RefCell::new(OrderGuard::new());
        Self {
            config,
            pnl: PnlTracker::default(),
            fee_model,
            quote_asset,
            virtual_positions: HashMap::new(),
            position_lots: HashMap::new(),
            virtual_balance: balance_map,
            futures_cash: if is_futures { initial_cash } else { Decimal::ZERO },
            wallet: HashMap::new(),
            funding_net: Decimal::ZERO,
            liquidation_count: 0,
            liquidation_long: 0,
            liquidation_short: 0,
            leverage: Decimal::from_f64_retain(p.leverage).unwrap_or(Decimal::ONE),
            mmr: Decimal::from_f64_retain(p.mmr_pct).unwrap_or(Decimal::ZERO) / dec!(100),
            funding_rate_8h: Decimal::from_f64_retain(p.funding_rate_8h).unwrap_or(Decimal::ZERO),
            rejected_count: 0,
            guard,
            pending_orders: Vec::new(),
            fill_queue: Vec::new(),
            current_bar: None,
            closed_klines: Vec::new(),
            finalized: false,
            portfolio_bars: HashMap::new(),
            portfolio_klines: HashMap::new(),
            portfolio_prices: HashMap::new(),
            holdings_snapshots: Vec::new(),
            signal_klines: HashMap::new(),
            tf_caches: HashMap::new(),
            declarations: Vec::new(),
            first_fill_price: None,
            first_fill_time: None,
            entry_equity: None,
            default_exchange: "bn".to_string(),
            slippage_bps,
            initial_equity: initial_cash,
            equity_curve: vec![initial_cash],
            turnover: Decimal::ZERO,
            total_bars: 0,
            first_open: None,
            first_pos_size: None,
            first_notional: None,
            first_cash_after_build: None,
        }
    }

    fn is_futures(&self) -> bool {
        self.config.market == "futures"
    }

    /// 工程护栏前置检查 (019-R5): 超频 → `Rejected` ack (与资金不足/无仓可平同形, 计入 rejected_count),
    /// 并 `tracing::warn!(target: "order_guard")` 输出上限与窗口计数。策略循环不中断。
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

    /// 当前挂单数 (023 验收用: "任意时刻挂单 ≤ 2 张" 的可断言入口)。
    pub fn pending_count(&self) -> usize {
        self.pending_orders.len()
    }

    /// 撮合参考 bar: 组合模式按 pair 路由 portfolio_bars, 单标的回落 current_bar。
    fn bar_for(&self, pair: &str) -> Option<&Kline> {
        if self.portfolio_bars.is_empty() {
            self.current_bar.as_ref()
        } else {
            self.portfolio_bars.get(&self.resolve_key(pair)).or(self.current_bar.as_ref())
        }
    }

    /// 装载美股信号线 (组合信号模式, bs_momentum Lua 化)。键 = 原始 pair 名
    /// (如 "TSLABUSDT"), resolve_key 归一存储; 空 map = 信号通道关闭 (现行为)。
    /// 由 runner (run_portfolio_backtest) 在 on_init 前调用; 信号线全量预装
    /// (装配层按窗口截取, 严禁全量 Nasdaq 装载), 截断发生在 klines_for。
    pub fn set_signal_klines(&mut self, map: HashMap<String, Vec<Kline>>) {
        self.signal_klines =
            map.into_iter().map(|(pair, bars)| (self.resolve_key(&pair), bars)).collect();
    }

    /// ctx 可见历史 K 线: 组合信号模式 (signal_klines 非空) 返回该 pair 信号线
    /// **截至当前执行日的已收盘段** (bar.open_time < 全局 tick 时间), 无前视;
    /// 信号容器空 → 回落成交轨 portfolio_klines/closed_klines (单标的零改动)。
    fn klines_for(&self, pair: &str) -> Option<Vec<Kline>> {
        if !self.signal_klines.is_empty() {
            // 全局 tick 时间 = 当前 portfolio_bars 各 bar open_time 最大值 (与
            // step_portfolio 收旧 bar 的 last_bar 同口径; 组合统一时间轴)。数据缺口日
            // 该 pair 无当日 bar 时, tick 时间仍由其它 pair bar 决定, 截断不悬空 (Y1)。
            let cutoff = self.portfolio_bars.values().map(|k| k.open_time).max();
            let Some(cutoff) = cutoff else {
                return Some(Vec::new()); // on_init 期尚无 tick → 空信号段 (策略零单幂等)
            };
            // 该 pair 无信号线 (池外/数据缺失) → 空段; 信号线不足的标的由脚本按
            // 长度门槛跳过 ("宁缺毋滥"), 不给成交轨序列 (91 天 bStock 会误导打分)。
            return Some(
                self.signal_klines
                    .get(&self.resolve_key(pair))
                    .map(|bars| {
                        // 007 v2 尾窗 (R1 十年回测性能前提): 十年 ≈2514 根/标的, 若每 tick
                        // 克隆全部已收盘段 (2514 tick × 70 只 × ~1250 根均值 ≈ 2 亿根克隆)
                        // 会卡死; 打分只需最近 ≥253 根 (ROC252/EMA200/52w 高), 尾窗 400 根足够。
                        // 信号线升序契约 (nasdaq.rs parse reverse / db.rs 升序) →
                        // partition_point 定位已收盘边界 O(log n), 只克隆最近 SIGNAL_TAIL 根
                        // (无前视不变: 截断仍按全局 tick 时间, 封顶只裁旧根不裁未来)。
                        let end = bars.partition_point(|k| k.open_time < cutoff);
                        let start = end.saturating_sub(SIGNAL_TAIL);
                        bars[start..end].to_vec()
                    })
                    .unwrap_or_default(),
            );
        }
        if self.portfolio_klines.is_empty() {
            // ctx:klines 按 primary 声明尾窗截断: 策略经 need_klines 声明它需要多少根主时钟历史,
            // 引擎按声明供给尾窗 —— 不再每 tick 全量克隆已收盘序列(2026-09-24 撤销尾窗后,
            // 1m 主时钟 86400 根下每 tick 全量克隆是 O(n²), 回测从 ~13min 拖慢; 2026-09-25 修)。
            // 未声明 primary(旧策略未声明) → 回落全量, 行为不变。
            let tail = self
                .declarations
                .iter()
                .find(|d| d.role == "primary")
                .map(|d| d.min_bars as usize);
            match tail {
                Some(t) if t > 0 => {
                    let start = self.closed_klines.len().saturating_sub(t);
                    Some(self.closed_klines[start..].to_vec())
                }
                _ => Some(self.closed_klines.clone()),
            }
        } else {
            self.portfolio_klines
                .get(&self.resolve_key(pair))
                .cloned()
                .or_else(|| Some(self.closed_klines.clone()))
        }
    }

    /// 方向仓键。
    fn pos_key(&self, pair: &str, side: OrderSide) -> (String, OrderSide) {
        (self.resolve_key(pair), side)
    }

    /// 当前现金 (spot = quote 余额; futures = 未入钱包现金)。
    fn cash(&self) -> Decimal {
        if self.is_futures() {
            self.futures_cash
        } else {
            self.virtual_balance.get(&self.quote_asset).map(|b| b.free).unwrap_or(Decimal::ZERO)
        }
    }

    fn set_cash(&mut self, v: Decimal) {
        if self.is_futures() {
            self.futures_cash = v;
        } else {
            let quote = self.quote_asset.clone();
            let e = self.virtual_balance.entry(quote.clone()).or_insert_with(|| Balance {
                asset: quote,
                free: Decimal::ZERO,
                locked: Decimal::ZERO,
            });
            e.free = v;
        }
    }

    fn add_cash(&mut self, delta: Decimal) {
        self.set_cash(self.cash() + delta);
    }

    /// hedge 模式判定 (013): hedge 才按侧独立钱包/独立强平。
    fn is_hedge(&self) -> bool {
        self.config.position_mode == "hedge"
    }

    /// 逐仓钱包键 (013, FR-001): one-way = symbol 级单钱包 (`pair`); hedge = 按侧独立 (`pair|long` / `pair|short`)。
    ///
    /// 依据: 真实清算实测 (specs/backtest.md §十一 ④) —— hedge 两侧逐仓保证金独立占用、独立清算,
    /// 一侧被清算时另一侧钱包不受影响。所有按方向的资金操作都经此寻址, one-way 自动退化为 symbol 级。
    fn wallet_key_of(&self, pair: &str, side: OrderSide) -> String {
        let key = self.resolve_key(pair);
        if self.is_hedge() {
            let tag = match side {
                OrderSide::Buy => "long",
                OrderSide::Sell => "short",
            };
            format!("{key}|{tag}")
        } else {
            key
        }
    }

    /// 该 pair 某方向逐仓钱包余额 (合约; 现货恒 0)。
    fn wallet_of(&self, pair: &str, side: OrderSide) -> Decimal {
        let key = self.wallet_key_of(pair, side);
        self.wallet.get(&key).copied().unwrap_or(Decimal::ZERO)
    }

    fn add_wallet(&mut self, pair: &str, side: OrderSide, delta: Decimal) {
        let key = self.wallet_key_of(pair, side);
        *self.wallet.entry(key).or_default() += delta;
    }

    /// 单方向仓 (直接键查; 仅测试用 — 生产查询走 trait position_directional)。
    #[cfg(test)]
    fn position_side(&self, pair: &str, side: OrderSide) -> Option<Position> {
        self.virtual_positions.get(&self.pos_key(pair, side)).cloned()
    }

    /// 净仓视图 (K6/D9): 合并同对多空, net = long − short。
    /// - 只有单侧仓 → 原样返回 (现货/one-way 与旧行为一致)。
    /// - 多空并存 (hedge) → 占优方 side, size = |净|, entry = 占优方开仓均价。
    /// - 净 0 → None。
    fn net_position(&self, pair: &str) -> Option<Position> {
        let long = self.virtual_positions.get(&self.pos_key(pair, OrderSide::Buy));
        let short = self.virtual_positions.get(&self.pos_key(pair, OrderSide::Sell));
        let (long_sz, short_sz) = (
            long.map(|p| p.size).unwrap_or(Decimal::ZERO),
            short.map(|p| p.size).unwrap_or(Decimal::ZERO),
        );
        let net = long_sz - short_sz;
        if net.is_zero() {
            return None;
        }
        if net > Decimal::ZERO {
            long.cloned().map(|mut p| {
                p.size = net;
                p
            })
        } else {
            short.cloned().map(|mut p| {
                p.size = -net;
                p
            })
        }
    }

    /// 推送一根 K 线: 更新当前 bar (前一根移入已收盘序列), 撮合 pending 订单。
    ///
    /// 权益曲线采样: 旧 bar 移入收盘序列时, 按该 bar 的 close 估值
    /// (现金 + Σ 持仓×close, 含未实现盈亏; 该 bar 内成交已发生, 当根反映)。
    /// 最后一根未收盘 bar 的估值由 `report()` 补齐。
    pub fn step_bar(&mut self, kline: Kline) {
        if self.first_open.is_none() {
            self.first_open = Some(kline.open);
        }
        if let Some(prev) = self.current_bar.take() {
            // 合约 bar 收尾结算 (K4/K5 顺序): 该 bar 撮合后持仓 → (跨 8h → 资金费) → 强平检查。
            // 该 bar 的 pending 撮合与市价成交在其为 current 期间已全部发生, 结算状态完整;
            // 之后才收盘估值入曲线。现货路径不结算 (资金费/强平不生效, 回归约束)。
            if self.is_futures() {
                self.settle_closed_bar(&prev);
            }
            self.equity_curve.push(self.mark_to_market(&prev.close));
            self.closed_klines.push(prev);
        }
        self.current_bar = Some(kline.clone());
        self.total_bars += 1;
        self.match_pending();
    }

    /// 组合模式 (多标的轮动回测, M2): 推进一个"统一 tick" —— 输入为各 pair 的
    /// 已对齐 K 线 (键 = resolve_key(pair) 前的原始 pair 名)。撮合/结算/估值全部按 pair 路由。
    ///
    /// 语义与 step_bar 对齐: 上一 tick 的 bar 收进该 pair 的已收盘序列 + 估值入曲线,
    /// 当前 tick bar 成为撮合参考 (市价单按 bar.open 成交)。空档 pair (该 tick 无新 bar)
    /// 沿用其最近收盘价估值 (轮动回测日线对齐, 停牌/未上市标的自然跳过)。
    pub fn step_portfolio(&mut self, bars: &[(String, Kline)]) {
        if bars.is_empty() {
            return;
        }
        // 上一 tick 各 pair bar → 已收盘序列 + 组合权益估值 (按各 pair 最近收盘)。
        if !self.portfolio_bars.is_empty() {
            let last_bar = {
                let mut items: Vec<&Kline> = self.portfolio_bars.values().collect();
                items.sort_by_key(|k| k.open_time);
                items.last().cloned().cloned()
            };
            for (pair, bar) in std::mem::take(&mut self.portfolio_bars) {
                self.portfolio_klines.entry(pair.clone()).or_default().push(bar);
            }
            // 组合结算 (资金费/强平; 合约模式): 以 tick 收尾 bar 触发 —— settle 内部
            // 按 virtual_positions 全局结算 + 逐对强平 (bar 参考取该 tick 末 bar)。
            if self.is_futures() {
                if let Some(bar) = last_bar {
                    self.settle_closed_bar(&bar);
                }
            }
            self.equity_curve.push(self.mark_to_market_portfolio());
            self.holdings_snapshots.push(self.holdings_snapshot());
        }
        // 本 tick bars → portfolio_bars (撮合参考)。
        for (pair, bar) in bars {
            let key = self.resolve_key(pair);
            self.portfolio_prices.insert(key.clone(), bar.close);
            self.portfolio_bars.insert(key, bar.clone());
        }
        self.total_bars += 1;
        // 市价单即时按各 pair 当前 bar open 撮合 (match_pending 已按 pair 取 bar)。
        self.match_pending();
    }

    /// 组合估值: 现金 + Σ 各持仓 pair size × 该 pair 最近收盘价 (现货);
    /// 合约 = 现金 + Σ钱包 + Σ未实现 (各 pair 最近收盘价, 方向计价)。
    fn mark_to_market_portfolio(&self) -> Decimal {
        let cash = self.cash();
        if self.is_futures() {
            let mut equity = cash;
            for w in self.wallet.values() {
                equity += *w;
            }
            for p in self.virtual_positions.values() {
                if p.size <= Decimal::ZERO {
                    continue;
                }
                let px = self
                    .portfolio_prices
                    .get(&p.pair)
                    .copied()
                    .or_else(|| p.mark_price.checked_mul(dec!(1)))
                    .unwrap_or(p.entry_price);
                let u = match p.side {
                    OrderSide::Buy => (px - p.entry_price) * p.size,
                    OrderSide::Sell => (p.entry_price - px) * p.size,
                };
                equity += u;
            }
            equity
        } else {
            let mut pos_value = Decimal::ZERO;
            for p in self.virtual_positions.values() {
                let px = self.portfolio_prices.get(&p.pair).copied().unwrap_or(p.entry_price);
                pos_value += p.size * px;
            }
            cash + pos_value
        }
    }

    /// 当前持仓快照 (组合报告 M2): 各 (展示 pair, 净 size>0), 按 pair 排序。
    /// 展示 pair = 去交易所前缀后的原始口径 (bn:TSLABUSDT → TSLABUSDT, 与名单/输入一致)。
    fn holdings_snapshot(&self) -> Vec<(String, Decimal)> {
        let mut v: Vec<(String, Decimal)> = self
            .virtual_positions
            .iter()
            .filter(|(_, p)| p.size > Decimal::ZERO)
            .map(|((key, _), p)| {
                let display = match key.split_once(':') {
                    Some((prefix, base)) if prefix == self.default_exchange => base.to_string(),
                    _ => key.clone(),
                };
                (display, p.size)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// 换手率 (报告 M2/单标的通用): 单边成交额 / 平均权益 (净值曲线均值; 1.0 = 100%)。
    /// 口径注明: turnover 累计双边名义成交额 (每笔 fill 计价), 单边 = /2; 无成交/无曲线 → 0。
    fn turnover_ratio_of(&self, curve: &[Decimal]) -> f64 {
        if curve.is_empty() {
            return 0.0;
        }
        let avg = curve.iter().fold(Decimal::ZERO, |acc, v| acc + v) / Decimal::from(curve.len());
        if avg <= Decimal::ZERO {
            return 0.0;
        }
        (self.turnover / Decimal::from(2) / avg).to_f64().unwrap_or(0.0)
    }

    /// 合约 bar 收尾结算 (K4/K5; 013 FR-004 修正 L1): ① bar 时间跨度内跨越的 8h 结算点**逐个**结算资金费
    /// (与 interval 无关: 1m~4h 恰好 1 次, 1d 恰好 3 次; 旧实现按 `hour() ∈ {0,8,16}` 会超结/欠结);
    /// ② 强平检查 —— one-way 按对整体, hedge 按侧独立 (FR-002)。
    fn settle_closed_bar(&mut self, bar: &Kline) {
        if self.funding_rate_8h > Decimal::ZERO {
            let n = funding_settlements_in_bar(
                bar.open_time.timestamp_millis(),
                bar.close_time.timestamp_millis(),
            );
            for _ in 0..n {
                self.settle_funding(bar.close);
            }
        }
        // pair 键去重 —— 一对此多空两键, 防重复结算。
        let pair_keys: Vec<String> = self
            .virtual_positions
            .iter()
            .filter(|(_, p)| p.size > Decimal::ZERO)
            .map(|((k, _), _)| k.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        if self.is_hedge() {
            for k in &pair_keys {
                for side in [OrderSide::Buy, OrderSide::Sell] {
                    self.check_liquidation_side(k, side, bar);
                }
            }
        } else {
            for k in pair_keys {
                self.check_liquidation(&k, bar);
            }
        }
    }

    /// 资金费结算一次 (FR-003): one-way 按净仓(单钱包, 同对多空自然抵消);
    /// hedge 按侧独立 —— 多头钱包付、空头钱包收, **不跨侧抵消** (真实账户每侧独立钱包, 实测依据见 §十一 ④)。
    fn settle_funding(&mut self, price: Decimal) {
        let rows: Vec<((String, OrderSide), Decimal)> = self
            .virtual_positions
            .iter()
            .filter(|(_, p)| p.size > Decimal::ZERO)
            .map(|((k, s), p)| ((k.clone(), *s), p.size))
            .collect();
        if self.is_hedge() {
            for ((k, side), size) in rows {
                let delta = match side {
                    OrderSide::Buy => -self.funding_rate_8h * size * price,
                    OrderSide::Sell => self.funding_rate_8h * size * price,
                };
                if delta != Decimal::ZERO {
                    self.add_wallet(&k, side, delta);
                    self.funding_net += delta;
                }
            }
        } else {
            let mut per_pair: std::collections::HashMap<String, Decimal> =
                std::collections::HashMap::new();
            for ((k, side), size) in rows {
                let signed = match side {
                    OrderSide::Buy => -size,
                    OrderSide::Sell => size,
                };
                *per_pair.entry(k).or_default() += self.funding_rate_8h * signed * price;
            }
            for (k, net) in per_pair {
                if net != Decimal::ZERO {
                    self.add_wallet(&k, OrderSide::Buy, net);
                    self.funding_net += net;
                }
            }
        }
    }

    /// hedge 单侧强平判定 (013 FR-002, 依据 2026-09-05 真实清算实测 ①: 价格下行只清算 LONG 侧,
    /// SHORT 侧持仓与其强平价全程不变) ——
    /// E_side(p) = wallet_side + 未实现_side(p) ≤ MMR × 名义_side(p); f(p) = A + B·p 为线性:
    /// 多头 B = size(1−MMR) > 0 → 跌穿 (bar.low ≤ p_t); 空头 B = −size(1+MMR) < 0 → 涨穿 (bar.high ≥ p_t)。
    /// 触发只清该侧整仓 (平仓费从该侧钱包扣), 另一侧持仓/钱包不受影响。
    fn check_liquidation_side(&mut self, pair_key: &str, side: OrderSide, bar: &Kline) {
        let (size, entry) = match self.virtual_positions.get(&(pair_key.to_string(), side)) {
            Some(p) if p.size > Decimal::ZERO => (p.size, p.entry_price),
            _ => return,
        };
        let w = self.wallet_of(pair_key, side);
        let (a, b) = match side {
            OrderSide::Buy => (w - entry * size, size * (Decimal::ONE - self.mmr)),
            OrderSide::Sell => (w + entry * size, -size * (Decimal::ONE + self.mmr)),
        };
        if b == Decimal::ZERO {
            return;
        }
        let p_t = -a / b;
        let hit = match side {
            OrderSide::Buy => bar.low <= p_t,
            OrderSide::Sell => bar.high >= p_t,
        };
        if !hit {
            return;
        }
        let realized = self.close_position(pair_key, side, size, p_t);
        let fee = self.fee_model.calc_fee(p_t, size, false);
        self.add_wallet(pair_key, side, realized - fee);
        self.liquidation_count += 1;
        match side {
            OrderSide::Buy => self.liquidation_long += 1,
            OrderSide::Sell => self.liquidation_short += 1,
        }
        let fill = OrderFill {
            trade_id: Some(uuid::Uuid::new_v4().to_string()),
            exchange_order_id: format!("LIQ-{}", uuid::Uuid::new_v4()),
            client_order_id: format!(
                "LIQ-{pair_key}-{}",
                if side == OrderSide::Buy { "long" } else { "short" }
            ),
            pair: pair_key.to_string(),
            side,
            fill_price: p_t,
            fill_size: size,
            fee,
            timestamp: bar.close_time,
        };
        self.pnl.record_fill(&fill);
        self.fill_queue.push(fill);
        self.sweep_wallet_if_flat(pair_key);
    }

    /// 单对强平检查 (K4/D10): 组合触发判定 —— 该对权益 E(p) = wallet + Σ未实现(p),
    /// 触发线 E(p) ≤ MMR × Σ名义(p); f(p) = E − MMR×名义 = A + B·p 为线性,
    /// B>0 (净多) → 价格跌穿越 (bar.low ≤ p_t), B<0 (净空/对冲) → 价格涨穿越 (bar.high ≥ p_t)。
    /// 单侧仓时 p_t 与逐仓强平价公式恒等; 触发按 p_t 平浮亏仓 (open 计价), 平后重算,
    /// 盈利侧使钱包恢复则自动保留; 每对每根 bar 至多计数一次; 平仓费 (taker) 从钱包扣。
    fn check_liquidation(&mut self, pair_key: &str, bar: &Kline) {
        let mut counted = false;
        // 首轮组合触发不设方向门; 平掉浮亏仓后, 剩余仓的重判按 bar 运动方向收敛
        // (跌 bar 只判下行爆仓, 涨 bar 只判上行爆仓) —— 避免用平仓前的高/低点误判
        // 平仓后剩余仓 (bar 内路径不建模, 见 specs/backtest.md §七.6; 保守取方向一致侧)。
        let mut first_round = true;
        let down_bar = bar.close < bar.open;
        loop {
            let long = self.virtual_positions.get(&(pair_key.to_string(), OrderSide::Buy));
            let short = self.virtual_positions.get(&(pair_key.to_string(), OrderSide::Sell));
            let (s_l, e_l) = long
                .filter(|p| p.size > Decimal::ZERO)
                .map(|p| (p.size, p.entry_price))
                .unwrap_or((Decimal::ZERO, Decimal::ZERO));
            let (s_s, e_s) = short
                .filter(|p| p.size > Decimal::ZERO)
                .map(|p| (p.size, p.entry_price))
                .unwrap_or((Decimal::ZERO, Decimal::ZERO));
            if s_l <= Decimal::ZERO && s_s <= Decimal::ZERO {
                break;
            }
            // one-way 为 symbol 级单钱包 (两侧寻址同一键), 取任一方向等价
            let w = self.wallet_of(pair_key, OrderSide::Buy);
            // f(p) = E(p) − MMR×Σ名义(p) = A + B·p (线性穿越判定)
            let a = w - e_l * s_l + e_s * s_s;
            let b = (s_l - s_s) - self.mmr * (s_l + s_s);
            let hit = if b == Decimal::ZERO {
                false
            } else {
                let p_t = -a / b;
                if b > Decimal::ZERO {
                    // 净多: 下行爆仓 (bar.low ≤ p_t)。
                    if !first_round && !down_bar {
                        false
                    } else {
                        bar.low <= p_t
                    }
                } else if !first_round && down_bar {
                    // 净空/对冲: 上行爆仓; 跌 bar 中不判 (方向门)。
                    false
                } else {
                    bar.high >= p_t
                }
            };
            if !hit {
                break;
            }
            if !counted {
                self.liquidation_count += 1;
                counted = true;
            }
            first_round = false;
            // 平浮亏仓 (以 bar open 计价未实现; 仅在仍有仓的侧中选择更亏的一侧;
            // 平后重算, 盈利侧使钱包恢复则保留)。
            let u_l = (bar.open - e_l) * s_l;
            let u_s = (e_s - bar.open) * s_s;
            let side = match (s_l > Decimal::ZERO, s_s > Decimal::ZERO) {
                (true, false) => OrderSide::Buy,
                (false, true) => OrderSide::Sell,
                (true, true) => {
                    if u_l <= u_s {
                        OrderSide::Buy
                    } else {
                        OrderSide::Sell
                    }
                }
                (false, false) => break,
            };
            let size = if side == OrderSide::Buy { s_l } else { s_s };
            if size <= Decimal::ZERO {
                break;
            }
            let w2 = self.wallet_of(pair_key, OrderSide::Buy);
            let a2 = w2 - e_l * s_l + e_s * s_s;
            let b2 = (s_l - s_s) - self.mmr * (s_l + s_s);
            let p_t = -a2 / b2;
            // 按强平价平掉整仓; 平仓费 (taker) 从钱包扣; fill 记录带 "LIQ" 标识。
            let realized = self.close_position(pair_key, side, size, p_t);
            let fee = self.fee_model.calc_fee(p_t, size, false);
            self.add_wallet(pair_key, side, realized - fee);
            let fill = OrderFill {
                trade_id: Some(uuid::Uuid::new_v4().to_string()),
                exchange_order_id: format!("LIQ-{}", uuid::Uuid::new_v4()),
                client_order_id: format!("LIQ-{}", pair_key),
                pair: pair_key.to_string(),
                side,
                fill_price: p_t,
                fill_size: size,
                fee,
                timestamp: bar.close_time,
            };
            self.pnl.record_fill(&fill);
            self.fill_queue.push(fill);
            // 该对全平 → 钱包余额转回现金。
            if self.side_size(pair_key, OrderSide::Buy) <= Decimal::ZERO
                && self.side_size(pair_key, OrderSide::Sell) <= Decimal::ZERO
            {
                self.sweep_wallet_if_flat(pair_key);
                break;
            }
        }
    }

    /// 按给定价格估值总权益 (K8): 现货 = 现金 + Σ(持仓 × price);
    /// 合约 = 现金 + Σ逐仓钱包 + Σ未实现 (各仓按方向计价)。
    fn mark_to_market(&self, price: &Decimal) -> Decimal {
        let cash = self.cash();
        if self.is_futures() {
            let mut equity = cash;
            for w in self.wallet.values() {
                equity += *w;
            }
            for p in self.virtual_positions.values() {
                if p.size <= Decimal::ZERO {
                    continue;
                }
                let u = match p.side {
                    OrderSide::Buy => (price - p.entry_price) * p.size,
                    OrderSide::Sell => (p.entry_price - price) * p.size,
                };
                equity += u;
            }
            equity
        } else {
            let mut pos_value = Decimal::ZERO;
            for p in self.virtual_positions.values() {
                pos_value += p.size * *price;
            }
            cash + pos_value
        }
    }

    /// 期末名义敞口 (合约): Σ|size| × price (多空分别计入; 现货无意义 → None)。
    fn nominal_exposure(&self, price: Decimal) -> Option<Decimal> {
        if !self.is_futures() {
            return None;
        }
        let mut total = Decimal::ZERO;
        for p in self.virtual_positions.values() {
            total += p.size.abs() * price;
        }
        Some(total)
    }

    /// 组合模式报告 (多标的轮动回测): 净值曲线指标 + 成交统计 + 组合权益。
    ///
    /// 单标的语义字段 (price_change/final_pos_size 等) 对组合无意义 → 置默认;
    /// 有意义字段: equity 曲线 (回撤/年化/夏普/波动/Calmar) + 已实现/费/笔数/胜率。
    /// 持仓序列/换手/成本等组合明细由组合 runner 另出 (M2-6)。
    /// 收尾结算 (013 FR-005, 修 L2): bar 结算挂在"下一根 bar 收尾"路径上 (push_closed_bar),
    /// 回测结束若停在 8h 边界 bar, 末段资金费/强平不进报告 —— 报告生成前补一次。幂等。
    /// 调用方: `Engine::backtest` 与 CLI 回测路径在 `report()` 前调用。
    pub fn finalize(&mut self) {
        if self.finalized || !self.is_futures() {
            return;
        }
        self.finalized = true;
        // 优先结算**最后一根未收盘的 bar**(它承载了最后一段撮合与持仓), 否则最近收盘的 bar
        let last = self.current_bar.clone().or_else(|| self.closed_klines.last().cloned());
        if let Some(last) = last {
            self.settle_closed_bar(&last);
        }
    }

    /// hedge 按侧明细 (013 FR-006): 各侧强平次数 + 收盘各侧逐仓钱包余额 (Σ 各交易对)。
    fn hedge_side_stats(&self) -> HedgeSides {
        let mut s = HedgeSides {
            liquidation_count_long: self.liquidation_long,
            liquidation_count_short: self.liquidation_short,
            ..Default::default()
        };
        for (k, v) in &self.wallet {
            if k.ends_with("|long") {
                s.wallet_long += *v;
            } else if k.ends_with("|short") {
                s.wallet_short += *v;
            }
        }
        s
    }

    pub fn report_portfolio(&self) -> BacktestReport {
        let curve = {
            let mut c = self.equity_curve.clone();
            if !self.portfolio_bars.is_empty() {
                c.push(self.mark_to_market_portfolio());
            }
            if c.len() <= 1 {
                c.push(self.mark_to_market_portfolio());
            }
            c
        };
        let max_dd = crate::pnl::max_drawdown(&curve);
        // 时间跨度: 组合首 tick 开盘 → 末 tick 收盘 (取任一 bar 时间近似; 组合 tick 日线对齐)。
        let span_seconds = self
            .portfolio_klines
            .values()
            .flat_map(|ks| ks.iter())
            .map(|k| k.open_time)
            .min()
            .zip(self.portfolio_bars.values().map(|b| b.close_time).max())
            .map(|(s, e)| (e - s).num_seconds() as f64)
            .unwrap_or(0.0);
        let rf = self
            .config
            .get_f64("risk_free_rate")
            .unwrap_or(crate::metrics::DEFAULT_RISK_FREE_RATE_PCT);
        let annual_ret = crate::metrics::annual_return(&curve, span_seconds);
        let gross_profit = self.pnl.gross_profit().to_f64().unwrap_or(0.0);
        let gross_loss = self.pnl.gross_loss().to_f64().unwrap_or(0.0);
        let avg = crate::metrics::avg_win_loss(
            gross_profit,
            self.pnl.winning_trades(),
            gross_loss,
            self.pnl.losing_trades(),
        );
        let final_equity = self.mark_to_market_portfolio();
        let initial = self.initial_equity;
        BacktestReport {
            total_bars: self.total_bars,
            total_trades: self.pnl.trade_count(),
            realized_pnl: self.pnl.realized_pnl(),
            total_fees: self.pnl.total_fees(),
            net_pnl: self.pnl.net_pnl(),
            win_rate: self.pnl.win_rate(),
            max_drawdown: max_dd,
            fee_ratio: if self.turnover > Decimal::ZERO {
                self.pnl.total_fees() / self.turnover
            } else {
                Decimal::ZERO
            },
            fills: self.pnl.fills().to_vec(),
            price_change_pct: 0.0,
            coin_change_pct: None,
            cash_change_pct: if initial > Decimal::ZERO {
                ((self.cash() - initial) / initial).to_f64().unwrap_or(0.0) * 100.0
            } else {
                0.0
            },
            equity_change_pct: if initial > Decimal::ZERO {
                ((final_equity - initial) / initial).to_f64().unwrap_or(0.0) * 100.0
            } else {
                0.0
            },
            final_price: Decimal::ZERO,
            final_pos_size: Decimal::ZERO,
            final_cash: self.cash(),
            final_equity,
            base_coin_size: Decimal::ZERO,
            base_cash: initial,
            annual_return: annual_ret,
            annual_volatility: crate::metrics::annual_volatility(&curve, span_seconds),
            sharpe: crate::metrics::sharpe(&curve, span_seconds, rf),
            sortino: crate::metrics::sortino(&curve, span_seconds, rf),
            calmar: crate::metrics::calmar(annual_ret, max_dd.to_f64().unwrap_or(0.0)),
            profit_factor: crate::metrics::profit_factor(gross_profit, gross_loss),
            avg_win: avg.map(|a| a.0),
            avg_loss: avg.map(|a| a.1),
            risk_free_rate: rf,
            rejected_count: self.rejected_count,
            leverage: if self.is_futures() { self.leverage.to_f64() } else { None },
            funding_net: if self.is_futures() { Some(self.funding_net) } else { None },
            hedge_sides: if self.is_hedge() { Some(self.hedge_side_stats()) } else { None },
            liquidation_count: if self.is_futures() { Some(self.liquidation_count) } else { None },
            position_mode: if self.is_futures() {
                Some(self.config.position_mode.clone())
            } else {
                None
            },
            final_notional: None,
            nominal_exposure_pct: None,
            turnover_ratio: self.turnover_ratio_of(&curve),
            equity_curve: curve,
            holdings_snapshots: self.holdings_snapshots.clone(),
            // 基准对照 (023 T8) 只定义于单标的路径 —— 组合路径留 None, 不编造(见 plan Task 8)。
            benchmark_entry_price: None,
            entry_equity: None,
            strategy_return_since_entry_pct: None,
            benchmark_return_pct: None,
            benchmark_max_drawdown: None,
            benchmark_annual_volatility: None,
        }
    }

    /// 生成回测报告。
    pub fn report(&self) -> BacktestReport {
        let final_price = self.current_bar.as_ref().map(|k| k.close).unwrap_or(Decimal::ZERO);
        // 期末持仓: 净仓视图 (现货恒 Buy; 合约 one-way 单侧; hedge 合并后取净)。
        let (final_pos_size, base_coin_size) = if self.is_futures() {
            // 合约: 持仓币数语义不适用 (K8) —— 报告主仓 = 首对有仓 pair 的净仓 (供展示)。
            let key = self
                .virtual_positions
                .iter()
                .filter(|(_, p)| p.size > Decimal::ZERO)
                .map(|((k, _), _)| k.clone())
                .next();
            match key {
                Some(k) => (self.net_size_of(&k), self.first_pos_size.unwrap_or(Decimal::ZERO)),
                None => (Decimal::ZERO, Decimal::ZERO),
            }
        } else {
            match self.virtual_positions.values().next() {
                Some(p) => (p.size, self.first_pos_size.unwrap_or(Decimal::ZERO)),
                None => (Decimal::ZERO, Decimal::ZERO),
            }
        };
        let final_cash = self.cash();
        // 期末总权益 (K8): 合约 = 现金 + Σ钱包 + Σ未实现 (mark_to_market 统一)。
        let final_equity = if self.is_futures() {
            self.mark_to_market(&final_price)
        } else {
            final_cash + final_pos_size * final_price
        };
        let initial = self.initial_equity;
        let pct = |v: Decimal| -> f64 {
            if initial <= Decimal::ZERO {
                0.0
            } else {
                ((v - initial) / initial).to_f64().unwrap_or(0.0) * 100.0
            }
        };
        let price_change_pct = match self.first_open {
            Some(open) if open > Decimal::ZERO && final_price > Decimal::ZERO => {
                ((final_price - open) / open).to_f64().unwrap_or(0.0) * 100.0
            }
            _ => 0.0,
        };
        // 持仓币数变化 (K8): 现货维持; 合约无意义 → None (CLI 合约模式输出名义敞口)。
        let coin_change_pct = if self.is_futures() {
            None
        } else if base_coin_size > Decimal::ZERO {
            Some(
                ((final_pos_size - base_coin_size) / base_coin_size).to_f64().unwrap_or(0.0)
                    * 100.0,
            )
        } else {
            None
        };
        // 名义敞口变化 (K8 合约指标): 期末 Σ|size|×price vs 建仓后基准。
        let final_notional = self.nominal_exposure(final_price);
        let first_notional = self.first_notional;
        let nominal_exposure_pct = match (final_notional, first_notional) {
            (Some(f), Some(b)) if b > Decimal::ZERO => {
                Some(((f - b) / b).to_f64().unwrap_or(0.0) * 100.0)
            }
            _ => None,
        };
        // 现金变化基准 = 建仓后现金 (滚雪球口径); 未建仓回退期初余额。
        let base_cash = self.first_cash_after_build.unwrap_or(initial);
        let cash_change_pct = if base_cash > Decimal::ZERO {
            ((final_cash - base_cash) / base_cash).to_f64().unwrap_or(0.0) * 100.0
        } else {
            0.0
        };
        // 权益曲线: 补齐最后一根未收盘 bar 的收盘估值, 供回撤与风险指标。
        let curve = {
            let mut c = self.equity_curve.clone();
            if let Some(bar) = &self.current_bar {
                c.push(self.mark_to_market(&bar.close));
            }
            c
        };
        let max_dd = crate::pnl::max_drawdown(&curve);
        // 时间跨度 (秒): 首根 open_time → 末根 close_time, 精确到秒。
        let span_seconds = match (self.closed_klines.first(), &self.current_bar) {
            (Some(first), Some(cur)) => (cur.close_time - first.open_time).num_seconds() as f64,
            (None, Some(cur)) => (cur.close_time - cur.open_time).num_seconds() as f64,
            _ => 0.0,
        };
        // 无风险利率: 参数覆盖, 缺省 3.8% (3M 美国国债, 2026-08 快照)。
        let rf = self
            .config
            .get_f64("risk_free_rate")
            .unwrap_or(crate::metrics::DEFAULT_RISK_FREE_RATE_PCT);
        let annual_ret = crate::metrics::annual_return(&curve, span_seconds);
        let gross_profit = self.pnl.gross_profit().to_f64().unwrap_or(0.0);
        let gross_loss = self.pnl.gross_loss().to_f64().unwrap_or(0.0);
        let avg = crate::metrics::avg_win_loss(
            gross_profit,
            self.pnl.winning_trades(),
            gross_loss,
            self.pnl.losing_trades(),
        );
        // ---- 023 T8: 基准对照 ----
        // 口径(users 指定): 以**首次成交价同时点、同本金**满仓买入持有到期末; 建仓前策略空仓
        // 不计入对比(排除择时运气)。回撤用价格序列直接算(比率与尺度无关)。
        let (bm_entry, bm_ret, bm_dd, bm_vol, strat_since_entry) =
            match (self.first_fill_price, self.first_fill_time) {
                (Some(p0), Some(t0)) if p0 > Decimal::ZERO => {
                    let mut prices: Vec<Decimal> = vec![p0];
                    prices.extend(
                        self.closed_klines.iter().filter(|k| k.open_time >= t0).map(|k| k.close),
                    );
                    let last_time = match &self.current_bar {
                        Some(cb) => {
                            if cb.open_time >= t0 {
                                prices.push(cb.close);
                            }
                            cb.open_time
                        }
                        None => t0,
                    };
                    let span = (last_time - t0).num_seconds().max(0) as f64;
                    let px0 = p0.to_f64().unwrap_or(0.0);
                    let px_last = prices.last().map(|p| p.to_f64().unwrap_or(px0)).unwrap_or(px0);
                    let (ret, vol) = if px0 > 0.0 {
                        (
                            Some((px_last / px0 - 1.0) * 100.0),
                            crate::metrics::annual_volatility(&prices, span),
                        )
                    } else {
                        (None, None)
                    };
                    (
                        Some(p0),
                        ret,
                        Some(crate::pnl::max_drawdown(&prices)),
                        vol,
                        match self.entry_equity {
                            Some(e0) if e0 > Decimal::ZERO => {
                                ((final_equity - e0) / e0).to_f64().map(|v| v * 100.0)
                            }
                            _ => None,
                        },
                    )
                }
                _ => (None, None, None, None, None),
            };

        BacktestReport {
            total_bars: self.total_bars,
            total_trades: self.pnl.trade_count(),
            realized_pnl: self.pnl.realized_pnl(),
            total_fees: self.pnl.total_fees(),
            net_pnl: self.pnl.net_pnl(),
            win_rate: self.pnl.win_rate(),
            max_drawdown: max_dd,
            fee_ratio: if self.turnover > Decimal::ZERO {
                self.pnl.total_fees() / self.turnover
            } else {
                Decimal::ZERO
            },
            fills: self.pnl.fills().to_vec(),
            price_change_pct,
            coin_change_pct,
            cash_change_pct,
            equity_change_pct: pct(final_equity),
            final_price,
            final_pos_size,
            final_cash,
            final_equity,
            base_coin_size,
            base_cash,
            annual_return: annual_ret,
            annual_volatility: crate::metrics::annual_volatility(&curve, span_seconds),
            sharpe: crate::metrics::sharpe(&curve, span_seconds, rf),
            sortino: crate::metrics::sortino(&curve, span_seconds, rf),
            calmar: crate::metrics::calmar(annual_ret, max_dd.to_f64().unwrap_or(0.0)),
            profit_factor: crate::metrics::profit_factor(gross_profit, gross_loss),
            avg_win: avg.map(|a| a.0),
            avg_loss: avg.map(|a| a.1),
            risk_free_rate: rf,
            rejected_count: self.rejected_count,
            // 合约字段: 现货回测恒 None/空; 合约有效 (金额/次数 T5 结算, 报告 T6 收敛)。
            leverage: if self.is_futures() { self.leverage.to_f64() } else { None },
            funding_net: if self.is_futures() { Some(self.funding_net) } else { None },
            hedge_sides: if self.is_hedge() { Some(self.hedge_side_stats()) } else { None },
            liquidation_count: if self.is_futures() { Some(self.liquidation_count) } else { None },
            position_mode: if self.is_futures() {
                Some(self.config.position_mode.clone())
            } else {
                None
            },
            final_notional,
            nominal_exposure_pct,
            turnover_ratio: self.turnover_ratio_of(&curve),
            equity_curve: curve,
            holdings_snapshots: Vec::new(),
            benchmark_entry_price: bm_entry,
            entry_equity: self.entry_equity,
            strategy_return_since_entry_pct: strat_since_entry,
            benchmark_return_pct: bm_ret,
            benchmark_max_drawdown: bm_dd,
            benchmark_annual_volatility: bm_vol,
        }
    }

    fn try_match_ohlc(&self, req: &OrderRequest, bar: &Kline) -> Option<Decimal> {
        match req.order_type {
            OrderType::Market => {
                // 市价滑点: 买向上卖向下, 成交价 = bar.open × (1 ∓ slippage_bps/10000)。
                // 与 DryRun 对手方最优价成交语义对齐 (实盘等价: 对手价 ≈ 实时价 ± 滑点)。
                let slippage = Decimal::from(self.slippage_bps) / dec!(10000);
                let factor = match req.side {
                    OrderSide::Buy => Decimal::ONE + slippage,
                    OrderSide::Sell => Decimal::ONE - slippage,
                };
                Some(bar.open * factor)
            }
            OrderType::Limit => {
                let limit = req.price?;
                let crossed = match req.side {
                    OrderSide::Buy => bar.low <= limit,
                    OrderSide::Sell => bar.high >= limit,
                };
                if crossed {
                    Some(limit)
                } else {
                    None
                }
            }
        }
    }

    /// reduce_only/平仓单可平量: 无仓可平返回 None (整单拒); 有则裁剪请求 size。
    fn reduce_available(&self, req: &OrderRequest) -> Option<Decimal> {
        self.split_fill(req).0.map(|(_, sz)| sz)
    }

    fn match_pending(&mut self) {
        let drained: Vec<(String, OrderRequest)> = self.pending_orders.drain(..).collect();
        let mut remaining = Vec::new();

        for (id, mut req) in drained {
            // 撮合参考 bar: 组合模式按 req.pair 路由, 单标的回落 current_bar。
            let bar = match self.bar_for(&req.pair) {
                Some(b) => b.clone(),
                None => {
                    remaining.push((id, req));
                    continue;
                }
            };
            if req.reduce_only {
                // reduce_only 单在成交时校验可平量; 无仓可平 → 丢弃并计数。
                match self.reduce_available(&req) {
                    Some(sz) => req.size = sz,
                    None => {
                        self.rejected_count += 1;
                        continue;
                    }
                }
            }
            // L4: 成交那一刻既不平也不开 (目标方向无仓) → 本 bar 不成交(不計費/不記筆數), 保持挂单。
            // 与"资金不足"同一约定: 下一刻仓位变化后它可能变为有效 (如预挂的保护性平仓单)。
            // 修复前这里会真的成交: 零动作却收手续费并记一笔成交, 成本与笔数虚增。
            if self.is_noop_fill(&req) {
                remaining.push((id, req));
                continue;
            }
            if let Some(fill_price) = self.try_match_ohlc(&req, &bar) {
                // 资金/持仓不足 (Bug A 修复): 限价单保持 pending, 下 bar 再试 (K2/D4)。
                if self.has_funds(&req, &fill_price) {
                    self.execute_fill(&id, &req, fill_price);
                } else {
                    remaining.push((id, req));
                }
            } else {
                remaining.push((id, req));
            }
        }
        self.pending_orders = remaining;
    }

    /// 方向仓当前 size (无仓 = 0)。
    fn side_size(&self, pair: &str, side: OrderSide) -> Decimal {
        self.virtual_positions
            .get(&self.pos_key(pair, side))
            .map(|p| p.size)
            .unwrap_or(Decimal::ZERO)
    }

    /// 净仓 size (long − short 的绝对值; 无仓/对冲相抵 = 0)。
    fn net_size_of(&self, pair: &str) -> Decimal {
        let long = self.side_size(pair, OrderSide::Buy);
        let short = self.side_size(pair, OrderSide::Sell);
        if long > short {
            long - short
        } else {
            short - long
        }
    }

    /// 订单方向语义 (K6): 返回本单要减仓的方向 (close) 与要加仓的方向 (open)。
    /// - spot: 恒作用于 Buy 仓 —— buy 加多, sell 平多 (禁空, 卖超由资金校验拦)。
    /// - futures position_side="long"/"short": 严格单方向操作 (hedge), 不动反仓。
    /// - futures position_side=None (one-way/BOTH): 反向下单 = 先平反仓再开本向 (先平后开)。
    fn fill_effect(&self, req: &OrderRequest) -> (Option<OrderSide>, Option<OrderSide>) {
        if !self.is_futures() {
            return match req.side {
                OrderSide::Buy => (None, Some(OrderSide::Buy)),
                OrderSide::Sell => (Some(OrderSide::Buy), None),
            };
        }
        match (req.position_side.as_deref(), req.side) {
            (Some("long"), OrderSide::Buy) => (None, Some(OrderSide::Buy)),
            (Some("long"), OrderSide::Sell) => (Some(OrderSide::Buy), None),
            (Some("short"), OrderSide::Sell) => (None, Some(OrderSide::Sell)),
            (Some("short"), OrderSide::Buy) => (Some(OrderSide::Sell), None),
            (_, OrderSide::Buy) => (Some(OrderSide::Sell), Some(OrderSide::Buy)),
            (_, OrderSide::Sell) => (Some(OrderSide::Buy), Some(OrderSide::Sell)),
        }
    }

    /// 拆单: 把一次请求按当前持仓拆成 (平仓动作, 开仓动作), 不改状态。
    /// reduce_only / 现货卖出 → 无开仓 (余量丢弃; 资金校验保证正常路径不出现余量)。
    fn split_fill(&self, req: &OrderRequest) -> FillSplit {
        let (close_side, open_side) = self.fill_effect(req);
        let mut remain = req.size;
        let mut close = None;
        if let Some(cs) = close_side {
            let avail = self.side_size(&req.pair, cs);
            let sz = remain.min(avail);
            if sz > Decimal::ZERO {
                close = Some((cs, sz));
                remain -= sz;
            }
        }
        let mut open = None;
        let can_open = !req.reduce_only && open_side.is_some();
        if can_open && remain > Decimal::ZERO {
            open = Some((open_side.unwrap(), remain));
        }
        (close, open)
    }

    /// 空操作判定 (L4): 定向单既不平也不开 —— 目标方向无仓。
    ///
    /// 例: hedge 模式下 `sell + position_side="long"`(= 平多) 而多仓为 0。
    /// 此前这类请求会走到 `execute_fill`, 被**无条件计费并记一笔零动作成交** —— 成本与交易笔数虚增,
    /// 报告比真实更差; 真交所对该组合无仓可平。
    ///
    /// 处置分两种(实盘语义不同):
    /// - **市价单**无法挂: 直接拒单并计数(不再静默成交);
    /// - **限价单**成交那一刻若为零动作: 保持 pending 等下个 bar(与"资金不足"同一约定),
    ///   不计费/不记笔数 —— 下一刻仓位变化后它可能变为有效的保护性平仓单。
    fn is_noop_fill(&self, req: &OrderRequest) -> bool {
        let (close, open) = self.split_fill(req);
        close.is_none() && open.is_none()
    }

    /// 资金/持仓前置校验 (K2, Bug A 修复): 撮合成交前调用; 不足 = 拒单/挂单 (由调用方决定)。
    fn has_funds(&self, req: &OrderRequest, price: &Decimal) -> bool {
        let (close, open) = self.split_fill(req);
        if !self.is_futures() {
            let fee = self.fee_model.calc_fee(
                *price,
                req.size,
                matches!(req.order_type, OrderType::Limit),
            );
            return match req.side {
                OrderSide::Buy => self.cash() >= req.size * price + fee,
                OrderSide::Sell => {
                    // 现货卖 = 平多: base 足够才可成交。
                    let base = parse_pair(&req.pair).1.to_string();
                    let held =
                        self.virtual_balance.get(&base).map(|b| b.free).unwrap_or(Decimal::ZERO);
                    held >= req.size
                }
            };
        }
        // 合约: 平仓部分要求反仓存在 (split 已 min); 开仓部分需现金 ≥ M = 名义/杠杆。
        // (开仓手续费从钱包扣, M 默认 ≫ fee, 划入后钱包恒 ≥ 0。)
        if let Some((_, sz)) = open {
            let margin = (sz * price) / self.leverage;
            if self.cash() < margin {
                return false;
            }
        }
        let _ = close;
        true
    }

    /// 平仓 (LIFO 配对, per-side 批次): 减少 (pair, side) 仓。
    /// realized 按方向计: Buy 侧批次 = 开多买价, 平多 = 卖价 − 买价;
    /// Sell 侧批次 = 开空卖价, 平空 = 卖价 − 买回价 (真盈亏 = lot.0 − fill_price, D1 修复)。
    /// 返回已实现盈亏 (quote); 仓清零时 entry 复位, 部分平仓后 entry 重算为剩余批次加权均价
    /// (避免以含已平批次的混合均价估值剩余仓 → 未实现/强平权益基差错误, D2 修复)。
    fn close_position(
        &mut self,
        pair: &str,
        side: OrderSide,
        size: Decimal,
        fill_price: Decimal,
    ) -> Decimal {
        let key = self.pos_key(pair, side);
        let close_size = size.min(self.side_size(pair, side));
        if close_size <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        // 方向系数: Buy = +1 (批次为买价, fill 为卖价); Sell = −1 (批次为卖价, fill 为买回价)。
        let sgn = if side == OrderSide::Buy { dec!(1) } else { dec!(-1) };
        let mut realized = Decimal::ZERO;
        let mut remaining = close_size;
        if let Some(lots) = self.position_lots.get_mut(&key) {
            while remaining > Decimal::ZERO {
                match lots.last_mut() {
                    Some(lot) => {
                        let lot_size = lot.1.min(remaining);
                        realized += (fill_price - lot.0) * lot_size * sgn;
                        lot.1 -= lot_size;
                        remaining -= lot_size;
                        if lot.1 <= Decimal::ZERO {
                            lots.pop();
                        }
                    }
                    None => break,
                }
            }
        }
        // 批次不足 (防御; 正常路径批次 = 开仓量) → 用 entry 均价兜底 (同方向口径)。
        if remaining > Decimal::ZERO {
            if let Some(p) = self.virtual_positions.get(&key) {
                realized += (fill_price - p.entry_price) * remaining * sgn;
            }
        }
        // 剩余批次加权均价 (部分平仓后 entry 重算用; 不可变借用先算, 避免与下方 get_mut 冲突)。
        let new_entry =
            self.position_lots.get(&key).filter(|lots| !lots.is_empty()).and_then(|lots| {
                let (sum_size, sum_notional) = lots
                    .iter()
                    .fold((Decimal::ZERO, Decimal::ZERO), |(ss, sn), (lot_px, lot_sz)| {
                        (ss + *lot_sz, sn + *lot_px * *lot_sz)
                    });
                if sum_size > Decimal::ZERO {
                    Some(sum_notional / sum_size)
                } else {
                    None
                }
            });
        if let Some(p) = self.virtual_positions.get_mut(&key) {
            p.size -= close_size;
            p.mark_price = fill_price;
            if p.size <= Decimal::ZERO {
                p.size = Decimal::ZERO;
                p.entry_price = Decimal::ZERO;
            } else if let Some(avg) = new_entry {
                p.entry_price = avg;
            }
        }
        if realized != Decimal::ZERO {
            self.pnl.record_pnl(realized);
        }
        realized
    }

    /// 开/加仓 (pair, side): 批次入队 (LIFO 配对源) + 加权均价。
    fn open_position(&mut self, pair: &str, side: OrderSide, size: Decimal, fill_price: Decimal) {
        if size <= Decimal::ZERO {
            return;
        }
        let key = self.pos_key(pair, side);
        self.position_lots.entry(key.clone()).or_default().push((fill_price, size));
        let lev = if self.is_futures() { Some(self.leverage) } else { None };
        let e = self.virtual_positions.entry(key.clone()).or_insert_with(|| Position {
            pair: key.0.clone(),
            side,
            size: Decimal::ZERO,
            entry_price: Decimal::ZERO,
            mark_price: fill_price,
            liquidation_price: None,
            unrealized_pnl: Decimal::ZERO,
            leverage: lev,
        });
        let old_notional = e.entry_price * e.size;
        e.size += size;
        if e.size != Decimal::ZERO {
            e.entry_price = (old_notional + fill_price * size) / e.size;
        }
        e.mark_price = fill_price;
    }

    /// 该对两侧仓都清空时 → 各侧钱包余额转回现金 (K3; hedge 两侧各自回笼, one-way 两键相同)。
    fn sweep_wallet_if_flat(&mut self, pair: &str) {
        if self.side_size(pair, OrderSide::Buy) <= Decimal::ZERO
            && self.side_size(pair, OrderSide::Sell) <= Decimal::ZERO
        {
            for side in [OrderSide::Buy, OrderSide::Sell] {
                let key = self.wallet_key_of(pair, side);
                if let Some(w) = self.wallet.remove(&key) {
                    self.add_cash(w);
                }
            }
        }
    }

    /// 撮合成交统一入口 (K2/K3): 前置资金校验已由调用方通过; 此处记账。
    /// 现货: 仓位 (恒 Buy 侧) + 资金簿, 手续费真实扣余额 (Bug B 修复)。
    /// 合约: 逐仓钱包转账 —— 平仓 wallet += realized − 平仓费; 开仓 cash −= M, wallet += M − 开仓费。
    fn execute_fill(&mut self, order_id: &str, req: &OrderRequest, fill_price: Decimal) {
        // 023 T8: 首笔成交留痕 —— 成交价(基准买入价)/时刻(起算点)/当时权益(公平对照的起点)。
        // 记录必须在仓位与现金变动**之前**。
        if self.first_fill_price.is_none() {
            let key = self.resolve_key(&req.pair);
            self.first_fill_price = Some(fill_price);
            self.first_fill_time = self.current_bar.as_ref().map(|b| b.open_time);
            self.entry_equity = Some(self.cash() + self.net_size_of(&key) * fill_price);
        }
        let (close, open) = self.split_fill(req);
        let is_maker = matches!(req.order_type, OrderType::Limit);
        let fee = self.fee_model.calc_fee(fill_price, req.size, is_maker);

        if !self.is_futures() {
            // ---- 现货记账 ----
            if let Some((cs, sz)) = close {
                self.close_position(&req.pair, cs, sz, fill_price);
            }
            if let Some((os, sz)) = open {
                self.open_position(&req.pair, os, sz, fill_price);
            }
            let base = parse_pair(&req.pair).1.to_string();
            let notional = req.size * fill_price;
            // 手续费实扣 (Bug B): 买 cash −= 名义+fee; 卖 cash += 名义−fee。
            match req.side {
                OrderSide::Buy => {
                    self.add_cash(-(notional + fee));
                    let e = self.virtual_balance.entry(base.clone()).or_insert_with(|| Balance {
                        asset: base.clone(),
                        free: Decimal::ZERO,
                        locked: Decimal::ZERO,
                    });
                    e.free += req.size;
                }
                OrderSide::Sell => {
                    let e = self.virtual_balance.entry(base.clone()).or_insert_with(|| Balance {
                        asset: base.clone(),
                        free: Decimal::ZERO,
                        locked: Decimal::ZERO,
                    });
                    e.free -= req.size;
                    self.add_cash(notional - fee);
                }
            }
        } else {
            // ---- 合约记账 (K3 逐仓钱包) ----
            // 手续费按 close/open 名义占比拆分 (同对翻转单一次成交两段各自计费)。
            let total_notional = req.size * fill_price;
            let close_notional = close.map(|(_, sz)| sz * fill_price).unwrap_or(Decimal::ZERO);
            let close_fee = if total_notional > Decimal::ZERO {
                fee * close_notional / total_notional
            } else {
                Decimal::ZERO
            };
            let open_fee = fee - close_fee;
            if let Some((cs, sz)) = close {
                let realized = self.close_position(&req.pair, cs, sz, fill_price);
                // 平仓: 盈亏与平仓费走**被平方向**的钱包 (hedge 分侧; one-way symbol 级)。
                self.add_wallet(&req.pair, cs, realized - close_fee);
                // 该对平净 (两侧都无仓) → 钱包余额整体转回现金。
                self.sweep_wallet_if_flat(&req.pair);
            }
            if let Some((os, sz)) = open {
                // 开仓: 现金划入**开仓方向**的钱包 M = 名义/杠杆, 开仓费从该钱包扣。
                let margin = (sz * fill_price) / self.leverage;
                self.add_cash(-margin);
                self.add_wallet(&req.pair, os, margin - open_fee);
                self.open_position(&req.pair, os, sz, fill_price);
            }
        }

        // 首次出现持仓 → 记录币/现金变化基准 (建仓后; 必须在资金变动后)。
        if self.first_pos_size.is_none() {
            let sz = self.net_size_of(&req.pair);
            if sz > Decimal::ZERO {
                self.first_pos_size = Some(sz);
                self.first_cash_after_build = Some(self.cash());
                if self.is_futures() {
                    self.first_notional = self.nominal_exposure(fill_price);
                }
            }
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
        };
        self.pnl.record_fill(&fill);
        self.fill_queue.push(fill);
        // 成交额累加 (手续费占比分母); 权益曲线不再在此采样 —— 由 step_bar/report 按 bar 收盘估值。
        self.turnover += fill_price * req.size;
    }
}

impl Context for BacktestContext {
    fn price(&self, pair: &str) -> Option<Decimal> {
        // 前视修复: on_tick 只能见当前 bar 的 open (bar 开始价, 实盘 tick 时刻已知)。
        // close/high/low 属于未走完的 bar, 对策略不可见; 否则回测会"看到未来"虚高。
        // 组合模式: 按 pair 路由当前 bar (单标的回落 current_bar, 行为不变)。
        self.bar_for(pair).map(|b| b.open)
    }

    fn orderbook(&self, _pair: &str) -> Option<OrderBook> {
        None
    }

    fn position(&self, pair: &str) -> Option<Position> {
        // 净仓语义 (K6/D9): 现货恒 Buy; one-way 下单侧仓; hedge 下合并多空 (net 0 → None)。
        self.net_position(pair)
    }

    fn position_directional(&self, pair: &str, side: OrderSide) -> Option<Position> {
        // 带方向查询 (D9/hedge): 现货无方向概念 → 仅 Buy (即净仓); 合约返回 (pair, side) 仓。
        if !self.is_futures() {
            return if side == OrderSide::Buy { self.net_position(pair) } else { None };
        }
        let key = self.pos_key(pair, side);
        let p = self.virtual_positions.get(&key)?;
        if p.size > Decimal::ZERO {
            Some(p.clone())
        } else {
            None
        }
    }

    fn balance(&self, asset: &str) -> Option<Decimal> {
        if self.is_futures() {
            // 合约: quote 现金可见; base 币无现货持有概念 → None (Lua 侧为 0)。
            if asset == self.quote_asset {
                Some(self.cash())
            } else {
                None
            }
        } else {
            self.virtual_balance.get(asset).map(|b| b.total())
        }
    }

    fn place_order(&mut self, req: OrderRequest) -> CoreResult<OrderAck> {
        // 撤单指令 (023): 必须在风控**之前**短路 —— 该指令 size = 0, 走普通路径会被
        // 判"数量无效/为 0"直接拒掉; 撤单也不占限频、不计 rejected_count。
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
            tracing::info!(target: "strategy.backtest", pair = %key, cancelled, "撤单指令: 清挂单");
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
            self.rejected_count += 1;
            return Ok(rejected);
        }
        let exchange_order_id = uuid::Uuid::new_v4().to_string();
        let mut req = req;
        if req.reduce_only {
            match self.reduce_available(&req) {
                Some(sz) => req.size = sz,
                None => {
                    // 无仓可平 → 拒单 (计数; 不挂单)。
                    self.rejected_count += 1;
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

        let bar = match self.bar_for(&req.pair) {
            Some(b) => b.clone(),
            None => {
                // 无参考 bar (组合模式该 pair 尚无 tick; 单标的 run_backtest 时序下不可达,
                // 仅 on_init/空数据可能触发): 市价单拒 (计数, 与资金不足口径一致, 2026-09-09),
                // 限价挂 pending。
                if req.order_type == OrderType::Market {
                    self.rejected_count += 1;
                }
                let ack = OrderAck {
                    exchange_order_id: exchange_order_id.clone(),
                    client_order_id: req.client_order_id.clone(),
                    pair: req.pair.clone(),
                    side: req.side,
                    price: req.price.unwrap_or(Decimal::ZERO),
                    size: req.size,
                    filled_size: Decimal::ZERO,
                    status: if req.order_type == OrderType::Market {
                        OrderStatus::Rejected
                    } else {
                        OrderStatus::Open
                    },
                };
                if req.order_type != OrderType::Market {
                    self.pending_orders.push((exchange_order_id, req));
                }
                return Ok(ack);
            }
        };
        let ack = match self.try_match_ohlc(&req, &bar) {
            Some(fill_price) => {
                if self.is_noop_fill(&req) {
                    // L4: 成交那一刻既不平也不开 (目标方向无仓)。
                    // 市价单无法挂 → 拒单计数; 限价单按"资金不足"同一约定挂 pending, 下个 bar 再试。
                    let is_market = req.order_type == OrderType::Market;
                    if is_market {
                        self.rejected_count += 1;
                    }
                    let ack = OrderAck {
                        exchange_order_id: exchange_order_id.clone(),
                        client_order_id: req.client_order_id.clone(),
                        pair: req.pair.clone(),
                        side: req.side,
                        price: if is_market { fill_price } else { req.price.unwrap_or(fill_price) },
                        size: req.size,
                        filled_size: Decimal::ZERO,
                        status: if is_market { OrderStatus::Rejected } else { OrderStatus::Open },
                    };
                    if !is_market {
                        self.pending_orders.push((exchange_order_id, req));
                    }
                    ack
                } else if self.has_funds(&req, &fill_price) {
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
                } else if req.order_type == OrderType::Market {
                    // 资金不足 (Bug A 修复): 市价单 Rejected 丢弃并记录, 禁止静默免费成交。
                    self.rejected_count += 1;
                    OrderAck {
                        exchange_order_id: exchange_order_id.clone(),
                        client_order_id: req.client_order_id.clone(),
                        pair: req.pair.clone(),
                        side: req.side,
                        price: req.price.unwrap_or(fill_price),
                        size: req.size,
                        filled_size: Decimal::ZERO,
                        status: OrderStatus::Rejected,
                    }
                } else {
                    // 限价单触及但资金不足 → 保持 pending, 下 bar 再试 (K2/D4)。
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
        Ok(ack)
    }

    fn cancel_order(&mut self, _pair: &str, order_id: &str) -> CoreResult<()> {
        let before = self.pending_orders.len();
        self.pending_orders.retain(|(id, _)| id != order_id);
        if self.pending_orders.len() < before {
            Ok(())
        } else {
            Err(ricow_core::CoreError::OrderNotFound(order_id.to_string()))
        }
    }

    fn update_orderbook(&mut self, _pair: &str, _ob: OrderBook) {}

    fn record_fill(&mut self, fill: &OrderFill) {
        self.pnl.record_fill(fill);
    }

    fn drain_fills(&mut self) -> Vec<OrderFill> {
        std::mem::take(&mut self.fill_queue)
    }

    fn log(&self, msg: &str) {
        tracing::info!(target: "strategy.backtest", name = %self.config.name, "{msg}");
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
        // 只含已收盘 bar (Task 2 前视语义: 当前未收盘 bar 对策略不可见)。
        self.klines_for(pair)
    }

    /// 当前 tick 时间 (UTC); 组合模式 = 本 tick 各 bar open_time 最大值 (与 klines_for 的
    /// 截断口径同源, 无前视); 单标的 = 当前 bar 的 open_time。供盘中策略按"每日固定时刻"下单。
    fn now_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        if !self.portfolio_bars.is_empty() {
            self.portfolio_bars.values().map(|k| k.open_time).max()
        } else {
            self.current_bar.as_ref().map(|b| b.open_time)
        }
    }

    fn need_klines(&mut self, role: &str, tf: &str, min_bars: u32) {
        if role == "primary" && self.declarations.iter().any(|d| d.role == "primary") {
            tracing::error!(target: "context", tf, "主时钟(primary)重复声明: 至多一个 primary 序列");
            return;
        }
        if let Some(existing) = self.declarations.iter_mut().find(|d| d.role == role && d.tf == tf) {
            existing.min_bars = existing.min_bars.max(min_bars);
            return;
        }
        self.declarations.push(Declaration { role: role.to_string(), tf: tf.to_string(), min_bars });
    }

    fn declarations(&self) -> Vec<Declaration> {
        self.declarations.clone()
    }

    fn tf_klines(&self, pair: &str, tf: &str) -> Option<Vec<Kline>> {
        let now_ms = self.now_utc()?.timestamp_millis();
        let c = self.tf_caches.get(&tf_key(&self.resolve_key(pair), tf))?;
        Some(c.visible(now_ms).to_vec())
    }

    fn tf_cache_ref(&self, pair: &str, tf: &str) -> Option<Arc<TfCache>> {
        self.tf_caches.get(&tf_key(&self.resolve_key(pair), tf)).cloned()
    }

    fn set_tf_klines(&mut self, pair: &str, tf: &str, bars: Vec<Kline>) {
        let Some(tf_ms) = crate::multiframe::tf_ms_of(tf) else {
            tracing::warn!(target: "multiframe", tf, "未知高周期标签, 忽略预装");
            return;
        };
        let key = tf_key(&self.resolve_key(pair), tf);
        self.tf_caches.insert(key, Arc::new(TfCache::new(tf_ms, bars)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Context;
    use crate::Strategy;
    use rust_decimal_macros::dec;

    /// 默认测试 K 线: 固定 1970-01-01T05:00Z (hour=5, 避开 UTC 0/8/16 资金费结算边界,
    /// 防 futures 测试被随机结算污染; 资金费测试用 kline_at 显式构造边界时间)。
    fn kline(open: Decimal, high: Decimal, low: Decimal, close: Decimal) -> Kline {
        let t = 5 * 3600;
        Kline {
            open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
            open,
            high,
            low,
            close,
            volume: Decimal::ONE,
            close_time: chrono::DateTime::from_timestamp(t + 3600, 0).unwrap(),
        }
    }

    /// 带时间戳的 K 线 (1 小时一根), 供时间跨度相关测试。
    fn kline_at(t: i64, open: Decimal, high: Decimal, low: Decimal, close: Decimal) -> Kline {
        Kline {
            open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
            open,
            high,
            low,
            close,
            volume: Decimal::ONE,
            close_time: chrono::DateTime::from_timestamp(t + 3600, 0).unwrap(),
        }
    }

    #[test]
    fn test_limit_buy_fills_when_low_crosses() {
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // bar low = 2990 <= 3000 → 限价买单成交
        ctx.step_bar(kline(dec!(3000), dec!(3010), dec!(2990), dec!(3005)));
        let req = OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(3000), dec!(1));
        let ack = ctx.place_order(req).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        assert_eq!(ack.price, dec!(3000));
        assert_eq!(ctx.report().total_trades, 1);
    }

    #[test]
    fn test_limit_buy_rests_when_no_cross() {
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // bar low = 3010 > 3000 → 不成交, 挂单
        ctx.step_bar(kline(dec!(3010), dec!(3020), dec!(3010), dec!(3015)));
        let req = OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(3000), dec!(1));
        let ack = ctx.place_order(req).unwrap();
        assert_eq!(ack.status, OrderStatus::Open);
        assert_eq!(ctx.report().total_trades, 0);
    }

    #[test]
    fn test_balance_key_uses_base_asset_not_prefixed_pair() {
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.step_bar(kline(dec!(3000), dec!(3010), dec!(2990), dec!(3005)));
        // 带交易所前缀的 pair
        let req = OrderRequest::new_limit("binance:ETH", OrderSide::Buy, dec!(3000), dec!(1));
        let ack = ctx.place_order(req).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        // 余额键应是去前缀的资产 "ETH", 不是 "binance:ETH"
        assert_eq!(ctx.balance("ETH"), Some(dec!(1)));
        assert!(ctx.balance("binance:ETH").is_none());
    }

    #[test]
    fn test_price_returns_open_not_close() {
        // 前视回归: on_tick 只能见当前 bar 的 open, 不能见未收盘的 close。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.step_bar(kline(dec!(100), dec!(210), dec!(90), dec!(200)));
        // open=100, close=200: 若返回 close 则策略"看到了未来", 前视 bug。
        assert_eq!(ctx.price("ETH"), Some(dec!(100)));
    }

    /// 023 测试用: 第 `minute` 分钟的 1m bar(价格随分钟单调上行, 幅度固定)。
    fn bar_1m_023(minute: i64) -> Kline {
        let ms = minute * 60_000;
        let open = dec!(100) + Decimal::from(minute) / dec!(100);
        Kline {
            open_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap(),
            open,
            high: open + dec!(0.5),
            low: open - dec!(0.5),
            close: open,
            volume: dec!(1),
            close_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms + 59_999)
                .unwrap(),
        }
    }

    #[test]
    fn test_tf_klines_preloaded_visibility() {
        // 高周期 (1h) 通道: 装配层用全段(含预热)重采样预装; `tf_klines` 返回可见前缀
        // (无前视, 与 now_utc/klines_for 同源); 指标由策略侧显式现算。
        let mut params = std::collections::HashMap::new();
        params.insert("pair".to_string(), crate::config::ConfigValue::String("ETHUSDT".into()));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let bars: Vec<Kline> = (0..20 * 60).map(bar_1m_023).collect();
        let tf_ms = crate::tf_ms_of("1h").unwrap();
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
        );
        // 装配层语义: 用全段(20 桶, 含"未来")预装 —— 可见性必须由 tick 时间卡住。
        ctx.set_tf_klines("ETHUSDT", "1h", crate::resample_complete(&bars, tf_ms));

        // 推进到 14:00(即第 841 根) → 已收盘 1m bar = 0..839 分钟 → 完整桶 0..13 共 14 个
        // < period+1 = 15 → ATR 不可算(None)。注: 当前 bar 自己那一分钟尚未收盘, 故 14:00
        // 只能看到 0..13 桶 —— 这个 off-by-one 是本单测第一次跑红抓出来的。
        for k in &bars[..=840] {
            ctx.step_bar(k.clone());
        }
        let visible = ctx.tf_klines("ETHUSDT", "1h").expect("已预装 1h 序列");
        assert_eq!(visible.len(), 14, "可见桶 14 < 15(period+1)");
        assert_eq!(crate::indicators_api::atr(&visible, 14), None, "可见桶不足必须 None");

        // 推进到 15:00(第 901 根) → 完整桶 0..14 共 15 个 → 有值, 且等于对"可见段"
        // (前 900 根 = 0..899 分钟)独立重采样的直算结果。
        for k in &bars[841..=900] {
            ctx.step_bar(k.clone());
        }
        let visible = ctx.tf_klines("ETHUSDT", "1h").unwrap();
        let got = crate::indicators_api::atr(&visible, 14).expect("15 桶应有 ATR");
        let expected =
            crate::indicators_api::atr(&crate::resample_complete(&bars[..900], tf_ms), 14)
                .expect("独立重采样应可算 ATR");
        assert!((got - expected).abs() < 1e-9, "got={got} expected={expected}");

        // 无前视: 跨入下一桶后可见前缀才增长。
        for k in &bars[901..=960] {
            ctx.step_bar(k.clone());
        }
        let visible = ctx.tf_klines("ETHUSDT", "1h").unwrap();
        assert!(visible.len() > 15, "跨桶后可见前缀应增长");
    }

    #[test]
    #[ignore = "性能对照用例: cargo test -p ricow_strategy --lib klines_tail_perf -- --ignored --nocapture"]
    fn test_klines_tail_perf_30k_bars() {
        // 023 Task 4: 单标的路径 klines 尾窗化。3 万根历史下每 tick 克隆量必须 ≤ 100 根,
        // 1 万次调用应在毫秒~百毫秒级(尾窗化前是每次克隆 3 万根 → 秒级)。
        let mut params = std::collections::HashMap::new();
        params.insert("pair".to_string(), crate::config::ConfigValue::String("ETHUSDT".into()));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
        );
        for i in 0..30_000i64 {
            ctx.step_bar(bar_1m_023(i));
        }
        // 末根仍是 current_bar(未收盘), 故 closed_klines = 29999 根 —— 引擎既有语义。
        assert_eq!(ctx.closed_klines.len(), 29_999, "历史确实堆积到近 3 万根");
        let t0 = std::time::Instant::now();
        for _ in 0..10_000 {
            let k = ctx.klines("ETHUSDT").expect("应有尾窗");
            assert_eq!(k.len(), 100, "尾窗长度必须封顶 100");
        }
        let dt = t0.elapsed();
        println!("1 万次 ctx.klines(3 万根历史) 耗时 {dt:?}");
        // 对照: 旧行为 = 每 tick 克隆**全量**历史, 直接测同一份历史的 1 千次全量克隆。
        let t1 = std::time::Instant::now();
        for _ in 0..1_000 {
            let all = ctx.closed_klines.clone();
            assert!(all.len() > 29_000);
        }
        let old = t1.elapsed();
        println!(
            "对照(旧行为) 1 千次全量克隆 {old:?} → 等效 1 万次 ≈ {:.1}s(即尾窗化前 1m 回测不可行)",
            old.as_secs_f64() * 10.0
        );
        assert!(dt.as_secs_f64() < 2.0, "尾窗克隆过慢: {dt:?} (尾窗化回归?)");
    }

    #[test]
    fn test_cancel_pending_instruction_clears_resting_orders() {
        // 023 Task 5: 撤单指令 (action = CancelPending) —— ①在风控之前短路(不被 size=0 拒掉);
        // ②清掉本策略挂单; ③返 Cancelled 而不是 Rejected; ④不计 rejected_count。
        let mut params = std::collections::HashMap::new();
        params.insert("pair".to_string(), crate::config::ConfigValue::String("ETHUSDT".into()));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
        );
        ctx.step_bar(bar_1m_023(0));
        let px = ctx.price("ETHUSDT").expect("有价");
        // 两张远离现价的限价单 → 不成交, 进挂单队列。
        let buy = OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, px * dec!(0.5), dec!(1));
        let sell = OrderRequest::new_limit("ETHUSDT", OrderSide::Sell, px * dec!(2), dec!(1));
        assert_eq!(ctx.place_order(buy).unwrap().status, OrderStatus::Open);
        assert_eq!(ctx.place_order(sell).unwrap().status, OrderStatus::Open);
        assert_eq!(ctx.pending_orders.len(), 2);

        let cancel = OrderRequest {
            action: OrderAction::CancelPending,
            ..OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, px, Decimal::ZERO)
        };
        let ack = ctx.place_order(cancel).unwrap();
        assert_eq!(ack.status, OrderStatus::Cancelled, "撤单必须返 Cancelled");
        assert!(ctx.pending_orders.is_empty(), "挂单应被清空");
        assert_eq!(ctx.rejected_count, 0, "撤单不该计 rejected_count");

        // 只撤指定 pair: 另一个 pair 的挂单不受影响。
        let other = OrderRequest::new_limit("BNBUSDT", OrderSide::Buy, px * dec!(0.5), dec!(1));
        assert_eq!(ctx.place_order(other).unwrap().status, OrderStatus::Open);
        let cancel_eth = OrderRequest {
            action: OrderAction::CancelPending,
            ..OrderRequest::new_limit("ETHUSDT", OrderSide::Buy, px, Decimal::ZERO)
        };
        let _ = ctx.place_order(cancel_eth).unwrap();
        assert_eq!(ctx.pending_orders.len(), 1, "非目标 pair 的挂单必须保留");
    }

    #[test]
    fn test_realized_pnl_lifo_matching() {
        // 网格 LIFO 语义: 买 100/90/80 各 1 份, 卖 84 平 1 份 → 配对最近买入 80 → +4。
        // 均价口径 (84 - 90 = -6) 在深跌反弹场景失真, LIFO 是网格真实口径。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "shannon_spot_grid".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let buy = |_p: i64| OrderRequest {
            client_order_id: "t".into(),
            pair: "BNBUSDT".into(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            price: None,
            size: dec!(1),
            reduce_only: false,
            position_side: None,
            action: OrderAction::Place,
        };
        let sell = |_p: i64| OrderRequest {
            client_order_id: "t".into(),
            pair: "BNBUSDT".into(),
            side: OrderSide::Sell,
            order_type: OrderType::Market,
            price: None,
            size: dec!(1),
            reduce_only: false,
            position_side: None,
            action: OrderAction::Place,
        };
        ctx.execute_fill("b1", &buy(0), dec!(100));
        ctx.execute_fill("b2", &buy(0), dec!(90));
        ctx.execute_fill("b3", &buy(0), dec!(80));
        assert_eq!(ctx.pnl.realized_pnl(), Decimal::ZERO); // 只有买入, 未平仓
        ctx.execute_fill("s1", &sell(0), dec!(84));
        // LIFO: (84-80)×1 = +4 (均价口径会是 (84-90)×1 = -6)。
        assert_eq!(ctx.pnl.realized_pnl(), dec!(4));
        // 持仓剩 2 (100/90 批次), 批次队列同步。
        let lots = ctx.position_lots.get(&("bn:BNBUSDT".into(), OrderSide::Buy)).unwrap();
        assert_eq!(lots.len(), 2);
    }

    /// 测试策略: price 高于阈值买、低于阈值卖 (用市价单, 无 reduce_only)。
    /// 若 price() 泄漏 close (前视), 该策略在"open 恒 100、close 在 50/200 间跳"的序列上
    /// 会不断交易并盈利; 修复后只见 open=100, 永不触发, 0 交易 0 盈亏。
    struct ThresholdStrategy {
        threshold: Decimal,
    }

    impl Strategy for ThresholdStrategy {
        fn on_tick(&mut self, ctx: &mut dyn Context) -> Vec<OrderRequest> {
            match ctx.price("ETH") {
                Some(p) if p > self.threshold => {
                    vec![OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))]
                }
                Some(p) if p < self.threshold => {
                    vec![OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))]
                }
                _ => vec![],
            }
        }
    }

    #[test]
    fn test_lookahead_no_fake_profit() {
        // 前视回归 (策略级): "见 close 必赚" 序列, 修复后不虚高。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut strat = ThresholdStrategy { threshold: dec!(150) };
        // open 恒等于阈值 150 (只见 open 时永不触发); close 在 200/50 间跳 ——
        // 若策略能看到 close, 每根 bar 都会触发交易; 修复后 0 交易。
        let bars = vec![
            kline(dec!(150), dec!(210), dec!(90), dec!(200)),
            kline(dec!(150), dec!(205), dec!(45), dec!(50)),
            kline(dec!(150), dec!(210), dec!(90), dec!(200)),
            kline(dec!(150), dec!(205), dec!(45), dec!(50)),
        ];
        strat.on_init(&mut ctx);
        for k in &bars {
            ctx.step_bar(k.clone());
            for req in strat.on_tick(&mut ctx) {
                let _ = ctx.place_order(req);
            }
            ctx.drain_fills();
        }
        let report = ctx.report();
        // 修复后 price() 恒为 open=150, 阈值 150 永不触发: 0 成交。
        assert_eq!(report.total_trades, 0, "前视泄漏会让该策略虚高交易");
        assert_eq!(report.net_pnl, Decimal::ZERO);
    }

    /// 滑点回归: 同序列跑 slippage=0 与 slippage>0, 断言滑点降低净盈亏。
    struct BuySellStrategy {
        tick: u32,
    }

    impl Strategy for BuySellStrategy {
        fn on_tick(&mut self, _ctx: &mut dyn Context) -> Vec<OrderRequest> {
            self.tick += 1;
            match self.tick {
                1 => vec![OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))],
                2 => vec![OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))],
                _ => vec![],
            }
        }
    }

    fn run_market_roundtrip(slippage_bps: i64) -> Decimal {
        let mut params = HashMap::new();
        params.insert("slippage_bps".into(), crate::config::ConfigValue::Integer(slippage_bps));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut strat = BuySellStrategy { tick: 0 };
        strat.on_init(&mut ctx);
        // bar1 open=100: 买 1 ETH (市价, 当根成交); bar2 open=110: 卖 1 ETH (市价, 当根成交)。
        let bars = vec![
            kline(dec!(100), dec!(105), dec!(95), dec!(104)),
            kline(dec!(110), dec!(115), dec!(105), dec!(114)),
        ];
        for k in &bars {
            ctx.step_bar(k.clone());
            for req in strat.on_tick(&mut ctx) {
                let _ = ctx.place_order(req);
            }
            ctx.drain_fills();
        }
        ctx.report().net_pnl
    }

    #[test]
    fn test_slippage_reduces_net_pnl() {
        let no_slip = run_market_roundtrip(0);
        let with_slip = run_market_roundtrip(10);
        assert!(with_slip < no_slip, "滑点应降低净盈亏: no_slip={no_slip}, with_slip={with_slip}");
    }

    #[test]
    fn test_klines_closed_only() {
        // 前视语义: klines() 只含已收盘 bar, 不含当前未收盘 bar。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // 未 push 任何 bar: 空。
        assert_eq!(ctx.klines("ETH"), Some(vec![]));
        ctx.step_bar(kline(dec!(100), dec!(105), dec!(95), dec!(104)));
        // 第 1 根正在走: 已收盘序列仍为空。
        assert_eq!(ctx.klines("ETH"), Some(vec![]));
        ctx.step_bar(kline(dec!(104), dec!(110), dec!(100), dec!(108)));
        ctx.step_bar(kline(dec!(108), dec!(115), dec!(105), dec!(112)));
        // 3 根后: 只有前 2 根已收盘, 第 3 根 (当前) 不可见。
        let closed = ctx.klines("ETH").unwrap();
        assert_eq!(closed.len(), 2);
        assert_eq!(closed[0].open, dec!(100));
        assert_eq!(closed[1].open, dec!(104));
    }

    #[test]
    fn test_report_has_drawdown_and_fee_ratio() {
        // 一次买 + 一次卖 (slippage=0), 报告应含最大回撤与手续费占比。
        let mut params = HashMap::new();
        params.insert("slippage_bps".into(), crate::config::ConfigValue::Integer(0));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDC".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut strat = BuySellStrategy { tick: 0 };
        strat.on_init(&mut ctx);
        for k in [
            kline(dec!(100), dec!(105), dec!(95), dec!(104)),
            kline(dec!(110), dec!(115), dec!(105), dec!(114)),
        ] {
            ctx.step_bar(k);
            for req in strat.on_tick(&mut ctx) {
                let _ = ctx.place_order(req);
            }
            ctx.drain_fills();
        }
        let report = ctx.report();
        // 有成交且有手续费 → fee_ratio > 0 (10bps 双边)。
        assert!(report.fee_ratio > Decimal::ZERO, "fee_ratio = {}", report.fee_ratio);
        // 手续费占比 = 总手续费/总成交额, 应小于 1%。
        assert!(report.fee_ratio < dec!(0.01), "fee_ratio = {}", report.fee_ratio);
        // 最大回撤 >= 0 (有交易产生费用, 权益先降后升)。
        assert!(report.max_drawdown >= Decimal::ZERO);
    }

    #[test]
    fn test_equity_curve_marks_to_market_each_bar() {
        // 市价建仓后价涨: 权益曲线按每根 bar 收盘估值, 含未实现盈亏。
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // bar1: 市价买入 1 ETH @ open=100 (当根成交), close=104。
        ctx.step_bar(kline(dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        // bar2: 价涨 close=114, 无操作。
        ctx.step_bar(kline(dec!(110), dec!(115), dec!(105), dec!(114)));
        // 已收盘 bar1 → 曲线 = [初始, mark(bar1 close)] = 2 点 (当前 bar2 由 report 补齐)。
        assert_eq!(ctx.equity_curve.len(), 2, "每根已收盘 bar 一个估值点 + 初始点");
        assert_eq!(ctx.equity_curve[0], dec!(100000));
        // mark(bar1 close=104) = 现金(建仓后) + 1×104 > 初始 (含未实现浮盈)。
        assert!(ctx.equity_curve[1] > dec!(100000), "bar1 收盘点应含浮盈: {}", ctx.equity_curve[1]);
        // bar3 价继续涨 close=118, 无操作 → 曲线 = [初始, mark(bar1), mark(bar2)] = 3 点。
        ctx.step_bar(kline(dec!(116), dec!(120), dec!(114), dec!(118)));
        assert_eq!(ctx.equity_curve.len(), 3);
        let last = ctx.equity_curve.last().unwrap();
        assert!(*last > dec!(100000), "末点应含浮盈: {last}");
        // report() 补齐最后一根未收盘 bar 后计算回撤, 不 panic、数值合理。
        let report = ctx.report();
        assert!(report.max_drawdown >= Decimal::ZERO);
        assert!(report.equity_change_pct > 0.0, "期末总价值应高于初始");
    }

    #[test]
    fn test_report_metrics_and_rf_config() {
        // 市价建仓 + 价涨: 曲线有波动 → 风险指标非 None; rf 缺省 3.8、可被参数覆盖。
        let mut params = HashMap::new();
        params.insert("risk_free_rate".into(), crate::config::ConfigValue::Float(5.0));
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params,
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx = BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.step_bar(kline_at(0, dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        ctx.step_bar(kline_at(3600, dec!(102), dec!(103), dec!(97), dec!(98)));
        ctx.step_bar(kline_at(7200, dec!(110), dec!(120), dec!(108), dec!(118)));
        let report = ctx.report();
        assert_eq!(report.risk_free_rate, 5.0, "参数应覆盖默认 rf");
        assert!(report.annual_return.is_some(), "有波动应算得出年化收益");
        assert!(report.annual_volatility.is_some());
        assert!(report.sharpe.is_some());
        assert!(report.sortino.is_some(), "含回调序列有下行偏差 → 索提诺有定义");
        assert!(report.calmar.is_some(), "含回调序列有回撤 → Calmar 有定义");
        assert!(report.profit_factor.is_none(), "只有买入无平仓 → 无盈亏比");
        assert!(report.avg_win.is_none());

        // 缺省 rf = 3.8。
        let config2 = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        };
        let mut ctx2 = BacktestContext::new(
            config2,
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx2.step_bar(kline_at(0, dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx2.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        ctx2.step_bar(kline_at(3600, dec!(102), dec!(103), dec!(97), dec!(98)));
        ctx2.step_bar(kline_at(7200, dec!(110), dec!(120), dec!(108), dec!(118)));
        let report2 = ctx2.report();
        assert_eq!(report2.risk_free_rate, 3.8, "缺省 rf = 3.8");
        // 更高 rf → 夏普更低。
        assert!(report2.sharpe.unwrap() > report.sharpe.unwrap(), "rf 升高应拉低夏普");
    }

    /// 带市场/持仓模式/现金的测试上下文 (费率/杠杆等走三层默认; 合约默认 taker 5bps)。
    fn test_ctx(market: &str, position_mode: &str, cash: i64) -> BacktestContext {
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: market.into(),
            position_mode: position_mode.into(),
            backtest: None,
        };
        BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: Decimal::from(cash), locked: Decimal::ZERO },
        )
    }

    /// 019-R5: 固定工程护栏接线 —— 同一 tick 第 101 单被拒, 且策略循环不中断 (前 100 单正常)。
    #[test]
    fn test_order_guard_rate_limit_wired_into_backtest() {
        let mut ctx = test_ctx("spot", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        for i in 1..=100 {
            let ack =
                ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
            assert_eq!(ack.status, OrderStatus::Filled, "第 {i} 单应正常成交 (固定上限 100 单/秒)");
        }
        let a101 =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        assert_eq!(
            a101.status,
            OrderStatus::Rejected,
            "同 tick 第 101 单应被工程护栏拒绝: {a101:?}"
        );
        assert_eq!(ctx.report().rejected_count, 1, "护栏拒单应如实计数");
        assert_eq!(ctx.balance("ETH"), Some(dec!(100)), "被拒订单不得成交");
    }

    /// 019-R5: 固定护栏不误伤常规节奏 (单 tick 少量下单恒放行)。
    #[test]
    fn test_order_guard_defaults_do_not_misfire() {
        let mut ctx = test_ctx("spot", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        for _ in 0..5 {
            let ack =
                ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
            assert_eq!(ack.status, OrderStatus::Filled, "固定护栏下小程序下单不应被拒");
        }
        assert_eq!(ctx.report().rejected_count, 0, "固定护栏不得误拒");
    }

    #[test]
    fn test_report_metrics_none_without_data() {
        let ctx = test_ctx("spot", "one-way", 100000);
        let report = ctx.report();
        assert!(report.annual_return.is_none());
        assert!(report.annual_volatility.is_none());
        assert!(report.sharpe.is_none());
        assert!(report.sortino.is_none());
        assert!(report.calmar.is_none());
        assert!(report.profit_factor.is_none());
        assert!(report.avg_win.is_none());
        assert_eq!(report.risk_free_rate, 3.8);
        assert_eq!(report.rejected_count, 0);
        assert!(report.leverage.is_none(), "现货报告合约字段为 None");
        assert!(report.liquidation_count.is_none());
    }

    // ---- T4: Bug A/B 回归 (specs/backtest.md §四) ----

    #[test]
    fn test_market_buy_rejected_when_cash_insufficient() {
        // Bug A 修复: 现金耗尽后, 市价买单被拒 (Rejected 丢弃), 持仓不再增长。
        let mut ctx = test_ctx("spot", "one-way", 150);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        // 买 1 ETH @100 需 100 + 0.1 费 = 100.1 ≤ 150 → 成交。
        let ack1 =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        assert_eq!(ack1.status, OrderStatus::Filled);
        // 再买: 现金 49.9 < 100.1 → 拒单, 持仓/余额不变。
        let ack2 =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        assert_eq!(ack2.status, OrderStatus::Rejected, "现金不足市价单必须拒 (Bug A)");
        assert_eq!(ctx.report().rejected_count, 1, "拒单应如实计数");
        assert_eq!(ctx.balance("ETH"), Some(dec!(1)), "持仓不得免费增长");
        assert_eq!(ctx.balance("USDT"), Some(dec!(49.9)), "现金只扣一次 (100 + 0.1 费)");
    }

    #[test]
    fn test_market_sell_rejected_without_base() {
        // 现货禁空 (specs/backtest.md §四.4): 无币卖出 → 拒单, 不产生虚拟空仓。
        let mut ctx = test_ctx("spot", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let ack =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))).unwrap();
        assert_eq!(ack.status, OrderStatus::Rejected, "无 base 现货卖必须拒 (禁空)");
        assert_eq!(ctx.report().rejected_count, 1);
        assert!(ctx.position("ETH").is_none(), "不得产生虚拟空仓");
    }

    #[test]
    fn test_limit_buy_insufficient_stays_pending() {
        // Bug A 修复 (K2/D4): 限价单触及但现金不足 → 保持 pending, 下 bar 再试 (不静默成交)。
        let mut ctx = test_ctx("spot", "one-way", 50);
        ctx.step_bar(kline(dec!(101), dec!(102), dec!(99), dec!(101)));
        let req = OrderRequest::new_limit("ETH", OrderSide::Buy, dec!(100), dec!(1));
        let ack = ctx.place_order(req.clone()).unwrap();
        // bar low=99 ≤ 100 触及, 但 100.1 > 50 → Open + 挂单。
        assert_eq!(ack.status, OrderStatus::Open);
        // 下根 bar 仍触及但资金不足 → 继续 pending, 不成交。
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(95), dec!(100)));
        let report = ctx.report();
        assert_eq!(report.total_trades, 0, "资金不足限价单不得成交");
        assert_eq!(report.rejected_count, 0, "限价单挂起不算拒单");
        assert_eq!(ctx.pending_orders.len(), 1, "订单保持 pending");
    }

    #[test]
    fn test_spot_fee_deducted_from_cash() {
        // Bug B 修复: 手续费真实扣余额。买 1 @100 扣 100+0.1; 卖 1 @110 得 110−0.11。
        let mut ctx = test_ctx("spot", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        ctx.step_bar(kline(dec!(110), dec!(115), dec!(105), dec!(114)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))).unwrap();
        let report = ctx.report();
        assert_eq!(report.final_cash, dec!(100009.79), "现金 = 初始 − 名义−费 + 回款 (费实扣)");
        assert_eq!(report.total_fees, dec!(0.21));
        assert_eq!(report.realized_pnl, dec!(10));
        assert_eq!(report.net_pnl, dec!(9.79));
    }

    // ---- T4: 合约记账 (K3 逐仓钱包推演) ----

    #[test]
    fn test_futures_roundtrip_wallet_accounting() {
        // K3 推演 (杠杆 1x, 合约默认费率 taker 5bps):
        // 开多 M=10000 → cash 9万, wallet 9995 (M − 开仓费 5);
        // 平多盈利 1000 @110 → wallet += 1000 − 5.5 → 10989.5, 平净后整体回现金 → 100989.5。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100))).unwrap();
        assert_eq!(ctx.balance("USDT"), Some(dec!(90000)), "开仓现金 −= M (不动现金付费)");
        ctx.step_bar(kline(dec!(110), dec!(115), dec!(105), dec!(114)));
        let req =
            OrderRequest::new_market("ETH", OrderSide::Sell, dec!(100)).with_reduce_only(true);
        let ack = ctx.place_order(req).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        let report = ctx.report();
        assert_eq!(report.final_cash, dec!(100989.5), "总权益 = 初始 + 已实现 − Σ费 (防双重计费)");
        assert_eq!(report.realized_pnl, dec!(1000));
        assert_eq!(report.total_fees, dec!(10.5), "开仓费 5 + 平仓费 5.5");
        assert_eq!(report.net_pnl, dec!(989.5));
        assert!(ctx.position("ETH").is_none(), "平净后无持仓");
    }

    // ---- T4 补充: one-way 反向单先平后开 (K6) ----

    #[test]
    fn test_futures_one_way_reversal_close_then_open() {
        // one-way 反向单 = 先平后开: 多 50 时卖 150 → 平 50 + 开空 100。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(105), dec!(95), dec!(104)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(50))).unwrap();
        ctx.step_bar(kline(dec!(110), dec!(115), dec!(105), dec!(114)));
        let ack =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(150))).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled);
        let pos = ctx.position("ETH").unwrap();
        assert_eq!(pos.side, OrderSide::Sell, "平多后余量开空");
        assert_eq!(pos.size, dec!(100));
        // 空头盈利 (价格跌) 走同一钱包: 平空 100 @90。
        ctx.step_bar(kline(dec!(90), dec!(95), dec!(85), dec!(92)));
        let req = OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100)).with_reduce_only(true);
        ctx.place_order(req).unwrap();
        assert!(ctx.position("ETH").is_none());
    }

    // ---- T5: 强平与资金费 (specs/backtest.md §五.4/5) ----

    fn close_enough(a: Decimal, b: Decimal) -> bool {
        (a - b).abs() < dec!(0.01)
    }

    /// 带杠杆等 backtest 参数的测试上下文。
    fn test_ctx_bt(market: &str, mode: &str, cash: i64, bt: BacktestToml) -> BacktestContext {
        let config = StrategyConfig {
            name: "t".into(),
            strategy_type: "custom".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: market.into(),
            position_mode: mode.into(),
            backtest: Some(bt),
        };
        BacktestContext::new(
            config,
            Balance { asset: "USDT".into(), free: Decimal::from(cash), locked: Decimal::ZERO },
        )
    }

    #[test]
    fn test_futures_long_liquidation_at_liq_price() {
        // 2x 单侧多仓深跌 → 按强平价强平 (K4), 计数 +1, 钱包余额回现金。
        // mmr_pct 显式 2.5: 断言值按此算 (默认 2026-09-05 起改 1.0, 测试自包含不随默认漂移)。
        let bt = BacktestToml { leverage: Some(2.0), mmr_pct: Some(2.5), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "one-way", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(200))).unwrap();
        // 触发 bar: 跌至 45 (多仓强平价 ≈ 51.33)。
        ctx.step_bar(kline(dec!(55), dec!(56), dec!(45), dec!(50)));
        ctx.step_bar(kline(dec!(52), dec!(54), dec!(51), dec!(53)));
        let report = ctx.report();
        assert!(ctx.position("ETH").is_none(), "爆仓后无持仓");
        assert_eq!(report.liquidation_count, Some(1));
        // 现金 = 9万 + wallet(9990 + realized@liq − 平仓费) ≈ 90251.53
        assert!(
            close_enough(report.final_cash, dec!(90251.53)),
            "final_cash = {}",
            report.final_cash
        );
        // 强平 fill 有 LIQ 标识且计了平仓费。
        assert!(
            report.fills.iter().any(|f| f.client_order_id.starts_with("LIQ")),
            "强平 fill 应带 LIQ 标识"
        );
    }

    #[test]
    fn test_l4_market_noop_order_rejected_without_fee_or_fill() {
        // L4: hedge 下对"无多仓"发市价 sell + position_side="long"(= 平多)。
        // 修复前: 成交但零动作 —— 仍收手续费并记一笔成交 (成本/笔数虚增)。
        let bt = BacktestToml { leverage: Some(1.0), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "hedge", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let mut noop = OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1));
        noop.position_side = Some("long".into());

        let ack = ctx.place_order(noop).unwrap();
        assert_eq!(ack.status, OrderStatus::Rejected, "无多仓的市价平多单应被拒");
        assert_eq!(ack.filled_size, Decimal::ZERO);

        let r = ctx.report();
        assert_eq!(r.total_trades, 0, "零动作不得记成交笔数");
        assert_eq!(r.total_fees, Decimal::ZERO, "零动作不得计费");
        assert_eq!(r.rejected_count, 1);
        assert!(r.fills.is_empty(), "不得产生 fill 记录");
    }

    #[test]
    fn test_l4_limit_noop_rests_without_fee_then_fills_once_valid() {
        // 限价单的零动作在"成交那一刻"不成交、不计费、保持挂单; 等仓位出现后可正常平仓。
        let bt = BacktestToml { leverage: Some(1.0), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "hedge", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let mut exit = OrderRequest::new_limit("ETH", OrderSide::Sell, dec!(1), dec!(99));
        exit.position_side = Some("long".into());
        let ack = ctx.place_order(exit).unwrap();
        assert_eq!(ack.status, OrderStatus::Open, "零动作限价单应挂 pending (不成交)");
        assert_eq!(ctx.report().total_fees, Decimal::ZERO, "未成交不得计费");

        // 下一 bar 价格穿越, 仍无多仓 → 依旧不成交不计费
        ctx.step_bar(kline(dec!(99), dec!(100), dec!(98), dec!(99)));
        assert_eq!(ctx.report().total_trades, 0, "无仓可平不得凭空成交");
        assert_eq!(ctx.report().total_fees, Decimal::ZERO);

        // 开多仓后价格再次穿越 → 该挂单恢复为有效的平多单, 正常成交
        let mut open_long = OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1));
        open_long.position_side = Some("long".into());
        assert_eq!(ctx.place_order(open_long).unwrap().status, OrderStatus::Filled);
        ctx.step_bar(kline(dec!(99), dec!(100), dec!(98), dec!(99)));
        assert_eq!(ctx.report().total_trades, 2, "买开 + 挂单平多 = 2 笔");
        assert!(
            ctx.position_side("ETH", OrderSide::Buy).map(|p| p.size).unwrap_or_default()
                == Decimal::ZERO,
            "挂单应作为平多成交"
        );
    }

    #[test]
    fn test_l4_one_way_sell_without_position_still_opens_short() {
        // 反向保护: one-way 无仓卖出 = 合法开空, L4 不得误拒。
        let bt = BacktestToml { leverage: Some(1.0), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "one-way", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let ack =
            ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))).unwrap();
        assert_eq!(ack.status, OrderStatus::Filled, "one-way 无仓卖出是开空, 不应被拒");
        assert_eq!(ctx.report().total_trades, 1);
    }

    #[test]
    fn test_futures_hedge_side_independent_liquidation() {
        // 013 FR-002: hedge 两侧**独立钱包 + 独立强平** —— 依据 2026-09-05 真实清算实测
        // (specs/backtest.md §十一 ①: 价格下行只清算 LONG 侧, SHORT 侧持仓与其强平价全程不变)。
        //
        // 10x hedge 多 300 空 200, mmr 2.5% (显式, 不随默认漂移):
        //   LONG  独立强平价 = (w − e·size) / (size·(1−MMR)) = (3000 − 30000) / (300·0.975) ≈ 92.31
        //   SHORT 独立强平价 = (w + e·size) / (size·(1+MMR)) = (2000 + 20000) / (200·1.025) ≈ 107.32
        // 旧实现是"按对共享钱包"(组合权益穿负) → SHORT 会被多头亏损牵连而**提前**清算, 与真实不符。
        let bt = BacktestToml { leverage: Some(10.0), mmr_pct: Some(2.5), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "hedge", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let mut long = OrderRequest::new_market("ETH", OrderSide::Buy, dec!(300));
        long.position_side = Some("long".into());
        ctx.place_order(long).unwrap();
        let mut short = OrderRequest::new_market("ETH", OrderSide::Sell, dec!(200));
        short.position_side = Some("short".into());
        ctx.place_order(short).unwrap();

        // 跌 bar: low 55 ≤ LONG 独立强平价 92.31 → 只清 LONG; SHORT 独立钱包不受影响。
        ctx.step_bar(kline(dec!(62), dec!(63), dec!(55), dec!(58)));
        ctx.step_bar(kline(dec!(56), dec!(57), dec!(54), dec!(55)));
        let long_left =
            ctx.position_side("ETH", OrderSide::Buy).map(|p| p.size).unwrap_or_default();
        assert_eq!(long_left, Decimal::ZERO, "LONG 侧应被独立清算");
        assert_eq!(ctx.report().liquidation_count, Some(1));
        let short_pos = ctx.position_side("ETH", OrderSide::Sell);
        assert!(short_pos.is_some(), "SHORT 侧钱包独立 → 不因 LONG 被清算而消失 (真实实测 ①)");
        assert_eq!(short_pos.unwrap().size, dec!(200), "SHORT 侧持仓原样保留");

        // 涨 bar 穿越 SHORT 独立强平价 107.32 → 此时才轮到此侧 (证明两侧可先后独立爆仓)
        ctx.step_bar(kline(dec!(105), dec!(110), dec!(104), dec!(109)));
        ctx.step_bar(kline(dec!(108), dec!(109), dec!(106), dec!(108)));
        let short_left =
            ctx.position_side("ETH", OrderSide::Sell).map(|p| p.size).unwrap_or_default();
        assert_eq!(short_left, Decimal::ZERO, "价格穿越 SHORT 独立强平价后该侧才被清算");
        assert_eq!(ctx.report().liquidation_count, Some(2), "两侧先后各计一次");
    }

    #[test]
    fn test_funding_rate_long_pays_at_8h_boundary() {
        // 1x 多仓跨 UTC 08:00 边界 → 多头支付 rate×名义 (rate 默认 0.0001, 名义按该 bar close)。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline_at(2 * 3600, dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100))).unwrap();
        // 08:00 bar (open_time hour = 8): 结算在下一根 push 时触发; close=110 → 名义 11000 → 付 1.1。
        ctx.step_bar(kline_at(8 * 3600, dec!(108), dec!(112), dec!(107), dec!(110)));
        ctx.step_bar(kline_at(9 * 3600, dec!(110), dec!(111), dec!(109), dec!(110)));
        let report = ctx.report();
        let net = report.funding_net.unwrap();
        assert!(close_enough(net, dec!(-1.1)), "多头净付 0.0001×100×110 = 1.1, got {net}");
        // 现金不变 (资金费走钱包); 强平 0 次。
        assert_eq!(report.liquidation_count, Some(0));
    }

    #[test]
    fn test_funding_hedge_same_size_offset() {
        // hedge 同量多空 (013 起两侧钱包独立: 多头付、空头收, **不跨侧抵消**) → 账户级净额仍为 0。
        let mut ctx = test_ctx("futures", "hedge", 100000);
        ctx.step_bar(kline_at(2 * 3600, dec!(100), dec!(101), dec!(99), dec!(100)));
        let mut long = OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100));
        long.position_side = Some("long".into());
        ctx.place_order(long).unwrap();
        let mut short = OrderRequest::new_market("ETH", OrderSide::Sell, dec!(100));
        short.position_side = Some("short".into());
        ctx.place_order(short).unwrap();
        ctx.step_bar(kline_at(8 * 3600, dec!(108), dec!(112), dec!(107), dec!(110)));
        ctx.step_bar(kline_at(9 * 3600, dec!(110), dec!(111), dec!(109), dec!(110)));
        let report = ctx.report();
        assert_eq!(report.funding_net, Some(Decimal::ZERO), "两侧独立结算后账户级净额 = 0");
    }

    #[test]
    fn test_funding_settlements_in_bar_is_interval_independent() {
        // 013 FR-004 (修 L1): 结算点 = bar 覆盖时段 [open, close) 内的 8h 边界 (UTC 00/08/16)。
        const H: i64 = 3600 * 1000;
        // 1m bar 12:00–12:01 → 不含边界
        assert_eq!(funding_settlements_in_bar(12 * H, 12 * H + 60_000), 0);
        // 1h bar 08:00–09:00 → 含 08:00 边界恰好 1 次 (旧实现按 hour()==8 也命中, 行为一致)
        assert_eq!(funding_settlements_in_bar(8 * H, 9 * H), 1);
        // 1m bar 00:00–00:01 → 1 次 (旧实现 00:00–00:59 段 60 根全命中 → 每日 180 次, 超结)
        assert_eq!(funding_settlements_in_bar(0, 60_000), 1);
        // 4h bar 04:00–08:00 → 不含 08:00 (半开区间, 结算点归下一根) → 0; 08:00–12:00 → 1
        assert_eq!(funding_settlements_in_bar(4 * H, 8 * H), 0);
        assert_eq!(funding_settlements_in_bar(8 * H, 12 * H), 1);
        // 1d bar 00:00–24:00 → 3 次 (00/08/16; 旧实现每日仅 1 次 → 欠结 2/3)
        assert_eq!(funding_settlements_in_bar(0, 24 * H), 3);
        // 零长/倒置区间安全
        assert_eq!(funding_settlements_in_bar(8 * H, 8 * H), 0);
        assert_eq!(funding_settlements_in_bar(9 * H, 8 * H), 0);
    }

    #[test]
    fn test_hedge_funding_flows_between_independent_side_wallets() {
        // 013 FR-003: hedge 两侧钱包独立 —— 多头付、空头收, **不跨侧抵消**;
        // one-way 才按净仓抵消 (见 test_funding_hedge_same_size_offset 的账户级净额 = 0)。
        let mut ctx = test_ctx("futures", "hedge", 100000);
        ctx.step_bar(kline_at(2 * 3600, dec!(100), dec!(101), dec!(99), dec!(100)));
        let mut long = OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100));
        long.position_side = Some("long".into());
        ctx.place_order(long).unwrap();
        let mut short = OrderRequest::new_market("ETH", OrderSide::Sell, dec!(100));
        short.position_side = Some("short".into());
        ctx.place_order(short).unwrap();
        // 跨 08:00 结算: 名义 100×110 = 11000, rate 0.0001 → 每侧 1.1
        ctx.step_bar(kline_at(8 * 3600, dec!(108), dec!(112), dec!(107), dec!(110)));
        ctx.step_bar(kline_at(9 * 3600, dec!(110), dec!(111), dec!(109), dec!(110)));

        let key = ctx.resolve_key("ETH");
        let long_wallet = ctx.wallet.get(&format!("{key}|long")).copied().unwrap_or_default();
        let short_wallet = ctx.wallet.get(&format!("{key}|short")).copied().unwrap_or_default();
        // 1x: 开仓保证金各 11000 → 多头钱包 11000 − 1.1 − 开仓费, 空头钱包 11000 + 1.1 − 开仓费
        assert!(
            long_wallet < short_wallet,
            "多头钱包应少于空头钱包 (多头付、空头收, 不抵消): long={long_wallet} short={short_wallet}"
        );
        let diff = short_wallet - long_wallet;
        assert!(close_enough(diff, dec!(2.2)), "两侧差 2×资金费 1.1, got {diff}");
    }

    #[test]
    fn test_finalize_settles_last_bar() {
        // 013 FR-005 (修 L2): 回测停在 8h 边界 bar 时, 末段资金费仍须进报告。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline_at(2 * 3600, dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100))).unwrap();
        // 08:00 边界 bar 作为**最后一根** (无后继 push → 靠 finalize 补结算)
        ctx.step_bar(kline_at(8 * 3600, dec!(108), dec!(112), dec!(107), dec!(110)));
        let before = ctx.report().funding_net.unwrap();
        assert_eq!(before, Decimal::ZERO, "未 finalize 前末段结算尚未发生 (L2 现象)");
        ctx.finalize();
        let after = ctx.report().funding_net.unwrap();
        assert!(
            close_enough(after, dec!(-1.1)),
            "finalize 后补上 0.0001×100×110 = 1.1, got {after}"
        );
        ctx.finalize(); // 幂等
        assert!(close_enough(ctx.report().funding_net.unwrap(), dec!(-1.1)), "finalize 幂等");
    }

    #[test]
    fn test_futures_liquidation_distance_matches_real_calibration() {
        // SC-005: 2026-09-05 DASHUSDT 真实清算实测 50x + 首档 MMR 1.5% → 开仓即算的强平距离 ≈0.53%(含手续费);
        // 纯保证金公式 = 1 − (1/L − 1)/(1−MMR) = 1 − (−0.98/0.985) = 0.508%。
        // 断言边界行为: 未穿越 (low 高于强平价) 不清算; 穿越 (low 低于) 清算 —— 距离量级与实测同阶。
        let bt = BacktestToml { leverage: Some(50.0), mmr_pct: Some(1.5), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "one-way", 100000, bt);
        // 开仓 bar 窄幅 (low 99.95 > 强平价 99.492) —— 否则下一根 push 的结算会立刻触发
        ctx.step_bar(kline(dec!(100), dec!(100.2), dec!(99.95), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(10))).unwrap();
        // 上一根 bar 的 low 99.95 未穿 ≈99.492 → 不清算
        ctx.step_bar(kline(dec!(100), dec!(100.2), dec!(99.95), dec!(100)));
        assert_eq!(ctx.report().liquidation_count, Some(0), "未穿越 ≈0.51% 距离不应清算");
        // low 99.40 < 99.492 → 穿越 → 清算
        ctx.step_bar(kline(dec!(100), dec!(100.1), dec!(99.40), dec!(99.6)));
        ctx.step_bar(kline(dec!(99.6), dec!(99.7), dec!(99.5), dec!(99.6)));
        assert_eq!(ctx.report().liquidation_count, Some(1), "穿越 ≈0.51% 距离后应清算");
    }

    #[test]
    fn test_spot_has_no_liquidation_or_funding() {
        // 现货回归: 资金费/强平不生效; 深跌持仓保留 (specs/backtest.md §四.4)。
        let mut ctx = test_ctx("spot", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(10))).unwrap();
        // 跨 8h 边界的深跌 bar。
        ctx.step_bar(kline_at(8 * 3600, dec!(55), dec!(56), dec!(45), dec!(50)));
        ctx.step_bar(kline_at(9 * 3600, dec!(52), dec!(54), dec!(51), dec!(53)));
        let report = ctx.report();
        assert!(report.liquidation_count.is_none(), "现货无强平");
        assert!(report.funding_net.is_none(), "现货无资金费");
        let pos = ctx.position("ETH").unwrap();
        assert_eq!(pos.size, dec!(10), "现货深跌持仓保留 (无杠杆无强平)");
    }

    // ---- D1/D2 修复回归 (审核 2026-09-04): 平空盈亏符号 + 部分平仓后 entry 重算 ----

    #[test]
    fn test_futures_short_profit_roundtrip_amounts() {
        // D1 回归: 空头盈利平仓须如实入账 (曾符号反, 盈利 1000 记成 −1000)。
        // 1x 开空 100@100 → wallet 9995 (M 10000 − 开仓费 5); 平空 @90:
        // realized = (100−90)×100 = +1000, 平仓费 4.5 → 平净 sweep → cash 100990.5。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(100))).unwrap();
        assert_eq!(ctx.balance("USDT"), Some(dec!(90000)), "开空现金 −= M");
        ctx.step_bar(kline(dec!(90), dec!(95), dec!(85), dec!(92)));
        ctx.place_order(
            OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100)).with_reduce_only(true),
        )
        .unwrap();
        let report = ctx.report();
        assert_eq!(report.realized_pnl, dec!(1000), "空头盈利须为正 (D1)");
        assert_eq!(report.total_fees, dec!(9.5), "开仓费 5 + 平仓费 4.5");
        assert_eq!(report.final_cash, dec!(100990.5), "总权益 = 初始 + 盈利 − Σ费");
        assert_eq!(report.net_pnl, dec!(990.5));
        assert!(ctx.position("ETH").is_none(), "平净后无持仓");
    }

    #[test]
    fn test_futures_short_loss_roundtrip_amounts() {
        // D1 回归: 空头亏损须如实入账 (曾符号反, 亏损 1000 记成 +1000)。
        // 开空 100@100; 平空 @110: realized = (100−110)×100 = −1000。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(100))).unwrap();
        ctx.step_bar(kline(dec!(110), dec!(115), dec!(105), dec!(114)));
        ctx.place_order(
            OrderRequest::new_market("ETH", OrderSide::Buy, dec!(100)).with_reduce_only(true),
        )
        .unwrap();
        let report = ctx.report();
        assert_eq!(report.realized_pnl, dec!(-1000), "空头亏损须为负 (D1)");
        assert_eq!(report.final_cash, dec!(98989.5), "初始 − 亏损 − Σ费 5+5.5");
        assert!(ctx.position("ETH").is_none());
    }

    #[test]
    fn test_futures_partial_close_recompute_entry() {
        // D2 回归: 部分平仓后 entry 须重算为剩余批次加权均价, 权益不虚跳。
        // 多 1@100 + 1@90 (entry 95); 平 1@95 (LIFO 平 90 批, realized +5);
        // 剩余 1 成本 100 → 价 100 时未实现 0, 权益 = 99810 现金 + 194.8575 钱包。
        // 若 entry 不重算 (仍 95) → 权益虚高 5 (100009.8575)。
        let mut ctx = test_ctx("futures", "one-way", 100000);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        ctx.step_bar(kline(dec!(90), dec!(91), dec!(89), dec!(90)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Buy, dec!(1))).unwrap();
        ctx.step_bar(kline(dec!(95), dec!(96), dec!(94), dec!(95)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(1))).unwrap();
        // 剩余仓 entry = 未平批次 (100) 的成本, 非混合均价 95。
        let pos = ctx.position("ETH").unwrap();
        assert_eq!(pos.size, dec!(1));
        assert_eq!(pos.entry_price, dec!(100), "部分平仓后 entry 重算为剩余批次均价 (D2)");
        // 价格回到 100: 无持仓浮盈 → 权益 = 100000 + 5 已实现 − Σ费 (0.05+0.045+0.0475)。
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        let report = ctx.report();
        assert_eq!(
            report.final_equity,
            dec!(100004.8575),
            "部分平仓后权益不得虚跳 (D2), got {}",
            report.final_equity
        );
    }

    #[test]
    fn test_futures_short_liquidation_losses_money() {
        // D1 回归 (强平路径): 空头被强平须亏损扣钱包 (曾符号反 → 强平反而虚盈回血)。
        // 2x 空 200@100, 涨至高 170 ≥ 强平价 (≈146.29) → 平空亏 realized, 现金 < 初始。
        let bt = BacktestToml { leverage: Some(2.0), ..Default::default() };
        let mut ctx = test_ctx_bt("futures", "one-way", 100000, bt);
        ctx.step_bar(kline(dec!(100), dec!(101), dec!(99), dec!(100)));
        ctx.place_order(OrderRequest::new_market("ETH", OrderSide::Sell, dec!(200))).unwrap();
        ctx.step_bar(kline(dec!(160), dec!(170), dec!(150), dec!(165)));
        ctx.step_bar(kline(dec!(150), dec!(152), dec!(148), dec!(151)));
        let report = ctx.report();
        assert_eq!(report.liquidation_count, Some(1));
        assert!(ctx.position("ETH").is_none(), "爆仓后无持仓");
        assert!(
            report.realized_pnl < Decimal::ZERO,
            "空头强平 realized 须为负 (D1), got {}",
            report.realized_pnl
        );
        assert!(
            report.final_cash < dec!(100000) && report.final_cash > dec!(90000),
            "强平后现金应低于初始 (亏损), got {}",
            report.final_cash
        );
        assert!(
            report.fills.iter().any(|f| f.client_order_id.starts_with("LIQ")),
            "强平 fill 应带 LIQ 标识"
        );
    }

    // ---- 组合模式 (M2 bs_momentum 多标的回测) ----

    /// 组合模式测试配置。
    fn portfolio_config() -> StrategyConfig {
        StrategyConfig {
            name: "p".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    #[test]
    fn test_portfolio_two_pairs_tick_and_match() {
        // 双标的日线对齐推进: 每 tick 两标的各一根 bar; 市价单按各自 pair 当前 bar open 撮合。
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        // tick1: A open 100, B open 200 (均为日线; 下一 tick 收尾时估 closed)。
        ctx.step_portfolio(&[
            ("TSLABUSDT".into(), kline(dec!(100), dec!(110), dec!(95), dec!(105))),
            ("NVDABUSDT".into(), kline(dec!(200), dec!(220), dec!(190), dec!(210))),
        ]);
        // tick1 下市价单: 各按自己 pair 当前 bar open 成交 (100 / 200)。
        let ack_a = ctx
            .place_order(OrderRequest::new_market("TSLABUSDT", OrderSide::Buy, dec!(50)))
            .unwrap();
        let ack_b = ctx
            .place_order(OrderRequest::new_market("NVDABUSDT", OrderSide::Buy, dec!(25)))
            .unwrap();
        assert_eq!(ack_a.status, OrderStatus::Filled);
        assert_eq!(ack_b.status, OrderStatus::Filled);
        // 50×100 + 25×200 = 10000 名义 (费 10bps×2 已含在 cash 扣减)。
        assert!(ctx.position("TSLABUSDT").is_some());
        assert!(ctx.position("NVDABUSDT").is_some());
        // tick2: A 涨到 110, B 跌到 190 → 收盘估值应为现金 + 50×110 + 25×190。
        ctx.step_portfolio(&[
            ("TSLABUSDT".into(), kline(dec!(110), dec!(115), dec!(105), dec!(112))),
            ("NVDABUSDT".into(), kline(dec!(190), dec!(195), dec!(185), dec!(192))),
        ]);
        let r = ctx.report_portfolio();
        // 现金 = 100000 − (5000+5000) − 手续费 ≈ 89900 (fee 20bps 均值计);
        // 权益 = 现金 + 50×112 + 25×192 ≈ 89900 + 5600 + 4800 ≈ 100300 (略低于, 费已扣)。
        assert!(r.final_equity > dec!(100000), "组合双标的盈利: {}", r.final_equity);
        assert_eq!(r.total_trades, 2);
        // ctx:klines 按 pair 路由 (已收盘序列只含 tick1 的 bar)。
        let ks_a = ctx.klines("TSLABUSDT").unwrap();
        assert_eq!(ks_a.len(), 1);
        assert_eq!(ks_a[0].close, dec!(105), "tick1 A 收盘 105");
        // price 前视安全: 当前 tick bar open。
        assert_eq!(ctx.price("TSLABUSDT"), Some(dec!(110)));
        assert_eq!(ctx.price("NVDABUSDT"), Some(dec!(190)));
    }

    #[test]
    fn test_portfolio_insufficient_cash_rejected() {
        // 组合模式现金不足 → 市价单拒 (Bug A 语义保持)。
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(1000), locked: Decimal::ZERO },
        );
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(100), dec!(110), dec!(95), dec!(105)),
        )]);
        let ack = ctx
            .place_order(OrderRequest::new_market("TSLABUSDT", OrderSide::Buy, dec!(50)))
            .unwrap();
        assert_eq!(ack.status, OrderStatus::Rejected, "资金不足市价单必须拒");
        assert_eq!(ctx.report_portfolio().rejected_count, 1);
    }

    #[test]
    fn test_portfolio_report_m2_fields() {
        // M2-6: 组合报告 M2 字段齐全 — 净值曲线/换手率/持仓快照 (含空仓 tick), 单标的恒空快照。
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
        );
        // tick0: 无仓 (空快照点); tick1 买入; tick2 持有。
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(100), dec!(110), dec!(95), dec!(105)),
        )]);
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(106), dec!(112), dec!(100), dec!(110)),
        )]);
        ctx.place_order(OrderRequest::new_market("TSLABUSDT", OrderSide::Buy, dec!(10))).unwrap();
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(111), dec!(115), dec!(108), dec!(114)),
        )]);
        let report = ctx.report_portfolio();
        // 曲线: [初始, tick0末, tick1末, tick2末(补估)]。
        assert!(report.equity_curve.len() >= 4, "曲线应含初始+3 tick 估值");
        assert_eq!(report.equity_curve[0], dec!(10000), "曲线起点 = 初始现金");
        // 快照: tick0末(空) + tick1末(持仓) + tick2末… 记录点 = 每 step_portfolio 收旧 bars 时。
        assert_eq!(
            report.holdings_snapshots.len(),
            2,
            "tick0/tick1 末各一快照 (末 tick 补估无快照)"
        );
        assert!(report.holdings_snapshots[0].is_empty(), "tick0 末空仓快照");
        let snap1 = &report.holdings_snapshots[1];
        assert_eq!(snap1.len(), 1, "tick1 末应持仓 TSLABUSDT");
        assert_eq!(snap1[0].0, "TSLABUSDT");
        assert!(snap1[0].1 > Decimal::ZERO);
        // 换手率 > 0 (有 1 笔买入)。
        assert!(report.turnover_ratio > 0.0, "有成交则换手率 > 0");
        // 手续费/成本已披露。
        assert!(report.total_fees > Decimal::ZERO);
        // 单标的路径: 快照恒空 (回归约束)。
        let mut solo = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
        );
        solo.step_bar(kline(dec!(100), dec!(110), dec!(95), dec!(105)));
        solo.place_order(OrderRequest::new_market("TSLABUSDT", OrderSide::Buy, dec!(1))).unwrap();
        let solo_report = solo.report();
        assert!(solo_report.holdings_snapshots.is_empty(), "单标的快照恒空");
        assert!(!solo_report.equity_curve.is_empty(), "单标的亦有净值曲线");
    }

    // ---- 组合信号模式 (T2 bs_momentum Lua 化: signal_klines 容器 + 全局 tick 截断) ----

    const DAY: i64 = 86_400;

    /// 合成美股信号线: day 0 起每天一根 (UTC 00:00 锚, 与 nasdaq.rs 时间契约一致), n 根。
    fn signal_line(n: usize) -> Vec<Kline> {
        (0..n)
            .map(|i| {
                let t = i as i64 * DAY;
                let p = rust_decimal::Decimal::from(100 + i as i64);
                Kline {
                    open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
                    open: p,
                    high: p + dec!(1),
                    low: p - dec!(1),
                    close: p,
                    volume: Decimal::ONE,
                    close_time: chrono::DateTime::from_timestamp(t + DAY, 0).unwrap(),
                }
            })
            .collect()
    }

    /// 成交轨日线 bar (UTC 日对齐: open_time = 当日 00:00)。
    fn exec_bar(day: i64, px: i64) -> Kline {
        let t = day * DAY;
        let p = rust_decimal::Decimal::from(px);
        Kline {
            open_time: chrono::DateTime::from_timestamp(t, 0).unwrap(),
            open: p,
            high: p + dec!(1),
            low: p - dec!(1),
            close: p,
            volume: Decimal::ONE,
            close_time: chrono::DateTime::from_timestamp(t + DAY, 0).unwrap(),
        }
    }

    /// T2: signal 截断按全局 tick 时间 — tick 推进到 day k → 返回信号线 < day k 全段;
    /// on_init 期 (无 tick) 空段 (策略零单幂等); 信号线尽头后不再增长。
    #[test]
    fn test_signal_klines_truncated_to_global_tick() {
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig: HashMap<String, Vec<Kline>> = HashMap::new();
        sig.insert("TSLABUSDT".into(), signal_line(300));
        ctx.set_signal_klines(sig);
        assert!(ctx.klines_for("TSLABUSDT").unwrap().is_empty(), "on_init 期空段 (幂等零单)");
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(10, 200))]);
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), 10, "day10 tick 可见 day0..9 共 10 根, got {}", ks.len());
        assert_eq!(
            ks.last().unwrap().open_time,
            chrono::DateTime::from_timestamp(9 * DAY, 0).unwrap(),
            "末根 = tick 日前最后一根信号线"
        );
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(15, 210))]);
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), 15, "day15 tick → day0..14");
        // 信号线 300 根尽头: day400 tick 后不再增长。
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(400, 300))]);
        assert_eq!(ctx.klines_for("TSLABUSDT").unwrap().len(), 300);
    }

    /// T2: 无前视 — 改未来 (> 当前 tick) 价格不改变已返回段; tick 越过该日后新值才可见。
    #[test]
    fn test_signal_klines_no_lookahead() {
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig: HashMap<String, Vec<Kline>> = HashMap::new();
        let mut line = signal_line(300);
        line[15].close = dec!(9999); // tick10 时的"未来 bar"
        sig.insert("TSLABUSDT".into(), line);
        ctx.set_signal_klines(sig);
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(10, 200))]);
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), 10);
        assert!(ks.iter().all(|k| k.close != dec!(9999)), "未来价格不得渗入已返回段 (无前视)");
        // 越过 day15 → day15 bar 出现且带 9999 (此时已是历史)。
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(16, 210))]);
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), 16);
        assert!(ks.iter().any(|k| k.close == dec!(9999)), "越过日后该 bar 应可见");
    }

    /// 007 v2: 信号段尾窗封顶 (R1 十年回测性能前提) — 超长信号线只返回最近 SIGNAL_TAIL 根,
    /// 且无前视保留 (截断仍按全局 tick 时间, 封顶只裁旧根不裁未来)。
    #[test]
    fn test_signal_klines_tail_capped_at_signal_tail() {
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig: HashMap<String, Vec<Kline>> = HashMap::new();
        let mut line = signal_line(2600); // ≈ 十年 (2514) 量级
        line[2599].close = dec!(99999); // 未来 marker (当前 tick 远未到)
        sig.insert("TSLABUSDT".into(), line);
        ctx.set_signal_klines(sig);
        // tick 推进到 day 2500 → 信号线 < day2500 共 2500 根 → 应裁到 SIGNAL_TAIL 根。
        ctx.step_portfolio(&[("TSLABUSDT".into(), exec_bar(2500, 200))]);
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), SIGNAL_TAIL, "2500 根信号段应封顶为 SIGNAL_TAIL, got {}", ks.len());
        assert_eq!(
            ks.last().unwrap().open_time,
            chrono::DateTime::from_timestamp(2499 * DAY, 0).unwrap(),
            "末根 = tick 日前最后一根 (day2499)"
        );
        assert_eq!(
            ks.first().unwrap().open_time,
            chrono::DateTime::from_timestamp((2500 - SIGNAL_TAIL) as i64 * DAY, 0).unwrap(),
            "首根 = 末根前 SIGNAL_TAIL-1 根 (升序保留尾部)"
        );
        assert!(
            ks.iter().all(|k| k.close != dec!(99999)),
            "未来 marker (day2599) 不得渗入 (无前视)"
        );
        // 短段 (< SIGNAL_TAIL) 不裁: 回到既有行为。
        let mut ctx2 = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig2: HashMap<String, Vec<Kline>> = HashMap::new();
        sig2.insert("TSLABUSDT".into(), signal_line(300));
        ctx2.set_signal_klines(sig2);
        ctx2.step_portfolio(&[("TSLABUSDT".into(), exec_bar(250, 200))]);
        assert_eq!(
            ctx2.klines_for("TSLABUSDT").unwrap().len(),
            250,
            "短段 (250 < SIGNAL_TAIL) 维持现行为不裁"
        );
    }

    /// T2: 数据缺口日 (该 pair 当日无 bar) — 全局 tick 时间由其它 pair bar 决定, 截断不悬空 (Y1)。
    #[test]
    fn test_signal_klines_gap_day_uses_global_tick() {
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        let mut sig: HashMap<String, Vec<Kline>> = HashMap::new();
        sig.insert("TSLABUSDT".into(), signal_line(300));
        sig.insert("NVDABUSDT".into(), signal_line(300));
        ctx.set_signal_klines(sig);
        ctx.step_portfolio(&[
            ("TSLABUSDT".into(), exec_bar(5, 100)),
            ("NVDABUSDT".into(), exec_bar(5, 200)),
        ]);
        // day6: A 缺口 (无 bar), 只有 B — A 截断仍按全局 tick (B 的 day6)。
        ctx.step_portfolio(&[("NVDABUSDT".into(), exec_bar(6, 210))]);
        let ks_a = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks_a.len(), 6, "A 缺口日截断仍按全局 tick day6 (day0..5), got {}", ks_a.len());
        let ks_b = ctx.klines_for("NVDABUSDT").unwrap();
        assert_eq!(ks_b.len(), 6);
    }

    /// T2 回归: 容器空 (set 空 map) → 回落现组合已收盘序列逻辑, 信号路径不介入。
    #[test]
    fn test_signal_klines_empty_map_falls_back_to_portfolio() {
        let mut ctx = BacktestContext::new(
            portfolio_config(),
            Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
        );
        ctx.set_signal_klines(HashMap::new());
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(100), dec!(110), dec!(95), dec!(105)),
        )]);
        ctx.step_portfolio(&[(
            "TSLABUSDT".into(),
            kline(dec!(106), dec!(112), dec!(100), dec!(110)),
        )]);
        // 无 signal → ctx:klines 走 portfolio_klines (已收盘序列: 第一 tick 的 bar)。
        let ks = ctx.klines_for("TSLABUSDT").unwrap();
        assert_eq!(ks.len(), 1, "回落组合已收盘序列");
        assert_eq!(ks[0].close, dec!(105), "tick1 bar 收盘 105");
    }
}
