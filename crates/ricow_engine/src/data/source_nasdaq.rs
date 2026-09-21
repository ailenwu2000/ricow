//! Nasdaq 官方日线数据源适配 (028 T011): 免 key, **只有日线**。
//!
//! 复权口径(如实说明): Nasdaq 返回的价格**已按拆股复权、未按分红复权**(源内固定口径),
//! 响应里没有独立的复权列 → 本源 `supports_adj_close() = false`, 复权值写 NULL,
//! 策略请求 `PriceMode::AdjClose` 会拿到明确报错(不静默混口径, 见 FR-015)。
//!
//! 资产类别(assetclass)解析: 先查既有 bStock 映射(`us_tickers::BSTOCK_MAP` 带权威 class),
//! 未收录的 ticker(如 QQQ/SPY)按 `stocks` → `etf` 次序探测(首次非空即用)。之所以要探测:
//! Nasdaq 对类别不符一律回 `data=null`, 无法从响应区分"类别错"与"该区间无数据"。
//!
//! 请求口径(2026-09-20 直连实测): 日期参数是 `YYYY-MM-DD`; `todate` 传较早日期会回 0 行
//! (QQQ todate=2026-08-14 → totalRecords 0, todate=2026-09-19 → 34 行) —— 因此本适配器
//! 统一请求 `[from_date, 今天]`, 再由 `normalize_range` 本地裁到 `[from_ms, to_ms)`。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ricow_core::{CoreError, CoreResult, Interval, Kline, KlineSource, SourceRegistry};

use super::normalize_range;
use crate::nasdaq::NasdaqClient;
use crate::us_tickers::{AssetClass, BSTOCK_MAP};

/// 注册名。
pub const NAME: &str = "nasdaq";

/// Nasdaq 日线来源。
pub struct NasdaqSource {
    client: NasdaqClient,
}

impl Default for NasdaqSource {
    fn default() -> Self {
        Self::new()
    }
}

impl NasdaqSource {
    pub fn new() -> Self {
        Self { client: NasdaqClient::new() }
    }
}

/// 把 Nasdaq 源注册进来源表(装配层调用)。
pub fn register(registry: &mut SourceRegistry) {
    registry.register(std::sync::Arc::new(NasdaqSource::new()));
}

/// 毫秒 → Nasdaq 日期串 `YYYY-MM-DD`。
///
/// ⚠️ 实测口径 (2026-09-20 直连 `api.nasdaq.com` 验证): 该接口的日期参数是
/// `YYYY-MM-DD`, 传 `MM/DD/YYYY` 会回 `rCode=400 Bad or No parameter fromdate`。
fn nasdaq_date(ms: i64) -> CoreResult<String> {
    let dt = DateTime::<Utc>::from_timestamp_millis(ms)
        .ok_or_else(|| CoreError::InvalidArgument(format!("时间戳非法: {ms}")))?;
    Ok(dt.format("%Y-%m-%d").to_string())
}

/// 已知资产类别(bStock 映射里带权威 class)。
fn known_assetclass(ticker: &str) -> Option<AssetClass> {
    BSTOCK_MAP.iter().find(|m| m.us.eq_ignore_ascii_case(ticker)).map(|m| m.assetclass)
}

#[async_trait]
impl KlineSource for NasdaqSource {
    fn name(&self) -> &'static str {
        NAME
    }

    /// 源原生只有日线; 其他周期要由装配层重采样(plan d6), 本源不假装支持。
    fn supported_intervals(&self) -> &'static [Interval] {
        &[Interval::D1]
    }

    async fn fetch_klines(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        if interval != Interval::D1 {
            return Err(CoreError::InvalidArgument(format!(
                "nasdaq 源只有日线 (1d), 收到 {}",
                interval.label()
            )));
        }
        if from_ms >= to_ms {
            return Ok(Vec::new());
        }
        let from_date = nasdaq_date(from_ms)?;
        // 上界不用 `to − 1ms`: 实测该接口对"较早的 todate"会回 0 行
        // (2026-09-20 实测 QQQ: todate=2026-08-14 → totalRecords 0; todate=2026-09-19 → 34 行),
        // 因此统一请求到**今天**, 再用 normalize_range 本地裁到 [from, to) —— 结果一致且稳定。
        let to_date = nasdaq_date(Utc::now().timestamp_millis())?;

        let candidates: Vec<AssetClass> = match known_assetclass(symbol) {
            Some(class) => vec![class],
            None => vec![AssetClass::Stock, AssetClass::Etf],
        };
        let mut last_err: Option<CoreError> = None;
        for class in candidates {
            match self.client.get_daily_klines_between(symbol, class, &from_date, &to_date).await {
                Ok(bars) if !bars.is_empty() => {
                    return Ok(normalize_range(bars, from_ms, to_ms));
                }
                Ok(_) => continue, // 空 = 类别不符或区间无交易日, 试下一个候选
                Err(e) => last_err = Some(e),
            }
        }
        match last_err {
            Some(e) => Err(e),
            None => Ok(Vec::new()),
        }
    }

    /// 见模块头: 源内已按拆股复权, 但没有独立复权列可填 → 不声明支持。
    fn supports_adj_close(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nasdaq_date_format_is_iso() {
        // 2026-09-01T00:00:00Z = 1788220800000 ms; 接口只认 YYYY-MM-DD。
        assert_eq!(nasdaq_date(1_788_220_800_000).unwrap(), "2026-09-01");
        assert_eq!(nasdaq_date(1_788_220_800_000 + 86_400_000).unwrap(), "2026-09-02");
        assert!(nasdaq_date(i64::MAX).is_err(), "非法时间戳报错");
    }

    #[test]
    fn test_known_assetclass_uses_bstock_map() {
        assert_eq!(known_assetclass("AAPL"), Some(AssetClass::Stock));
        assert_eq!(known_assetclass("EWY"), Some(AssetClass::Etf));
        assert_eq!(known_assetclass("aapl"), Some(AssetClass::Stock), "大小写不敏感");
        assert_eq!(known_assetclass("ZZZZ"), None, "未收录 → 交给 stocks/etf 探测");
    }

    #[tokio::test]
    async fn test_non_daily_interval_is_rejected_without_network() {
        let src = NasdaqSource::new();
        let err = src.fetch_klines("QQQ", Interval::H1, 0, 3_600_000).await.unwrap_err();
        assert!(err.to_string().contains("只有日线"), "{err}");
    }

    #[tokio::test]
    async fn test_reversed_window_returns_empty_without_network() {
        let src = NasdaqSource::new();
        assert!(src.fetch_klines("QQQ", Interval::D1, 3_600_000, 0).await.unwrap().is_empty());
    }
}
