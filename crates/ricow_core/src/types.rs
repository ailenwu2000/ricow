//! 核心数据类型: 行情 / 订单 / 持仓 / 余额 / 事件等, 跨交易所共享。

use std::fmt;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};
use crate::interval::Interval;

// ---- Market ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Market {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub is_perpetual: bool,
    pub min_size: Decimal,
    pub tick_size: Decimal,
    /// 数量步进 (LOT_SIZE.stepSize): 下单数量须为 step_size 整数倍; None = 无步进约束 (兼容旧缓存数据)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_size: Option<Decimal>,
    /// 最小名义价值 (MIN_NOTIONAL.minNotional, 计价资产口径): 对齐后不足即拒单;
    /// None = 交易所无该过滤器或旧缓存数据 (不拦截, 交交易所判定)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_notional: Option<Decimal>,
    pub max_leverage: Option<u32>,
    /// 保证金模式: None = 默认(normal), Some("strictIsolated"), Some("noCross")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_mode: Option<String>,
    /// 是否已下架
    #[serde(default)]
    pub is_delisted: bool,
}

// ---- Kline ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Kline {
    pub open_time: DateTime<Utc>,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: Decimal,
    pub close_time: DateTime<Utc>,
}

// ---- OrderBook ----

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OrderBook {
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PriceLevel {
    pub price: Decimal,
    pub size: Decimal,
}

impl OrderBook {
    /// 创建已排序的 OrderBook, 确保 bids 降序 / asks 升序。
    pub fn new_sorted(mut bids: Vec<PriceLevel>, mut asks: Vec<PriceLevel>) -> Self {
        bids.sort_by_key(|p| std::cmp::Reverse(p.price));
        asks.sort_by_key(|p| p.price);
        Self { bids, asks, timestamp: Utc::now() }
    }

    pub fn best_bid(&self) -> Option<&PriceLevel> {
        self.bids.first()
    }

    pub fn best_ask(&self) -> Option<&PriceLevel> {
        self.asks.first()
    }

    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid.price + ask.price) / Decimal::from(2)),
            _ => None,
        }
    }

    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }
}

// ---- Order enums ----

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "buy"),
            OrderSide::Sell => write!(f, "sell"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum OrderType {
    Limit,
    Market,
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Limit => write!(f, "limit"),
            OrderType::Market => write!(f, "market"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum OrderStatus {
    Open,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
    Expired,
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderStatus::Open => write!(f, "open"),
            OrderStatus::Filled => write!(f, "filled"),
            OrderStatus::PartiallyFilled => write!(f, "partially_filled"),
            OrderStatus::Cancelled => write!(f, "cancelled"),
            OrderStatus::Rejected => write!(f, "rejected"),
            OrderStatus::Expired => write!(f, "expired"),
        }
    }
}

// ---- Order request / ack / fill / update ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub price: Option<Decimal>,
    pub size: Decimal,
    pub reduce_only: bool,
    /// 持仓方向 (合约 hedge 模式): None = one-way/BOTH 语义 (兼容旧脚本);
    /// Some("long")/Some("short") 显式指定持仓方向 (K7, specs/backtest.md §五.3)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_side: Option<String>,
}

impl OrderRequest {
    pub fn new_limit(
        pair: impl Into<String>,
        side: OrderSide,
        price: Decimal,
        size: Decimal,
    ) -> Self {
        Self {
            client_order_id: Uuid::new_v4().to_string(),
            pair: pair.into(),
            side,
            order_type: OrderType::Limit,
            price: Some(price),
            size,
            reduce_only: false,
            position_side: None,
        }
    }

    pub fn new_market(pair: impl Into<String>, side: OrderSide, size: Decimal) -> Self {
        Self {
            client_order_id: Uuid::new_v4().to_string(),
            pair: pair.into(),
            side,
            order_type: OrderType::Market,
            price: None,
            size,
            reduce_only: false,
            position_side: None,
        }
    }

    pub fn with_reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub price: Decimal,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub status: OrderStatus,
}

/// 未成交挂单 (停机清理用: 撤单兜底与残留明细)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderInfo {
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub price: Decimal,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub status: OrderStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderFill {
    pub trade_id: Option<String>,
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub fill_price: Decimal,
    pub fill_size: Decimal,
    pub fee: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub exchange_order_id: String,
    pub client_order_id: String,
    pub pair: String,
    pub status: OrderStatus,
    pub filled_size: Decimal,
    pub remaining_size: Decimal,
    pub avg_price: Option<Decimal>,
    pub timestamp: DateTime<Utc>,
}

// ---- Balance / Position ----

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Balance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

impl Balance {
    pub fn total(&self) -> Decimal {
        self.free + self.locked
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub pair: String,
    pub side: OrderSide,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub liquidation_price: Option<Decimal>,
    pub unrealized_pnl: Decimal,
    pub leverage: Option<Decimal>,
}

/// 解析跨所 pair 字符串。
///
/// 格式: `"exchange:pair"` → `("exchange", "pair")`, 无前缀回退 `"ETH"` → `("", "ETH")`。
/// 用于 Context 中统一路由交易所前缀。
pub fn parse_pair(pair: &str) -> (&str, &str) {
    pair.split_once(':').unwrap_or(("", pair))
}

// ---- Events ----

/// 资金费流水一条 (014): 以交易所账单口径为准(负 = 支付, 正 = 收取)。
///
/// 放在 `ricow_core` 而非交易所 crate: 它是**账户级事实**, 回测/实盘/报告都要用, 与 `OrderFill` 同级;
/// 具体交易所如何拉取(端点/签名)是 `ricow_binance` 的实现细节。
#[derive(Debug, Clone, PartialEq)]
pub struct FundingIncome {
    /// 交易对 (合约资金费按 symbol 计; 空串 = 账户级条目)。
    pub symbol: String,
    /// 净额: 负 = 支付, 正 = 收取。
    pub income: Decimal,
    /// 计价资产 (通常 USDT)。
    pub asset: String,
    /// 结算时间 (毫秒)。
    pub time_ms: i64,
    /// 交易所流水号 (落库幂等键)。
    pub tran_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserEvent {
    Order(OrderUpdate),
    Fill(OrderFill),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderBookUpdate {
    pub pair: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub timestamp: DateTime<Utc>,
}

// ---- 数据服务: 带复权列的 bar (028 T012) ----

/// K 线 + 可选的复权收盘。
///
/// 为什么不是一个新 bar 类型: 既有 [`Kline`] 有 33 处字面量构造点(加字段会连锁炸全仓),
/// 且两个并行的 bar 类型本身就是双轨。这里只做"给 K 线挂一列可选复权价"的包装:
/// 数据源能提供复权收盘时(Yahoo)填 [`Bar::adj_close`], 不能提供时(Nasdaq/币安)为 `None`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bar {
    pub kline: Kline,
    /// 复权收盘(源原生提供时才有); `None` = 该源不提供。
    pub adj_close: Option<Decimal>,
}

impl Bar {
    /// 无复权列的 bar。
    pub fn new(kline: Kline) -> Self {
        Self { kline, adj_close: None }
    }

    /// 带复权收盘的 bar。
    pub fn with_adj_close(kline: Kline, adj_close: Option<Decimal>) -> Self {
        Self { kline, adj_close }
    }

    /// 拆成 `(Kline, adj_close)`(缓存写入的列口径)。
    pub fn into_parts(self) -> (Kline, Option<Decimal>) {
        (self.kline, self.adj_close)
    }
}

// ---- 数据服务: 序列键与口径 (028 T005) ----

/// 价格口径: 序列暴露给策略的 OHLC 取原始价还是复权价。
///
/// `AdjClose` 仅在数据源提供复权收盘时有意义(Yahoo 提供 `adjclose`, 币安不提供);
/// 源不支持该模式时**报错**, 不静默回落原始价(防口径混杂)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PriceMode {
    /// 原始收盘(默认)。
    Close,
    /// 复权收盘(Yahoo `adjclose`)。
    AdjClose,
}

impl PriceMode {
    /// 配置/日志口径标签。
    pub fn label(self) -> &'static str {
        match self {
            PriceMode::Close => "close",
            PriceMode::AdjClose => "adjclose",
        }
    }

    /// 标签 → 口径; 未知返回 `None`。
    pub fn from_label(label: &str) -> Option<PriceMode> {
        match label {
            "close" => Some(PriceMode::Close),
            "adjclose" => Some(PriceMode::AdjClose),
            _ => None,
        }
    }
}

impl fmt::Display for PriceMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 序列口径 (028 FR-005): 源原生粒度 vs 由更细粒度重采样而来。
///
/// 为什么显式暴露: 同一 `interval` 可能是源原生给的, 也可能是平台用 5m 合成 15m ——
/// 策略需要知道这件事, 否则"换了个来源指标突然不一样"无法解释。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SeriesMode {
    /// 源原生提供该周期。
    Native,
    /// 源不提供该周期, 由更细粒度重采样(仅完整桶, 见 `resample::resample_to_interval`)。
    Resampled,
}

impl SeriesMode {
    /// 配置/日志口径标签。
    pub fn label(self) -> &'static str {
        match self {
            SeriesMode::Native => "native",
            SeriesMode::Resampled => "resampled",
        }
    }
}

impl fmt::Display for SeriesMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 序列装载结果 (028 FR-005): 尾窗 K 线 + 口径 + 实际取数粒度。
///
/// 装配层拿它构造 `SeriesInfo`(策略可见的"这条序列是什么"); 重采样时 `feed_interval` 与
/// `key.interval` 不同, 策略据此知道"我这条 15m 是平台用 5m 合成的"。
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesWindow {
    pub bars: Vec<Kline>,
    pub mode: SeriesMode,
    /// 实际取数粒度: `Native` 时 = 请求周期; `Resampled` 时 = 参与合成的细粒度。
    pub feed_interval: Interval,
}

impl SeriesWindow {
    /// 原生口径结果(直接来自源的该周期)。
    pub fn native(bars: Vec<Kline>, interval: Interval) -> Self {
        Self { bars, mode: SeriesMode::Native, feed_interval: interval }
    }

    /// 重采样口径结果。
    pub fn resampled(bars: Vec<Kline>, feed_interval: Interval) -> Self {
        Self { bars, mode: SeriesMode::Resampled, feed_interval }
    }
}

/// 序列键(数据服务的唯一标识): `(source, symbol, interval)`。
///
/// - `source`: 数据源注册名(`binance_spot` / `binance_futures` / `nasdaq` / `yahoo`);
/// - `symbol`: **源原生写法**(`ETHUSDT` / `QQQ`) —— 引擎不做跨源翻译、不猜;
/// - `interval`: 周期标签见 [`Interval`]。
///
/// 校验: `source` / `symbol` 非空, 且不含缓存键分隔符 `|`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SeriesKey {
    pub source: String,
    pub symbol: String,
    pub interval: Interval,
}

impl SeriesKey {
    /// 构造并校验。
    pub fn new(source: &str, symbol: &str, interval: Interval) -> CoreResult<Self> {
        Self::validate_part("source", source)?;
        Self::validate_part("symbol", symbol)?;
        Ok(Self { source: source.to_string(), symbol: symbol.to_string(), interval })
    }

    fn validate_part(field: &str, value: &str) -> CoreResult<()> {
        if value.trim().is_empty() {
            return Err(CoreError::InvalidArgument(format!("{field} 不能为空")));
        }
        if value.contains('|') {
            return Err(CoreError::InvalidArgument(format!("{field} 不能含分隔符 '|': {value}")));
        }
        Ok(())
    }

    /// 缓存键(`data_klines` 主键口径): `source|symbol|interval`。
    pub fn cache_key(&self) -> String {
        format!("{}|{}|{}", self.source, self.symbol, self.interval.label())
    }

    /// 缓存键反解; 格式不符或周期标签未知返回 `None`。
    pub fn parse_cache_key(raw: &str) -> Option<SeriesKey> {
        let mut parts = raw.split('|');
        let source = parts.next()?;
        let symbol = parts.next()?;
        let interval_label = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let interval = Interval::from_label(interval_label)?;
        SeriesKey::new(source, symbol, interval).ok()
    }
}

impl fmt::Display for SeriesKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}@{}", self.source, self.symbol, self.interval)
    }
}

/// 序列元信息(策略可见): 口径与新鲜度。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeriesMeta {
    pub key: SeriesKey,
    pub price_mode: PriceMode,
    /// `true` = 由主序列重采样得到(源无该原生周期); `false` = 源原生周期。
    pub resampled: bool,
    /// `true` = 最近一次增量回补失败, 数据可能过期(策略自行判断, 引擎不静默用旧数据冒充新数据)。
    pub stale: bool,
    /// 请求的尾窗根数(下限 / 上限由序列句柄校验)。
    pub requested_bars: usize,
}

impl SeriesMeta {
    /// 新建(默认非重采样、非 stale)。
    pub fn new(key: SeriesKey, price_mode: PriceMode, requested_bars: usize) -> Self {
        Self { key, price_mode, resampled: false, stale: false, requested_bars }
    }
}
