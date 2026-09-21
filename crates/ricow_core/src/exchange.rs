//! 交易所抽象层 — 所有交易所实现须满足此 trait。

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use crate::error::{CoreError, CoreResult};
use crate::interval::Interval;
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

    /// 取 `[from_ms, to_ms)` 区间的 K 线 (升序, 已按 `open_time` 去重)。
    ///
    /// 数据服务([`crate::KlineSource`])需要"区间"语义; 交易所原生支持 `startTime/endTime`
    /// 的端(币安)覆写本方法, 只提供"最近 N 根"的端保持默认实现(报错), 由各自的数据源适配层
    /// 决定退化做法(028 T010)。
    async fn get_klines_range(
        &self,
        _pair: &str,
        _interval: &str,
        _from_ms: i64,
        _to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        Err(CoreError::InvalidArgument("该交易端不支持按区间取 K 线 (只提供最近 N 根)".into()))
    }
}

// ---- 数据源抽象 (028 T006) ----

/// K 线数据源: 与 [`Exchange`](交易路径) 并列的**取数**抽象。
///
/// 新增一个来源 = 实现本 trait + 在 [`SourceRegistry`] 注册一个名字 ——
/// 不改策略、不改引擎主流程(验收 SC-002)。
#[async_trait]
pub trait KlineSource: Send + Sync {
    /// 注册名(小写, 如 `yahoo` / `binance_spot`)。
    fn name(&self) -> &'static str;

    /// 支持的**原生**周期; 不在此列而策略请求的周期, 由装配层重采样(plan d6)。
    fn supported_intervals(&self) -> &'static [Interval];

    /// 取 `[from_ms, to_ms)` 的 K 线, 按 `open_time` 升序; 空 = 该区间无数据。
    async fn fetch_klines(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>>;

    /// 是否提供复权收盘 (决定 `PriceMode::AdjClose` 是否可用)。默认 false。
    fn supports_adj_close(&self) -> bool {
        false
    }

    /// 带复权列的取数入口(028 T012)。
    ///
    /// 默认实现 = 复用 [`fetch_klines`](KlineSource::fetch_klines), `adj_close` 全 `None`;
    /// 能提供复权收盘的源(Yahoo)覆写本方法, 一次请求里把 OHLCV 与 `adjclose` 一起交回。
    async fn fetch_bars(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<crate::types::Bar>> {
        Ok(self
            .fetch_klines(symbol, interval, from_ms, to_ms)
            .await?
            .into_iter()
            .map(crate::types::Bar::new)
            .collect())
    }
}

/// 数据源注册表: 装配层注册, 策略按名字取。
#[derive(Default)]
pub struct SourceRegistry {
    sources: Vec<Arc<dyn KlineSource>>,
}

impl SourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册(同名**替换** —— 允许装配层重注册与测试反复构造)。
    pub fn register(&mut self, source: Arc<dyn KlineSource>) {
        let name = source.name();
        if let Some(slot) = self.sources.iter_mut().find(|s| s.name() == name) {
            *slot = source;
            return;
        }
        self.sources.push(source);
    }

    /// 已注册来源名(按注册顺序)。
    pub fn names(&self) -> Vec<&'static str> {
        self.sources.iter().map(|s| s.name()).collect()
    }

    /// 按名取源; 未注册返回 `None`。
    pub fn get(&self, name: &str) -> Option<Arc<dyn KlineSource>> {
        self.sources.iter().find(|s| s.name() == name).cloned()
    }

    /// 按名取源; 未知来源名**报错并列出可用来源**。
    pub fn require(&self, name: &str) -> CoreResult<Arc<dyn KlineSource>> {
        self.get(name).ok_or_else(|| {
            CoreError::InvalidArgument(format!(
                "未知数据源 '{name}'; 可用来源: {}",
                self.names().join(", ")
            ))
        })
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯逻辑替身(已知向量式): 只用于注册表行为断言, 不替代任何真实数据源路径。
    struct FakeSource {
        name: &'static str,
        intervals: &'static [Interval],
        adj: bool,
    }

    #[async_trait]
    impl KlineSource for FakeSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn supported_intervals(&self) -> &'static [Interval] {
            self.intervals
        }

        async fn fetch_klines(
            &self,
            _symbol: &str,
            _interval: Interval,
            _from_ms: i64,
            _to_ms: i64,
        ) -> CoreResult<Vec<Kline>> {
            Ok(Vec::new())
        }

        fn supports_adj_close(&self) -> bool {
            self.adj
        }
    }

    fn fake(name: &'static str, adj: bool) -> Arc<dyn KlineSource> {
        Arc::new(FakeSource { name, intervals: &[Interval::D1], adj })
    }

    #[test]
    fn test_registry_register_and_get_by_name() {
        let mut reg = SourceRegistry::new();
        reg.register(fake("yahoo", true));
        reg.register(fake("nasdaq", false));
        assert_eq!(reg.names(), vec!["yahoo", "nasdaq"]);
        assert_eq!(reg.len(), 2);
        assert!(reg.get("yahoo").is_some());
        assert!(reg.get("binance_spot").is_none());
    }

    #[test]
    fn test_registry_unknown_source_error_lists_available() {
        let mut reg = SourceRegistry::new();
        reg.register(fake("yahoo", true));
        reg.register(fake("nasdaq", false));
        // 不用 unwrap_err(): 其 Ok 分支要求 `dyn KlineSource: Debug`。
        let err = match reg.require("stooq") {
            Ok(_) => panic!("未知来源名必须报错"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("未知数据源 'stooq'"), "{err}");
        assert!(err.contains("yahoo") && err.contains("nasdaq"), "{err}");
    }

    #[test]
    fn test_registry_reregister_replaces_same_name() {
        let mut reg = SourceRegistry::new();
        reg.register(fake("yahoo", false));
        reg.register(fake("yahoo", true));
        assert_eq!(reg.len(), 1, "同名注册应替换而非追加");
        assert!(reg.get("yahoo").unwrap().supports_adj_close());
    }

    #[test]
    fn test_supports_adj_close_default_is_false() {
        struct Bare;
        #[async_trait]
        impl KlineSource for Bare {
            fn name(&self) -> &'static str {
                "bare"
            }
            fn supported_intervals(&self) -> &'static [Interval] {
                &[Interval::H1]
            }
            async fn fetch_klines(
                &self,
                _symbol: &str,
                _interval: Interval,
                _from_ms: i64,
                _to_ms: i64,
            ) -> CoreResult<Vec<Kline>> {
                Ok(Vec::new())
            }
        }
        let bare = Bare;
        assert!(!bare.supports_adj_close(), "默认不提供复权收盘");
        assert_eq!(bare.supported_intervals(), &[Interval::H1]);
    }
}
