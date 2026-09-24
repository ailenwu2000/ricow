//! 核心数据类型: 行情 / 订单 / 持仓 / 余额 / 事件等, 跨交易所共享。

use std::fmt;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

/// 订单指令类型 (2026-09-17, 023 香农 ETF 指数增加策略)。
///
/// 策略层没有订单号通道 (`on_tick` 只回订单数组, 不回 ack; 拒单/撤单经 `on_order_update`
/// 回传, 但策略侧无 client_order_id 映射), 因此"成交后撤掉自己的其余挂单"只能按**本实例归属**
/// 整体撤: 回测/模拟盘清 `pending_orders`(策略是 context 内唯一下单方), 实盘按 `clientOrderId` 归属前缀撤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OrderAction {
    /// 普通下单 (默认)。
    #[default]
    Place,
    /// 撤销本策略当前全部挂单。**不占下单限频、不计 `rejected_count`**;
    /// 必须在风控/对齐层之前短路 (该指令 size = 0, 走普通路径会被判"数量为 0"拒掉)。
    CancelPending,
}

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
    /// 订单指令 (默认 Place = 普通下单)。见 [`OrderAction`]。
    #[serde(default)]
    pub action: OrderAction,
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
            action: OrderAction::Place,
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
            action: OrderAction::Place,
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

impl OrderAck {
    /// 转成 [`OrderUpdate`] (审计 #3): 引擎在拒单/撤单等终态时回传给策略 `on_order_update`。
    pub fn to_update(&self, timestamp: DateTime<Utc>) -> OrderUpdate {
        OrderUpdate {
            exchange_order_id: self.exchange_order_id.clone(),
            client_order_id: self.client_order_id.clone(),
            pair: self.pair.clone(),
            status: self.status,
            filled_size: self.filled_size,
            remaining_size: self.size - self.filled_size,
            avg_price: None,
            timestamp,
        }
    }
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
