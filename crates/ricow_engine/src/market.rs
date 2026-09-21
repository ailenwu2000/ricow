//! 行情循环: 订阅盘口并喂给策略上下文。

use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use ricow_core::{CoreError, CoreResult, Exchange, OrderBook, OrderBookUpdate};

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

/// 多标的盘口流: `(pair, 本轮盘口)`。
pub type PairOrderBookStream = Pin<Box<dyn Stream<Item = (String, OrderBookUpdate)> + Send>>;

/// 多标的盘口订阅 (028 FR-023): 每个 pair 一条既有订阅(**各自带断线指数退避守护**),
/// 合并成一条 `(pair, update)` 流供主循环消费。
///
/// 为什么在引擎层合并而不是改交易所层: 断线重连语义已按"每订阅"实现并验证过, 在 ws 层再加一层
/// 多 symbol 复用只会引入新状态; 合并流让主循环按 `(pair, update)` 消费, 零额外语义。
/// 单标的调用 = `pairs.len() == 1`, 与旧行为逐位一致。
pub async fn subscribe_orderbooks(
    exchange: &Arc<dyn Exchange>,
    pairs: &[String],
) -> CoreResult<PairOrderBookStream> {
    if pairs.is_empty() {
        return Err(CoreError::InvalidArgument("盘口订阅需要至少一个 pair".into()));
    }
    let mut streams: Vec<PairOrderBookStream> = Vec::with_capacity(pairs.len());
    for p in pairs {
        let s = exchange.subscribe_orderbook(p).await?;
        let tag = p.clone();
        streams.push(Box::pin(s.map(move |u| (tag.clone(), u))));
    }
    Ok(Box::pin(futures::stream::select_all(streams)))
}
