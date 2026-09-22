//! 交易所抽象层 — 所有交易所实现须满足此 trait。

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::CoreResult;
use crate::types::{
    Balance, FundingIncome, Kline, Market, OrderAck, OrderBook, OrderBookUpdate, OrderInfo,
    OrderRequest, Position, UserEvent,
};

/// 交易所抽象层。
///
/// 两个关键设计:
/// - `subscribe_orderbook` / `subscribe_user_events` 返回 Stream, 策略引擎直接消费, 无需轮询。
/// - 上层不感知签名细节, 各 Exchange 实现内部处理。
#[async_trait]
pub trait Exchange: Send + Sync {
    /// 交易所标识名
    fn name(&self) -> &'static str;

    /// 获取支持的交易对列表
    async fn get_markets(&self) -> CoreResult<Vec<Market>>;

    /// 获取历史 K 线
    async fn get_klines(&self, pair: &str, interval: &str, limit: u32) -> CoreResult<Vec<Kline>>;

    /// 截止到 `end_ms` 的 K 线 (回测按自然年月分段用)。缺省实现忽略 `end_ms`(降级为"到最新"),
    /// 支持的数据源 (现货/合约数据源) 覆盖之。
    async fn get_klines_until(
        &self,
        pair: &str,
        interval: &str,
        limit: u32,
        end_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        let _ = end_ms;
        self.get_klines(pair, interval, limit).await
    }

    /// 获取当前盘口快照
    async fn get_orderbook(&self, pair: &str, depth: u32) -> CoreResult<OrderBook>;

    /// 下单 (限价/市价)
    async fn place_order(&self, req: OrderRequest) -> CoreResult<OrderAck>;

    /// 撤单
    async fn cancel_order(&self, pair: &str, order_id: &str) -> CoreResult<()>;

    /// 查询未成交挂单 (停机清理: 撤单兜底与残留提示的数据源)
    async fn get_open_orders(&self, pair: &str) -> CoreResult<Vec<OrderInfo>>;

    /// 查询余额
    async fn get_balance(&self, asset: &str) -> CoreResult<Balance>;

    /// 查询持仓 (合约)
    async fn get_position(&self, pair: &str) -> CoreResult<Option<Position>>;

    /// 查询**定向**持仓: 合约 hedge 模式下多空两侧各一条; 现货/one-way 最多一条。
    ///
    /// 默认实现 = `get_position` 包装为单条 (不支持方向仓的市场无需改动)。
    /// 用途: 引擎装配持仓缓存与停机平仓 (012); `ctx.pos_size(pair, "long"/"short")` 的数据源。
    async fn get_positions_directional(&self, pair: &str) -> CoreResult<Vec<Position>> {
        Ok(self.get_position(pair).await?.into_iter().collect())
    }

    /// 订阅实时盘口 (返回 Stream, 策略引擎直接消费)
    async fn subscribe_orderbook(
        &self,
        pair: &str,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>>;

    /// 资金费流水 (014, 合约账户级事实): `start_ms` = 增量拉取水位, 以交易所账单为准(不自算费率×名义)。
    /// 默认实现 = 空(现货无资金费概念), 合约实现覆盖之。
    async fn funding_income(&self, _start_ms: i64, _limit: u32) -> CoreResult<Vec<FundingIncome>> {
        Ok(Vec::new())
    }

    /// 订阅用户事件 (成交/订单更新)
    async fn subscribe_user_events(
        &self,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = UserEvent> + Send>>>;
}
