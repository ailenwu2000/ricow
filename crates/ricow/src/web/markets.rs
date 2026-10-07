//! 032 US2 市场浏览端点 (FR-010 ~ FR-014):
//! - `GET /api/markets` — 现货/合约交易对视野 (复用 CLI 同一份 `pairs::current_view` + `filter_view`);
//! - `GET /api/markets/{symbol}/orderbook` — 实时订单簿 (免 key 公共行情);
//! - `GET /api/markets/{symbol}/klines` — K 线 (免 key 公共客户端, 现货 / 合约各一套)。
//!
//! 与 CLI 同源: 视野组装 / 过滤逻辑一行不复制; Decimal 走工作区 `serde-str`,
//! 直序 `ricow_core::{OrderBook,Kline}` 即 JSON 字符串金额 (与 trades 端点同口径, 防 JS 精度损失)。

use axum::extract::{rejection::QueryRejection, Path as UrlPath, Query, State};
use axum::Json;
use ricow_core::{Exchange, Kline, OrderBook};
use ricow_engine::PairsView;
use serde::Deserialize;

use super::{WebError, WebState};

/// 订单簿缺省档位数 (契约: depth 缺省 20)。
const DEFAULT_DEPTH: u32 = 20;
/// 订单簿档位上下限 (1..=50)。
const DEPTH_MIN: u32 = 1;
const DEPTH_MAX: u32 = 50;
/// K 线缺省根数与上下限 (1..=500)。
const DEFAULT_LIMIT: u32 = 200;
const LIMIT_MIN: u32 = 1;
const LIMIT_MAX: u32 = 500;
/// K 线周期白名单 (缺省 1h)。
const INTERVALS: [&str; 6] = ["1m", "5m", "15m", "1h", "4h", "1d"];

// ---- 查询参数原始形态 (全部接字符串: 非法值要回中文 400, 不用 serde 默认英文拒绝体) ----

/// `GET /api/markets` 的查询参数。
#[derive(Debug, Deserialize)]
pub(super) struct MarketsQuery {
    /// `0`(缺省)= 当前配置视野; `1` = 本次全量 (不写盘); 其余 → 400。
    all: Option<String>,
    /// 代码子串 (大小写不敏感, 交给 `filter_view`)。
    q: Option<String>,
    /// 市场限定: spot / futures; 缺省 = 两组都返回; 非法 → 400。
    market: Option<String>,
}

/// `GET .../orderbook` 的查询参数。
#[derive(Debug, Deserialize)]
pub(super) struct OrderBookQuery {
    market: Option<String>,
    depth: Option<String>,
}

/// `GET .../klines` 的查询参数。
#[derive(Debug, Deserialize)]
pub(super) struct KlinesQuery {
    market: Option<String>,
    interval: Option<String>,
    limit: Option<String>,
}

// ---- 参数解析 (纯函数: 不联网 / 不读盘, 单测覆盖合法/缺省/非法各类) ----

/// `all=0|1` → bool; 缺省/空串按 `0`; 其余取值硬失败 (不静默当缺省)。
fn parse_all(v: Option<&str>) -> Result<bool, String> {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(format!("all 只支持 0/1, 收到: {other}")),
    }
}

/// 归一化市场限定: 容忍大小写与前后空白 (与 `filter_view` 同口径);
/// 040: `pub(super)` 供 `realtime.rs` 的流端点复用 —— 同一份校验不复制第二份。
/// `required` = 明细端点用, 缺省即硬失败。列表端点缺省 = 两组都返回 (`None`)。
pub(super) fn parse_market(
    v: Option<&str>,
    required: bool,
) -> Result<Option<&'static str>, String> {
    // 比较走小写归一化, 错误回显用户原文(trim 后) —— 报错要能对得上自己发出去的值。
    let Some(raw) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return if required {
            Err("market 必填, 仅支持 spot / futures".to_string())
        } else {
            Ok(None)
        };
    };
    match raw.to_ascii_lowercase().as_str() {
        "spot" => Ok(Some("spot")),
        "futures" => Ok(Some("futures")),
        _ => Err(format!("market 只支持 spot / futures, 收到: {raw}")),
    }
}

/// 订单簿档位: 缺省 20, 夹取前先硬校验 1..=50 (越界 400, 不静默夹取)。
fn parse_depth(v: Option<&str>) -> Result<u32, String> {
    let Some(s) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_DEPTH);
    };
    let n: u32 = s
        .parse()
        .map_err(|_| format!("depth 必须是 {DEPTH_MIN}..={DEPTH_MAX} 的整数, 收到: {s}"))?;
    if !(DEPTH_MIN..=DEPTH_MAX).contains(&n) {
        return Err(format!("depth 超出范围 {DEPTH_MIN}..={DEPTH_MAX}, 收到: {n}"));
    }
    Ok(n)
}

/// K 线周期: 缺省 1h, 白名单外硬失败。
pub(super) fn parse_interval(v: Option<&str>) -> Result<&'static str, String> {
    match v.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok("1h"),
        Some(s) => INTERVALS
            .iter()
            .copied()
            .find(|iv| *iv == s)
            .ok_or_else(|| format!("interval 仅支持 {} , 收到: {s}", INTERVALS.join("/"))),
    }
}

/// K 线根数: 缺省 200, 范围 1..=500 (越界 400)。
fn parse_limit(v: Option<&str>) -> Result<u32, String> {
    let Some(s) = v.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_LIMIT);
    };
    let n: u32 = s
        .parse()
        .map_err(|_| format!("limit 必须是 {LIMIT_MIN}..={LIMIT_MAX} 的整数, 收到: {s}"))?;
    if !(LIMIT_MIN..=LIMIT_MAX).contains(&n) {
        return Err(format!("limit 超出范围 {LIMIT_MIN}..={LIMIT_MAX}, 收到: {n}"));
    }
    Ok(n)
}

/// 交易对符号规整: 去空白并转大写 (币安符号一律大写; 不让空白混进取数路径)。
pub(super) fn normalize_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

// ---- handler ----

/// `GET /api/markets`: 视野 (600s 缓存) + 市场/子串过滤, 直序 [`PairsView`]。
///
/// 参数先全量校验 (非法 market/all 在联网取视野之前就 400); `all=1` 仅本次强制全量,
/// 不写配置 —— 持久化切换走 US1 的 `POST /api/config/keys`。
pub(super) async fn list_markets(
    State(state): State<WebState>,
    // 手动接 Result: 查询串解析失败也要回统一中文 WebError (同 keys.rs 的 JSON 处理)。
    query: Result<Query<MarketsQuery>, QueryRejection>,
) -> Result<Json<PairsView>, WebError> {
    let Query(q) = query.map_err(|e| WebError::bad_request(format!("查询参数非法: {e}")))?;
    let force_all = parse_all(q.all.as_deref()).map_err(WebError::bad_request)?;
    let market = parse_market(q.market.as_deref(), false).map_err(WebError::bad_request)?;
    let view = crate::commands::pairs::current_view(&state.root, force_all).await?;
    Ok(Json(ricow_engine::filter_view(&view, market, q.q.as_deref())))
}

/// `GET /api/markets/{symbol}/orderbook?market=spot|futures&depth=1..50`:
/// 直序 [`OrderBook`] (bids 降序 / asks 升序已由交易所客户端保证)。
pub(super) async fn get_orderbook(
    UrlPath(symbol): UrlPath<String>,
    query: Result<Query<OrderBookQuery>, QueryRejection>,
) -> Result<Json<OrderBook>, WebError> {
    let Query(q) = query.map_err(|e| WebError::bad_request(format!("查询参数非法: {e}")))?;
    // 参数全在联网之前校验 (400 不触发任何交易所请求)。
    let market = parse_market(q.market.as_deref(), true).map_err(WebError::bad_request)?;
    let depth = parse_depth(q.depth.as_deref()).map_err(WebError::bad_request)?;
    let symbol = normalize_symbol(&symbol);
    let label = market_label(market);
    let book = fetch_orderbook(market.unwrap_or("spot"), &symbol, depth)
        .await
        .map_err(|e| WebError::bad_gateway(format!("获取{label}订单簿失败({symbol}): {e}")))?;
    Ok(Json(book))
}

/// `GET /api/markets/{symbol}/klines?market=&interval=&limit=`: 直序 `Vec<Kline>`。
pub(super) async fn get_klines(
    UrlPath(symbol): UrlPath<String>,
    query: Result<Query<KlinesQuery>, QueryRejection>,
) -> Result<Json<Vec<Kline>>, WebError> {
    let Query(q) = query.map_err(|e| WebError::bad_request(format!("查询参数非法: {e}")))?;
    let market = parse_market(q.market.as_deref(), true).map_err(WebError::bad_request)?;
    let interval = parse_interval(q.interval.as_deref()).map_err(WebError::bad_request)?;
    let limit = parse_limit(q.limit.as_deref()).map_err(WebError::bad_request)?;
    let symbol = normalize_symbol(&symbol);
    let label = market_label(market);
    let rows =
        fetch_klines(market.unwrap_or("spot"), &symbol, interval, limit).await.map_err(|e| {
            WebError::bad_gateway(format!("获取{label}K 线失败({symbol} {interval}): {e}"))
        })?;
    Ok(Json(rows))
}

/// 错误文案里的市场中文名 (market 已过白名单)。
fn market_label(market: Option<&'static str>) -> &'static str {
    match market {
        Some("futures") => "合约",
        _ => "现货",
    }
}

/// 按市场取订单簿: 全部走免 key 公共行情。
///
/// - 现货: 与 CLI 同一构造路径 `commands::bn_exchange()` (BnSpotExchange);
/// - 合约: 无凭据 `FuturesClient` + `BnFuturesExchange` (公共 depth 端点不需要 key)。
async fn fetch_orderbook(
    market: &str,
    symbol: &str,
    depth: u32,
) -> ricow_core::CoreResult<OrderBook> {
    if market == "futures" {
        let ex = ricow_binance::BnFuturesExchange::new(ricow_binance::FuturesClient::new()?);
        ex.get_orderbook(symbol, depth).await
    } else {
        let ex = crate::commands::bn_exchange()?;
        ex.get_orderbook(symbol, depth).await
    }
}

/// 按市场取 K 线: 现货 `BinanceClient` / 合约 `FuturesDataClient`, 同名同签名 (免 key)。
async fn fetch_klines(
    market: &str,
    symbol: &str,
    interval: &str,
    limit: u32,
) -> ricow_core::CoreResult<Vec<Kline>> {
    if market == "futures" {
        ricow_binance::FuturesDataClient::new()?.get_klines(symbol, interval, limit).await
    } else {
        ricow_binance::BinanceClient::new()?.get_klines(symbol, interval, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- all ----

    #[test]
    fn test_parse_all_defaults_false_and_accepts_01() {
        assert!(!parse_all(None).unwrap());
        assert!(!parse_all(Some("")).unwrap());
        assert!(!parse_all(Some("  ")).unwrap());
        assert!(!parse_all(Some("0")).unwrap());
        assert!(parse_all(Some("1")).unwrap());
        assert!(parse_all(Some(" 1 ")).unwrap(), "容忍前后空白");
    }

    #[test]
    fn test_parse_all_rejects_other_values() {
        for bad in ["2", "true", "yes", "all"] {
            let err = parse_all(Some(bad)).unwrap_err();
            assert!(err.contains("all 只支持 0/1"), "{err}");
            assert!(err.contains(bad), "错误要带原文: {err}");
        }
    }

    // ---- market ----

    #[test]
    fn test_parse_optional_market_accepts_known_and_blank() {
        assert_eq!(parse_market(None, false).unwrap(), None);
        assert_eq!(parse_market(Some(""), false).unwrap(), None);
        assert_eq!(parse_market(Some("  "), false).unwrap(), None);
        assert_eq!(parse_market(Some("SPOT"), false).unwrap(), Some("spot"));
        assert_eq!(parse_market(Some(" Futures "), false).unwrap(), Some("futures"));
    }

    #[test]
    fn test_parse_required_market_missing_is_error() {
        let err = parse_market(None, true).unwrap_err();
        assert!(err.contains("market 必填"), "{err}");
        assert!(parse_market(Some("spot"), true).is_ok());
        assert!(parse_market(Some("futures"), true).is_ok());
    }

    #[test]
    fn test_parse_market_rejects_typo_hard() {
        // 拼错硬失败 (不静默当作"两个市场都返回")。
        for bad in ["future", "spots", "ETF", "现货"] {
            let err = parse_market(Some(bad), false).unwrap_err();
            assert!(err.contains("market 只支持 spot / futures"), "{err}");
            assert!(err.contains(bad), "错误要带原文: {err}");
        }
    }

    // ---- depth ----

    #[test]
    fn test_parse_depth_default_and_bounds() {
        assert_eq!(parse_depth(None).unwrap(), DEFAULT_DEPTH);
        assert_eq!(parse_depth(Some("")).unwrap(), DEFAULT_DEPTH);
        assert_eq!(parse_depth(Some("1")).unwrap(), 1);
        assert_eq!(parse_depth(Some("20")).unwrap(), 20);
        assert_eq!(parse_depth(Some("50")).unwrap(), 50, "边界值 50 合法");
    }

    #[test]
    fn test_parse_depth_rejects_out_of_range_and_garbage() {
        for bad in ["0", "51", "100", "-1", "abc", "1.5"] {
            assert!(parse_depth(Some(bad)).is_err(), "depth={bad} 应拒绝");
        }
        let err = parse_depth(Some("99")).unwrap_err();
        assert!(err.contains("1..=50") && err.contains("99"), "{err}");
    }

    // ---- interval ----

    #[test]
    fn test_parse_interval_whitelist_and_default() {
        assert_eq!(parse_interval(None).unwrap(), "1h");
        assert_eq!(parse_interval(Some("")).unwrap(), "1h");
        for iv in INTERVALS {
            assert_eq!(parse_interval(Some(iv)).unwrap(), iv);
        }
        for bad in ["2m", "1H", "1w", "hour"] {
            // 注意: 周期只容忍前后空白(与其他参数一致), 不容忍大小写 —— 白名单逐字匹配 ("1H" 拒绝)。
            assert!(parse_interval(Some(bad)).is_err(), "interval={bad} 应拒绝");
        }
        // 前后空白容忍: trim 后命中白名单即合法。
        assert_eq!(parse_interval(Some(" 1h ")).unwrap(), "1h");
        let err = parse_interval(Some("3m")).unwrap_err();
        assert!(err.contains("3m") && err.contains("1m/5m/15m/1h/4h/1d"), "{err}");
    }

    // ---- limit ----

    #[test]
    fn test_parse_limit_default_and_bounds() {
        assert_eq!(parse_limit(None).unwrap(), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some("1")).unwrap(), 1);
        assert_eq!(parse_limit(Some("500")).unwrap(), 500, "边界值 500 合法");
    }

    #[test]
    fn test_parse_limit_rejects_out_of_range_and_garbage() {
        for bad in ["0", "501", "1000", "-3", "x", "2.0"] {
            assert!(parse_limit(Some(bad)).is_err(), "limit={bad} 应拒绝");
        }
        let err = parse_limit(Some("501")).unwrap_err();
        assert!(err.contains("1..=500") && err.contains("501"), "{err}");
    }

    // ---- symbol ----

    #[test]
    fn test_normalize_symbol_trims_and_uppercases() {
        assert_eq!(normalize_symbol(" btcusdt "), "BTCUSDT");
        assert_eq!(normalize_symbol("AaplUsdt"), "AAPLUSDT");
    }
}
