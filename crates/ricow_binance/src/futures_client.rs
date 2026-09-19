//! Binance USDT-M 合约 (fapi) 签名交易客户端。
//!
//! 与现货 `client::BinanceClient` 平行: HMAC-SHA256 签名 REST 客户端, 但面向 fapi
//! (`/fapi/v1/order` `/fapi/v2/account` 等), 覆盖下单/撤单/账户/持仓/杠杆/保证金模式/双向持仓。
//! 仅公共行情数据源见 `futures_data` (K 线/MMR, 无密钥)。
//!
//! - 域名: `fapi.binance.com`; `RICOW_FAPI_BASE_URL` 环境变量可覆盖 → demo 平台 `https://demo-fapi.binance.com`
//!   (2026-09-04 实测 demo key 与旧 testnet.binance.vision 不互通, 勿混用, 见 specs/testnet.md)。
//! - 错误体与现货同构 `{-code, -msg}`, 复用 `check_bn_response` 解析。
//! - 2026-09-04 建 (testnet 联调计划 T4, specs/plans/2026-09-04_213220-bn-demo-live-test)。

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ricow_core::{
    Balance, CoreError, CoreResult, FundingIncome, Kline, Market, OrderBook, OrderInfo,
};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::client::{build_query_string, check_bn_response, parse_levels, sign_hmac_sha256};

/// USDT-M 合约主网 REST。
const FAPI_MAINNET_REST: &str = "https://fapi.binance.com";

/// 构建带超时的 reqwest 客户端。
fn build_http() -> CoreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CoreError::Network(e.to_string()))
}

/// Binance USDT-M 合约签名交易客户端。
///
/// `Clone` 仅为 WebSocket 侧需要(keepalive 任务持有独立副本发签名 PUT), 不涉及任何共享可变状态。
#[derive(Clone)]
pub struct FuturesClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    secret_key: Option<String>,
}

impl fmt::Debug for FuturesClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FuturesClient")
            .field("base_url", &self.base_url)
            .field("has_key", &self.api_key.is_some())
            .finish()
    }
}

impl FuturesClient {
    /// 主网无凭据客户端 (只读账户类端点不可用; 主要用于对称测试/未来扩展)。
    pub fn new() -> CoreResult<Self> {
        let http = build_http()?;
        // 域名可配置 (代理/测试环境): RICOW_FAPI_BASE_URL 覆盖, 缺省主网。
        let base_url =
            std::env::var("RICOW_FAPI_BASE_URL").unwrap_or_else(|_| FAPI_MAINNET_REST.to_string());
        Ok(Self { http, base_url, api_key: None, secret_key: None })
    }

    /// 带凭据构造 (测试网联调/实盘共用)。base_url 可显式传, None 时读 `RICOW_FAPI_BASE_URL` env。
    pub fn with_credentials(
        api_key: String,
        secret_key: String,
        base_url: Option<String>,
    ) -> CoreResult<Self> {
        let http = build_http()?;
        let base_url = base_url.unwrap_or_else(|| {
            std::env::var("RICOW_FAPI_BASE_URL").unwrap_or_else(|_| FAPI_MAINNET_REST.to_string())
        });
        Ok(Self { http, base_url, api_key: Some(api_key), secret_key: Some(secret_key) })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 显式指定 REST 域名 (覆盖 `RICOW_FAPI_BASE_URL`; demo/测试网用)。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn ensure_credentials(&self) -> CoreResult<(&str, &str)> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| CoreError::Auth("fapi api key not configured".into()))?;
        let secret_key = self
            .secret_key
            .as_deref()
            .ok_or_else(|| CoreError::Auth("fapi secret key not configured".into()))?;
        Ok((api_key, secret_key))
    }

    // ---- 公开数据 (免 key, 对称 futures_data) ----

    /// 最新标记价格与资金费率 (premiumIndex)。
    pub async fn get_premium_index(&self, symbol: &str) -> CoreResult<Value> {
        let url = format!("{}/fapi/v1/premiumIndex?symbol={symbol}", self.base_url);
        self.get_json(&url).await
    }

    /// 合约交易对列表 (fapi exchangeInfo): 过滤 PERPETUAL + TRADING, 解析 LOT_SIZE/PRICE_FILTER/MIN_NOTIONAL。
    pub async fn get_exchange_info(&self) -> CoreResult<Vec<Market>> {
        let url = format!("{}/fapi/v1/exchangeInfo", self.base_url);
        let raw: Value = self.get_json(&url).await?;
        let symbols =
            raw["symbols"].as_array().ok_or_else(|| CoreError::Parse("missing symbols".into()))?;

        let markets: Vec<Market> = symbols.iter().filter_map(parse_futures_symbol).collect();

        Ok(markets)
    }

    /// 交易所服务器时间 (`GET /fapi/v1/time`) —— 合约实盘时钟预检的数据源。
    ///
    /// 必须用**合约自己的**时间: 实测现货/合约 demo 服务器时间相差 1.5~1.9s, 用现货口径校合约会白做。
    pub async fn server_time(&self) -> CoreResult<DateTime<Utc>> {
        let url = format!("{}/fapi/v1/time", self.base_url);
        let v: Value = self.get_json(&url).await?;
        let ms = v["serverTime"]
            .as_i64()
            .ok_or_else(|| CoreError::Parse("missing serverTime".into()))?;
        DateTime::from_timestamp_millis(ms)
            .ok_or_else(|| CoreError::Parse(format!("invalid serverTime: {ms}")))
    }

    /// 合约 K 线 (`GET /fapi/v1/klines`): 与现货同构解析, 复用通用分页实现。
    pub async fn get_klines(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> CoreResult<Vec<Kline>> {
        crate::client::fetch_klines_paged(
            &self.http,
            &self.base_url,
            "/fapi/v1/klines",
            symbol,
            interval,
            limit,
            None,
        )
        .await
    }

    /// 合约盘口快照 (`GET /fapi/v1/depth`)。
    /// 截止到 `end_ms` 的 K 线 (回测按自然年月分段用)。
    pub async fn get_klines_ending_at(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
        end_ms: i64,
    ) -> CoreResult<Vec<Kline>> {
        crate::client::fetch_klines_paged(
            &self.http,
            &self.base_url,
            "/fapi/v1/klines",
            symbol,
            interval,
            limit,
            Some(end_ms),
        )
        .await
    }

    pub async fn get_depth(&self, symbol: &str, limit: u32) -> CoreResult<OrderBook> {
        let url = format!(
            "{}/fapi/v1/depth?symbol={}&limit={limit}",
            self.base_url,
            symbol.to_uppercase()
        );
        let v: Value = self.get_json(&url).await?;
        Ok(OrderBook {
            bids: parse_levels(v["bids"].as_array()),
            asks: parse_levels(v["asks"].as_array()),
            timestamp: Utc::now(),
        })
    }

    /// listenKey 生命周期 (`POST/PUT/DELETE /fapi/v1/listenKey`) —— 合约用户数据流入口。
    ///
    /// 现货的 legacy listenKey 已于 2026-02-20 被币安下线(改走 WS-API), 但**合约 listenKey 仍在服务**
    /// (2026-09-13 实测 200); 有效期 60 分钟, 30 分钟续期一次。
    pub async fn create_listen_key(&self) -> CoreResult<String> {
        let v = self.signed_post("/fapi/v1/listenKey", &[]).await?;
        v["listenKey"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| CoreError::Parse("missing listenKey".into()))
    }

    /// 续期 listenKey (建议每 30 分钟)。
    pub async fn keepalive_listen_key(&self, listen_key: &str) -> CoreResult<()> {
        let params = vec![("listenKey".to_string(), listen_key.to_string())];
        self.signed_put("/fapi/v1/listenKey", &params).await?;
        Ok(())
    }

    /// 关闭 listenKey (停机清理, 失败不影响主流程)。
    pub async fn close_listen_key(&self, listen_key: &str) -> CoreResult<()> {
        let params = vec![("listenKey".to_string(), listen_key.to_string())];
        self.signed_delete("/fapi/v1/listenKey", &params).await?;
        Ok(())
    }

    /// 按资产查余额 (`GET /fapi/v2/balance`): 缺失资产返回 0 (不报错)。
    pub async fn balance_of(&self, asset: &str) -> CoreResult<Balance> {
        let v = self.signed_get("/fapi/v2/balance", &[]).await?;
        let target = asset.to_uppercase();
        let found = v.as_array().and_then(|arr| {
            arr.iter().find(|b| {
                b["asset"].as_str().map(|a| a.eq_ignore_ascii_case(&target)).unwrap_or(false)
            })
        });
        match found {
            Some(b) => Ok(Balance {
                asset: target,
                free: Decimal::from_str(b["availableBalance"].as_str().unwrap_or("0"))
                    .unwrap_or_default(),
                locked: Decimal::from_str(b["balance"].as_str().unwrap_or("0")).unwrap_or_default(),
            }),
            None => Ok(Balance { asset: target, free: Decimal::ZERO, locked: Decimal::ZERO }),
        }
    }

    /// 资金费/收益流水 (`GET /fapi/v1/income`): 014 FR-001 —— 实盘资金费以**交易所账单**为准(不自算)。
    ///
    /// `income_type` 常用 `FUNDING_FEE`; `start_ms` = 增量拉取水位(毫秒); `limit` 上限 1000。
    pub async fn income(
        &self,
        income_type: &str,
        start_ms: i64,
        limit: u32,
    ) -> CoreResult<Vec<FundingIncome>> {
        let params = vec![
            ("incomeType".to_string(), income_type.to_string()),
            ("startTime".to_string(), start_ms.to_string()),
            ("limit".to_string(), limit.to_string()),
        ];
        let v = self.signed_get("/fapi/v1/income", &params).await?;
        Ok(parse_income(&v))
    }

    // ---- 账户 / 持仓 (签名 GET) ----

    /// 合约账户 (fapi/v2/account): 返回 JSON, 含 availableBalance/positions 等原始结构。
    pub async fn get_account(&self) -> CoreResult<Value> {
        self.signed_get("/fapi/v2/account", &[]).await
    }

    /// 可用余额 (availableBalance, USDT 计价口径; 从 account JSON 提取, 便于调用方断言)。
    pub async fn available_balance(&self) -> CoreResult<Decimal> {
        let v = self.get_account().await?;
        v["availableBalance"]
            .as_str()
            .and_then(|s| Decimal::from_str(s).ok())
            .ok_or_else(|| CoreError::Parse("missing availableBalance".into()))
    }

    /// 某 symbol 持仓 (positionRisk): 返回该 symbol 全部 positionSide 条目 (含空仓 amount=0)。
    pub async fn get_positions(&self, symbol: &str) -> CoreResult<Value> {
        let params = vec![("symbol".to_string(), symbol.to_uppercase())];
        self.signed_get("/fapi/v2/positionRisk", &params).await
    }

    /// 提取 positionRisk 数组中 amount 非零的首条持仓 (无则 None)。
    pub fn extract_position(
        &self,
        raw: &Value,
        position_side: &str,
    ) -> Option<serde_json::Map<String, Value>> {
        raw.as_array()?.iter().find_map(|p| {
            let amt = p["positionAmt"].as_str().unwrap_or("0");
            let amt: f64 = amt.parse().ok()?;
            if amt.abs() > 1e-12 && p["positionSide"].as_str() == Some(position_side) {
                p.as_object().cloned()
            } else {
                None
            }
        })
    }

    // ---- 写操作 (签名 POST/DELETE) ----

    /// 下单。position_side: Some("LONG"/"SHORT") = 双向持仓模式; None = one-way (BOTH)。
    /// reduce_only=true 时平仓单。price=None = 市价单 (无 timeInForce)。
    /// client_order_id: 调用方指定的订单号 (幂等键 + 停机撤单归属判定); None/空串时用 `ricow-f-{毫秒}` 兜底。
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &self,
        symbol: &str,
        side: &str,
        order_type: &str,
        quantity: &str,
        price: Option<&str>,
        position_side: Option<&str>,
        reduce_only: bool,
        client_order_id: Option<&str>,
    ) -> CoreResult<Value> {
        let params = build_order_params(
            symbol,
            side,
            order_type,
            quantity,
            price,
            position_side,
            reduce_only,
            client_order_id,
            Utc::now().timestamp_millis(),
        );
        self.signed_post("/fapi/v1/order", &params).await
    }

    /// 撤单 (按 clientOrderId; 返回被撤订单 JSON)。
    pub async fn cancel_order(&self, symbol: &str, client_order_id: &str) -> CoreResult<Value> {
        let params = vec![
            ("symbol".into(), symbol.to_uppercase()),
            ("origClientOrderId".into(), client_order_id.to_string()),
        ];
        self.signed_delete("/fapi/v1/order", &params).await
    }

    /// 查询未成交挂单 (`GET /fapi/v1/openOrders`, 按 symbol)。
    /// 响应结构与现货同构, 复用 `spot::parse_open_orders`。
    pub async fn get_open_orders(&self, symbol: &str) -> CoreResult<Vec<OrderInfo>> {
        let params = vec![("symbol".into(), symbol.to_uppercase())];
        let resp = self.signed_get("/fapi/v1/openOrders", &params).await?;
        Ok(crate::spot::parse_open_orders(&resp, symbol))
    }

    /// 设置杠杆 (1-125, 按 symbol; 返回 {"leverage": N})。
    pub async fn set_leverage(&self, symbol: &str, leverage: u32) -> CoreResult<Value> {
        let params = vec![
            ("symbol".into(), symbol.to_uppercase()),
            ("leverage".into(), leverage.to_string()),
        ];
        self.signed_post("/fapi/v1/leverage", &params).await
    }

    /// 设置保证金模式: is_isolated=true → ISOLATED (逐仓), false → CROSSED (全仓)。
    pub async fn set_margin_type(&self, symbol: &str, is_isolated: bool) -> CoreResult<Value> {
        let params = vec![
            ("symbol".into(), symbol.to_uppercase()),
            (
                "marginType".into(),
                if is_isolated { "ISOLATED".to_string() } else { "CROSSED".to_string() },
            ),
        ];
        self.signed_post("/fapi/v1/marginType", &params).await
    }

    /// 双向持仓模式: dual=true → 同对可同时 LONG+SHORT; false → one-way (BOTH)。
    /// 账户级设置, 需先无持仓/无挂单。
    pub async fn set_position_side_dual(&self, dual: bool) -> CoreResult<Value> {
        let params = vec![(
            "dualSidePosition".into(),
            if dual { "true".to_string() } else { "false".to_string() },
        )];
        self.signed_post("/fapi/v1/positionSide/dual", &params).await
    }

    /// 调整逐仓保证金 (POST /fapi/v1/positionMargin)。
    /// `amount`: 正数增加; `reduce=true` 时 amount 为要从仓位移出的金额 (type=2 减少)。
    /// 减少保证金会拉高强平价; 减到低于维持保证金时交易所拒绝或触发强平。
    pub async fn adjust_position_margin(
        &self,
        symbol: &str,
        amount: &str,
        reduce: bool,
        position_side: Option<&str>,
    ) -> CoreResult<Value> {
        let mut params: Vec<(String, String)> = vec![
            ("symbol".into(), symbol.to_uppercase()),
            ("amount".into(), amount.to_string()),
            ("type".into(), if reduce { "2".into() } else { "1".into() }),
        ];
        if let Some(ps) = position_side {
            params.push(("positionSide".into(), ps.to_uppercase()));
        }
        self.signed_post("/fapi/v1/positionMargin", &params).await
    }

    /// 查询当前持仓模式: true = 双向 (hedge), false = one-way。
    pub async fn position_side_dual(&self) -> CoreResult<bool> {
        let v = self.signed_get("/fapi/v1/positionSide/dual", &[]).await?;
        v["dualSidePosition"]
            .as_bool()
            .ok_or_else(|| CoreError::Parse("missing dualSidePosition".into()))
    }

    // ---- HTTP helpers (签名) ----

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> CoreResult<T> {
        let resp =
            self.http.get(url).send().await.map_err(|e| CoreError::Network(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| CoreError::Network(e.to_string()))?;
        if !status.is_success() {
            return Err(CoreError::Exchange(format!("fapi {status}: {text}")));
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

    async fn signed_put(&self, path: &str, params: &[(String, String)]) -> CoreResult<Value> {
        let mut p = params.to_vec();
        self.add_timestamp_and_sign(&mut p)?;
        let qs = build_query_string(&p);
        let url = format!("{}{}?{}", self.base_url, path, qs);
        let (api_key, _) = self.ensure_credentials()?;
        let resp = self
            .http
            .put(&url)
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

impl Default for FuturesClient {
    fn default() -> Self {
        Self::new().expect("FuturesClient::new")
    }
}

/// `GET /fapi/v1/income` 响应 → 资金费记录 (纯函数, 便于单测)。
///
/// 账目字段(`tranId` / `income` / `time`)缺失或格式异常的行**整行跳过** —— 不伪造 0 值。
pub fn parse_income(v: &Value) -> Vec<FundingIncome> {
    let arr = match v.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    arr.iter()
        .filter_map(|row| {
            let tran_id = row["tranId"].as_i64()?.to_string();
            let income = Decimal::from_str(row["income"].as_str()?).ok()?;
            Some(FundingIncome {
                symbol: row["symbol"].as_str().unwrap_or("").to_string(),
                income,
                asset: row["asset"].as_str().unwrap_or("").to_string(),
                time_ms: row["time"].as_i64()?,
                tran_id,
            })
        })
        .collect()
}

/// 解析 fapi exchangeInfo 的单个 symbol (纯函数, 便于单测): 仅取 PERPETUAL + TRADING。
///
/// 过滤器: LOT_SIZE(minQty/stepSize) / PRICE_FILTER(tickSize) / MIN_NOTIONAL(minNotional)。
pub fn parse_futures_symbol(s: &Value) -> Option<Market> {
    if s["status"].as_str()? != "TRADING" {
        return None;
    }
    if s["contractType"].as_str()? != "PERPETUAL" {
        return None;
    }
    let symbol = s["symbol"].as_str()?;
    let base = s["baseAsset"].as_str()?;
    let quote = s["quoteAsset"].as_str()?;
    let min_size = crate::client::find_filter_value(s, "LOT_SIZE", "minQty")
        .and_then(|v| Decimal::from_str(v).ok())
        .unwrap_or(Decimal::ONE);
    let tick_size = crate::client::find_filter_value(s, "PRICE_FILTER", "tickSize")
        .and_then(|v| Decimal::from_str(v).ok())
        .unwrap_or(Decimal::ONE);
    let step_size = crate::client::find_filter_value(s, "LOT_SIZE", "stepSize")
        .and_then(|v| Decimal::from_str(v).ok());
    // fapi 的 MIN_NOTIONAL 过滤器字段名为 notional (现货是 minNotional); 两者都试, 兼容不同环境。
    let min_notional = crate::client::find_filter_value(s, "MIN_NOTIONAL", "notional")
        .or_else(|| crate::client::find_filter_value(s, "MIN_NOTIONAL", "minNotional"))
        .and_then(|v| Decimal::from_str(v).ok())
        .filter(|d| !d.is_zero());

    Some(Market {
        symbol: symbol.to_string(),
        base_asset: base.to_string(),
        quote_asset: quote.to_string(),
        is_perpetual: true,
        min_size,
        tick_size,
        step_size,
        min_notional,
        max_leverage: None,
        margin_mode: None,
        is_delisted: false,
    })
}

/// 构造 fapi 下单参数 (纯函数, 便于单测)。
///
/// `client_order_id` 非空 → 原样使用 (幂等键 + 停机撤单归属判定依赖它);
/// None/空串 → 用 `ricow-f-{now_ms}` 兜底 (与旧行为一致, 仍有框架前缀可识别)。
#[allow(clippy::too_many_arguments)]
pub fn build_order_params(
    symbol: &str,
    side: &str,
    order_type: &str,
    quantity: &str,
    price: Option<&str>,
    position_side: Option<&str>,
    reduce_only: bool,
    client_order_id: Option<&str>,
    now_ms: i64,
) -> Vec<(String, String)> {
    let cid = match client_order_id {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => format!("ricow-f-{now_ms}"),
    };
    let mut params: Vec<(String, String)> = vec![
        ("symbol".into(), symbol.to_uppercase()),
        ("side".into(), side.to_uppercase()),
        ("type".into(), order_type.to_uppercase()),
        ("quantity".into(), quantity.to_string()),
        ("newClientOrderId".into(), cid),
    ];
    if let Some(p) = price {
        params.push(("price".into(), p.to_string()));
        params.push(("timeInForce".into(), "GTC".into()));
    }
    if let Some(ps) = position_side {
        params.push(("positionSide".into(), ps.to_uppercase()));
    }
    if reduce_only {
        params.push(("reduceOnly".into(), "true".into()));
    }
    params
}

/// 解析 fapi 错误体 `{"code": -2019, "msg": "..."}` (check_bn_response 已处理 HTTP 层,
/// 此函数供需要 code 级判断的调用方使用; 保持与现货一致的错误传播即可)。
#[allow(dead_code)]
pub(crate) fn fapi_error_code(v: &Value) -> Option<i64> {
    v.get("code").and_then(|c| c.as_i64())
}

/// 从 account JSON 提取 USDT 计价可用余额 (多个口径: availableBalance 为可开仓余额)。
pub fn parse_available_balance(account: &Value) -> Option<Decimal> {
    account["availableBalance"].as_str().and_then(|s| Decimal::from_str(s).ok())
}

/// 从 positionRisk JSON 数组提取某侧持仓量 (positionAmt, 空仓=0)。
/// 注意: one-way 模式 positionSide=BOTH (实测 demo-fapi 2026-09-04); 双向模式才是 LONG/SHORT。
pub fn position_amount(raw: &Value, position_side: &str) -> Decimal {
    raw.as_array()
        .and_then(|arr| arr.iter().find(|p| p["positionSide"].as_str() == Some(position_side)))
        .and_then(|p| p["positionAmt"].as_str())
        .and_then(|s| Decimal::from_str(s).ok())
        .unwrap_or(Decimal::ZERO)
}

/// 从 positionRisk JSON 数组提取某侧强平价 (liquidationPrice, 无仓/无值时 None)。
pub fn position_liquidation_price(raw: &Value, position_side: &str) -> Option<Decimal> {
    raw.as_array()
        .and_then(|arr| arr.iter().find(|p| p["positionSide"].as_str() == Some(position_side)))?
        .get("liquidationPrice")?
        .as_str()
        .and_then(|s| Decimal::from_str(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn client_with_fake_keys() -> FuturesClient {
        // 仅拼装/解析测试, 不发网络请求; base_url 用不存在的域。
        FuturesClient::with_credentials(
            "fake-api-key".into(),
            "fake-secret".into(),
            Some("https://127.0.0.1:1".into()),
        )
        .unwrap()
    }

    #[test]
    fn test_extract_position_finds_nonzero_side() {
        let raw: Value = serde_json::json!([
            { "symbol": "BTCUSDT", "positionSide": "LONG", "positionAmt": "0.000", "entryPrice": "0.0" },
            { "symbol": "BTCUSDT", "positionSide": "SHORT", "positionAmt": "0.001", "entryPrice": "80000.0", "liquidationPrice": "80500.0" }
        ]);
        let c = client_with_fake_keys();
        let p = c.extract_position(&raw, "SHORT").expect("SHORT 仓应存在");
        assert_eq!(p["positionAmt"].as_str(), Some("0.001"));
        // LONG 空仓不应命中
        assert!(c.extract_position(&raw, "LONG").is_none());
    }

    #[test]
    fn test_parse_available_balance_ok() {
        let v: Value = serde_json::json!({ "availableBalance": "123.45" });
        assert_eq!(parse_available_balance(&v).unwrap(), Decimal::from_str("123.45").unwrap());
        let empty: Value = serde_json::json!({});
        assert!(parse_available_balance(&empty).is_none());
    }

    #[test]
    fn test_position_amount_and_liq_price() {
        let raw: Value = serde_json::json!([
            { "positionSide": "LONG", "positionAmt": "0.000", "liquidationPrice": "0" },
            { "positionSide": "SHORT", "positionAmt": "-0.002", "liquidationPrice": "82000.5" }
        ]);
        assert_eq!(position_amount(&raw, "LONG"), Decimal::ZERO);
        assert_eq!(position_amount(&raw, "SHORT"), Decimal::from_str("-0.002").unwrap());
        assert_eq!(
            position_liquidation_price(&raw, "SHORT"),
            Some(Decimal::from_str("82000.5").unwrap())
        );
        assert_eq!(position_liquidation_price(&raw, "LONG"), Some(Decimal::ZERO));
    }

    #[test]
    fn test_place_order_params_shape_via_signed_path_offline() {
        // 无法离线断言签名 URL (timestamp 动态), 此测试仅确保构造不 panic + 参数 helper 存在;
        // 真实参数正确性由 T5 demo 联调覆盖。
        let c = client_with_fake_keys();
        assert!(c.api_key.is_some());
        assert!(c.secret_key.is_some());
        assert_eq!(c.base_url(), "https://127.0.0.1:1");
    }

    #[test]
    fn test_new_reads_env_base_url() {
        std::env::set_var("RICOW_FAPI_BASE_URL", "https://demo-fapi.binance.com");
        let c = FuturesClient::new().unwrap();
        assert_eq!(c.base_url(), "https://demo-fapi.binance.com");
        std::env::remove_var("RICOW_FAPI_BASE_URL");
        let c2 = FuturesClient::new().unwrap();
        assert_eq!(c2.base_url(), "https://fapi.binance.com");
    }

    #[test]
    fn test_parse_income_rows_and_skips_bad() {
        let v = serde_json::json!([
            {"symbol": "ETHUSDT", "incomeType": "FUNDING_FEE", "income": "-0.12345678",
             "asset": "USDT", "time": 1789290000000i64, "tranId": 303904994i64},
            {"symbol": "ETHUSDT", "incomeType": "FUNDING_FEE", "income": "0.05",
             "asset": "USDT", "time": 1789318800000i64, "tranId": 303904995i64},
            {"symbol": "ETHUSDT", "income": "not-a-number", "time": 1, "tranId": 1},
            {"symbol": "ETHUSDT", "income": "0.01", "time": 1},
        ]);
        let got = parse_income(&v);
        assert_eq!(got.len(), 2, "格式异常/缺 tranId 的行整行跳过");
        assert_eq!(got[0].tran_id, "303904994");
        assert_eq!(got[0].income, dec!(-0.12345678), "精确小数不丢精度");
        assert_eq!(got[0].asset, "USDT");
        assert_eq!(got[1].income, dec!(0.05));
        // 空数组与非数组响应 → 空 (不 panic)
        assert!(parse_income(&serde_json::json!([])).is_empty());
        assert!(parse_income(&serde_json::json!({"code": -1021})).is_empty());
    }

    #[test]
    fn test_parse_futures_symbol_filters() {
        // 样例取自 fapi exchangeInfo 结构 (2026-09 实测: MIN_NOTIONAL 字段名为 notional)。
        let s = serde_json::json!({
            "symbol": "ETHUSDT",
            "status": "TRADING",
            "contractType": "PERPETUAL",
            "baseAsset": "ETH",
            "quoteAsset": "USDT",
            "filters": [
                {"filterType": "PRICE_FILTER", "tickSize": "0.01000000"},
                {"filterType": "LOT_SIZE", "minQty": "0.00100000", "stepSize": "0.00100000"},
                {"filterType": "MIN_NOTIONAL", "notional": "50"}
            ]
        });
        let m = parse_futures_symbol(&s).expect("应解析出 Market");
        assert_eq!(m.symbol, "ETHUSDT");
        assert!(m.is_perpetual);
        assert_eq!(m.min_size, Decimal::from_str("0.001").unwrap());
        assert_eq!(m.tick_size, Decimal::from_str("0.01").unwrap());
        assert_eq!(m.step_size, Some(Decimal::from_str("0.001").unwrap()));
        assert_eq!(m.min_notional, Some(Decimal::from(50)));

        // 非 PERPETUAL / 非 TRADING → None
        let delivery = serde_json::json!({
            "symbol": "X", "status": "TRADING", "contractType": "CURRENT_QUARTER",
            "baseAsset": "X", "quoteAsset": "USDT", "filters": []
        });
        assert!(parse_futures_symbol(&delivery).is_none());
        let halted = serde_json::json!({
            "symbol": "X", "status": "SETTLING", "contractType": "PERPETUAL",
            "baseAsset": "X", "quoteAsset": "USDT", "filters": []
        });
        assert!(parse_futures_symbol(&halted).is_none());
    }

    #[test]
    fn test_parse_futures_symbol_min_notional_fallback_field() {
        // 兼容字段名 minNotional 的环境; 缺过滤器 → None (不拦截)。
        let with_min_notional = serde_json::json!({
            "symbol": "BTCUSDT", "status": "TRADING", "contractType": "PERPETUAL",
            "baseAsset": "BTC", "quoteAsset": "USDT",
            "filters": [
                {"filterType": "MIN_NOTIONAL", "minNotional": "100"}
            ]
        });
        assert_eq!(
            parse_futures_symbol(&with_min_notional).unwrap().min_notional,
            Some(Decimal::from(100))
        );

        let none = serde_json::json!({
            "symbol": "BTCUSDT", "status": "TRADING", "contractType": "PERPETUAL",
            "baseAsset": "BTC", "quoteAsset": "USDT", "filters": []
        });
        assert_eq!(parse_futures_symbol(&none).unwrap().min_notional, None);
    }

    #[test]
    fn test_build_order_params_uses_caller_client_order_id() {
        // F1 修复判据: 传入的 client_order_id 必须出现在 newClientOrderId (旧版硬编码丢弃)。
        let params = build_order_params(
            "ethusdt",
            "buy",
            "limit",
            "0.05",
            Some("3000.1"),
            Some("long"),
            true,
            Some("grid-eth-0001"),
            1_700_000_000_000,
        );
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("newClientOrderId").as_deref(), Some("grid-eth-0001"));
        assert_eq!(get("symbol").as_deref(), Some("ETHUSDT"), "symbol 应大写");
        assert_eq!(get("side").as_deref(), Some("BUY"));
        assert_eq!(get("type").as_deref(), Some("LIMIT"));
        assert_eq!(get("price").as_deref(), Some("3000.1"));
        assert_eq!(get("timeInForce").as_deref(), Some("GTC"));
        assert_eq!(get("positionSide").as_deref(), Some("LONG"));
        assert_eq!(get("reduceOnly").as_deref(), Some("true"));
    }

    #[test]
    fn test_build_order_params_fallback_id_and_market_shape() {
        // 缺省 (None 或空串) → 生成含框架前缀的 id; 市价单无 price/timeInForce/positionSide/reduceOnly。
        let p = build_order_params("ETHUSDT", "SELL", "MARKET", "1", None, None, false, None, 42);
        let get = |k: &str| p.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("newClientOrderId").as_deref(), Some("ricow-f-42"));
        assert!(get("price").is_none());
        assert!(get("timeInForce").is_none());
        assert!(get("positionSide").is_none());
        assert!(get("reduceOnly").is_none());

        let empty =
            build_order_params("ETHUSDT", "SELL", "MARKET", "1", None, None, false, Some(""), 7);
        let cid = empty.iter().find(|(n, _)| n == "newClientOrderId").unwrap();
        assert_eq!(cid.1, "ricow-f-7", "空串按缺省处理");
    }
}
