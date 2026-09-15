//! `futures_data` — Binance USDT-M 合约 (fapi) 公共市场数据源。
//!
//! 仅公共端点, 无需 API 密钥 (specs/backtest.md §五): K 线与 MMR 元数据。
//!
//! - 域名: `fapi.binance.com` (RICOW_FAPI_BASE_URL 可覆盖, 与现货口径一致)。
//! - K 线 JSON 与现货同构, 分页逻辑同 `client::get_klines`。
//! - MMR: 逐仓首档维持保证金率走**内置表** `tier1_mmr_pct(symbol)` (数据源 = 签名
//!   `/fapi/v1/leverageBracket` bracket1, 2026-09-05 demo 实测 30 对; 公共 exchangeInfo 的
//!   `maintMarginPercent` 为深档误导值不可用 — 详见 specs/backtest.md §十一 T7)。
//!
//! 注意: 公共数据**不含费率字段** — 合约回测默认费率 (maker 2bps / taker 5bps, BN USDT-M
//! 标准) 在配置层处理 (specs/backtest.md §三), 不在数据源内。

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ricow_core::{CoreError, CoreResult, Kline};
use rust_decimal::Decimal;
use serde_json::Value;

/// USDT-M 合约主网 REST。
const FAPI_MAINNET_REST: &str = "https://fapi.binance.com";

/// Binance USDT-M 公共数据客户端。
pub struct FuturesDataClient {
    http: reqwest::Client,
    base_url: String,
}

impl std::fmt::Debug for FuturesDataClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FuturesDataClient").field("base_url", &self.base_url).finish()
    }
}

impl FuturesDataClient {
    pub fn new() -> CoreResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CoreError::Network(e.to_string()))?;
        // 域名可配置 (代理/备用环境): RICOW_FAPI_BASE_URL 覆盖, 缺省主网。
        let base_url =
            std::env::var("RICOW_FAPI_BASE_URL").unwrap_or_else(|_| FAPI_MAINNET_REST.to_string());
        Ok(Self { http, base_url })
    }

    /// 分页拉取 K 线 (BN API 单请求上限 1000; startTime 游标升序拼接, 同现货 client)。
    pub async fn get_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> CoreResult<Vec<Kline>> {
        const MAX_PAGE: u32 = 1000;
        let interval_ms = match interval {
            "1m" => 60_000_i64,
            "5m" => 300_000,
            "15m" => 900_000,
            "1h" => 3_600_000,
            "4h" => 14_400_000,
            "1d" => 86_400_000,
            other => {
                return Err(CoreError::InvalidArgument(format!("unsupported interval: {other}")))
            }
        };

        let mut all: Vec<Kline> = Vec::new();
        // 起点 = now − limit×interval (不传 startTime 只回最新 1000 根)。
        let now_ms = Utc::now().timestamp_millis();
        let mut start_time: Option<i64> = Some(now_ms - (limit as i64) * interval_ms);
        while (all.len() as u32) < limit {
            let page = (limit - all.len() as u32).min(MAX_PAGE);
            let mut url = format!(
                "{}/fapi/v1/klines?symbol={symbol}&interval={interval}&limit={page}",
                self.base_url
            );
            if let Some(st) = start_time {
                url.push_str(&format!("&startTime={st}"));
            }
            let raw: Vec<Vec<Value>> = self.get_json(&url).await?;
            if raw.is_empty() {
                break;
            }
            let page_klines: Vec<Kline> =
                raw.iter().filter_map(|row| parse_kline_row(row)).collect();
            if page_klines.is_empty() {
                break;
            }
            let last_open = page_klines.last().unwrap().open_time.timestamp_millis();
            all.extend(page_klines);
            if raw.len() < page as usize {
                break; // 到最新 (不足一整页)
            }
            // 跳过当前 (可能未收盘) bar, 从下一根继续。
            start_time = Some(last_open + interval_ms);
        }
        Ok(all)
    }

    /// 股票类永续 (EQUITY) 池枚举 — fapi exchangeInfo 按 `underlyingType=="EQUITY"` 过滤。
    ///
    /// 数据事实 (2026-09-06 免 key 实测): 币安美股代币永续的 contractType=`TRADIFI_PERPETUAL`
    /// (非普通 PERPETUAL), quote=USDT, TRADING ~155 个 (TSLAUSDT/NVDAUSDT/SPYUSDT…)。
    /// 这是免 key 的股票池锚点: bStock 现货识别 = spot base 去尾 B ∈ 本列表。
    /// 注意: 现有签名客户端 get_exchange_info 过滤 contractType=="PERPETUAL", 会漏掉本池。
    pub async fn get_equity_pool(&self) -> CoreResult<Vec<String>> {
        let url = format!("{}/fapi/v1/exchangeInfo", self.base_url);
        let raw: Value = self.get_json(&url).await?;
        let symbols =
            raw["symbols"].as_array().ok_or_else(|| CoreError::Parse("missing symbols".into()))?;
        Ok(equity_bases_from_symbols(symbols))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> CoreResult<T> {
        let resp =
            self.http.get(url).send().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CoreError::Exchange(format!("fapi HTTP {status}: {body}")));
        }
        resp.json().await.map_err(|e| CoreError::Network(e.to_string()))
    }
}

/// 逐仓首档维持保证金率 (%): 内置表查询 (symbol → 首档 MMR, 数据源 = demo-fapi
/// `/fapi/v1/leverageBracket` bracket1, 2026-09-05 签名实测 30 对)。
/// 背景: 公共 exchangeInfo 的 `maintMarginPercent` 为深档误导值 (BTCUSDT 返回 2.5% 实为第 4 档,
/// 名义 300万-2000万), 用作首档会让杠杆 >1/MMR 的仓位开仓即虚触发强平 (specs/backtest.md §十一 T7)。
/// leverageBracket 需签名密钥, 公共回测不可达 → 内置实测表; 表外 symbol 回落 1.0% (保守)。
/// 用户仍可用 `--mmr-pct` 显式覆盖 (三层配置最上层)。
pub fn tier1_mmr_pct(symbol: &str) -> f64 {
    let sym = symbol.to_ascii_uppercase();
    match sym.as_str() {
        // 0.4%
        "BTCUSDT" | "BCHUSDT" => 0.4,
        // 0.5%
        "ETHUSDT" | "BNBUSDT" | "XRPUSDT" | "ETCUSDT" => 0.5,
        // 0.6%
        "DOGEUSDT" | "UNIUSDT" | "OPUSDT" | "ARBUSDT" | "ATOMUSDT" | "FILUSDT" => 0.6,
        // 0.65%
        "ADAUSDT" | "LINKUSDT" | "DOTUSDT" | "LTCUSDT" | "TRXUSDT" => 0.65,
        // 1.0%
        "SOLUSDT" | "AVAXUSDT" | "APTUSDT" => 1.0,
        // 1.5%
        "TONUSDT" | "NEARUSDT" | "TIAUSDT" | "WIFUSDT" | "ORDIUSDT" | "DASHUSDT" | "XMRUSDT" => 1.5,
        // 2.0%
        "SUIUSDT" | "INJUSDT" | "SEIUSDT" => 2.0,
        // 未知回落 (保守: 偏早强平, 不利好虚报盈利)。
        _ => 1.0,
    }
}

/// 从 fapi exchangeInfo symbols 数组提取 EQUITY 池 base 列表 (TRADING + underlyingType=EQUITY)。
///
/// 纯函数 (便于单测; 网络解析入口 = FuturesDataClient::get_equity_pool)。
fn equity_bases_from_symbols(symbols: &[Value]) -> Vec<String> {
    let mut bases: Vec<String> = symbols
        .iter()
        .filter_map(|s| {
            if s["status"].as_str()? != "TRADING" {
                return None;
            }
            if s["underlyingType"].as_str()? != "EQUITY" {
                return None;
            }
            s["baseAsset"].as_str().map(|b| b.to_string())
        })
        .collect();
    bases.sort();
    bases.dedup();
    bases
}

/// 解析一行 fapi K 线 (与现货同构: [open_time, open, high, low, close, volume, close_time, ...])。
fn parse_kline_row(row: &[Value]) -> Option<Kline> {
    let open_time = row.first()?.as_i64()?;
    let close_time = row.get(6)?.as_i64()?;
    Some(Kline {
        open_time: DateTime::from_timestamp_millis(open_time)?,
        open: Decimal::from_str(row.get(1)?.as_str()?).ok()?,
        high: Decimal::from_str(row.get(2)?.as_str()?).ok()?,
        low: Decimal::from_str(row.get(3)?.as_str()?).ok()?,
        close: Decimal::from_str(row.get(4)?.as_str()?).ok()?,
        volume: Decimal::from_str(row.get(5)?.as_str()?).ok()?,
        close_time: DateTime::from_timestamp_millis(close_time)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier1_mmr_pct_table_hits() {
        // 内置首档表 (数据源: demo-fapi leverageBracket bracket1, 2026-09-05 实测)。
        assert_eq!(tier1_mmr_pct("BTCUSDT"), 0.4);
        assert_eq!(tier1_mmr_pct("btcusdt"), 0.4, "大小写不敏感");
        assert_eq!(tier1_mmr_pct("ETHUSDT"), 0.5);
        assert_eq!(tier1_mmr_pct("DASHUSDT"), 1.5);
        assert_eq!(tier1_mmr_pct("SUIUSDT"), 2.0);
    }

    #[test]
    fn test_tier1_mmr_pct_unknown_fallback() {
        // 表外 symbol → 回落 1.0 (保守)。
        assert_eq!(tier1_mmr_pct("XXXUSDT"), 1.0);
        assert_eq!(tier1_mmr_pct(""), 1.0);
    }

    #[test]
    fn test_parse_kline_row_ok() {
        let row = serde_json::json!([
            1750000000000_i64,
            "100.0",
            "102.0",
            "99.5",
            "101.5",
            "12.3",
            1750003599999_i64,
            "0",
            "1",
            "0",
            "0",
            "0"
        ]);
        let k = parse_kline_row(row.as_array().unwrap()).expect("解析成功");
        assert_eq!(k.open, Decimal::from_str("100.0").unwrap());
        assert_eq!(k.close, Decimal::from_str("101.5").unwrap());
        assert_eq!(k.close_time.timestamp_millis(), 1750003599999);
    }

    #[test]
    fn test_equity_bases_filters_equity_only() {
        // fapi exchangeInfo symbols 样例: EQUITY 永续 (TRADIFI_PERPETUAL) + crypto 永续 (PERPETUAL)
        // + 非 TRADING + 去重。
        let symbols = serde_json::json!([
            {"symbol": "TSLAUSDT", "baseAsset": "TSLA", "status": "TRADING", "underlyingType": "EQUITY", "contractType": "TRADIFI_PERPETUAL"},
            {"symbol": "NVDAUSDT", "baseAsset": "NVDA", "status": "TRADING", "underlyingType": "EQUITY", "contractType": "TRADIFI_PERPETUAL"},
            {"symbol": "SPYUSDT",  "baseAsset": "SPY",  "status": "TRADING", "underlyingType": "EQUITY", "contractType": "TRADIFI_PERPETUAL"},
            // crypto 永续不得混入
            {"symbol": "BTCUSDT",  "baseAsset": "BTC",  "status": "TRADING", "underlyingType": "COIN",  "contractType": "PERPETUAL"},
            // 下架 EQUITY 不得入池
            {"symbol": "XUSDT",    "baseAsset": "X",    "status": "SETTLING", "underlyingType": "EQUITY", "contractType": "TRADIFI_PERPETUAL"},
            // 重复 symbol 去重
            {"symbol": "TSLAUSDT", "baseAsset": "TSLA", "status": "TRADING", "underlyingType": "EQUITY", "contractType": "TRADIFI_PERPETUAL"},
        ]);
        let bases = equity_bases_from_symbols(symbols.as_array().unwrap());
        assert_eq!(bases, vec!["NVDA".to_string(), "SPY".to_string(), "TSLA".to_string()]);
    }
}
