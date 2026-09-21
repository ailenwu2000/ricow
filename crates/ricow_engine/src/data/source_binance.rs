//! 币安数据源适配 (028 T010): 把"交易端" [`Exchange`] 当作一个 K 线来源接入数据服务。
//!
//! 为什么是适配而不是新写一个 REST 客户端: 交易端已经封装了域名切换(demo/镜像)、签名、
//! 分页与解析; 数据服务只要"按区间取 K 线"这一个动词, 用 [`Exchange::get_klines_range`]
//! 覆写(币安原生支持 `startTime/endTime`)即可, 不必再维护第二套请求代码。
//!
//! 注册名与交易端同名: `binance_spot` / `binance_futures` —— 策略里
//! `data:series{source="binance_spot", symbol="ETHUSDT", interval="1h"}` 即可取数。

use std::sync::Arc;

use async_trait::async_trait;
use ricow_core::{CoreResult, Exchange, Interval, Kline, KlineSource, SourceRegistry};

use super::normalize_range;

/// 现货注册名。
pub const SPOT: &str = "binance_spot";
/// 合约注册名。
pub const FUTURES: &str = "binance_futures";

/// 币安 K 线来源(现货或合约交易端的薄适配)。
pub struct BinanceSource {
    name: &'static str,
    exchange: Arc<dyn Exchange>,
}

impl BinanceSource {
    /// 现货来源。
    pub fn spot(exchange: Arc<dyn Exchange>) -> Self {
        Self { name: SPOT, exchange }
    }

    /// 合约来源。
    pub fn futures(exchange: Arc<dyn Exchange>) -> Self {
        Self { name: FUTURES, exchange }
    }
}

#[async_trait]
impl KlineSource for BinanceSource {
    fn name(&self) -> &'static str {
        self.name
    }

    /// 币安 REST 支持的 14 个周期全给(周期表见 `ricow_core::Interval`)。
    fn supported_intervals(&self) -> &'static [Interval] {
        &Interval::ALL
    }

    async fn fetch_klines(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        if from_ms >= to_ms {
            return Ok(Vec::new());
        }
        let bars = self.exchange.get_klines_range(symbol, interval.label(), from_ms, to_ms).await?;
        Ok(normalize_range(bars, from_ms, to_ms))
    }

    /// 币安不提供复权收盘(无股息/拆股概念)。
    fn supports_adj_close(&self) -> bool {
        false
    }
}

/// 把交易端注册成数据源(装配层调用; 合约端可能不存在, 用 `Option`)。
pub fn register(
    registry: &mut SourceRegistry,
    spot: Arc<dyn Exchange>,
    futures: Option<Arc<dyn Exchange>>,
) {
    registry.register(Arc::new(BinanceSource::spot(spot)));
    if let Some(f) = futures {
        registry.register(Arc::new(BinanceSource::futures(f)));
    }
}
