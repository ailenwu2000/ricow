//! Binance USDT-M 合约 WebSocket: 盘口 (fstream depth) + 用户数据流 (listenKey)。
//!
//! 端点 (2026-09-13 实测): demo `wss://demo-fstream.binance.com/ws/...`; 主网 `wss://fstream.binance.com/ws/...`。
//! - **市场流**: `/<symbol>@depth@100ms` → `{"e":"depthUpdate","b":[...],"a":[...],"E":ts}`(增量, 复用现货同一套合并逻辑);
//! - **用户流**: `POST /fapi/v1/listenKey` 取 key → `/<listenKey>` 直推事件(无 `subscriptionId` 外壳):
//!   `ORDER_TRADE_UPDATE`(订单/成交)、`TRADE_LITE`(轻量成交, 与前者重复 → 忽略)、
//!   `ACCOUNT_UPDATE`(余额/持仓)、`ACCOUNT_CONFIG_UPDATE`(杠杆)。
//!
//! 现货的 legacy listenKey 已被币安下线(改走 WS-API), 合约 listenKey **仍在服务**(实测 200):
//! 不要把现货的 WS-API 订阅逻辑套到合约上。

use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::{SinkExt, Stream, StreamExt};
use ricow_core::{
    CoreError, CoreResult, Kline, OrderBookUpdate, OrderFill, OrderSide, OrderStatus, OrderUpdate,
    UserEvent,
};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::futures_client::FuturesClient;
use crate::ws::{backoff_delay, run_depth_ws, run_kline_ws};

const FAPI_MAINNET_WS: &str = "wss://fstream.binance.com/ws";
const FAPI_DEMO_WS: &str = "wss://demo-fstream.binance.com/ws";
/// 旧合约 testnet (stream.binancefuture.com); demo 平台走 `demo-fstream`。
const FAPI_TESTNET_WS: &str = "wss://stream.binancefuture.com/ws";

/// 用户流订阅就绪等待上限 (合约 WS 连上即生效, 无订阅确认帧)。
const SUBSCRIBE_READY_TIMEOUT: Duration = Duration::from_secs(15);

impl FuturesClient {
    /// 市场流 WS base (demo 必须走 demo-fstream, 主网流不认 demo 账户事件)。
    fn ws_base(&self) -> &'static str {
        let base = self.base_url();
        if base.contains("testnet") {
            FAPI_TESTNET_WS
        } else if base.contains("demo") {
            FAPI_DEMO_WS
        } else {
            FAPI_MAINNET_WS
        }
    }

    /// 订阅合约盘口 (增量 `depthUpdate`, 与现货共用合并实现)。
    pub async fn subscribe_depth(
        &self,
        symbol: &str,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>> {
        let stream = format!("{}@depth@100ms", symbol.to_lowercase());
        let ws_url = format!("{}/{stream}", self.ws_base());
        let symbol_owned = symbol.to_string();

        // 快照闭包 (审计 H-4): 重连成功后 REST 拉快照重建 book, 再应用其后增量。
        let snap_client = self.clone();
        let snap_symbol = symbol.to_uppercase();
        let snapshot: Arc<crate::ws::DepthSnapshotFn> = Arc::new(move || {
            let c = snap_client.clone();
            let sym = snap_symbol.clone();
            Box::pin(async move { c.depth_snapshot(&sym, 50).await.ok() })
        });

        let (tx, rx) = mpsc::channel::<OrderBookUpdate>(256);
        tokio::spawn(async move {
            run_depth_ws(&ws_url, &symbol_owned, tx, Some(snapshot)).await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// 订阅合约 K 线流 (040 FR-1)。
    ///
    /// 流名与现货**同构** (`<symbol>@kline_<interval>`), 差别只在 WS base (`fstream`) ——
    /// 帧格式也一致 (`{"e":"kline","k":{...}}`), 故解析复用 `ws::parse_kline_frame`。
    pub async fn subscribe_klines(
        &self,
        symbol: &str,
        interval: &str,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = Kline> + Send>>> {
        let stream_name = format!("{}@kline_{interval}", symbol.to_lowercase());
        let ws_url = format!("{}/{stream_name}", self.ws_base());
        let symbol_owned = symbol.to_string();

        let (tx, rx) = mpsc::channel::<Kline>(256);
        tokio::spawn(async move {
            run_kline_ws(&ws_url, &symbol_owned, tx).await;
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// 订阅合约用户数据流: listenKey + fapi WS, **连接就绪后**才返回 Stream。
    ///
    /// 就绪语义与现货同口径(011 实测教训): 未就绪即下单会漏成交。
    pub async fn subscribe_user_data(
        &self,
    ) -> CoreResult<Pin<Box<dyn Stream<Item = UserEvent> + Send>>> {
        let listen_key = self.create_listen_key().await?;
        let ws_url = format!("{}/{}", self.ws_base(), listen_key);

        // 当前有效 listenKey 的共享单元: keepalive 任务续期它, 重连重建时更新它,
        // 保证续期目标始终是守护循环正在用的 key (审计 H-3)。
        let key_cell = Arc::new(std::sync::Mutex::new(listen_key));

        let (tx, rx) = mpsc::channel::<UserEvent>(256);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<CoreResult<()>>();
        let client = self.clone();
        let run_cell = Arc::clone(&key_cell);
        let run_tx = tx.clone();
        tokio::spawn(async move {
            run_user_data_ws(client, ws_url, run_cell, run_tx, ready_tx).await;
        });
        // 审计 H-2: keepalive 持有事件通道的 tx —— 实盘会话结束 (receiver drop) 时
        // `tx.closed()` 触发, 续期循环随之退出。此前该任务无任何退出条件, 每次启动
        // 实盘都会永久泄漏一个 30 分钟间隔的续期任务 (对币安产生幽灵流量)。
        self.spawn_listen_key_keepalive(key_cell, tx);

        match tokio::time::timeout(SUBSCRIBE_READY_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(()))) => {
                tracing::info!(target: "bn.ws", "futures user data stream ready (listenKey)");
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => {
                Err(CoreError::Network("合约用户数据流订阅任务提前退出 (未建立连接)".into()))
            }
            Err(_) => Err(CoreError::Network(format!(
                "合约用户数据流订阅超时 ({}s 内未建立连接)",
                SUBSCRIBE_READY_TIMEOUT.as_secs()
            ))),
        }
    }

    /// listenKey 续期任务 (每 30 分钟 PUT 一次; listenKey 有效期 60 分钟)。
    ///
    /// 审计 H-2: 循环 `select!` 监听 `tx.closed()` —— 事件通道的 receiver 已 drop
    /// (实盘会话结束/引擎停机) 即退出, 不再向币安发送幽灵续期请求。
    /// 续期目标始终读 `key_cell` (审计 H-3): 重连重建 key 后, 续期跟着切到新 key。
    fn spawn_listen_key_keepalive(
        &self,
        key_cell: Arc<std::sync::Mutex<String>>,
        tx: mpsc::Sender<UserEvent>,
    ) {
        let client = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30 * 60));
            interval.tick().await; // 立即返回一次, 跳过
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let key = key_cell.lock().map(|k| k.clone()).unwrap_or_default();
                        match client.keepalive_listen_key(&key).await {
                            Ok(()) => tracing::debug!(target: "bn.ws", "listenKey 续期 OK"),
                            Err(e) => tracing::warn!(target: "bn.ws", "listenKey 续期失败: {e}"),
                        }
                    }
                    // receiver 已 drop → 会话已结束, 停止续期
                    _ = tx.closed() => {
                        tracing::debug!(target: "bn.ws", "用户流已关闭, listenKey 续期任务退出");
                        break;
                    }
                }
            }
        });
    }
}

/// 消息空闲上限 (审计 H-3): 用户流事件稀疏, 但合约服务器 Ping 每 3 分钟一次;
/// 超过此时长无任何消息 (含 Ping) 必为 TCP 半开挂死, `read.next()` 无 timeout
/// 会永久挂起, 订单/成交事件静默丢失。
const USER_DATA_READ_IDLE: Duration = Duration::from_secs(600);

/// 用户流守护: 断线指数退避重连 (审计 H-3)。
///
/// - 消费端退出 (`tx.is_closed()`) 即停止重连, 不再泄漏守护任务;
/// - **每次重连前重新 create listenKey**: 断线期间旧 key 可能已过期 (60 分钟有效期),
///   复用旧 URL 会无限重连失败 (`listenKeyExpired` → 用同一过期 URL 重试是必然复现路径);
/// - 重建的新 key 写回 `key_cell`, keepalive 任务随之切换续期目标。
async fn run_user_data_ws(
    client: FuturesClient,
    mut ws_url: String,
    key_cell: Arc<std::sync::Mutex<String>>,
    tx: mpsc::Sender<UserEvent>,
    ready: tokio::sync::oneshot::Sender<CoreResult<()>>,
) {
    let mut ready = Some(ready);
    let mut retry = 0u32;
    loop {
        if tx.is_closed() {
            return; // 消费端 (live 会话) 已退出: 不再重连
        }
        match user_data_session(&ws_url, &tx, &mut ready).await {
            Ok(()) => break,
            Err(e) => {
                if let Some(r) = ready.take() {
                    // 首次连接就失败 → 交给调用方 (不静默重连)
                    let _ = r.send(Err(e));
                    return;
                }
                if tx.is_closed() {
                    break;
                }
                retry += 1;
                let delay = backoff_delay(retry);
                tracing::warn!(target: "bn.ws", error = %e, retry, delay_ms = delay.as_millis(), "futures user data WS disconnected, reconnecting");
                tokio::time::sleep(delay).await;
                // 重建 listenKey (失败则保留旧 URL, 下一轮再试)
                match client.create_listen_key().await {
                    Ok(k) => {
                        if let Ok(mut cell) = key_cell.lock() {
                            *cell = k.clone();
                        }
                        ws_url = format!("{}/{}", client.ws_base(), k);
                        tracing::info!(target: "bn.ws", "已重建 listenKey, 用新 URL 重连");
                    }
                    Err(e) => {
                        tracing::warn!(target: "bn.ws", error = %e, "重建 listenKey 失败, 下一轮重试");
                    }
                }
            }
        }
    }
}

async fn user_data_session(
    ws_url: &str,
    tx: &mpsc::Sender<UserEvent>,
    ready: &mut Option<tokio::sync::oneshot::Sender<CoreResult<()>>>,
) -> CoreResult<()> {
    let (ws_stream, _) =
        connect_async(ws_url).await.map_err(|e| CoreError::Network(e.to_string()))?;
    // 连接成功即订阅生效 (合约 listenKey 流无订阅确认帧)
    if let Some(r) = ready.take() {
        let _ = r.send(Ok(()));
    }
    let (mut write, mut read) = ws_stream.split();

    loop {
        // 审计 H-3: 读超时兜底, 半开连接不再永久挂起 (超时 → Err → 上层重连)
        let msg = match tokio::time::timeout(USER_DATA_READ_IDLE, read.next()).await {
            Ok(m) => m,
            Err(_) => {
                return Err(CoreError::Network(format!(
                    "用户流空闲超时 ({}s 无任何消息, 含 Ping), 视为半开连接",
                    USER_DATA_READ_IDLE.as_secs()
                )))
            }
        };
        match msg {
            Some(Ok(Message::Text(text))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some("listenKeyExpired") = v.get("e").and_then(|e| e.as_str()) {
                        return Err(CoreError::Exchange(
                            "listenKey 已过期 (需重新订阅用户流)".into(),
                        ));
                    }
                    if let Some(event) = parse_futures_user_event(&v) {
                        if tx.send(event).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
            Some(Ok(Message::Ping(data))) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Some(Ok(Message::Close(_))) => {
                return Err(CoreError::Network("server closed connection".into()))
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(CoreError::Network(e.to_string())),
            None => return Err(CoreError::Network("WS stream ended".into())),
        }
    }
}

/// 合约用户流事件解析 (纯函数, 便于单测)。
///
/// 只取 `ORDER_TRADE_UPDATE` 做订单/成交回写:
/// - `TRADE_LITE` 与它的成交信息重复(同一笔成交推两条) → 忽略, 否则成交量会翻倍;
/// - `ACCOUNT_UPDATE` / `ACCOUNT_CONFIG_UPDATE` 不驱动策略(持仓由引擎按需查询)。
pub fn parse_futures_user_event(v: &Value) -> Option<UserEvent> {
    match v.get("e")?.as_str()? {
        "ORDER_TRADE_UPDATE" => parse_order_trade_update(v),
        _ => None,
    }
}

fn parse_order_trade_update(v: &Value) -> Option<UserEvent> {
    let o = v.get("o")?;
    let status = match o.get("X")?.as_str()? {
        "NEW" | "PENDING_CANCEL" => OrderStatus::Open,
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELED" => OrderStatus::Cancelled,
        "EXPIRED" => OrderStatus::Expired,
        "REJECTED" => OrderStatus::Rejected,
        _ => return None,
    };
    let side = match o.get("S")?.as_str()? {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => return None,
    };
    let pair = o.get("s")?.as_str()?.to_string();
    let client_order_id = o.get("c").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let exchange_order_id =
        o.get("i").and_then(|x| x.as_u64()).map(|id| id.to_string()).unwrap_or_default();
    let ts = v
        .get("E")
        .and_then(|x| x.as_u64())
        .or_else(|| o.get("T").and_then(|x| x.as_u64()))
        .map(|ms| ms as i64)
        .unwrap_or_else(|| Utc::now().timestamp_millis());
    let filled_size =
        Decimal::from_str(o.get("z").and_then(|x| x.as_str()).unwrap_or("0")).unwrap_or_default();
    let orig_size =
        Decimal::from_str(o.get("q").and_then(|x| x.as_str()).unwrap_or("0")).unwrap_or_default();
    let avg_price = o.get("ap").and_then(|x| x.as_str()).and_then(|s| Decimal::from_str(s).ok());

    if o.get("x").and_then(|x| x.as_str()) == Some("TRADE") {
        let fill_price = Decimal::from_str(o.get("L")?.as_str()?).ok()?;
        let fill_size = Decimal::from_str(o.get("l")?.as_str()?).ok()?;
        let fee = Decimal::from_str(o.get("n").and_then(|x| x.as_str()).unwrap_or("0"))
            .unwrap_or_default();
        Some(UserEvent::Fill(OrderFill {
            trade_id: o.get("t").and_then(|x| x.as_i64()).filter(|t| *t > 0).map(|t| t.to_string()),
            exchange_order_id,
            client_order_id,
            pair,
            side,
            fill_price,
            fill_size,
            fee,
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
            // hedge 方向仓 (032): fapi `o.ps` = LONG/SHORT/BOTH; BOTH(one-way) → None。
            position_side: match o.get("ps").and_then(|x| x.as_str()) {
                Some("LONG") => Some("long".into()),
                Some("SHORT") => Some("short".into()),
                _ => None,
            },
        }))
    } else {
        Some(UserEvent::Order(OrderUpdate {
            exchange_order_id,
            client_order_id,
            pair,
            status,
            filled_size,
            remaining_size: orig_size - filled_size,
            avg_price: avg_price.filter(|p| !p.is_zero()),
            timestamp: DateTime::from_timestamp_millis(ts).unwrap_or_default(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_ws_base_maps_hosts() {
        let mk = |url: &str| {
            FuturesClient::new().expect("client").with_base_url(url).expect("白名单内域名")
        };
        assert_eq!(mk("https://demo-fapi.binance.com").ws_base(), FAPI_DEMO_WS);
        assert_eq!(mk("https://fapi.binance.com").ws_base(), FAPI_MAINNET_WS);
    }

    #[test]
    fn test_parse_order_trade_update_fill() {
        // 实测样例 (2026-09-13 demo 合约市价买入)
        let v: Value = serde_json::json!({
            "e": "ORDER_TRADE_UPDATE", "E": 1_789_290_032_080_i64, "T": 1_789_290_032_079_i64,
            "o": {
                "s": "ETHUSDT", "c": "probe-f-1789290031546", "S": "BUY", "o": "MARKET",
                "q": "0.04", "p": "0", "ap": "2493.70", "x": "TRADE", "X": "FILLED",
                "i": 16793066060_i64, "l": "0.04", "z": "0.04", "L": "2493.70", "n": "0.0997",
                "N": "USDT", "t": 326518260_i64
            }
        });
        match parse_futures_user_event(&v).expect("应解析") {
            UserEvent::Fill(f) => {
                assert_eq!(f.pair, "ETHUSDT");
                assert_eq!(f.client_order_id, "probe-f-1789290031546");
                assert_eq!(f.side, OrderSide::Buy);
                assert_eq!(f.fill_price, dec!(2493.70));
                assert_eq!(f.fill_size, dec!(0.04));
                assert_eq!(f.fee, dec!(0.0997));
                assert_eq!(f.trade_id.as_deref(), Some("326518260"));
                assert_eq!(f.exchange_order_id, "16793066060");
                assert_eq!(f.timestamp.timestamp_millis(), 1_789_290_032_080);
            }
            other => panic!("expected Fill, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_order_trade_update_order_and_unknown_events() {
        let new_order: Value = serde_json::json!({
            "e": "ORDER_TRADE_UPDATE", "E": 1_i64,
            "o": {"s": "ETHUSDT", "c": "cid", "S": "SELL", "o": "LIMIT", "q": "0.5",
                  "x": "NEW", "X": "NEW", "i": 7, "z": "0", "ap": "0", "t": 0}
        });
        match parse_futures_user_event(&new_order).expect("应解析") {
            UserEvent::Order(u) => {
                assert_eq!(u.status, OrderStatus::Open);
                assert_eq!(u.remaining_size, dec!(0.5));
                assert!(u.avg_price.is_none(), "零均价的 ap 应视为 None");
            }
            other => panic!("expected Order, got {other:?}"),
        }

        // TRADE_LITE 与 ORDER_TRADE_UPDATE 重复 → 忽略 (否则成交量翻倍)
        let lite: Value = serde_json::json!({
            "e": "TRADE_LITE", "E": 1_i64, "s": "ETHUSDT", "q": "0.04", "p": "0.00",
            "m": false, "c": "probe-f", "S": "BUY", "L": "2493.70", "l": "0.04",
            "t": 326518260_i64, "i": 16793066060_i64
        });
        assert!(parse_futures_user_event(&lite).is_none(), "TRADE_LITE 必须忽略");

        // 账户/配置类事件不产生策略事件
        for e in ["ACCOUNT_UPDATE", "ACCOUNT_CONFIG_UPDATE", "listenKeyExpired"] {
            assert!(parse_futures_user_event(&serde_json::json!({"e": e})).is_none(), "{e} 应忽略");
        }
        assert!(parse_futures_user_event(&serde_json::json!({"foo": 1})).is_none());
    }
}
