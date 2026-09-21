//! Nasdaq 官方 API 美股日线适配器 (信号数据源, 免 key)。
//!
//! 数据事实 (2026-09-06/07 本机实测, 详见 hyper-local-development refs/bstocks-market-data-2026-09.md):
//! - `GET /api/quote/{ticker}/historical?assetclass=stocks|etf&fromdate=..&todate=..&limit=9999`
//!   返回 ~10 年日线 (服务端窗口上限 ~2514 根), **拆股已复权、分红未复权**;
//! - ETF 必须传 `assetclass=etf` (SPY/QQQ/TQQQ/SOXL/EWY/IBIT 全通), 否则 data=null;
//! - 带点代码用点号 (BRK.B);
//! - 返回 JSON: `data.tradesTable.rows` (倒序: 最新在前), 字段 date=MM/DD/YYYY,
//!   close 等价格带 `$` 前缀 (清洗时去掉), volume 逗号分隔, 停牌日 volume="N/A";
//! - 无 SLA; 失败语义 (调用方策略级): 重试 3 次退避后仍失败 → 该标的当日信号缺失,
//!   跳过当日调仓、维持现持仓 (幂等安全)。适配器本身只读, 无写风险。
//!
//! # 时间契约 (2026-09-09 定稿, 截断无前视的前提)
//! - 美股日线按**交易日**生成 (自带日历)。`date=MM/DD/YYYY` 是当地交易日日期,
//!   无精确时分 → bar `open_time` 以**当日 UTC 00:00** 为锚 (见 `parse_us_date`),
//!   `close_time = open_time + 24h`。
//! - 信号语义: 美股 T 日 16:00 ET 收盘 (UTC ≈ 20-21:00) 方知 T 日行情 → 引擎截断条件
//!   `signal bar.open_time < 全局 tick 时间` 下, T 日信号线在 bStock UTC bar T+1
//!   (open_time = T+1 00:00) 的 tick 首见, 成交于其后首根新开盘 bar — 无前视。
//! - 跨年/跨月无特例: 12/31 的 bar open_time = 12-31T00:00:00Z, close_time =
//!   次年 01-01T00:00:00Z; 对齐/截断一律按 DateTime<Utc> 全序比较, 勿按字符串日期。
//!
//! 缓存: 调用方负责落库 (统一走 `data_klines`, source = "nasdaq"), 本模块只做网络拉取 + 解析。

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ricow_core::{CoreError, CoreResult, Kline};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::us_tickers::AssetClass;

/// Nasdaq API 主站。
const NASDAQ_API: &str = "https://api.nasdaq.com";

/// Nasdaq 日线客户端。
///
/// 起始日期不再是写死的常量 (028 T011): 调用方按需要的区间传 `fromdate`/`todate`
/// (均为 `MM/DD/YYYY`, 闭区间); 服务端窗口上限约 2514 根, 超出即拿不满。
pub struct NasdaqClient {
    http: reqwest::Client,
}

impl Default for NasdaqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NasdaqClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .expect("reqwest client 构建");
        Self { http }
    }

    /// 拉取单只美股/ETF 日线, 区间 `[fromdate, todate]` (均 `MM/DD/YYYY`, **闭区间**)。
    ///
    /// 失败 = Err (调用方按失败语义处理)。028 T011: 起始日由调用方给, 不再写死 2016-01-01。
    pub async fn get_daily_klines_between(
        &self,
        ticker: &str,
        assetclass: AssetClass,
        fromdate: &str,
        todate: &str,
    ) -> CoreResult<Vec<Kline>> {
        let mut last_err: Option<CoreError> = None;
        // 失败语义: 重试 3 次, 指数退避 1s/2s/4s。
        for attempt in 0..3 {
            match self.fetch_once(ticker, assetclass, fromdate, todate).await {
                Ok(klines) => return Ok(klines),
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| CoreError::Network("nasdaq 拉取失败".into())))
    }

    async fn fetch_once(
        &self,
        ticker: &str,
        assetclass: AssetClass,
        fromdate: &str,
        todate: &str,
    ) -> CoreResult<Vec<Kline>> {
        let url = format!(
            "{NASDAQ_API}/api/quote/{ticker}/historical?assetclass={}&fromdate={fromdate}&todate={todate}&limit=9999",
            assetclass.as_str()
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("nasdaq HTTP: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::Exchange(format!("nasdaq HTTP {}", resp.status())));
        }
        let v: Value =
            resp.json().await.map_err(|e| CoreError::Parse(format!("nasdaq JSON: {e}")))?;
        parse_historical(&v).ok_or_else(|| {
            CoreError::Parse(format!("nasdaq 响应无 tradesTable 数据 (ticker={ticker})"))
        })
    }
}

/// 解析 historical 响应 → K 线 (升序)。
///
/// 响应结构: `data.tradesTable.rows` (倒序, 最新在前); date=MM/DD/YYYY;
/// close/open/high/low 带 `$` 前缀 (ETF 有时不带), volume 逗号分隔, 停牌日 "N/A"。
/// 纯函数 (便于单测)。
pub fn parse_historical(v: &Value) -> Option<Vec<Kline>> {
    let rows = v["data"]["tradesTable"]["rows"].as_array()?;
    let mut out: Vec<Kline> = Vec::with_capacity(rows.len());
    for row in rows {
        let date_str = row["date"].as_str()?;
        let close = parse_price(row["close"].as_str())?;
        // date 转 UTC: 美股日线以当地日 0 点为锚 (信号按交易日, 精确时分无意义)。
        let open_time = parse_us_date(date_str)?;
        out.push(Kline {
            open_time,
            open: row["open"].as_str().and_then(|s| parse_price(Some(s))).unwrap_or(close),
            high: row["high"].as_str().and_then(|s| parse_price(Some(s))).unwrap_or(close),
            low: row["low"].as_str().and_then(|s| parse_price(Some(s))).unwrap_or(close),
            close,
            volume: parse_volume(row["volume"].as_str()).unwrap_or(Decimal::ZERO),
            close_time: open_time + chrono::Duration::hours(24),
        });
    }
    out.reverse(); // 升序 (与币安 klines 一致)
    Some(out)
}

/// "$354.08" → 354.08 (去掉 $ 与逗号; 无前缀也接受)。
fn parse_price(s: Option<&str>) -> Option<Decimal> {
    let s = s?.trim().trim_start_matches('$').replace(',', "");
    Decimal::from_str(&s).ok()
}

/// "65,018,210" / "N/A" → Decimal (停牌日/无量为 0)。
fn parse_volume(s: Option<&str>) -> Option<Decimal> {
    let s = s?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("N/A") {
        return Some(Decimal::ZERO);
    }
    let s = s.replace(',', "");
    Decimal::from_str(&s).ok()
}

/// "09/04/2026" → DateTime<Utc> (当天 UTC 00:00 锚; 信号粒度到日)。
/// 时间契约见模块头注释 — 交易日日期 → UTC 日 0 点, 无本地时区偏移。
fn parse_us_date(s: &str) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let month: u32 = parts[0].parse().ok()?;
    let day: u32 = parts[1].parse().ok()?;
    let year: i32 = parts[2].parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(0, 0, 0).map(|d| d.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_historical_orders_ascending() {
        let v = serde_json::json!({
            "data": {"tradesTable": {"rows": [
                {"date": "09/04/2026", "close": "$354.08", "volume": "65,018,210",
                 "open": "$362.07", "high": "$364.69", "low": "$351.32"},
                {"date": "09/03/2026", "close": "$350.00", "volume": "60,000,000",
                 "open": "$351.00", "high": "$352.00", "low": "$349.00"}
            ]}}
        });
        let klines = parse_historical(&v).expect("解析成功");
        assert_eq!(klines.len(), 2);
        assert!(klines[0].open_time < klines[1].open_time, "应升序");
        assert_eq!(klines[0].close, Decimal::from_str("350.00").unwrap());
        assert_eq!(klines[1].close, Decimal::from_str("354.08").unwrap());
        assert_eq!(klines[1].volume, Decimal::from_str("65018210").unwrap());
    }

    #[test]
    fn test_parse_historical_etf_no_dollar_and_na_volume() {
        // ETF 响应 close 无 $ 前缀; 停牌日 volume=N/A → 0。
        let v = serde_json::json!({
            "data": {"tradesTable": {"rows": [
                {"date": "09/04/2026", "close": "770.19", "volume": "3,000,000",
                 "open": "771.0", "high": "772.0", "low": "769.0"},
                {"date": "09/01/2022", "close": "18.94", "volume": "N/A",
                 "open": "18.94", "high": "18.94", "low": "18.94"}
            ]}}
        });
        let klines = parse_historical(&v).expect("解析成功");
        assert_eq!(klines[0].volume, Decimal::ZERO, "停牌日量 = 0");
        assert_eq!(klines[1].close, Decimal::from_str("770.19").unwrap());
    }

    #[test]
    fn test_parse_price_cleanup() {
        assert_eq!(parse_price(Some("$354.08")), Decimal::from_str("354.08").ok());
        assert_eq!(parse_price(Some("770.19")), Decimal::from_str("770.19").ok());
        assert_eq!(parse_price(Some("1,234.56")), Decimal::from_str("1234.56").ok());
        assert_eq!(parse_price(None), None);
    }

    #[test]
    fn test_parse_us_date() {
        let dt = parse_us_date("09/04/2026").unwrap();
        assert_eq!(dt.date_naive().to_string(), "2026-09-04");
        assert!(parse_us_date("2026-09-04").is_none(), "必须 MM/DD/YYYY");
    }

    #[test]
    fn test_us_date_cross_year_boundary_utc_anchor() {
        // 时间契约: 交易日日期 → 当日 UTC 00:00 锚; 跨年日无特例。
        let dec31 = parse_us_date("12/31/2026").unwrap();
        assert_eq!(dec31.to_rfc3339(), "2026-12-31T00:00:00+00:00");
        assert_eq!((dec31 + chrono::Duration::hours(24)).to_rfc3339(), "2027-01-01T00:00:00+00:00");
        // close_time 契约 = open_time + 24h (含跨年)。
        let v = serde_json::json!({
            "data": {"tradesTable": {"rows": [
                {"date": "12/31/2026", "close": "$100.00", "volume": "1,000",
                 "open": "$99.00", "high": "$101.0", "low": "$98.0"}
            ]}}
        });
        let klines = parse_historical(&v).expect("解析成功");
        assert_eq!(klines[0].open_time.to_rfc3339(), "2026-12-31T00:00:00+00:00");
        assert_eq!(klines[0].close_time.to_rfc3339(), "2027-01-01T00:00:00+00:00");
        // 截断比较基准: open_time 全序 (跨年 bar 恒 > 前年任何 bar)。
        assert!(parse_us_date("12/31/2025").unwrap() < klines[0].open_time);
    }

    #[test]
    fn test_parse_historical_missing_data() {
        // data=null (assetclass 错误等) → None。
        let v = serde_json::json!({"data": null, "status": {"rCode": 400}});
        assert!(parse_historical(&v).is_none());
    }
}
