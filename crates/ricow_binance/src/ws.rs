//! Binance 现货 WebSocket: 盘口 + 用户数据流 (listenKey)。

use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::{SinkExt, Stream, StreamExt};
use ricow_core::{
    CoreError, CoreResult, OrderBook, OrderBookUpdate, OrderFill, OrderSide, OrderStatus,
    OrderUpdate, UserEvent,
};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::client::{parse_levels, BinanceClient};

const SPOT_MAINNET_WS: &str = "wss://stream.binance.com:9443/ws";
const SPOT_TESTNET_WS: &str = "wss://testnet.binance.vision/ws";
/// demo 平台 (demo-api.binance.com) 的现货 WS —— 与 testnet.binance.vision 是两套环境 (specs/testnet.md)。
const SPOT_DEMO_WS: &str = "wss://demo-stream.binance.com/ws";

/// 现货 WebSocket API 端点 (用户数据流订阅用; 与市场行情 stream 是不同服务)。
const SPOT_WS_API_MAINNET: &str = "wss://ws-api.binance.com/ws-api/v3";
const SPOT_WS_API_TESTNET: &str = "wss://ws-api.testnet.binance.vision/ws-api/v3";
const SPOT_WS_API_DEMO: &str = "wss://demo-ws-api.binance.com/ws-api/v3";

/// 订阅就绪等待上限: 超时即视为订阅失败 (实盘不允许"没订阅就开跑")。
const SUBSCRIBE_READY_TIMEOUT: Duration = Duration::from_secs(15);

impl BinanceClient {
    /// 根据 REST base_url 返回对应市场行情 WS base (mainnet / testnet / demo)。
    ///
    /// demo 平台必须走 `demo-stream` (主网 stream 不认 demo 的订阅/凭证, 011 T013 修)。
    fn ws_base(&self) -> &'static str {
        let base = self.base_url();
        if base.contains("testnet") {
            SPOT_TESTNET_WS
        } else if base.contains("demo") {
            SPOT_DEMO_WS
        } else {
            SPOT_MAINNET_WS
        }
    }

    /// 对应 WebSocket API 端点 (用户数据流订阅; demo 必须走 `demo-ws-api`)。
    fn ws_api_base(&self) -> &'static str {
        let base = self.base_url();
        if base.contains("testnet") {
            SPOT_WS_API_TESTNET
        } else if base.contains("demo") {
            SPOT_WS_API_DEMO
        } else {
            SPOT_WS_API_MAINNET
        }
    }

    pub async fn subscribe_depth(
        &self,
        symbol: &str,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>> {
        let stream_name = format!("{}@depth@100ms", symbol.to_lowercase());
        let ws_url = format!("{}/{stream_name}", self.ws_base());
        let symbol_owned = symbol.to_string();

        let (tx, rx) = mpsc::channel::<OrderBookUpdate>(256);
        tokio::spawn(async move {
            run_depth_ws(&ws_url, &symbol_owned, tx).await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// 订阅现货用户数据流 (成交/订单/余额事件)。
    ///
    /// 机制 = 现货 **WebSocket API** 的 `userDataStream.subscribe.signature` (HMAC 签名即可):
    /// 连接 `wss://(demo-)ws-api.../ws-api/v3` → 发一条订阅请求 → 事件以
    /// `{"subscriptionId":N,"event":{...}}` 推送; 连接断开由本模块指数退避重连并**重新订阅**。
    ///
    /// 为什么不是 listenKey: 币安已于 2026-02-20 07:00 UTC 永久下线 legacy listenKey
    /// (`POST/PUT/DELETE /api/v3/userDataStream` 与 WS-API `userDataStream.start/ping/stop`),
    /// 主网与 demo 均返回 410 Gone (2026-09-13 实测), 本方法即官方指定替代 (specs/testnet.md)。
    /// **订阅就绪后**才返回 Stream: 首次握手失败或超时即报错。
    ///
    /// 就绪语义是必须的 —— 2026-09-13 实测踩坑: 若在订阅确认前就开始下单, 成交事件会落在
    /// 订阅建立之前, 引擎 `fills=0` 却真实持有了仓位 (引擎持仓/盈亏与交易所不一致)。
    pub async fn subscribe_user_data(
        &self,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = UserEvent> + Send>>> {
        let params = self.ws_api_subscribe_params()?;
        let request = build_subscribe_request(&params);
        let ws_url = self.ws_api_base().to_string();

        let (tx, rx) = mpsc::channel::<UserEvent>(256);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<CoreResult<u64>>();
        tokio::spawn(async move {
            run_user_data_ws(&ws_url, &request, tx, ready_tx).await;
        });

        match tokio::time::timeout(SUBSCRIBE_READY_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(subscription_id))) => {
                tracing::info!(
                    target: "bn.ws",
                    subscription_id,
                    "user data stream ready (WS-API)"
                );
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => {
                Err(CoreError::Network("用户数据流订阅任务提前退出 (未收到订阅确认)".into()))
            }
            Err(_) => Err(CoreError::Network(format!(
                "用户数据流订阅超时 ({}s 内未收到订阅确认)",
                SUBSCRIBE_READY_TIMEOUT.as_secs()
            ))),
        }
    }
}

fn backoff_delay(retry_count: u32) -> Duration {
    let base_secs = (1u64 << retry_count.min(6)).min(60);
    Duration::from_secs(base_secs)
}

// ---- Depth WS ----

/// 盘口订阅守护: 断线指数退避重连 (现货市场流 / 合约 fapi 流共用)。
///
/// `depthUpdate` 是**增量 diff**(`b`/`a` 只含变动档, `size=0` 表示删档), 故本函数维护累计盘口;
/// 覆盖式解析会让某一侧变空 → `mid_price()` 返回 None → 策略拿不到价 (2026-09-13 修正)。
pub(crate) async fn run_depth_ws(ws_url: &str, symbol: &str, tx: mpsc::Sender<OrderBookUpdate>) {
    let mut retry = 0u32;
    loop {
        match depth_session(ws_url, symbol, &tx).await {
            Ok(()) => break,
            Err(e) => {
                retry += 1;
                let delay = backoff_delay(retry);
                tracing::warn!(target: "bn.ws", symbol = %symbol, error = %e, retry, delay_ms = delay.as_millis(), "depth WS disconnected, reconnecting");
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// 累计盘口的最大保留档数 (单侧)。
const MAX_DEPTH_LEVELS: usize = 50;

/// 把一帧盘口消息合并进累计盘口; 返回是否产生了可用更新。
///
/// 支持两种帧: 增量 `{"e":"depthUpdate","b":[...],"a":[...],"E":ts}`(现货/合约市场流)
/// 与快照 `{"lastUpdateId":..,"bids":[...],"asks":[...]}`。
pub(crate) fn apply_depth_frame(book: &mut OrderBook, symbol: &str, v: &Value) -> bool {
    let (bids_raw, asks_raw, ts) = if v.get("e").and_then(|e| e.as_str()) == Some("depthUpdate") {
        (v["b"].as_array(), v["a"].as_array(), v["E"].as_u64())
    } else if v.get("bids").is_some() || v.get("asks").is_some() {
        (v["bids"].as_array(), v["asks"].as_array(), v["E"].as_u64())
    } else {
        return false;
    };
    let bids = parse_levels(bids_raw);
    let asks = parse_levels(asks_raw);
    if bids.is_empty() && asks.is_empty() {
        return false; // 空帧 = 无变动
    }
    merge_levels(&mut book.bids, &bids, true);
    merge_levels(&mut book.asks, &asks, false);
    book.timestamp =
        ts.map(|ms| ms as i64).and_then(DateTime::from_timestamp_millis).unwrap_or_else(Utc::now);
    let _ = symbol;
    !book.bids.is_empty() || !book.asks.is_empty()
}

/// 按价格合并档位 (`size=0` 删除该档), 保持排序并截断到 `MAX_DEPTH_LEVELS`。
fn merge_levels(
    levels: &mut Vec<ricow_core::PriceLevel>,
    updates: &[ricow_core::PriceLevel],
    descending: bool,
) {
    for u in updates {
        match levels.iter().position(|l| l.price == u.price) {
            Some(i) => {
                if u.size.is_zero() {
                    levels.remove(i);
                } else {
                    levels[i] = u.clone();
                }
            }
            None => {
                if !u.size.is_zero() {
                    levels.push(u.clone());
                }
            }
        }
    }
    if descending {
        levels.sort_by_key(|p| std::cmp::Reverse(p.price));
    } else {
        levels.sort_by_key(|p| p.price);
    }
    levels.truncate(MAX_DEPTH_LEVELS);
}

async fn depth_session(
    ws_url: &str,
    symbol: &str,
    tx: &mpsc::Sender<OrderBookUpdate>,
) -> CoreResult<()> {
    let (ws_stream, _) =
        connect_async(ws_url).await.map_err(|e| CoreError::Network(e.to_string()))?;
    let (mut write, mut read) = ws_stream.split();
    let mut book = ricow_core::OrderBook::default();

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if apply_depth_frame(&mut book, symbol, &v) {
                        let update = OrderBookUpdate {
                            pair: symbol.to_string(),
                            bids: book.bids.clone(),
                            asks: book.asks.clone(),
                            timestamp: book.timestamp,
                        };
                        if tx.send(update).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Ok(Message::Close(_)) => {
                return Err(CoreError::Network("server closed connection".into()))
            }
            Err(e) => return Err(CoreError::Network(e.to_string())),
            _ => {}
        }
    }
    Err(CoreError::Network("WS stream ended".into()))
}

// ---- User Data WS ----

async fn run_user_data_ws(
    ws_url: &str,
    request: &str,
    tx: mpsc::Sender<UserEvent>,
    ready: tokio::sync::oneshot::Sender<CoreResult<u64>>,
) {
    let mut ready = Some(ready);
    let mut retry = 0u32;
    loop {
        // 每次连接都是新会话 → 必须重新发订阅请求 (WS-API 连接有 24h 上限与空闲断开)
        match user_data_session(ws_url, request, &tx, &mut ready).await {
            Ok(()) => break,
            Err(e) => {
                if let Some(r) = ready.take() {
                    // 首次握手就失败: 把错误直接交给调用方 (不重试, 避免"静默重连 + 永远不就绪")
                    let _ = r.send(Err(e));
                    return;
                }
                retry += 1;
                let delay = backoff_delay(retry);
                tracing::warn!(target: "bn.ws", error = %e, retry, delay_ms = delay.as_millis(), "user data WS-API disconnected, reconnecting");
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn user_data_session(
    ws_url: &str,
    request: &str,
    tx: &mpsc::Sender<UserEvent>,
    ready: &mut Option<tokio::sync::oneshot::Sender<CoreResult<u64>>>,
) -> CoreResult<()> {
    let (ws_stream, _) =
        connect_async(ws_url).await.map_err(|e| CoreError::Network(e.to_string()))?;
    let (mut write, mut read) = ws_stream.split();
    write
        .send(Message::Text(request.into()))
        .await
        .map_err(|e| CoreError::Network(format!("WS-API 订阅请求发送失败: {e}")))?;

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    match parse_ws_api_frame(&v) {
                        WsApiFrame::Event(event) => {
                            if tx.send(event).await.is_err() {
                                return Ok(());
                            }
                        }
                        WsApiFrame::Subscribed(subscription_id) => {
                            // 通知调用方"订阅已就绪" (只通知首次)
                            if let Some(r) = ready.take() {
                                let _ = r.send(Ok(subscription_id));
                            }
                            tracing::info!(
                                target: "bn.ws",
                                subscription_id,
                                "user data stream subscribed (WS-API)"
                            );
                        }
                        WsApiFrame::Error { status, message } => {
                            // 订阅失败 (签名/权限/限频) 必须如实抛出, 不静默空转
                            return Err(CoreError::Exchange(format!(
                                "WS-API 用户流订阅失败: status={status} {message}"
                            )));
                        }
                        WsApiFrame::Ignored => {}
                    }
                }
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Ok(Message::Close(_)) => {
                return Err(CoreError::Network("server closed connection".into()))
            }
            Err(e) => return Err(CoreError::Network(e.to_string())),
            _ => {}
        }
    }
    Err(CoreError::Network("WS stream ended".into()))
}

// ---- WS-API (用户数据流) ----

/// WS-API 一帧的语义 (纯逻辑, 便于单测)。
#[derive(Debug)]
enum WsApiFrame {
    /// 用户流事件 (已解析为 UserEvent)
    Event(UserEvent),
    /// 订阅确认 (result.subscriptionId)
    Subscribed(u64),
    /// 请求失败 (status != 200)
    Error { status: i64, message: String },
    /// 无关帧 (心跳/其它)
    Ignored,
}

/// 构造 WS-API 订阅请求帧 `{"id":..,"method":"userDataStream.subscribe.signature","params":{..}}`。
fn build_subscribe_request(params: &[(String, String)]) -> String {
    let map: serde_json::Map<String, Value> =
        params.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    serde_json::json!({
        "id": "ricow-user-stream",
        "method": "userDataStream.subscribe.signature",
        "params": Value::Object(map),
    })
    .to_string()
}

/// WS-API 帧解析: 事件帧 `{"subscriptionId":N,"event":{...}}` / 响应帧 `{"id":..,"status":..}`。
fn parse_ws_api_frame(v: &Value) -> WsApiFrame {
    if let Some(event) = v.get("event") {
        return match parse_user_data(event) {
            Some(e) => WsApiFrame::Event(e),
            None => WsApiFrame::Ignored,
        };
    }
    if let Some(status) = v.get("status").and_then(|s| s.as_i64()) {
        if status == 200 {
            let subscription_id = v
                .get("result")
                .and_then(|r| r.get("subscriptionId"))
                .and_then(|s| s.as_u64())
                .unwrap_or_default();
            return WsApiFrame::Subscribed(subscription_id);
        }
        let message = v
            .get("error")
            .and_then(|e| e.get("msg"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();
        return WsApiFrame::Error { status, message };
    }
    WsApiFrame::Ignored
}

// ---- Parse helpers ----

/// 用户数据流事件分发: 现货 = `executionReport`, 合约 USDT-M = `ORDER_TRADE_UPDATE`。
fn parse_user_data(v: &Value) -> Option<UserEvent> {
    match v.get("e")?.as_str()? {
        "executionReport" => parse_spot_execution(v),
        "ORDER_TRADE_UPDATE" => parse_futures_order_update(v),
        _ => None,
    }
}

/// 现货用户数据流 `executionReport` (`x: TRADE` 即成交通知)。
fn parse_spot_execution(v: &Value) -> Option<UserEvent> {
    let status = match v.get("X")?.as_str()? {
        "NEW" | "PENDING_CANCEL" => OrderStatus::Open,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" => OrderStatus::Cancelled,
        "EXPIRED" => OrderStatus::Expired,
        "REJECTED" => OrderStatus::Rejected,
        _ => return None,
    };
    let side = match v.get("S")?.as_str()? {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => return None,
    };
    let pair = v.get("s")?.as_str()?.to_string();
    let client_order_id = v.get("c").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let exchange_order_id =
        v.get("i").and_then(|x| x.as_u64()).map(|id| id.to_string()).unwrap_or_default();
    let ts = v
        .get("T")
        .and_then(|x| x.as_u64())
        .or_else(|| v.get("E").and_then(|x| x.as_u64()))
        .map(|ms| ms as i64)
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    let filled_size =
        Decimal::from_str(v.get("z").and_then(|x| x.as_str()).unwrap_or("0")).unwrap_or_default();
    let orig_size =
        Decimal::from_str(v.get("q").and_then(|x| x.as_str()).unwrap_or("0")).unwrap_or_default();
    let remaining_size = orig_size - filled_size;

    if v.get("x").and_then(|x| x.as_str()) == Some("TRADE") {
        let fill_price = Decimal::from_str(v.get("L")?.as_str()?).ok()?;
        let fill_size = Decimal::from_str(v.get("l")?.as_str()?).ok()?;
        let fee = Decimal::from_str(v.get("n").and_then(|x| x.as_str()).unwrap_or("0"))
            .unwrap_or_default();
        Some(UserEvent::Fill(OrderFill {
            // 现货 tradeId 可能在撤单等事件里为 -1 → 视为无
            trade_id: v
                .get("t")
                .and_then(|x| x.as_i64())
                .filter(|t| *t >= 0)
                .map(|t| t.to_string()),
            exchange_order_id,
            client_order_id,
            pair,
            side,
            fill_price,
            fill_size,
            fee,
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
        }))
    } else {
        let avg_price = match (
            Decimal::from_str(v.get("Z").and_then(|x| x.as_str()).unwrap_or("0")).ok(),
            filled_size,
        ) {
            (Some(quote), q) if !q.is_zero() => Some(quote / q),
            _ => None,
        };
        Some(UserEvent::Order(OrderUpdate {
            exchange_order_id,
            client_order_id,
            pair,
            status,
            filled_size,
            remaining_size,
            avg_price,
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
        }))
    }
}

/// 合约 USDT-M 用户数据流 `ORDER_TRADE_UPDATE` (Phase 2 实盘用; 保留既有语义)。
fn parse_futures_order_update(v: &Value) -> Option<UserEvent> {
    let o = v.get("o")?;
    let status = match o.get("X")?.as_str()? {
        "NEW" => OrderStatus::Open,
        "FILLED" => OrderStatus::Filled,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "CANCELED" => OrderStatus::Cancelled,
        "EXPIRED" => OrderStatus::Expired,
        "REJECTED" => OrderStatus::Rejected,
        _ => return None,
    };
    let ts = v["T"].as_u64().map(|ms| ms as i64).unwrap_or_else(|| Utc::now().timestamp_millis());

    let side = match o.get("S")?.as_str()? {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => return None,
    };

    let filled_size = Decimal::from_str(o.get("z")?.as_str()?).ok().unwrap_or_default();
    let orig_size = Decimal::from_str(o.get("q")?.as_str()?).ok().unwrap_or_default();
    let remaining = orig_size - filled_size;
    let avg_price = o.get("ap").and_then(|v| v.as_str()).and_then(|s| Decimal::from_str(s).ok());

    let is_fill_event = o.get("x")?.as_str()? == "TRADE";
    if is_fill_event {
        let fill_price = Decimal::from_str(o.get("L")?.as_str()?).ok()?;
        let fill_size = Decimal::from_str(o.get("l")?.as_str()?).ok()?;
        let fee = Decimal::from_str(o.get("n")?.as_str()?).ok().unwrap_or_default();
        Some(UserEvent::Fill(OrderFill {
            trade_id: o.get("t").and_then(|v| v.as_u64()).map(|id| id.to_string()),
            exchange_order_id: o
                .get("i")
                .and_then(|v| v.as_u64())
                .map(|id| id.to_string())
                .unwrap_or_default(),
            client_order_id: o.get("c").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            pair: o.get("s")?.as_str()?.to_string(),
            side,
            fill_price,
            fill_size,
            fee,
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
        }))
    } else {
        Some(UserEvent::Order(OrderUpdate {
            exchange_order_id: o
                .get("i")
                .and_then(|v| v.as_u64())
                .map(|id| id.to_string())
                .unwrap_or_default(),
            client_order_id: o.get("c").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            pair: o.get("s")?.as_str()?.to_string(),
            status,
            filled_size,
            remaining_size: remaining,
            avg_price,
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_apply_depth_frame_merges_deltas() {
        let mut book = ricow_core::OrderBook::default();
        // 首帧: 买 3000/1.5, 卖 3001/2.0
        assert!(apply_depth_frame(
            &mut book,
            "ETHUSDT",
            &serde_json::json!({
                "e": "depthUpdate", "E": 1_700_000_000_000_u64,
                "b": [["3000.0", "1.5"]], "a": [["3001.0", "2.0"]]
            })
        ));
        assert_eq!(book.bids[0].price, dec!(3000));
        assert_eq!(book.asks[0].price, dec!(3001));
        assert_eq!(book.mid_price(), Some(dec!(3000.5)));

        // 增量帧只有卖档变化: 买单侧必须保留 (覆盖式会丢档 → 价格变 None)
        assert!(apply_depth_frame(
            &mut book,
            "ETHUSDT",
            &serde_json::json!({
                "e": "depthUpdate", "E": 1_700_000_001_000_u64,
                "b": [], "a": [["3001.0", "0"], ["3002.0", "3.0"]]
            })
        ));
        assert_eq!(book.bids.len(), 1, "买档不应被空增量清空");
        assert_eq!(book.asks.len(), 1, "size=0 的档应被删除");
        assert_eq!(book.asks[0].price, dec!(3002));
        assert_eq!(book.mid_price(), Some(dec!(3001)));

        // 空帧 (两侧都无变动) 不产生更新
        assert!(!apply_depth_frame(
            &mut book,
            "ETHUSDT",
            &serde_json::json!({
                "e": "depthUpdate", "E": 1_700_000_002_000_u64, "b": [], "a": []
            })
        ));
        // 无关帧
        assert!(!apply_depth_frame(&mut book, "ETHUSDT", &serde_json::json!({"foo": 1})));
    }

    #[test]
    fn test_apply_depth_frame_sorting_and_cap() {
        let mut book = ricow_core::OrderBook::default();
        let bids: Vec<Vec<String>> =
            (0..80).map(|i| vec![format!("{}", 3000 + i), "1".into()]).collect();
        apply_depth_frame(
            &mut book,
            "ETHUSDT",
            &serde_json::json!({
                "e": "depthUpdate", "E": 1_u64, "b": bids, "a": []
            }),
        );
        assert_eq!(book.bids.len(), MAX_DEPTH_LEVELS, "超过上限应截断");
        assert_eq!(book.bids[0].price, dec!(3079), "买档应降序 (最优在前)");
    }

    #[test]
    fn test_build_subscribe_request_and_ws_api_frame() {
        let params = vec![
            ("apiKey".to_string(), "KEY".to_string()),
            ("timestamp".to_string(), "1700000000000".to_string()),
            ("signature".to_string(), "SIG".to_string()),
        ];
        let req: Value = serde_json::from_str(&build_subscribe_request(&params)).unwrap();
        assert_eq!(req["method"], "userDataStream.subscribe.signature");
        assert_eq!(req["params"]["apiKey"], "KEY");
        assert_eq!(req["params"]["signature"], "SIG");

        // 订阅确认帧
        let ack = serde_json::json!({"id":"x","status":200,"result":{"subscriptionId":0}});
        assert!(matches!(parse_ws_api_frame(&ack), WsApiFrame::Subscribed(0)));

        // 订阅失败帧 → 如实识别 (不当作事件吞掉)
        let err = serde_json::json!({
            "id":"x","status":400,"error":{"code":-1102,"msg":"Mandatory parameter 'apiKey' was not sent."}
        });
        match parse_ws_api_frame(&err) {
            WsApiFrame::Error { status, message } => {
                assert_eq!(status, 400);
                assert!(message.contains("apiKey"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // 事件帧 (外层带 subscriptionId) → 解析出 executionReport
        let ev = serde_json::json!({
            "subscriptionId": 0,
            "event": {
                "e": "executionReport", "s": "ETHUSDT", "c": "grid-1", "S": "BUY",
                "q": "0.01", "x": "TRADE", "X": "FILLED", "i": 1,
                "l": "0.01", "z": "0.01", "L": "2500", "n": "0.0001",
                "t": 5, "T": 1_700_000_000_000_i64
            }
        });
        match parse_ws_api_frame(&ev) {
            WsApiFrame::Event(UserEvent::Fill(f)) => {
                assert_eq!(f.client_order_id, "grid-1");
                assert_eq!(f.fill_price, dec!(2500));
            }
            other => panic!("expected Fill event, got {other:?}"),
        }

        // 无关事件 (如余额变动) → Ignored, 不产生噪声事件
        let acct = serde_json::json!({
            "subscriptionId": 0,
            "event": {"e": "outboundAccountPosition", "E": 1, "u": 1, "B": []}
        });
        assert!(matches!(parse_ws_api_frame(&acct), WsApiFrame::Ignored));
        // 心跳/无字段帧
        assert!(matches!(parse_ws_api_frame(&serde_json::json!({"pong": 1})), WsApiFrame::Ignored));
    }

    #[test]
    fn test_ws_api_base_maps_hosts() {
        let mk = |url: &str| BinanceClient::new().expect("client").with_base_url(url);
        assert_eq!(mk("https://demo-api.binance.com").ws_api_base(), SPOT_WS_API_DEMO);
        assert_eq!(mk("https://api.binance.com").ws_api_base(), SPOT_WS_API_MAINNET);
        assert_eq!(mk("https://testnet.binance.vision").ws_api_base(), SPOT_WS_API_TESTNET);
    }

    #[test]
    fn test_ws_base_maps_demo_and_testnet_hosts() {
        let mk = |url: &str| BinanceClient::new().expect("client").with_base_url(url);
        assert_eq!(
            mk("https://demo-api.binance.com").ws_base(),
            SPOT_DEMO_WS,
            "demo 必须走 demo-stream (主网 WS 不认 demo listenKey)"
        );
        assert_eq!(mk("https://api.binance.com").ws_base(), SPOT_MAINNET_WS);
        assert_eq!(mk("https://testnet.binance.vision").ws_base(), SPOT_TESTNET_WS);
    }

    #[test]
    fn test_parse_spot_execution_report_trade() {
        // 现货 executionReport (币安文档字段子集, x=TRADE → 成交)
        let v = serde_json::json!({
            "e": "executionReport", "E": 1_700_000_000_000_i64, "s": "ETHUSDT",
            "c": "grid-eth-0001", "S": "BUY", "o": "LIMIT", "q": "0.05", "p": "3000",
            "x": "TRADE", "X": "FILLED", "i": 4293153, "l": "0.05", "z": "0.05",
            "L": "2999.5", "n": "0.0015", "T": 1_700_000_000_123_i64, "t": 88
        });
        match parse_user_data(&v).expect("应解析") {
            UserEvent::Fill(f) => {
                assert_eq!(f.pair, "ETHUSDT");
                assert_eq!(f.client_order_id, "grid-eth-0001");
                assert_eq!(f.side, OrderSide::Buy);
                assert_eq!(f.fill_price, dec!(2999.5));
                assert_eq!(f.fill_size, dec!(0.05));
                assert_eq!(f.fee, dec!(0.0015));
                assert_eq!(f.trade_id.as_deref(), Some("88"));
                assert_eq!(f.exchange_order_id, "4293153");
                assert_eq!(f.timestamp.timestamp_millis(), 1_700_000_000_123);
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_spot_execution_report_order_events() {
        // x=NEW → 订单事件 (非成交), 均价由累计成交额/量推算
        let v = serde_json::json!({
            "e": "executionReport", "s": "ETHUSDT", "c": "cid", "S": "SELL",
            "q": "1", "x": "NEW", "X": "NEW", "i": 7, "z": "0", "Z": "0", "t": -1, "T": 1_700_000_000_000_i64
        });
        match parse_user_data(&v).expect("应解析") {
            UserEvent::Order(u) => {
                assert_eq!(u.status, OrderStatus::Open);
                assert_eq!(u.client_order_id, "cid");
                assert!(u.avg_price.is_none(), "无成交时均价应为 None");
                assert_eq!(u.remaining_size, dec!(1));
            }
            other => panic!("expected Order, got {other:?}"),
        }

        // 部分成交: 均价 = Z / z
        let part = serde_json::json!({
            "e": "executionReport", "s": "ETHUSDT", "c": "cid", "S": "BUY",
            "q": "1", "x": "TRADE", "X": "PARTIALLY_FILLED", "i": 8,
            "l": "0.4", "z": "0.4", "L": "100", "n": "0", "Z": "40", "T": 1_700_000_000_000_i64
        });
        match parse_user_data(&part).expect("应解析") {
            UserEvent::Fill(f) => assert_eq!(f.fill_size, dec!(0.4)),
            other => panic!("expected Fill, got {other:?}"),
        }
        let canceled = serde_json::json!({
            "e": "executionReport", "s": "ETHUSDT", "c": "cid", "S": "BUY",
            "q": "1", "x": "CANCELED", "X": "CANCELED", "i": 9, "z": "0.4", "Z": "40",
            "t": -1, "T": 1_700_000_000_000_i64
        });
        match parse_user_data(&canceled).expect("应解析") {
            UserEvent::Order(u) => {
                assert_eq!(u.status, OrderStatus::Cancelled);
                assert_eq!(u.filled_size, dec!(0.4));
                assert_eq!(u.avg_price, Some(dec!(100)), "40/0.4 = 100");
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_user_data_fill() {
        // 合约 USDT-M order trade update (Phase 2 语义保留)
        let v = serde_json::json!({
            "e": "ORDER_TRADE_UPDATE",
            "E": 1700000000000_u64,
            "o": {
                "s": "ETHUSDT", "c": "client-1", "S": "BUY", "o": "LIMIT", "f": "GTC",
                "q": "0.01", "p": "3000", "x": "TRADE", "X": "FILLED",
                "z": "0.01", "l": "0.01", "L": "3000", "n": "0.001", "i": 99, "t": 88
            }
        });
        match parse_user_data(&v).unwrap() {
            UserEvent::Fill(f) => {
                assert_eq!(f.pair, "ETHUSDT");
                assert_eq!(f.side, OrderSide::Buy);
                assert_eq!(f.fill_price, dec!(3000));
            }
            _ => panic!("expected Fill event"),
        }
    }
}
