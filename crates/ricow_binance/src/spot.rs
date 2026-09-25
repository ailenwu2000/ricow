//! Binance 现货交易所实现 — 实现 `Exchange` trait。

use std::pin::Pin;
use std::str::FromStr;

use async_trait::async_trait;
use futures::Stream;
use ricow_core::{
    Balance, CoreResult, Exchange, Kline, Market, OrderAck, OrderBook, OrderBookUpdate, OrderInfo,
    OrderRequest, OrderSide, OrderType, Position, UserEvent,
};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::client::BinanceClient;

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

use crate::client::{format_dec, status_from_bn};

/// 解析 `openOrders` 响应 (现货 `/api/v3/openOrders` 与合约 `/fapi/v1/openOrders` 同构)。
///
/// 纯函数(便于单测): 非数组或字段缺失按空值降级, 不 panic。
pub(crate) fn parse_open_orders(resp: &Value, pair: &str) -> Vec<OrderInfo> {
    resp.as_array()
        .map(|arr| {
            arr.iter()
                .map(|o| OrderInfo {
                    exchange_order_id: o["orderId"]
                        .as_i64()
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    client_order_id: o["clientOrderId"].as_str().unwrap_or("").to_string(),
                    pair: o["symbol"].as_str().unwrap_or(pair).to_string(),
                    side: if o["side"].as_str().unwrap_or("BUY").eq_ignore_ascii_case("SELL") {
                        OrderSide::Sell
                    } else {
                        OrderSide::Buy
                    },
                    price: Decimal::from_str(o["price"].as_str().unwrap_or("0"))
                        .unwrap_or_default(),
                    size: Decimal::from_str(o["origQty"].as_str().unwrap_or("0"))
                        .unwrap_or_default(),
                    filled_size: Decimal::from_str(o["executedQty"].as_str().unwrap_or("0"))
                        .unwrap_or_default(),
                    status: status_from_bn(o["status"].as_str().unwrap_or("NEW")),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Binance 现货交易所实现。
pub struct BnSpotExchange {
    client: BinanceClient,
}

impl BnSpotExchange {
    pub fn new(client: BinanceClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Exchange for BnSpotExchange {
    fn name(&self) -> &'static str {
        "binance_spot"
    }

    async fn get_markets(&self) -> CoreResult<Vec<Market>> {
        self.client.get_exchange_info().await
    }

    async fn get_klines(&self, pair: &str, interval: &str, limit: u32) -> CoreResult<Vec<Kline>> {
        self.client.get_klines(pair, interval, limit).await
    }
    async fn get_klines_until(
        &self,
        pair: &str,
        interval: &str,
        limit: u32,
        end_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        self.client.get_klines_ending_at(pair, interval, limit, end_ms).await
    }

    async fn get_orderbook(&self, pair: &str, depth: u32) -> CoreResult<OrderBook> {
        self.client.get_depth(pair, depth).await
    }

    async fn place_order(&self, req: OrderRequest) -> CoreResult<OrderAck> {
        let side = side_to_str(req.side);
        let order_type = type_to_str(req.order_type);
        let quantity = format_dec(req.size);
        let price = req.price.map(format_dec);

        let resp = self
            .client
            .place_order(
                &req.pair,
                side,
                order_type,
                &quantity,
                price.as_deref(),
                &req.client_order_id,
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
        let resp = self.client.get_open_orders(pair).await?;
        Ok(parse_open_orders(&resp, pair))
    }

    async fn get_balance(&self, asset: &str) -> CoreResult<Balance> {
        let resp = self.client.get_account().await?;
        let balances = resp["balances"]
            .as_array()
            .ok_or_else(|| ricow_core::CoreError::Parse("missing balances".into()))?;
        let target = asset.to_uppercase();
        let found = balances.iter().find(|b| {
            b["asset"].as_str().map(|a| a.eq_ignore_ascii_case(&target)).unwrap_or(false)
        });

        match found {
            Some(b) => Ok(Balance {
                asset: target,
                free: Decimal::from_str(b["free"].as_str().unwrap_or("0")).unwrap_or_default(),
                locked: Decimal::from_str(b["locked"].as_str().unwrap_or("0")).unwrap_or_default(),
            }),
            None => Ok(Balance { asset: target, free: Decimal::ZERO, locked: Decimal::ZERO }),
        }
    }

    async fn get_position(&self, _pair: &str) -> CoreResult<Option<Position>> {
        // 现货无持仓概念, 始终返回 None。
        Ok(None)
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
    use ricow_core::OrderStatus;

    #[test]
    fn test_status_from_bn() {
        assert_eq!(status_from_bn("NEW"), OrderStatus::Open);
        assert_eq!(status_from_bn("PARTIALLY_FILLED"), OrderStatus::Open);
        assert_eq!(status_from_bn("FILLED"), OrderStatus::Filled);
        assert_eq!(status_from_bn("CANCELED"), OrderStatus::Cancelled);
        assert_eq!(status_from_bn("REJECTED"), OrderStatus::Rejected);
    }

    #[test]
    fn test_format_dec() {
        assert_eq!(format_dec(Decimal::from_str("3000.0").unwrap()), "3000");
        assert_eq!(format_dec(Decimal::from_str("0.010").unwrap()), "0.01");
    }

    #[test]
    fn test_parse_open_orders() {
        let resp = serde_json::json!([
            {
                "orderId": 12345,
                "clientOrderId": "ricow-abc",
                "symbol": "ETHUSDT",
                "side": "SELL",
                "price": "3000.50",
                "origQty": "0.02",
                "executedQty": "0",
                "status": "NEW"
            },
            {
                "orderId": 67890,
                "clientOrderId": "ricow-def",
                "symbol": "ETHUSDT",
                "side": "BUY",
                "price": "2500.00",
                "origQty": "0.10",
                "executedQty": "0.04",
                "status": "PARTIALLY_FILLED"
            }
        ]);
        let orders = parse_open_orders(&resp, "ETHUSDT");
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].exchange_order_id, "12345");
        assert_eq!(orders[0].side, OrderSide::Sell);
        assert_eq!(orders[0].price.to_string(), "3000.50");
        assert_eq!(orders[0].filled_size, Decimal::ZERO);
        assert_eq!(orders[0].status, OrderStatus::Open);
        assert_eq!(orders[1].side, OrderSide::Buy);
        assert_eq!(orders[1].filled_size.to_string(), "0.04");

        // 错误体 / 非数组 → 空列表, 不 panic
        assert!(parse_open_orders(&serde_json::json!({"code": -1121}), "ETHUSDT").is_empty());
    }
}
