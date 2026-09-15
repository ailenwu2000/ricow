//! 行情循环: 订阅盘口并喂给策略上下文。

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use ricow_core::{CoreResult, Exchange, OrderBook, OrderBookUpdate};

/// 订阅盘口 Stream (薄封装)。
pub async fn subscribe_orderbook(
    exchange: &Arc<dyn Exchange>,
    pair: &str,
) -> CoreResult<Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>> {
    exchange.subscribe_orderbook(pair).await
}

/// OrderBookUpdate → OrderBook 转换。
pub fn to_orderbook(u: OrderBookUpdate) -> OrderBook {
    OrderBook { bids: u.bids, asks: u.asks, timestamp: u.timestamp }
}

/// 从交易所拉取一次盘口快照 (轮询行情, 无需 WS)。
pub async fn fetch_orderbook(exchange: &Arc<dyn Exchange>, pair: &str) -> CoreResult<OrderBook> {
    exchange.get_orderbook(pair, 20).await
}
