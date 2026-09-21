//! Yahoo Finance v8 chart 数据源适配 (028 T012)。
//!
//! 接口: `GET https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?period1=<秒>&period2=<秒>&interval=<iv>`
//!
//! - 免 key; 需**浏览器 UA**(默认 UA 直接 429); 按 IP 限速 ≈2 req/s → 装配层用
//!   `DataHub::with_min_interval(500ms)` 给本源单独限速;
//! - 响应结构: `chart.result[0].timestamp[]` + `indicators.quote[0].{open,high,low,close,volume}`
//!   (逐位对齐, 可能含 `null` 行) + `indicators.adjclose[0].adjclose[]`(复权收盘) ——
//!   这是本项目里**唯一**能填 `Bar::adj_close` 的源(`supports_adj_close = true`);
//! - 原生 interval 支持集 = `1m/5m/15m/30m/1h/1d/1wk`(Yahoo 口径, 没有 3m/2h/4h/6h/8h/12h/3d);
//! - 服务端窗口: 1m 只保约 7 天, 5m 约 60 天, 1d/1wk 可回溯到上市(见 research.md)。
//!
//! ⚠️ 环境限制(2026-09-20 本机实测, 如实记录): 本机网络访问 Yahoo 的
//! `query1/query2` 两个域名的 `v8/finance/chart` 与 `v7/finance/download` **一律 HTTP 403**
//! (返回 Yahoo 中文拦截页), 换浏览器 UA / 加 `Accept`+`Referer` 均无效, 无浏览器守护可绕行。
//! 因此 `tests/yahoo_live_smoke.rs` 的**真网络**用例在本机无法通过(未实测, 不假装通过);
//! 解析逻辑改用固定的 v8 结构样例做单元测试覆盖。

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ricow_core::{Bar, CoreError, CoreResult, Interval, Kline, KlineSource, SourceRegistry};
use rust_decimal::Decimal;
use serde_json::Value;

use super::normalize_bars;

/// 注册名。
pub const NAME: &str = "yahoo";

/// v8 chart 端点(主域)。
const ENDPOINT: &str = "https://query1.finance.yahoo.com/v8/finance/chart/";

/// 浏览器 UA(默认 UA 会被 429 拦掉)。
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// 本源原生支持的周期(Yahoo 口径)。
const SUPPORTED: [Interval; 7] = [
    Interval::M1,
    Interval::M5,
    Interval::M15,
    Interval::M30,
    Interval::H1,
    Interval::D1,
    Interval::W1,
];

/// Yahoo Finance 来源。
pub struct YahooSource {
    http: reqwest::Client,
}

impl Default for YahooSource {
    fn default() -> Self {
        Self::new()
    }
}

impl YahooSource {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(BROWSER_UA)
            .build()
            .expect("reqwest client 构建");
        Self { http }
    }
}

/// 把 Yahoo 源注册进来源表(装配层调用)。
pub fn register(registry: &mut SourceRegistry) {
    registry.register(std::sync::Arc::new(YahooSource::new()));
}

/// `Interval` → Yahoo interval 标签; 不支持返回 `None`(由调用方报错, 不静默降级)。
fn yahoo_interval(interval: Interval) -> Option<&'static str> {
    match interval {
        Interval::M1 => Some("1m"),
        Interval::M5 => Some("5m"),
        Interval::M15 => Some("15m"),
        Interval::M30 => Some("30m"),
        Interval::H1 => Some("1h"),
        Interval::D1 => Some("1d"),
        Interval::W1 => Some("1wk"),
        _ => None,
    }
}

#[async_trait]
impl KlineSource for YahooSource {
    fn name(&self) -> &'static str {
        NAME
    }

    fn supported_intervals(&self) -> &'static [Interval] {
        &SUPPORTED
    }

    async fn fetch_klines(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        Ok(self
            .fetch_bars(symbol, interval, from_ms, to_ms)
            .await?
            .into_iter()
            .map(|b| b.kline)
            .collect())
    }

    /// 一次请求同时取回 OHLCV 与 `adjclose`(本源的核心价值: 复权长历史)。
    async fn fetch_bars(
        &self,
        symbol: &str,
        interval: Interval,
        from_ms: i64,
        to_ms: i64,
    ) -> CoreResult<Vec<Bar>> {
        if from_ms >= to_ms {
            return Ok(Vec::new());
        }
        let label = yahoo_interval(interval).ok_or_else(|| {
            CoreError::InvalidArgument(format!(
                "yahoo 源不支持周期 {} (支持: 1m/5m/15m/30m/1h/1d/1wk)",
                interval.label()
            ))
        })?;
        // 接口用**秒**; 半开区间 [from, to) → period1=from/1000, period2=to/1000。
        let url = format!(
            "{ENDPOINT}{symbol}?period1={}&period2={}&interval={label}",
            from_ms.div_euclid(1000),
            to_ms.div_euclid(1000)
        );
        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("yahoo HTTP: {e}")))?;
        let status = resp.status();
        let text =
            resp.text().await.map_err(|e| CoreError::Network(format!("yahoo 读取体: {e}")))?;
        if !status.is_success() {
            return Err(CoreError::Exchange(format!(
                "yahoo HTTP {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::Parse(format!("yahoo JSON: {e}")))?;
        let bars = parse_chart(&v).ok_or_else(|| {
            let detail = v
                .get("chart")
                .and_then(|c| c.get("error"))
                .map(|e| e.to_string())
                .unwrap_or_else(|| "无 chart.result".to_string());
            CoreError::Parse(format!("yahoo 响应不可解析 (symbol={symbol}): {detail}"))
        })?;
        Ok(normalize_bars(bars, from_ms, to_ms))
    }

    fn supports_adj_close(&self) -> bool {
        true
    }
}

/// 纯函数: v8 chart 响应 → `Bar` 列表(升序, 已去重)。
///
/// 逐位对齐假设: `timestamp[i]` 对应 `quote[0].open[i]...`. OHLC 任一为 `null` 的行(假期/停牌)
/// 整根跳过; `volume` 为 `null` 记 0; `adjclose` 缺失则 `adj_close = None`。
/// `close_time = open_time + 周期 − 1ms`(日线按 24h 计, 比真实收盘更晚 → 偏保守, 不会引入前视)。
pub(crate) fn parse_chart(v: &Value) -> Option<Vec<Bar>> {
    let result = v.get("chart")?.get("result")?.as_array()?.first()?;
    let ts = result.get("timestamp")?.as_array()?;
    let quote = result.get("indicators")?.get("quote")?.as_array()?.first()?;
    let (opens, highs, lows, closes, vols) = (
        quote.get("open")?.as_array()?,
        quote.get("high")?.as_array()?,
        quote.get("low")?.as_array()?,
        quote.get("close")?.as_array()?,
        quote.get("volume").and_then(|x| x.as_array()),
    );
    let adjs = result
        .get("indicators")
        .and_then(|i| i.get("adjclose"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.get("adjclose"))
        .and_then(|a| a.as_array());

    let mut out: Vec<Bar> = Vec::with_capacity(ts.len());
    for (i, t) in ts.iter().enumerate() {
        let Some(secs) = t.as_i64() else { continue };
        let (Some(open), Some(high), Some(low), Some(close)) =
            (dec(opens.get(i)), dec(highs.get(i)), dec(lows.get(i)), dec(closes.get(i)))
        else {
            continue; // 该行无行情(假期/停牌) → 跳过整根
        };
        let Some(open_time) = DateTime::<Utc>::from_timestamp_millis(secs.saturating_mul(1000))
        else {
            continue;
        };
        let volume = vols.and_then(|a| dec(a.get(i))).unwrap_or(Decimal::ZERO);
        // close_time: 日线及以上按 24h 计(偏保守), 分钟级按周期毫秒。
        let step_ms = if secs % 86_400 == 0 { 86_400_000 } else { 60_000 };
        let kline = Kline {
            open_time,
            open,
            high,
            low,
            close,
            volume,
            close_time: open_time + chrono::Duration::milliseconds(step_ms - 1),
        };
        let adj = adjs.and_then(|a| dec(a.get(i)));
        out.push(Bar::with_adj_close(kline, adj));
    }
    Some(out)
}

/// JSON 数值 → Decimal(`null` / 非数值 → `None`)。
fn dec(v: Option<&Value>) -> Option<Decimal> {
    let f = v?.as_f64()?;
    Decimal::from_f64_retain(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v8 响应样例(结构照官方返回; 含一行 `null` 假期行与复权列)。
    fn sample() -> Value {
        serde_json::json!({
            "chart": {
                "result": [{
                    "meta": {"symbol": "QQQ", "dataGranularity": "1d", "currency": "USD"},
                    "timestamp": [1788220800, 1788307200, 1788393600],
                    "indicators": {
                        "quote": [{
                            "open":   [100.0, null, 104.0],
                            "high":   [102.0, null, 105.0],
                            "low":    [ 99.0, null, 103.0],
                            "close":  [101.0, null, 104.5],
                            "volume": [1000,  null, 2000]
                        }],
                        "adjclose": [{"adjclose": [100.5, null, 104.0]}]
                    }
                }],
                "error": null
            }
        })
    }

    #[test]
    fn test_parse_chart_skips_null_rows_and_keeps_adjclose() {
        let bars = parse_chart(&sample()).expect("可解析");
        assert_eq!(bars.len(), 2, "null 假期行整根跳过");
        assert_eq!(bars[0].kline.close, Decimal::from_f64_retain(101.0).unwrap());
        assert_eq!(bars[0].adj_close, Some(Decimal::from_f64_retain(100.5).unwrap()));
        assert_eq!(bars[1].kline.close, Decimal::from_f64_retain(104.5).unwrap());
        assert_eq!(bars[1].kline.volume, Decimal::from(2000));
        assert!(bars[0].kline.open_time < bars[1].kline.open_time, "升序");
        // close_time = 日线 open + 24h − 1ms
        let delta = bars[0].kline.close_time - bars[0].kline.open_time;
        assert_eq!(delta.num_milliseconds(), 86_400_000 - 1);
    }

    #[test]
    fn test_parse_chart_missing_adjclose_is_none() {
        let mut v = sample();
        v["chart"]["result"][0]["indicators"]["adjclose"] = serde_json::json!(null);
        let bars = parse_chart(&v).expect("可解析");
        assert!(bars.iter().all(|b| b.adj_close.is_none()), "无复权列 → None");
    }

    #[test]
    fn test_parse_chart_error_or_empty_is_none() {
        let err = serde_json::json!({"chart": {"result": null, "error": {"code": "Not Found"}}});
        assert!(parse_chart(&err).is_none());
        assert!(parse_chart(&serde_json::json!({})).is_none());
    }

    #[test]
    fn test_yahoo_interval_mapping_and_unsupported() {
        assert_eq!(yahoo_interval(Interval::M1), Some("1m"));
        assert_eq!(yahoo_interval(Interval::H1), Some("1h"));
        assert_eq!(yahoo_interval(Interval::D1), Some("1d"));
        assert_eq!(yahoo_interval(Interval::W1), Some("1wk"));
        assert_eq!(yahoo_interval(Interval::H4), None, "Yahoo 没有 4h");
        assert_eq!(yahoo_interval(Interval::M3), None, "Yahoo 没有 3m");
    }

    #[tokio::test]
    async fn test_unsupported_interval_rejected_without_network() {
        let src = YahooSource::new();
        let err = src.fetch_klines("QQQ", Interval::D3, 0, 86_400_000).await.unwrap_err();
        assert!(err.to_string().contains("不支持周期"), "{err}");
    }

    #[tokio::test]
    async fn test_reversed_window_returns_empty_without_network() {
        let src = YahooSource::new();
        assert!(src.fetch_klines("QQQ", Interval::D1, 86_400_000, 0).await.unwrap().is_empty());
    }
}
