//! Binance 现货 REST 客户端: 公开端点 + HMAC-SHA256 签名端点。

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use ricow_core::{CoreError, CoreResult, Kline, Market, OrderBook, OrderStatus, PriceLevel};
use rust_decimal::Decimal;
use serde_json::Value;
use sha2::Sha256;

const SPOT_MAINNET_REST: &str = "https://api.binance.com";
const SPOT_TESTNET_REST: &str = "https://testnet.binance.vision";

type HmacSha256 = Hmac<Sha256>;

/// 构建带超时的 reqwest 客户端。
fn build_http() -> CoreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CoreError::Network(e.to_string()))
}

/// Binance 现货 HTTP 客户端。
pub struct BinanceClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    secret_key: Option<String>,
}

impl fmt::Debug for BinanceClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BinanceClient")
            .field("base_url", &self.base_url)
            .field("has_key", &self.api_key.is_some())
            .finish()
    }
}

impl BinanceClient {
    pub fn new() -> CoreResult<Self> {
        let http = build_http()?;
        // 域名可配置 (备用数据域名/代理环境): RICOW_BN_BASE_URL 覆盖, 缺省主网。
        let base_url =
            std::env::var("RICOW_BN_BASE_URL").unwrap_or_else(|_| SPOT_MAINNET_REST.to_string());
        Ok(Self { http, base_url, api_key: None, secret_key: None })
    }

    /// 币安现货测试网客户端 (testnet.binance.vision)。
    pub fn testnet() -> CoreResult<Self> {
        let http = build_http()?;
        Ok(Self { http, base_url: SPOT_TESTNET_REST.to_string(), api_key: None, secret_key: None })
    }

    pub fn with_credentials(mut self, api_key: String, secret_key: String) -> Self {
        self.api_key = Some(api_key);
        self.secret_key = Some(secret_key);
        self
    }

    /// 显式指定 REST 域名 (覆盖 `RICOW_BN_BASE_URL`; demo/测试网/自建代理用)。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// 现货 WS-API 用户数据流 (`userDataStream.subscribe.signature`) 的订阅参数。
    ///
    /// 依据 (2026-09-13 实测): legacy listenKey (`POST /api/v3/userDataStream`) 已被币安于
    /// 2026-02-20 07:00 UTC 永久下线 (主网/demo 均 410 Gone), 官方指定替代即本方法。
    /// 签名规则与 REST 同: 参数按字母序拼 `k=v&...` 后 HMAC-SHA256。
    pub(crate) fn ws_api_subscribe_params(&self) -> CoreResult<Vec<(String, String)>> {
        let (api_key, secret_key) = self.ensure_credentials()?;
        let params = vec![
            ("apiKey".to_string(), api_key.to_string()),
            ("timestamp".to_string(), Utc::now().timestamp_millis().to_string()),
        ];
        let signature = sign_hmac_sha256(&sorted_query_string(&params), secret_key);
        let mut out = params;
        out.push(("signature".to_string(), signature));
        Ok(out)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn ensure_credentials(&self) -> CoreResult<(&str, &str)> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| CoreError::Auth("API key not configured".into()))?;
        let secret_key = self
            .secret_key
            .as_deref()
            .ok_or_else(|| CoreError::Auth("secret key not configured".into()))?;
        Ok((api_key, secret_key))
    }

    // ---- Public Endpoints ----

    pub async fn get_exchange_info(&self) -> CoreResult<Vec<Market>> {
        let url = format!("{}/api/v3/exchangeInfo", self.base_url);
        let resp: Value = self.get_json(&url).await?;
        let symbols =
            resp["symbols"].as_array().ok_or_else(|| CoreError::Parse("missing symbols".into()))?;

        let markets: Vec<Market> = symbols.iter().filter_map(parse_spot_symbol).collect();

        Ok(markets)
    }

    pub async fn get_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> CoreResult<Vec<Kline>> {
        fetch_klines_paged(&self.http, &self.base_url, "/api/v3/klines", symbol, interval, limit)
            .await
    }

    pub async fn get_depth(&self, symbol: &str, limit: u32) -> CoreResult<OrderBook> {
        let url = format!("{}/api/v3/depth?symbol={symbol}&limit={limit}", self.base_url);
        let v: Value = self.get_json(&url).await?;
        let bids = parse_levels(v["bids"].as_array());
        let asks = parse_levels(v["asks"].as_array());
        Ok(OrderBook { bids, asks, timestamp: Utc::now() })
    }

    /// 交易所服务器时间 (`GET /api/v3/time`) —— 实盘启动前时钟预检的数据源。
    pub async fn server_time(&self) -> CoreResult<DateTime<Utc>> {
        let url = format!("{}/api/v3/time", self.base_url);
        let v: Value = self.get_json(&url).await?;
        parse_server_time(&v)
    }

    // ---- Signed Endpoints ----

    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &self,
        symbol: &str,
        side: &str,
        order_type: &str,
        quantity: &str,
        price: Option<&str>,
        client_order_id: &str,
    ) -> CoreResult<Value> {
        let mut params: Vec<(String, String)> = vec![
            ("symbol".into(), symbol.to_uppercase()),
            ("side".into(), side.to_uppercase()),
            ("type".into(), order_type.to_uppercase()),
            ("quantity".into(), quantity.to_string()),
            ("newClientOrderId".into(), client_order_id.to_string()),
        ];
        if let Some(p) = price {
            params.push(("price".into(), p.to_string()));
            params.push(("timeInForce".into(), "GTC".into()));
        }
        self.signed_post("/api/v3/order", &params).await
    }

    pub async fn cancel_order(&self, symbol: &str, client_order_id: &str) -> CoreResult<Value> {
        let params = vec![
            ("symbol".into(), symbol.to_uppercase()),
            ("origClientOrderId".into(), client_order_id.to_string()),
        ];
        self.signed_delete("/api/v3/order", &params).await
    }

    /// 查询未成交挂单 (`GET /api/v3/openOrders`, 按 symbol)。
    pub async fn get_open_orders(&self, symbol: &str) -> CoreResult<Value> {
        let params = vec![("symbol".into(), symbol.to_uppercase())];
        self.signed_get("/api/v3/openOrders", &params).await
    }

    pub async fn get_account(&self) -> CoreResult<Value> {
        self.signed_get("/api/v3/account", &[]).await
    }

    // ---- HTTP helpers ----

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> CoreResult<T> {
        let resp =
            self.http.get(url).send().await.map_err(|e| CoreError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(CoreError::Exchange(format!("BN {status}: {text}")));
        }
        serde_json::from_str(&text).map_err(|e| CoreError::Parse(format!("{e}: {text}")))
    }

    async fn signed_get(&self, path: &str, extra: &[(String, String)]) -> CoreResult<Value> {
        let mut params = extra.to_vec();
        self.add_timestamp_and_sign(&mut params)?;
        let qs = build_query_string(&params);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = self
            .http
            .get(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        check_bn_response(resp).await
    }

    async fn signed_post(&self, path: &str, params: &[(String, String)]) -> CoreResult<Value> {
        let mut p = params.to_vec();
        self.add_timestamp_and_sign(&mut p)?;
        let qs = build_query_string(&p);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = self
            .http
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        check_bn_response(resp).await
    }

    async fn signed_delete(&self, path: &str, params: &[(String, String)]) -> CoreResult<Value> {
        let mut p = params.to_vec();
        self.add_timestamp_and_sign(&mut p)?;
        let qs = build_query_string(&p);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = self
            .http
            .delete(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
            .map_err(|e| CoreError::Network(e.to_string()))?;
        check_bn_response(resp).await
    }

    fn add_timestamp_and_sign(&self, params: &mut Vec<(String, String)>) -> CoreResult<()> {
        let timestamp = Utc::now().timestamp_millis().to_string();
        params.push(("timestamp".into(), timestamp));
        let qs = build_query_string(params);
        let (_, secret_key) = self.ensure_credentials()?;
        let signature = sign_hmac_sha256(&qs, secret_key);
        params.push(("signature".into(), signature));
        Ok(())
    }
}

// ---- 可复用辅助函数 ----

pub(crate) async fn check_bn_response(resp: reqwest::Response) -> CoreResult<Value> {
    let status = resp.status();
    let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
    if !status.is_success() {
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v["msg"].as_str().map(String::from))
            .unwrap_or(text);
        return Err(CoreError::Exchange(format!("BN {status}: {msg}")));
    }
    let v: Value =
        serde_json::from_str(&text).map_err(|e| CoreError::Parse(format!("{e}: {text}")))?;
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 200 {
            let msg = v["msg"].as_str().unwrap_or("unknown").to_string();
            return Err(CoreError::Exchange(format!("BN error {code}: {msg}")));
        }
    }
    Ok(v)
}

pub(crate) fn build_query_string(params: &[(String, String)]) -> String {
    params.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

pub(crate) fn sign_hmac_sha256(data: &str, secret: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub(crate) fn find_filter_value<'a>(
    symbol: &'a Value,
    filter_type: &str,
    field: &str,
) -> Option<&'a str> {
    symbol["filters"]
        .as_array()?
        .iter()
        .find(|f| f["filterType"].as_str() == Some(filter_type))?
        .get(field)?
        .as_str()
}

/// 解析一根 K 线行 (币安 klines 数组元素, 现货/合约同构; 纯函数, 便于单测)。
///
/// 字段序: `[open_time, open, high, low, close, volume, close_time, ...]`。
pub(crate) fn parse_kline_row(row: &[Value]) -> Option<Kline> {
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

/// interval 字符串 → 毫秒 (现货/合约同小写口径; 纯函数, 便于单测)。
pub(crate) fn interval_ms(interval: &str) -> Option<i64> {
    Some(match interval {
        "1m" => 60_000,
        "3m" => 180_000,
        "5m" => 300_000,
        "15m" => 900_000,
        "30m" => 1_800_000,
        "1h" => 3_600_000,
        "2h" => 7_200_000,
        "4h" => 14_400_000,
        "6h" => 21_600_000,
        "8h" => 28_800_000,
        "12h" => 43_200_000,
        "1d" => 86_400_000,
        "3d" => 259_200_000,
        "1w" => 604_800_000,
        _ => return None,
    })
}

/// 分页拉取 K 线 (现货 `/api/v3/klines` 与合约 `/fapi/v1/klines` 同构; 单页上限 1000 根)。
///
/// 起点 = now - limit×interval (BN 不传 startTime 返回最新 1000 根, 必须显式指到过去)。
pub(crate) async fn fetch_klines_paged(
    http: &reqwest::Client,
    base_url: &str,
    path: &str,
    symbol: &str,
    interval: &str,
    limit: u32,
) -> CoreResult<Vec<Kline>> {
    const MAX_PAGE: u32 = 1000;
    let step = interval_ms(interval)
        .ok_or_else(|| CoreError::InvalidArgument(format!("unsupported interval: {interval}")))?;

    let mut all: Vec<Kline> = Vec::new();
    let now_ms = Utc::now().timestamp_millis();
    let mut start_time: Option<i64> = Some(now_ms - (limit as i64) * step);
    while (all.len() as u32) < limit {
        let page = (limit - all.len() as u32).min(MAX_PAGE);
        let mut url = format!("{base_url}{path}?symbol={symbol}&interval={interval}&limit={page}");
        if let Some(st) = start_time {
            url.push_str(&format!("&startTime={st}"));
        }
        let resp =
            http.get(&url).send().await.map_err(|e| CoreError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(CoreError::Exchange(format!("BN {status}: {text}")));
        }
        let raw: Vec<Vec<Value>> = serde_json::from_str(&text)
            .map_err(|e| CoreError::Parse(format!("{e}: {text}")))?;
        if raw.is_empty() {
            break; // 无更多历史
        }
        let page_klines: Vec<Kline> = raw.iter().filter_map(|r| parse_kline_row(r)).collect();
        if page_klines.is_empty() {
            break;
        }
        let last_open = page_klines.last().expect("非空").open_time.timestamp_millis();
        all.extend(page_klines);
        if raw.len() < page as usize {
            break; // 到最新 (不足一整页)
        }
        // 跳过当前 (可能未收盘) bar, 从下一根继续
        start_time = Some(last_open + step);
    }
    Ok(all)
}

/// 订单状态映射 (BN 字符串 → 内部枚举; 现货与合约同构, 纯函数便于单测)。
pub(crate) fn status_from_bn(s: &str) -> OrderStatus {
    match s {
        "NEW" | "PARTIALLY_FILLED" => OrderStatus::Open,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" => OrderStatus::Cancelled,
        "EXPIRED" => OrderStatus::Expired,
        "REJECTED" => OrderStatus::Rejected,
        _ => OrderStatus::Open,
    }
}

/// Decimal → 交易所字符串 (去掉无意义尾零, 纯函数便于单测)。
pub(crate) fn format_dec(d: Decimal) -> String {
    let s = d.to_string();
    if let Some(dot_pos) = s.find('.') {
        let int_part = &s[..dot_pos];
        let frac_part = s[dot_pos + 1..].trim_end_matches('0');
        if frac_part.is_empty() {
            int_part.to_string()
        } else {
            format!("{int_part}.{frac_part}")
        }
    } else {
        s
    }
}

/// 按参数名**字母序**拼 query string —— WS-API 的签名字符串规则 (纯函数, 便于单测)。
pub(crate) fn sorted_query_string(params: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    sorted.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// 解析 `/api/v3/time` 响应 `{"serverTime": 1699999999999}` (纯函数, 便于单测)。
pub fn parse_server_time(v: &Value) -> CoreResult<DateTime<Utc>> {
    let ms = v["serverTime"]
        .as_i64()
        .ok_or_else(|| CoreError::Parse("missing serverTime".into()))?;
    DateTime::from_timestamp_millis(ms)
        .ok_or_else(|| CoreError::Parse(format!("invalid serverTime: {ms}")))
}

/// 解析现货 exchangeInfo 的单个 symbol (纯函数, 便于单测): 仅取 TRADING + 允许现货交易。
///
/// 过滤器: LOT_SIZE(minQty/stepSize) / PRICE_FILTER(tickSize) / 最小名义 ——
/// 现货新版用 `NOTIONAL` 过滤器, 老版用 `MIN_NOTIONAL`; 两者字段名同为 `minNotional`, 依次取第一个命中的。
pub fn parse_spot_symbol(s: &Value) -> Option<Market> {
    if s["status"].as_str()? != "TRADING" {
        return None;
    }
    if !s["isSpotTradingAllowed"].as_bool().unwrap_or(false) {
        return None;
    }
    let symbol = s["symbol"].as_str()?;
    let base = s["baseAsset"].as_str()?;
    let quote = s["quoteAsset"].as_str()?;
    let min_size = find_filter_value(s, "LOT_SIZE", "minQty")
        .and_then(|v| Decimal::from_str(v).ok())
        .unwrap_or(Decimal::ONE);
    let tick_size = find_filter_value(s, "PRICE_FILTER", "tickSize")
        .and_then(|v| Decimal::from_str(v).ok())
        .unwrap_or(Decimal::ONE);
    let step_size =
        find_filter_value(s, "LOT_SIZE", "stepSize").and_then(|v| Decimal::from_str(v).ok());
    let min_notional = find_filter_value(s, "NOTIONAL", "minNotional")
        .or_else(|| find_filter_value(s, "MIN_NOTIONAL", "minNotional"))
        .and_then(|v| Decimal::from_str(v).ok())
        .filter(|d| !d.is_zero());

    Some(Market {
        symbol: symbol.to_string(),
        base_asset: base.to_string(),
        quote_asset: quote.to_string(),
        is_perpetual: false,
        min_size,
        tick_size,
        step_size,
        min_notional,
        max_leverage: None,
        margin_mode: None,
        is_delisted: false,
    })
}

/// 校验下单数量合法性: ≥ min_qty 且为 step_size 整数倍 (容忍浮点尾差, 1e-12 相对容差)。
///
/// 用于真实下单前对齐交易所 LOT_SIZE 规则, 防 -1111 精度拒单。step_size 为 None 时只查 min_qty。
pub fn validate_quantity(min_qty: Decimal, step_size: Option<Decimal>, qty: Decimal) -> bool {
    if qty < min_qty {
        return false;
    }
    match step_size {
        None => true,
        Some(s) if s.is_zero() => true,
        Some(s) => {
            let rem = qty % s;
            // 整数倍判定用相对容差: 浮点 Decimal 除法的余数可能带极小尾差。
            rem.is_zero() || (rem / s).abs() < Decimal::new(1, 12)
        }
    }
}

pub(crate) fn parse_levels(arr: Option<&Vec<Value>>) -> Vec<PriceLevel> {
    arr.map(|items| {
        items
            .iter()
            .filter_map(|item| {
                item.as_array().and_then(|pair| {
                    Some(PriceLevel {
                        price: Decimal::from_str(pair.first()?.as_str()?).ok()?,
                        size: Decimal::from_str(pair.get(1)?.as_str()?).ok()?,
                    })
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_hmac_sha256_known_vector() {
        // RFC 4231 测试向量: key = 0x0b*20, data = "Hi There"
        let secret =
            "\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b";
        let sig = sign_hmac_sha256("Hi There", secret);
        assert_eq!(sig, "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[test]
    fn test_build_query_string() {
        let params = vec![
            ("symbol".to_string(), "ETHUSDT".to_string()),
            ("side".to_string(), "BUY".to_string()),
        ];
        assert_eq!(build_query_string(&params), "symbol=ETHUSDT&side=BUY");
    }

    #[test]
    fn test_parse_levels() {
        let v = serde_json::json!([["3000.0", "1.5"], ["3001.0", "2.0"]]);
        let levels = parse_levels(v.as_array());
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].price, Decimal::from_str("3000.0").unwrap());
        assert_eq!(levels[1].size, Decimal::from_str("2.0").unwrap());
    }

    #[test]
    fn test_validate_quantity_basic() {
        let min = Decimal::from_str("0.001").unwrap();
        let step = Some(Decimal::from_str("0.001").unwrap());
        assert!(validate_quantity(min, step, Decimal::from_str("0.001").unwrap()));
        assert!(validate_quantity(min, step, Decimal::from_str("1.5").unwrap()));
        assert!(!validate_quantity(min, step, Decimal::from_str("0.0005").unwrap()), "低于 minQty");
        assert!(!validate_quantity(min, step, Decimal::from_str("1.5005").unwrap()), "非 step 整数倍");
        assert!(!validate_quantity(min, step, Decimal::ZERO), "零数量");
    }

    #[test]
    fn test_validate_quantity_no_step_or_zero_step() {
        let min = Decimal::from_str("0.001").unwrap();
        // None = 无步进约束, 只查 min_qty
        assert!(validate_quantity(min, None, Decimal::from_str("0.0011").unwrap()));
        assert!(!validate_quantity(min, None, Decimal::from_str("0.0009").unwrap()));
        // step=0 视为无步进约束
        assert!(validate_quantity(min, Some(Decimal::ZERO), Decimal::from_str("0.002").unwrap()));
    }

    #[test]
    fn test_validate_quantity_float_tail_tolerance() {
        // 0.3 由 step 0.1 累加 3 次: Decimal 除法余数可能带极小尾差, 应通过 (相对容差 1e-12)。
        let min = Decimal::from_str("0.1").unwrap();
        let step = Some(Decimal::from_str("0.1").unwrap());
        let qty = Decimal::from_str("0.1").unwrap() * Decimal::from(3); // 0.3
        assert!(validate_quantity(min, step, qty));
    }

    #[test]
    fn test_sorted_query_string_and_ws_api_signature() {
        let params = vec![
            ("timestamp".to_string(), "1700000000000".to_string()),
            ("apiKey".to_string(), "KEY".to_string()),
        ];
        assert_eq!(sorted_query_string(&params), "apiKey=KEY&timestamp=1700000000000");

        // WS-API 订阅参数: 签名必须等于对字母序串签名的结果 (与 REST 同一算法)
        let c = BinanceClient::new()
            .unwrap()
            .with_base_url("https://demo-api.binance.com")
            .with_credentials("KEY".into(), "SECRET".into());
        let p = c.ws_api_subscribe_params().unwrap();
        let api_key = p.iter().find(|(k, _)| k == "apiKey").unwrap().1.clone();
        let ts = p.iter().find(|(k, _)| k == "timestamp").unwrap().1.clone();
        let sig = p.iter().find(|(k, _)| k == "signature").unwrap().1.clone();
        assert_eq!(api_key, "KEY");
        assert_eq!(
            sig,
            sign_hmac_sha256(&format!("apiKey=KEY&timestamp={ts}"), "SECRET"),
            "签名串须按字母序拼接"
        );
        assert_eq!(sig.len(), 64, "HMAC-SHA256 hex 长度");
        // 无凭据时明确报错 (不静默发无效请求)
        let anon = BinanceClient::new().unwrap();
        assert!(anon.ws_api_subscribe_params().is_err());
    }

    #[test]
    fn test_parse_server_time() {
        let v = serde_json::json!({ "serverTime": 1_760_000_000_000_i64 });
        let t = parse_server_time(&v).expect("应解析");
        assert_eq!(t.timestamp_millis(), 1_760_000_000_000);
        assert!(parse_server_time(&serde_json::json!({})).is_err(), "缺字段应报错");
        assert!(
            parse_server_time(&serde_json::json!({"serverTime": "abc"})).is_err(),
            "类型错误应报错"
        );
    }

    #[test]
    fn test_parse_spot_symbol_filters() {
        // 样例取自币安 exchangeInfo 响应结构 (2026-09 实测字段名)。
        let s = serde_json::json!({
            "symbol": "ETHUSDT",
            "status": "TRADING",
            "isSpotTradingAllowed": true,
            "baseAsset": "ETH",
            "quoteAsset": "USDT",
            "filters": [
                {"filterType": "PRICE_FILTER", "tickSize": "0.01000000"},
                {"filterType": "LOT_SIZE", "minQty": "0.00010000", "stepSize": "0.00010000"},
                {"filterType": "NOTIONAL", "minNotional": "5.00000000", "applyMinToMarket": true}
            ]
        });
        let m = parse_spot_symbol(&s).expect("应解析出 Market");
        assert_eq!(m.symbol, "ETHUSDT");
        assert_eq!(m.base_asset, "ETH");
        assert!(!m.is_perpetual);
        assert_eq!(m.min_size, Decimal::from_str("0.0001").unwrap());
        assert_eq!(m.tick_size, Decimal::from_str("0.01").unwrap());
        assert_eq!(m.step_size, Some(Decimal::from_str("0.0001").unwrap()));
        assert_eq!(m.min_notional, Some(Decimal::from(5)));
    }

    #[test]
    fn test_parse_spot_symbol_legacy_min_notional_filter() {
        // 老版过滤器名 MIN_NOTIONAL 亦应取到同一字段名。
        let s = serde_json::json!({
            "symbol": "BTCUSDT", "status": "TRADING", "isSpotTradingAllowed": true,
            "baseAsset": "BTC", "quoteAsset": "USDT",
            "filters": [
                {"filterType": "PRICE_FILTER", "tickSize": "0.01000000"},
                {"filterType": "LOT_SIZE", "minQty": "0.00001000", "stepSize": "0.00001000"},
                {"filterType": "MIN_NOTIONAL", "minNotional": "10.00000000"}
            ]
        });
        let m = parse_spot_symbol(&s).unwrap();
        assert_eq!(m.min_notional, Some(Decimal::from(10)));
    }

    #[test]
    fn test_parse_spot_symbol_skips_non_trading_and_no_notional() {
        // 非 TRADING / 不允许现货交易 → None
        let base = serde_json::json!({
            "symbol": "X", "status": "BREAK", "isSpotTradingAllowed": true,
            "baseAsset": "X", "quoteAsset": "USDT", "filters": []
        });
        assert!(parse_spot_symbol(&base).is_none());
        let no_spot = serde_json::json!({
            "symbol": "X", "status": "TRADING", "isSpotTradingAllowed": false,
            "baseAsset": "X", "quoteAsset": "USDT", "filters": []
        });
        assert!(parse_spot_symbol(&no_spot).is_none());

        // 无名义过滤器 / minNotional=0 → None (不拦截), 其余字段仍按缺省解析
        let no_notional = serde_json::json!({
            "symbol": "X", "status": "TRADING", "isSpotTradingAllowed": true,
            "baseAsset": "X", "quoteAsset": "USDT",
            "filters": [
                {"filterType": "PRICE_FILTER", "tickSize": "0.10000000"},
                {"filterType": "NOTIONAL", "minNotional": "0.00000000"}
            ]
        });
        let m = parse_spot_symbol(&no_notional).unwrap();
        assert_eq!(m.min_notional, None, "0 视为无约束");
        assert_eq!(m.min_size, Decimal::ONE, "无 LOT_SIZE 时回退 1");
        assert_eq!(m.step_size, None);
    }
}
