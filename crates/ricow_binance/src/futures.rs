//! Binance USDT-M 合约交易所实现 — 实现 `Exchange` trait (012 合约实盘适配层)。
//!
//! 与现货 `BnSpotExchange` 平行: REST 复用 `FuturesClient`(签名端点已实测), WS 复用 `futures_ws`
//! (fapi 盘口 + listenKey 用户流)。上层(引擎/CLI/Lua)不需知道市场差异。

use std::pin::Pin;
use std::str::FromStr;

use async_trait::async_trait;
use futures::Stream;
use ricow_core::{
    Balance, CoreResult, Exchange, FundingIncome, Kline, Market, OrderAck, OrderBook,
    OrderBookUpdate, OrderInfo, OrderRequest, OrderSide, OrderType, Position, UserEvent,
};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::client::{format_dec, status_from_bn};
use crate::futures_client::FuturesClient;

fn side_to_str(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "BUY",
        OrderSide::Sell => "SELL",
    }
}

fn type_to_str(ot: OrderType) -> &'static str {
    match ot {
        OrderType::Limit => "LIMIT",
        OrderType::Market => "MARKET",
    }
}

/// 内部持仓方向 → fapi `positionSide` (纯函数, 便于单测)。
///
/// `None` = one-way 模式(不传该参数); `Some("long"/"short")` → `LONG`/`SHORT` (hedge)。
pub fn fapi_position_side(side: Option<&str>) -> Option<String> {
    match side.map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if s == "long" => Some("LONG".to_string()),
        Some(s) if s == "short" => Some("SHORT".to_string()),
        _ => None,
    }
}

fn dec_field(v: &Value, key: &str) -> Decimal {
    Decimal::from_str(v[key].as_str().unwrap_or("0")).unwrap_or_default()
}

fn opt_dec_field(v: &Value, key: &str) -> Option<Decimal> {
    Decimal::from_str(v[key].as_str()?).ok().filter(|d| !d.is_zero())
}

fn position_of(p: &Value, pair: &str, side: OrderSide, size: Decimal) -> Position {
    Position {
        pair: pair.to_string(),
        side,
        size,
        entry_price: dec_field(p, "entryPrice"),
        mark_price: dec_field(p, "markPrice"),
        liquidation_price: opt_dec_field(p, "liquidationPrice"),
        unrealized_pnl: dec_field(p, "unRealizedProfit"),
        leverage: opt_dec_field(p, "leverage"),
    }
}

/// `positionRisk` JSON → **净仓** Position (one-way `BOTH` 单条; hedge 多空合并取净); 无仓 → None。
///
/// 纯函数, 便于单测。注意 hedge 的 SHORT 侧 `positionAmt` 本身为负。
pub fn position_from_risk(raw: &Value, pair: &str) -> Option<Position> {
    let arr = raw.as_array()?;
    let mut net = Decimal::ZERO;
    let mut both: Option<&Value> = None;
    let mut long: Option<&Value> = None;
    let mut short: Option<&Value> = None;
    for p in arr {
        let amt = dec_field(p, "positionAmt");
        match p["positionSide"].as_str().unwrap_or("BOTH") {
            "LONG" => {
                net += amt;
                long = Some(p);
            }
            "SHORT" => {
                net += amt;
                short = Some(p);
            }
            _ => {
                net += amt;
                both = Some(p);
            }
        }
    }
    if net.is_zero() {
        return None;
    }
    let side = if net > Decimal::ZERO { OrderSide::Buy } else { OrderSide::Sell };
    let size = net.abs();
    // 取"有量"的那一行作价格来源: hedge 下可能同时存在空仓的 BOTH 占位行
    let src = both
        .filter(|p| !dec_field(p, "positionAmt").is_zero())
        .or(if side == OrderSide::Buy { long } else { short })
        .or(long)
        .or(short)?;
    Some(position_of(src, pair, side, size))
}

/// `positionRisk` JSON → **指定方向**持仓
///
/// 仅测试使用: 生产路径的方向性持仓由 `BnFuturesExchange::get_positions_directional` 组合
/// `risk_rows` + `position_of`（同一语义），本函数保留为可测的纯函数基线。 (hedge: LONG/SHORT 行; one-way: BOTH 行按净仓方向匹配)。
///
/// 纯函数, 便于单测。无该方向仓位 → None。
#[cfg(test)]
pub fn directional_position_from_risk(
    raw: &Value,
    pair: &str,
    side: OrderSide,
) -> Option<Position> {
    let arr = raw.as_array()?;
    let want = match side {
        OrderSide::Buy => "LONG",
        OrderSide::Sell => "SHORT",
    };
    if let Some(p) = arr.iter().find(|p| p["positionSide"].as_str() == Some(want)) {
        let amt = dec_field(p, "positionAmt");
        if !amt.is_zero() {
            return Some(position_of(p, pair, side, amt.abs()));
        }
    }
    let p = arr.iter().find(|p| p["positionSide"].as_str() == Some("BOTH"))?;
    let amt = dec_field(p, "positionAmt");
    if amt.is_zero() || (amt > Decimal::ZERO) != (side == OrderSide::Buy) {
        return None;
    }
    Some(position_of(p, pair, side, amt.abs()))
}

/// Binance USDT-M 合约交易所实现。
pub struct BnFuturesExchange {
    client: FuturesClient,
}

impl BnFuturesExchange {
    pub fn new(client: FuturesClient) -> Self {
        Self { client }
    }

    /// 底层客户端 (引擎装配需要设置杠杆/保证金/双向模式等合约专有配置)。
    pub fn client(&self) -> &FuturesClient {
        &self.client
    }

    /// 启动预配置: 杠杆 / 保证金类型 / 双向持仓模式 (幂等设置)。
    ///
    /// 交易所"无需变更"类返回(`No need to change margin type` / `-4059 No need to change position side`)
    /// 按成功处理; 其他错误如实抛出(不吞)。
    pub async fn prepare(
        &self,
        symbol: &str,
        leverage: u32,
        hedge: bool,
        isolated: bool,
    ) -> CoreResult<()> {
        self.client.set_leverage(symbol, leverage.max(1)).await?;
        if let Err(e) = self.client.set_margin_type(symbol, isolated).await {
            if !is_benign_change_error(&e.to_string()) {
                return Err(e);
            }
        }
        if let Err(e) = self.client.set_position_side_dual(hedge).await {
            if !is_benign_change_error(&e.to_string()) {
                return Err(e);
            }
        }
        Ok(())
    }
}

/// 交易所"已经是目标状态, 无需变更"类错误 (幂等设置时的正常返回)。
pub fn is_benign_change_error(msg: &str) -> bool {
    msg.contains("No need to change")
}

/// `positionRisk` JSON → 定向持仓列表 (非零方向仓; hedge 最多两条, one-way 最多一条)。
pub fn directional_positions_from_risk(raw: &Value, pair: &str) -> Vec<Position> {
    let Some(arr) = raw.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // hedge: LONG / SHORT 两侧独立
    for (tag, side) in [("LONG", OrderSide::Buy), ("SHORT", OrderSide::Sell)] {
        if let Some(p) = arr.iter().find(|p| p["positionSide"].as_str() == Some(tag)) {
            let amt = dec_field(p, "positionAmt");
            if !amt.is_zero() {
                out.push(position_of(p, pair, side, amt.abs()));
            }
        }
    }
    if out.is_empty() {
        // one-way: BOTH 一条, 方向由符号决定
        if let Some(p) = arr.iter().find(|p| p["positionSide"].as_str() == Some("BOTH")) {
            let amt = dec_field(p, "positionAmt");
            if !amt.is_zero() {
                let side = if amt > Decimal::ZERO { OrderSide::Buy } else { OrderSide::Sell };
                out.push(position_of(p, pair, side, amt.abs()));
            }
        }
    }
    out
}

#[async_trait]
impl Exchange for BnFuturesExchange {
    fn name(&self) -> &'static str {
        "binance_futures"
    }

    async fn get_markets(&self) -> CoreResult<Vec<Market>> {
        self.client.get_exchange_info().await
    }

    async fn get_klines(&self, pair: &str, interval: &str, limit: u32) -> CoreResult<Vec<Kline>> {
        self.client.get_klines(pair, interval, limit).await
    }

    /// 区间取数 (028 T010): 数据服务的 `KlineSource` 需要 `[from, to)` 语义。
    async fn get_klines_range(
        &self,
        pair: &str,
        interval: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        self.client.get_klines_range(pair, interval, from_ms, to_ms).await
    }

    async fn get_orderbook(&self, pair: &str, depth: u32) -> CoreResult<OrderBook> {
        self.client.get_depth(pair, depth).await
    }

    async fn place_order(&self, req: OrderRequest) -> CoreResult<OrderAck> {
        let quantity = format_dec(req.size);
        let price = req.price.map(format_dec);
        let position_side = fapi_position_side(req.position_side.as_deref());
        // hedge 模式下 fapi 拒绝同时带 reduceOnly (specs/testnet.md 实测) → 带 positionSide 时不传
        let reduce_only = req.reduce_only && position_side.is_none();
        let resp = self
            .client
            .place_order(
                &req.pair,
                side_to_str(req.side),
                type_to_str(req.order_type),
                &quantity,
                price.as_deref(),
                position_side.as_deref(),
                reduce_only,
                Some(&req.client_order_id),
            )
            .await?;

        Ok(OrderAck {
            exchange_order_id: resp["orderId"]
                .as_i64()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            client_order_id: resp["clientOrderId"].as_str().unwrap_or("").to_string(),
            pair: req.pair.clone(),
            side: req.side,
            price: Decimal::from_str(resp["price"].as_str().unwrap_or("0")).unwrap_or_default(),
            size: req.size,
            filled_size: Decimal::from_str(resp["executedQty"].as_str().unwrap_or("0"))
                .unwrap_or_default(),
            status: status_from_bn(resp["status"].as_str().unwrap_or("NEW")),
        })
    }

    async fn cancel_order(&self, pair: &str, order_id: &str) -> CoreResult<()> {
        self.client.cancel_order(pair, order_id).await?;
        Ok(())
    }

    async fn get_open_orders(&self, pair: &str) -> CoreResult<Vec<OrderInfo>> {
        self.client.get_open_orders(pair).await
    }

    async fn get_balance(&self, asset: &str) -> CoreResult<Balance> {
        self.client.balance_of(asset).await
    }

    /// 合约持仓: `positionRisk` 聚合为**净仓** (hedge 合并多空); 无仓 → None。
    async fn get_position(&self, pair: &str) -> CoreResult<Option<Position>> {
        let raw = self.client.get_positions(pair).await?;
        Ok(position_from_risk(&raw, pair))
    }

    /// 定向持仓: hedge 返回 LONG/SHORT 两条, one-way 返回 BOTH 一条 (方向由符号定)。
    async fn get_positions_directional(&self, pair: &str) -> CoreResult<Vec<Position>> {
        let raw = self.client.get_positions(pair).await?;
        Ok(directional_positions_from_risk(&raw, pair))
    }

    /// 资金费流水 (014): 走 fapi `/fapi/v1/income`, 以**交易所账单**为准(不自算费率×名义)。
    async fn funding_income(&self, start_ms: i64, limit: u32) -> CoreResult<Vec<FundingIncome>> {
        self.client.income("FUNDING_FEE", start_ms, limit).await
    }

    async fn subscribe_orderbook(
        &self,
        pair: &str,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>> {
        self.client.subscribe_depth(pair).await
    }

    async fn subscribe_user_events(
        &self,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = UserEvent> + Send>>> {
        self.client.subscribe_user_data().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn one_way_risk() -> Value {
        serde_json::json!([{
            "symbol": "ETHUSDT", "positionSide": "BOTH", "positionAmt": "0.040",
            "entryPrice": "2493.7", "markPrice": "2494.0", "liquidationPrice": "0",
            "unRealizedProfit": "0.01572713", "leverage": "1"
        }])
    }

    fn hedge_risk() -> Value {
        serde_json::json!([
            {"symbol": "ETHUSDT", "positionSide": "LONG", "positionAmt": "0.05",
             "entryPrice": "2500", "markPrice": "2510", "liquidationPrice": "2300",
             "unRealizedProfit": "0.5", "leverage": "3"},
            {"symbol": "ETHUSDT", "positionSide": "SHORT", "positionAmt": "-0.02",
             "entryPrice": "2520", "markPrice": "2510", "liquidationPrice": "2600",
             "unRealizedProfit": "0.2", "leverage": "3"},
            {"symbol": "ETHUSDT", "positionSide": "BOTH", "positionAmt": "0"}
        ])
    }

    #[test]
    fn test_fapi_position_side_mapping() {
        assert_eq!(fapi_position_side(Some("long")), Some("LONG".into()));
        assert_eq!(fapi_position_side(Some("SHORT")), Some("SHORT".into()));
        assert_eq!(fapi_position_side(None), None, "one-way 不传 positionSide");
        assert_eq!(fapi_position_side(Some("both")), None, "未知方向按 one-way 处理");
    }

    #[test]
    fn test_position_from_risk_one_way_and_empty() {
        let p = position_from_risk(&one_way_risk(), "ETHUSDT").expect("应有净仓");
        assert_eq!(p.side, OrderSide::Buy);
        assert_eq!(p.size, dec!(0.040));
        assert_eq!(p.entry_price, dec!(2493.7));
        assert_eq!(p.mark_price, dec!(2494.0));
        assert_eq!(p.liquidation_price, None, "liquidationPrice=0 视为无");
        assert_eq!(p.unrealized_pnl, dec!(0.01572713));
        assert_eq!(p.leverage, Some(Decimal::ONE));

        // 空仓 → None
        let empty = serde_json::json!([{"positionSide": "BOTH", "positionAmt": "0"}]);
        assert!(position_from_risk(&empty, "ETHUSDT").is_none());
        assert!(position_from_risk(&serde_json::json!([]), "ETHUSDT").is_none());
    }

    #[test]
    fn test_position_from_risk_hedge_net_and_directional() {
        let raw = hedge_risk();
        // 净仓 = 0.05 - 0.02 = 0.03 (多头占优)
        let net = position_from_risk(&raw, "ETHUSDT").expect("净仓");
        assert_eq!(net.side, OrderSide::Buy);
        assert_eq!(net.size, dec!(0.03), "hedge 合并取净");
        assert_eq!(net.entry_price, dec!(2500), "取占优方向的价格");

        // 定向: LONG / SHORT 各自精确可见 (Lua `ctx.pos_size` 语义)
        let long = directional_position_from_risk(&raw, "ETHUSDT", OrderSide::Buy).unwrap();
        assert_eq!(long.size, dec!(0.05));
        assert_eq!(long.entry_price, dec!(2500));
        let short = directional_position_from_risk(&raw, "ETHUSDT", OrderSide::Sell).unwrap();
        assert_eq!(short.size, dec!(0.02), "SHORT 的负 amt 取绝对值");
        assert_eq!(short.entry_price, dec!(2520));
    }

    #[test]
    fn test_directional_position_one_way_matches_sign() {
        let raw = one_way_risk();
        assert!(directional_position_from_risk(&raw, "ETHUSDT", OrderSide::Buy).is_some());
        assert!(
            directional_position_from_risk(&raw, "ETHUSDT", OrderSide::Sell).is_none(),
            "多头持仓不应报出空头方向仓"
        );

        // 空头净仓: 只有 Sell 方向可见
        let short = serde_json::json!([{"positionSide": "BOTH", "positionAmt": "-0.5",
            "entryPrice": "2500", "markPrice": "2490"}]);
        assert!(directional_position_from_risk(&short, "ETHUSDT", OrderSide::Buy).is_none());
        let p = directional_position_from_risk(&short, "ETHUSDT", OrderSide::Sell).unwrap();
        assert_eq!(p.size, dec!(0.5));
        assert_eq!(p.side, OrderSide::Sell);
    }
}
