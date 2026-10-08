//! Binance 现货 WebSocket: 盘口 + 用户数据流 (listenKey)。

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::{SinkExt, Stream, StreamExt};
use ricow_core::{
    CoreError, CoreResult, Kline, OrderBook, OrderBookUpdate, OrderFill, OrderSide, OrderStatus,
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

/// REST 盘口快照提供者 (审计 H-4)。
///
/// 返回 `(lastUpdateId, OrderBook)`; `None` = 拉取失败 (降级为无快照的增量模式)。
/// `run_depth_ws` 每次重连成功后调用一次: 先用快照整体替换累计盘口, 再应用其后的
/// 增量 —— 直接把 `@depth` 增量合并进空盘口会得到只剩"重连后变动档"的残缺 book,
/// 策略会以错误价格下单。
pub(crate) type DepthSnapshotFn =
    dyn Fn() -> Pin<Box<dyn Future<Output = Option<(u64, OrderBook)>> + Send>> + Send + Sync;

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

        // 快照闭包 (审计 H-4): 每次重连成功后由 run_depth_ws 调用, REST 拉一次含
        // lastUpdateId 的快照。limit=50 与累计盘口的 MAX_DEPTH_LEVELS 对齐。
        let snap_client = self.clone();
        let snap_symbol = symbol.to_uppercase();
        let snapshot: Arc<DepthSnapshotFn> = Arc::new(move || {
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

    /// 订阅现货 K 线流 (040 FR-1)。
    ///
    /// 流名 `<symbol>@kline_<interval>` (小写标的)。K 线帧是**自带完整 OHLCV 的整根快照**,
    /// 不需要盘口那样的 REST 快照同步与累计合并 —— 帧本身即可直接渲染。
    /// 未收盘的那根会以**同一 `open_time`** 反复推送(原地更新), 收盘后定格; 前端据此
    /// `update` 而非 `append`(见 `web/realtime.rs` 与 `markets.js`)。
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
        // 先签一次: 密钥缺失立即报错 (而非在守护任务里异步失败), 语义与旧版一致。
        self.ws_api_subscribe_params()?;
        let ws_url = self.ws_api_base().to_string();

        let (tx, rx) = mpsc::channel::<UserEvent>(256);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<CoreResult<u64>>();
        let client = self.clone();
        tokio::spawn(async move {
            // 审计 H-3: 传 client 而非一次性签名 —— 重连时重新签名 (旧签名 timestamp
            // 超出 recvWindow 后, 币安必拒, 重连会永远失败)
            run_user_data_ws(client, ws_url, tx, ready_tx).await;
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

/// 重连退避 (指数, 60s 封顶)。
///
/// 040: 提为 `pub(crate)` —— 本文件的盘口/K 线守护与 `futures_ws.rs` 的守护共用这一份;
/// 曾有一份私有副本留在 `futures_ws.rs`, 第三个使用者(K 线守护)出现时一并收编,
/// 免得退避口径分裂成三套 (D8)。
pub(crate) fn backoff_delay(retry_count: u32) -> Duration {
    let base_secs = (1u64 << retry_count.min(6)).min(60);
    Duration::from_secs(base_secs)
}

// ---- Depth WS ----

/// 快照就绪前缓冲的增量帧上限 (100ms 一帧 = 13 分钟余量; 快照请求 30s 内必然返回)。
const SNAPSHOT_PENDING_CAP: usize = 8192;

/// 盘口订阅守护: 断线指数退避重连 (现货市场流 / 合约 fapi 流共用)。
///
/// `depthUpdate` 是**增量 diff**(`b`/`a` 只含变动档, `size=0` 表示删档), 故本函数维护累计盘口;
/// 覆盖式解析会让某一侧变空 → `mid_price()` 返回 None → 策略拿不到价 (2026-09-13 修正)。
///
/// 审计 H-4: 每次重连成功后先经 `snapshot` 回调拉 REST 快照整体重建 book, 再应用其后
/// 增量 —— 增量直接合并进空盘口会得到残缺盘口, 策略会以错误价格下单。
pub(crate) async fn run_depth_ws(
    ws_url: &str,
    symbol: &str,
    tx: mpsc::Sender<OrderBookUpdate>,
    snapshot: Option<Arc<DepthSnapshotFn>>,
) {
    let mut retry = 0u32;
    loop {
        match depth_session(ws_url, symbol, &tx, snapshot.as_ref()).await {
            Ok(()) => break,
            Err(e) => {
                if tx.is_closed() {
                    // 消费端 (live 会话) 已退出: 守护循环随之退出, 不再无限重连。
                    break;
                }
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

/// 消息空闲上限 (审计 H-3): 深度流正常 100ms/帧, 服务器 Ping 最长 3 分钟一次;
/// 超过此时长无任何消息必为 TCP 半开挂死, 触发重连 (半开连接下 `read.next()` 无
/// timeout 会永久挂起, 行情静默中断且引擎侧看门狗只能事后告警)。
const DEPTH_READ_IDLE: Duration = Duration::from_secs(60);

/// 快照就绪后的缓冲重放 (纯逻辑, 便于单测): 以快照 book 为基准, 丢弃 `u <= last_update_id`
/// 的旧帧, 按序应用其余帧, 返回对应的更新序列 (调用方负责推送)。
fn replay_pending(
    book: &mut ricow_core::OrderBook,
    symbol: &str,
    last_update_id: u64,
    pending: &mut Vec<(u64, Value)>,
) -> Vec<OrderBookUpdate> {
    let mut out = Vec::new();
    for (u, v) in pending.drain(..) {
        if u > last_update_id && apply_depth_frame(book, symbol, &v) {
            out.push(OrderBookUpdate {
                pair: symbol.to_string(),
                bids: book.bids.clone(),
                asks: book.asks.clone(),
                timestamp: book.timestamp,
            });
        }
    }
    out
}

async fn depth_session(
    ws_url: &str,
    symbol: &str,
    tx: &mpsc::Sender<OrderBookUpdate>,
    snapshot: Option<&Arc<DepthSnapshotFn>>,
) -> CoreResult<()> {
    let (ws_stream, _) =
        connect_async(ws_url).await.map_err(|e| CoreError::Network(e.to_string()))?;
    let (mut write, mut read) = ws_stream.split();
    let mut book = ricow_core::OrderBook::default();

    // ---- 快照同步 (审计 H-4) ----
    // 连接成功后立即并发拉 REST 快照; 就绪前的增量帧按 `u`(final update id) 缓冲,
    // 快照返回后整体替换 book、丢弃 `u <= lastUpdateId` 的旧帧、按序应用其余。
    // (简化: 不做官方协议的 U/L 交叠细分 —— 帧条目是"该价位最终量"的绝对语义,
    // 单帧重复应用幂等, 边界帧的交叠风险远小于"空盘口合并增量"。)
    // 快照失败 → 退回无快照的增量模式并 warn (与修复前行为一致, 但有告警可查)。
    let mut snap_rx = snapshot.cloned().map(|f| {
        let (stx, srx) = tokio::sync::oneshot::channel::<Option<(u64, OrderBook)>>();
        tokio::spawn(async move {
            let _ = stx.send(f().await);
        });
        srx
    });
    let mut synced = snap_rx.is_none();
    let mut pending: Vec<(u64, Value)> = Vec::new();

    loop {
        // 审计 H-3: 读超时兜底, 半开连接不再永久挂起 (超时 → Err → 上层重连)
        let msg = match tokio::time::timeout(DEPTH_READ_IDLE, read.next()).await {
            Ok(m) => m,
            Err(_) => {
                return Err(CoreError::Network(format!(
                    "盘口流空闲超时 ({}s 无任何消息, 含 Ping), 视为半开连接",
                    DEPTH_READ_IDLE.as_secs()
                )))
            }
        };
        match msg {
            Some(Ok(Message::Text(text))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if !synced {
                        // 快照就绪检查 (try_recv: Err = 请求仍在进行, 继续缓冲)
                        let snap = match snap_rx.as_mut().expect("未同步时必有 snap_rx").try_recv()
                        {
                            Ok(snap) => snap,
                            Err(_) => {
                                // 快照仍在路上: 只缓冲带 `u` 的 depthUpdate 帧
                                if let Some(u) = v.get("u").and_then(Value::as_u64) {
                                    if pending.len() >= SNAPSHOT_PENDING_CAP {
                                        pending.remove(0);
                                    }
                                    pending.push((u, v));
                                }
                                continue;
                            }
                        };
                        synced = true;
                        match snap {
                            Some((last_update_id, snap_book)) => {
                                book = snap_book;
                                tracing::info!(
                                    target: "bn.ws", symbol = %symbol,
                                    "depth snapshot loaded, replaying {} buffered updates",
                                    pending.len()
                                );
                                // 应用快照之后的缓冲帧 (丢弃旧帧)
                                for update in
                                    replay_pending(&mut book, symbol, last_update_id, &mut pending)
                                {
                                    if tx.send(update).await.is_err() {
                                        return Ok(());
                                    }
                                }
                                continue;
                            }
                            None => {
                                tracing::warn!(
                                    target: "bn.ws", symbol = %symbol,
                                    "depth snapshot failed; incremental-only book may be partial"
                                );
                                pending.clear();
                                // synced 已置 true: 落回直接增量的旧行为
                            }
                        }
                    }
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

// ---- Kline WS (040) ----

/// K 线流消息空闲上限。
///
/// 与盘口 (`DEPTH_READ_IDLE` = 60s, 100ms 一帧) 不同, K 线只在**价格/量有变动**时才推:
/// 冷门标的的一根 1d K 线可能几分钟没有一帧。服务器 Ping 最长 3 分钟一次, 故取 5 分钟 ——
/// 超过即必为 TCP 半开 (而不是"这根 K 线正好没动")。
const KLINE_READ_IDLE: Duration = Duration::from_secs(300);

/// 解析一帧 K 线消息为 [`Kline`] (纯函数, 便于单测)。
///
/// 帧形如 `{"e":"kline","E":...,"s":"BTCUSDT","k":{"t":openMs,"T":closeMs,"o":..,"h":..,"l":..,"c":..,"v":..,"x":bool,..}}`。
/// 只认 `e == "kline"`; 数值一律是**字符串**(币安口径) —— 类型不符/字段缺失回 `None` (不猜、不补 0)。
/// 注意: `x`(是否已收盘) **不在此处消费** —— 前端按下标更新同一根即可, 无需区分。
pub(crate) fn parse_kline_frame(v: &Value) -> Option<Kline> {
    if v.get("e").and_then(Value::as_str) != Some("kline") {
        return None;
    }
    let k = v.get("k")?;
    let open_time = DateTime::from_timestamp_millis(k.get("t")?.as_i64()?)?;
    let close_time = DateTime::from_timestamp_millis(k.get("T")?.as_i64()?)?;
    Some(Kline {
        open_time,
        open: Decimal::from_str(k.get("o")?.as_str()?).ok()?,
        high: Decimal::from_str(k.get("h")?.as_str()?).ok()?,
        low: Decimal::from_str(k.get("l")?.as_str()?).ok()?,
        close: Decimal::from_str(k.get("c")?.as_str()?).ok()?,
        volume: Decimal::from_str(k.get("v")?.as_str()?).ok()?,
        close_time,
    })
}

/// K 线订阅守护: 断线指数退避重连 (现货/合约 K 线流共用)。
///
/// 与 [`run_depth_ws`] 的两点不同 (都不需要):
/// - **无 REST 快照同步** —— 每帧 K 线自带整根 OHLCV, 不像盘口增量那样依赖基准 book;
/// - **无累计合并** —— 帧即事实, 直接转发。
///
/// 退出条件同盘口: 消费端退出 (`tx.is_closed()`) 即停止重连, 不泄漏守护任务。
pub(crate) async fn run_kline_ws(ws_url: &str, symbol: &str, tx: mpsc::Sender<Kline>) {
    let mut retry = 0u32;
    loop {
        match kline_session(ws_url, &tx).await {
            Ok(()) => break,
            Err(e) => {
                if tx.is_closed() {
                    break;
                }
                retry += 1;
                let delay = backoff_delay(retry);
                tracing::warn!(
                    target: "bn.ws", symbol = %symbol, error = %e, retry,
                    delay_ms = delay.as_millis(), "kline WS disconnected, reconnecting"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// 单次 K 线连接会话: 连上 → 逐帧解析转发, 直到出错 (交 [`run_kline_ws`] 退避重连)。
async fn kline_session(ws_url: &str, tx: &mpsc::Sender<Kline>) -> CoreResult<()> {
    let (ws_stream, _) =
        connect_async(ws_url).await.map_err(|e| CoreError::Network(e.to_string()))?;
    let (mut write, mut read) = ws_stream.split();

    loop {
        let msg = match tokio::time::timeout(KLINE_READ_IDLE, read.next()).await {
            Ok(m) => m,
            Err(_) => {
                return Err(CoreError::Network(format!(
                    "K 线流空闲超时 ({}s 无任何消息, 含 Ping), 视为半开连接",
                    KLINE_READ_IDLE.as_secs()
                )))
            }
        };
        match msg {
            Some(Ok(Message::Text(text))) => {
                // 订阅确认帧 (`{"result":null,"id":1}`) 等非 K 线消息解析为 None → 跳过。
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if let Some(k) = parse_kline_frame(&v) {
                        if tx.send(k).await.is_err() {
                            return Ok(()); // 消费端已退出
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

/// 消息空闲上限 (审计 H-3): 用户流事件稀疏 (无成交可能长时间无事件帧), 但服务器
/// Ping 最长 3 分钟一次 —— 超过此时长**无任何消息 (含 Ping)** 必为 TCP 半开挂死
/// (网络切换/NAT 超时), `read.next()` 无 timeout 会永久挂起, 行情/成交静默中断。
const USER_DATA_READ_IDLE: Duration = Duration::from_secs(600);
/// 现货用户流守护: 断线指数退避重连 (审计 H-3)。
///
/// - 每轮重连**重新签名**订阅请求 —— 旧签名里的 `timestamp` 超出 recvWindow 后会被
///   币安拒绝, 复用旧请求会让"第一次断线 → 此后每次重连必失败"并无限刷屏;
/// - 消费端退出 (`tx.is_closed()`) 即停止重连, 不再泄漏守护任务。
async fn run_user_data_ws(
    client: BinanceClient,
    ws_url: String,
    tx: mpsc::Sender<UserEvent>,
    ready: tokio::sync::oneshot::Sender<CoreResult<u64>>,
) {
    let mut ready = Some(ready);
    let mut retry = 0u32;
    loop {
        if tx.is_closed() {
            return; // 消费端 (live 会话) 已退出: 不再重连
        }
        // 每次连接都是新会话 → 必须重新签名 + 重新发订阅请求 (WS-API 连接有 24h 上限与空闲断开)
        let request = match client.ws_api_subscribe_params() {
            Ok(p) => build_subscribe_request(&p),
            Err(e) => {
                // 签名失败 (密钥缺失): 首连交给调用方; 重连期 warn 后退避重试
                if let Some(r) = ready.take() {
                    let _ = r.send(Err(e));
                    return;
                }
                tracing::warn!(target: "bn.ws", error = %e, "重新签名用户流订阅请求失败, 稍后重试");
                tokio::time::sleep(backoff_delay(retry.saturating_add(1))).await;
                continue;
            }
        };
        match user_data_session(&ws_url, &request, &tx, &mut ready).await {
            Ok(()) => break,
            Err(e) => {
                if let Some(r) = ready.take() {
                    // 首次握手就失败: 把错误直接交给调用方 (不重试, 避免"静默重连 + 永远不就绪")
                    let _ = r.send(Err(e));
                    return;
                }
                if tx.is_closed() {
                    break;
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
            position_side: None, // 现货无方向仓概念 (032)
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
            // hedge 方向仓 (032): fapi `o.ps` = LONG/SHORT/BOTH; BOTH(one-way) → None。
            position_side: match o.get("ps").and_then(|x| x.as_str()) {
                Some("LONG") => Some("long".into()),
                Some("SHORT") => Some("short".into()),
                _ => None,
            },
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

    /// 审计 H-4 回归: 快照重放必须丢弃 `u <= lastUpdateId` 的旧帧、按序应用其后帧 ——
    /// 否则旧增量会把快照后的价位改回断线前的值 (残缺盘口的另一种形态)。
    #[test]
    fn test_replay_pending_drops_frames_at_or_before_snapshot() {
        // 快照: bid 3000/1.0 (lastUpdateId = 10)
        let mut book = ricow_core::OrderBook {
            bids: vec![ricow_core::PriceLevel { price: dec!(3000), size: dec!(1.0) }],
            asks: vec![],
            timestamp: Utc::now(),
        };
        // 缓冲: u=5 (旧帧, 应丢弃) / u=12 (应应用, 3000→2.0) / u=15 (应应用, 加 3001 卖档)
        let mut pending: Vec<(u64, Value)> = vec![
            (5, serde_json::json!({"e": "depthUpdate", "u": 5, "b": [["3000.0", "99"]], "a": []})),
            (
                12,
                serde_json::json!({"e": "depthUpdate", "u": 12, "b": [["3000.0", "2.0"]], "a": []}),
            ),
            (
                15,
                serde_json::json!({"e": "depthUpdate", "u": 15, "b": [], "a": [["3001.0", "3.0"]]}),
            ),
        ];
        let updates = replay_pending(&mut book, "ETHUSDT", 10, &mut pending);
        assert_eq!(updates.len(), 2, "快照前的帧不得产生更新");
        // 旧帧 (u=5) 被丢弃: 3000 绝不是旧帧写入的 99 (快照 1.0 → 新帧 2.0)
        assert_ne!(book.bids[0].size, dec!(99));
        // 新帧按序应用
        assert_eq!(book.bids[0].size, dec!(2.0));
        assert_eq!(book.asks[0].price, dec!(3001));
        assert!(pending.is_empty(), "重放必须清空缓冲");
        // 更新帧携带重放后的完整盘口
        assert_eq!(updates[1].bids[0].size, dec!(2.0));
        assert_eq!(updates[1].asks[0].price, dec!(3001));
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

    /// 040 T001: K 线帧解析 (现货)。
    ///
    /// 帧体按**币安官方 kline stream 字段口径**构造 (字段名/字符串数值/`x` 收盘标志),
    /// 数值是编的 —— 真实抓取的帧在 T008 真机取证时替换进来 (此处如实标注, 不假装真机数据)。
    #[test]
    fn test_parse_kline_frame_from_real_spot_frame() {
        let v: Value = serde_json::from_str(
            r#"{"e":"kline","E":1759808372860,"s":"ETHUSDT",
                "k":{"t":1759806000000,"T":1759809599999,"s":"ETHUSDT","i":"1h",
                     "o":"2497.43000000","c":"2501.09000000","h":"2510.00000000",
                     "l":"2488.12000000","v":"12345.67800000","q":"30000000.0",
                     "n":4321,"x":false,"V":"6000.0"}}"#,
        )
        .unwrap();
        let k = parse_kline_frame(&v).expect("真实 K 线帧应解析成功");
        assert_eq!(k.open_time.timestamp_millis(), 1_759_806_000_000);
        assert_eq!(k.close_time.timestamp_millis(), 1_759_809_599_999);
        assert_eq!(k.open, dec!(2497.43));
        assert_eq!(k.close, dec!(2501.09));
        assert_eq!(k.high, dec!(2510.00));
        assert_eq!(k.low, dec!(2488.12));
        assert_eq!(k.volume, dec!(12345.678));
    }

    /// 合约 fstream 的 K 线帧与现货**同构** —— 同一份解析必须都能吃下 (否则合约页面
    /// 会静默无数据)。帧体同样按官方口径构造, T008 用真机帧替换。
    #[test]
    fn test_parse_kline_frame_from_real_futures_frame() {
        let v: Value = serde_json::from_str(
            r#"{"e":"kline","E":1759808372861,"s":"BTCUSDT",
                "k":{"t":1759806000000,"T":1759809599999,"s":"BTCUSDT","i":"1h",
                     "o":"121500.10","c":"121777.20","h":"121900.00","l":"121300.00",
                     "v":"8888.888","x":true}}"#,
        )
        .unwrap();
        let k = parse_kline_frame(&v).expect("合约 K 线帧应解析成功");
        assert_eq!(k.open_time.timestamp_millis(), 1_759_806_000_000);
        assert_eq!(k.close, dec!(121777.20));
        assert_eq!(k.volume, dec!(8888.888));
    }

    /// 非 K 线消息与残缺帧一律 `None` —— 不猜、不补 0、不拿默认值顶替。
    #[test]
    fn test_parse_kline_frame_rejects_non_kline_and_malformed() {
        // 订阅确认帧 (`{"result":null,"id":1}`): 真实会先到, 必须被忽略而不是当成 K 线。
        assert!(parse_kline_frame(&serde_json::json!({"result": null, "id": 1})).is_none());
        // 盘口帧混进同一条连接时也不该被误读。
        assert!(
            parse_kline_frame(&serde_json::json!({"e": "depthUpdate", "b": [], "a": []})).is_none()
        );
        // 缺 `k`。
        assert!(parse_kline_frame(&serde_json::json!({"e": "kline"})).is_none());
        // `k` 在但缺价 / 时间。
        assert!(parse_kline_frame(&serde_json::json!({"e": "kline", "k": {"t": 1}})).is_none());
        // 价格不是字符串 (币安口径一律字符串) → 拒绝而非静默按 0 处理。
        assert!(parse_kline_frame(&serde_json::json!({
            "e": "kline",
            "k": {"t": 1_759_806_000_000_i64, "T": 1_759_809_599_999_i64,
                  "o": 2497.43, "h": "1", "l": "1", "c": "1", "v": "1"}
        }))
        .is_none());
        // 时间越界 (毫秒时间戳放不进 DateTime) → 拒绝。
        assert!(parse_kline_frame(&serde_json::json!({
            "e": "kline",
            "k": {"t": i64::MAX, "T": i64::MAX, "o": "1", "h": "1", "l": "1", "c": "1", "v": "1"}
        }))
        .is_none());
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
        let mk = |url: &str| {
            BinanceClient::new().expect("client").with_base_url(url).expect("白名单内域名")
        };
        assert_eq!(mk("https://demo-api.binance.com").ws_api_base(), SPOT_WS_API_DEMO);
        assert_eq!(mk("https://api.binance.com").ws_api_base(), SPOT_WS_API_MAINNET);
        assert_eq!(mk("https://testnet.binance.vision").ws_api_base(), SPOT_WS_API_TESTNET);
    }

    #[test]
    fn test_ws_base_maps_demo_and_testnet_hosts() {
        let mk = |url: &str| {
            BinanceClient::new().expect("client").with_base_url(url).expect("白名单内域名")
        };
        assert_eq!(
            mk("https://demo-api.binance.com").ws_base(),
            SPOT_DEMO_WS,
            "demo 必须走 demo-stream (主网 WS 不认 demo listenKey)"
        );
        assert_eq!(mk("https://api.binance.com").ws_base(), SPOT_MAINNET_WS);
        assert_eq!(mk("https://testnet.binance.vision").ws_base(), SPOT_TESTNET_WS);
    }

    #[test]
    fn test_base_url_whitelist_blocks_foreign_hosts() {
        // 审计 H-6: 环境变量/显式覆盖指向白名单外主机 → 构造期硬失败。
        for bad in ["https://evil.com", "http://169.254.169.254", "https://binance.com.evil.io"] {
            assert!(
                BinanceClient::new().unwrap().with_base_url(bad).is_err(),
                "{bad} 不应通过白名单"
            );
        }
        // 白名单内: 币安系域名 + 回环。
        for ok in [
            "https://api.binance.com",
            "https://demo-api.binance.com",
            "https://testnet.binance.vision",
            "http://127.0.0.1:8080",
        ] {
            assert!(BinanceClient::new().unwrap().with_base_url(ok).is_ok(), "{ok} 应放行");
        }
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
