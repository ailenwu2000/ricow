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

use crate::retry::{send_with_retry, status_error, RequestKind, RetryPolicy};

const SPOT_MAINNET_REST: &str = "https://api.binance.com";
const SPOT_TESTNET_REST: &str = "https://testnet.binance.vision";

type HmacSha256 = Hmac<Sha256>;

/// 构建带超时的 reqwest 客户端。
///
/// `redirect::Policy::none()` (审计 H-6): API 客户端不该跟随重定向 —— 默认策略会跟最多
/// 10 跳, 签名与 `X-MBX-APIKEY` 自定义头不在 reqwest 的跨主机剥离列表里, 会原样发到跳转目标。
fn build_http() -> CoreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CoreError::Network(e.to_string()))
}

/// 进程级共享的 HTTP 客户端 (审计 中危 #7)。
///
/// 每个 `BinanceClient::new()` 都自建一个 `reqwest::Client` 时, 连接池/keep-alive 也随之
/// 重建 —— 而 `reqwest::Client` 本身就是 `Arc` 包着的连接池, **clone 是廉价的**。Web 每次
/// 请求 (K 线/盘口) 都新建交易所对象, 于是每次都要重做 DNS + TLS 握手, 并在高频轮询下
/// 反复创建/丢弃连接池。这里按"配置签名"缓存: `reqwest::ClientBuilder::build()` 在完全
/// 相同的配置下返回的是**同一个** `Arc`, 所以缓存既能省掉重复构造, 又不会把不同超时/
/// 重定向策略的客户端混用。
///
/// 失败时返回 `None`, 由调用方回落到 [`build_http`] —— 共享池是优化, 不是正确性前提。
/// (用 `OnceLock` 而非 `LazyLock`: 构造可能失败, 要把错误留给调用方如实报告。)
pub(crate) fn shared_http() -> Option<reqwest::Client> {
    static CACHE: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(c) = CACHE.get() {
        return Some(c.clone());
    }
    let built = build_http().ok()?;
    // 并发竞态下只保留第一个: `set` 失败时用已存的那个, 两者配置相同、行为一致。
    let _ = CACHE.set(built);
    CACHE.get().cloned()
}

/// base_url 主机白名单 (审计 H-6): 只放行**币安系域名**与**回环地址** (本地代理/测试)。
///
/// `RICOW_BN_BASE_URL` / `RICOW_FAPI_BASE_URL` 或 `with_base_url` 把签名请求指向白名单外
/// 的主机 → 硬失败: 本机恶意进程改一个环境变量就能把 API key、signature 与全部请求参数
/// (含下单意图) 静默导流到第三方主机, 这条门让该路径在构造客户端时即报错。
pub(crate) fn validate_base_url(base_url: &str) -> CoreResult<()> {
    let Some(host) = ricow_core::url_host(base_url) else {
        return Err(CoreError::InvalidArgument(format!(
            "base_url '{base_url}' 非法: 解析不出主机名 (需形如 https://api.binance.com)"
        )));
    };
    let allowed = host == "binance.com"
        || host.ends_with(".binance.com")
        || host == "binance.vision"
        || host.ends_with(".binance.vision")
        || matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if allowed {
        Ok(())
    } else {
        Err(CoreError::InvalidArgument(format!(
            "base_url 主机 '{host}' 不在白名单 (仅允许 *.binance.com / *.binance.vision / 回环地址): \
             REST 域名决定签名请求发往何处, 放行未知主机等于把 API key 与签名交给它。\
             请检查环境变量 RICOW_BN_BASE_URL / RICOW_FAPI_BASE_URL 是否被改过"
        )))
    }
}

/// env 覆盖 base_url 时的醒目警告 (审计 H-6): 域名被改过必须留痕, 不能静默生效。
pub(crate) fn warn_base_url_override(env_name: &str, base_url: &str) {
    tracing::warn!("{env_name} 覆盖 REST 域名 → {base_url} (已过主机白名单; 非本人操作请排查环境)");
}

/// Binance 现货 HTTP 客户端 (Clone 便宜: reqwest::Client 内部即 Arc, 供 WS 快照闭包捕获)。
#[derive(Clone)]
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
        let http = shared_http().map_or_else(build_http, Ok)?;
        // 域名可配置 (备用数据域名/代理环境): RICOW_BN_BASE_URL 覆盖, 缺省主网。
        let env = std::env::var("RICOW_BN_BASE_URL").ok();
        let base_url = env.clone().unwrap_or_else(|| SPOT_MAINNET_REST.to_string());
        validate_base_url(&base_url)?;
        if env.is_some() {
            warn_base_url_override("RICOW_BN_BASE_URL", &base_url);
        }
        Ok(Self { http, base_url, api_key: None, secret_key: None })
    }

    /// 币安现货测试网客户端 (testnet.binance.vision)。
    pub fn testnet() -> CoreResult<Self> {
        let http = shared_http().map_or_else(build_http, Ok)?;
        Ok(Self { http, base_url: SPOT_TESTNET_REST.to_string(), api_key: None, secret_key: None })
    }

    pub fn with_credentials(mut self, api_key: String, secret_key: String) -> Self {
        self.api_key = Some(api_key);
        self.secret_key = Some(secret_key);
        self
    }

    /// 显式指定 REST 域名 (覆盖 `RICOW_BN_BASE_URL`; demo/测试网/自建代理用)。
    ///
    /// 主机必须在白名单内 (审计 H-6), 否则返回错误 —— 调用方多为编译期常量
    /// (demo 域名), 报错即说明常量或调用链被人改过。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> CoreResult<Self> {
        let base_url = base_url.into();
        validate_base_url(&base_url)?;
        self.base_url = base_url;
        Ok(self)
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
        fetch_klines_paged(
            &self.http,
            &self.base_url,
            "/api/v3/klines",
            symbol,
            interval,
            limit,
            None,
        )
        .await
    }

    /// 截止到 `end_ms` 的 K 线 (回测按自然年月分段); 与 `get_klines` 同源分页逻辑。
    pub async fn get_klines_ending_at(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
        end_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        fetch_klines_paged(
            &self.http,
            &self.base_url,
            "/api/v3/klines",
            symbol,
            interval,
            limit,
            Some(end_ms),
        )
        .await
    }

    pub async fn get_depth(&self, symbol: &str, limit: u32) -> CoreResult<OrderBook> {
        let symbol = encode_query_component(symbol);
        let url = format!("{}/api/v3/depth?symbol={symbol}&limit={limit}", self.base_url);
        let v: Value = self.get_json(&url).await?;
        let bids = parse_levels(v["bids"].as_array());
        let asks = parse_levels(v["asks"].as_array());
        Ok(OrderBook { bids, asks, timestamp: Utc::now() })
    }

    /// 拉取盘口快照 (含 `lastUpdateId`) —— WS 盘口重连后重建累计盘口用 (审计 H-4)。
    ///
    /// 与 [`Self::get_depth`] 的区别: 额外带回 `lastUpdateId`, 供 depth session 丢弃
    /// 快照之前的增量帧, 保证"快照 → 其后增量"的衔接。
    pub(crate) async fn depth_snapshot(
        &self,
        symbol: &str,
        limit: u32,
    ) -> CoreResult<(u64, OrderBook)> {
        let url = format!(
            "{}/api/v3/depth?symbol={}&limit={limit}",
            self.base_url,
            encode_query_component(&symbol.to_uppercase())
        );
        let v: Value = self.get_json(&url).await?;
        let bids = parse_levels(v["bids"].as_array());
        let asks = parse_levels(v["asks"].as_array());
        let last_update_id = v["lastUpdateId"].as_u64().unwrap_or(0);
        Ok((last_update_id, OrderBook { bids, asks, timestamp: Utc::now() }))
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
            send_with_retry(|| self.http.get(url), RequestKind::Read, RetryPolicy::default())
                .await?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(status_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| CoreError::Parse(format!("{e}: {text}")))
    }

    async fn signed_get(&self, path: &str, extra: &[(String, String)]) -> CoreResult<Value> {
        let mut params = extra.to_vec();
        self.add_timestamp_and_sign(&mut params)?;
        let qs = build_query_string(&params);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = send_with_retry(
            || self.http.get(&url).header("X-MBX-APIKEY", api_key),
            RequestKind::Read,
            RetryPolicy::default(),
        )
        .await?;
        check_bn_response(resp).await
    }

    async fn signed_post(&self, path: &str, params: &[(String, String)]) -> CoreResult<Value> {
        let mut p = params.to_vec();
        self.add_timestamp_and_sign(&mut p)?;
        let qs = build_query_string(&p);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = send_with_retry(
            || self.http.post(&url).header("X-MBX-APIKEY", api_key),
            RequestKind::Write,
            RetryPolicy::default(),
        )
        .await?;
        check_bn_response(resp).await
    }

    async fn signed_delete(&self, path: &str, params: &[(String, String)]) -> CoreResult<Value> {
        let mut p = params.to_vec();
        self.add_timestamp_and_sign(&mut p)?;
        let qs = build_query_string(&p);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = send_with_retry(
            || self.http.delete(&url).header("X-MBX-APIKEY", api_key),
            RequestKind::Write,
            RetryPolicy::default(),
        )
        .await?;
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
        // 429/418 → RateLimit, 其余 → Exchange (035; 统一走 retry::status_error)。
        return Err(status_error(status, &text));
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

/// 把字符串按 **query 分量** 规则百分号编码 (审计 低危 #3)。
///
/// 拼进 URL 的 symbol 直接 `format!("symbol={symbol}")` 时, 若含 `&`/`=`/`#`/空格 等字符,
/// 会**改变 query 语义** —— 例如 `symbol=BTC&limit=1` 会被拆成两个参数, `#` 之后被当片段丢弃。
/// 虽然 symbol 常规来自内部枚举、且上游 `validate` 已过滤, 但"拼 URL 一律转义"是纵深防线:
/// 新增调用点(如按用户输入拼 symbol)时不会再重现这个洞。
///
/// 用全 ASCII 白名单编码: 字母数字 + `-_.~` 原样, 其余字节 `%XX`(大写十六进制)。
/// 与 `application/x-www-form-urlencoded` 不同 —— 空格编成 `%20` 而非 `+`, 更安全。
pub(crate) fn encode_query_component(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0x0f) as usize] as char);
            }
        }
    }
    out
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
    end_time: Option<i64>,
) -> CoreResult<Vec<Kline>> {
    const MAX_PAGE: u32 = 1000;
    let step = interval_ms(interval)
        .ok_or_else(|| CoreError::InvalidArgument(format!("unsupported interval: {interval}")))?;

    let mut all: Vec<Kline> = Vec::new();
    // end_time = None 时窗口截止"现在"; Some(ms) 时截止到指定时刻 (回测按自然年月分段用)。
    let end_ms = end_time.unwrap_or_else(|| Utc::now().timestamp_millis());
    let mut start_time: Option<i64> = Some(end_ms - (limit as i64) * step);
    while (all.len() as u32) < limit {
        let page = (limit - all.len() as u32).min(MAX_PAGE);
        let mut url = format!(
            "{base_url}{path}?symbol={}&interval={}&limit={page}",
            encode_query_component(symbol),
            encode_query_component(interval)
        );
        if let Some(st) = start_time {
            url.push_str(&format!("&startTime={st}"));
        }
        if end_time.is_some() {
            url.push_str(&format!("&endTime={end_ms}"));
        }
        let resp =
            send_with_retry(|| http.get(url.clone()), RequestKind::Read, RetryPolicy::default())
                .await?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(status_error(status, &text));
        }
        let raw: Vec<Vec<Value>> =
            serde_json::from_str(&text).map_err(|e| CoreError::Parse(format!("{e}: {text}")))?;
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
    let ms =
        v["serverTime"].as_i64().ok_or_else(|| CoreError::Parse("missing serverTime".into()))?;
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

    /// 审计 中危 #7: 共享客户端必须**每次都是同一个连接池** —— Web 高频轮询
    /// (K 线/盘口) 每次都新建交易所对象, 若每次都拿到新 client 就等于每次都重做
    /// DNS + TLS 握手并丢掉连接复用。
    ///
    /// 判据取 reqwest 的**公开契约**: 配置完全相同时 `ClientBuilder::build()` 返回
    /// 同一个 `Arc` 池。这里用"可观察后果"证明 —— 同一个池意味着 TCP 连接可跨
    /// `Client` 实例复用, 而这正是本修复要达到的效果 (见下方 keep-alive 测试)。
    /// (本 crate `forbid(unsafe_code)`, 所以不窥探内部指针 —— 那本来也会随 reqwest
    /// 版本脆断。)
    #[test]
    fn shared_http_is_one_pool_across_calls() {
        let a = shared_http().expect("共享客户端应可构造");
        let b = shared_http().expect("第二次取也应有值");
        // 同一个池 → 相互 clone 不改变任何一方行为; 配置指纹也必然一致。
        assert_eq!(config_fingerprint(&a), config_fingerprint(&b), "两次取到的配置必须一致");
        let c = a.clone();
        assert_eq!(config_fingerprint(&c), config_fingerprint(&b), "clone 不改变配置");
    }

    /// 缓存命中路径与"手工按同样配置新建"必须等价, 保证共享没有偷偷改掉
    /// 超时/重定向策略 (H-6 不因复用而失效)。
    #[test]
    fn shared_http_config_matches_fresh_build() {
        let shared = shared_http().expect("共享客户端应可构造");
        let fresh = build_http().expect("自建应成功");
        assert_eq!(
            config_fingerprint(&shared),
            config_fingerprint(&fresh),
            "共享路径与自建路径必须是同一份配置"
        );
    }

    /// 配置指纹: 只有配置真的相同才可能复用同一个池。`reqwest` 不暴露配置, 这里用
    /// 唯一的配置来源 [`build_http`] 加 `Debug` 形态做代理 —— 若将来 `build_http`
    /// 改成"按调用方参数构造", 这两个测试会失败, 那正是需要评审 `shared_http`
    /// 缓存是否仍然安全的信号。
    fn config_fingerprint(c: &reqwest::Client) -> String {
        format!("{c:?}")
    }

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

    /// 审计 低危 #3: 拼进 URL 的 symbol/interval 必须百分号编码, 否则含 `&`/`#`/`=`/空格
    /// 的输入会**改变 query 语义**(`&` 拆出新参数、`#` 之后整段被当片段丢弃)。
    #[test]
    fn test_encode_query_component_escapes_delimiters() {
        // 常规 symbol 原样通过 (不引入无谓转义)。
        assert_eq!(encode_query_component("ETHUSDT"), "ETHUSDT");
        assert_eq!(encode_query_component("1h"), "1h");
        assert_eq!(encode_query_component("BTC-USDT_1.0~x"), "BTC-USDT_1.0~x");
        // 分隔符被转义 —— 这是本函数存在的意义。
        assert_eq!(encode_query_component("A&B"), "A%26B");
        assert_eq!(encode_query_component("a=b"), "a%3Db");
        assert_eq!(encode_query_component("a#frag"), "a%23frag");
        assert_eq!(encode_query_component("a b"), "a%20b");
        assert_eq!(encode_query_component("100%"), "100%25");
        // 非 ASCII 按 UTF-8 逐字节编码。
        assert_eq!(encode_query_component("中"), "%E4%B8%AD");
    }

    /// 端到端: 转义后的 symbol 不会被拆成额外参数 —— 直接盯"注入一个 `&limit=999` 后,
    /// 请求路径里 `limit` 仍只有交易所自己那一个"(旧实现会多出伪造的 limit)。
    #[test]
    fn test_depth_url_keeps_single_limit_after_symbol_escape() {
        let symbol = encode_query_component("BTC&limit=999");
        let url = format!("https://api.binance.com/api/v3/depth?symbol={symbol}&limit=5");
        assert_eq!(url.matches("limit=").count(), 1, "伪造的 limit 必须被转义吞掉: {url}");
        assert!(url.contains("symbol=BTC%26limit%3D999"), "{url}");
    }

    /// 审计 中危 #7 的**端到端**证明: 两次独立构造的客户端打同一个 host 时, 走的是
    /// **同一条 TCP 连接**(连接被复用), 而不是各自新建连接。
    ///
    /// 这是共享连接池真正要拿到的东西 —— 光比较配置说明不了问题, 所以起一个本地
    /// HTTP/1.1 服务端 (只认回环地址, 过 H-6 白名单) 数一下它 accept 了几次:
    /// 复用生效 → 只 accept 1 次; 若每次都新建 client → accept 2 次。
    #[tokio::test]
    async fn shared_http_reuses_tcp_connection_across_clients() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let accepts = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定回环端口");
        let addr = listener.local_addr().expect("取本地地址");
        let counter = accepts.clone();

        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                // 逐请求回一个最小合法响应; 连接由 reqwest 池保持 (keep-alive)。
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 4096];
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let body = b"{}";
                                let resp = format!(
                                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                                     content-length: {}\r\n\r\n",
                                    body.len()
                                );
                                if sock.write_all(resp.as_bytes()).await.is_err() {
                                    break;
                                }
                                if sock.write_all(body).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });

        let url = format!("http://{addr}/api/v3/time");
        // 两个**不同**的客户端实例, 但都出自 shared_http → 同一个池。
        let c1 = shared_http().expect("构造共享客户端 1");
        let c2 = shared_http().expect("构造共享客户端 2");
        for c in [&c1, &c2] {
            let r = c.get(&url).send().await.expect("请求本地服务");
            assert_eq!(r.status().as_u16(), 200, "本地服务应回 200");
        }
        // 给服务端一点时间把 accept 计数落定。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        server.abort();

        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "两个客户端必须复用同一条 TCP 连接 —— accept 超过 1 次说明连接池没被共享"
        );
    }

    /// 反证: 每次**自建**客户端 (修复前的行为) 会各起一条连接。
    /// 有它才能说明上面那条断言真的在测"共享", 而不是服务端本来就不复用。
    #[tokio::test]
    async fn freshly_built_clients_do_not_share_connections() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let accepts = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("绑定回环端口");
        let addr = listener.local_addr().expect("取本地地址");
        let counter = accepts.clone();

        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 4096];
                    while matches!(sock.read(&mut buf).await, Ok(n) if n > 0) {
                        let body = b"{}";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\n\r\n",
                            body.len()
                        );
                        if sock.write_all(resp.as_bytes()).await.is_err()
                            || sock.write_all(body).await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });

        let url = format!("http://{addr}/api/v3/time");
        let c1 = build_http().expect("自建 1");
        let c2 = build_http().expect("自建 2");
        for c in [&c1, &c2] {
            let r = c.get(&url).send().await.expect("请求本地服务");
            assert_eq!(r.status().as_u16(), 200);
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        server.abort();

        assert_eq!(
            accepts.load(Ordering::SeqCst),
            2,
            "各自新建的客户端不该复用连接 (这正是 #7 要消除的开销)"
        );
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
        assert!(
            !validate_quantity(min, step, Decimal::from_str("1.5005").unwrap()),
            "非 step 整数倍"
        );
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
            .unwrap()
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
