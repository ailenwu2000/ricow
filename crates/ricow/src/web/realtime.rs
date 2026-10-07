//! 040 行情实时化: 币安 WS → SSE。
//!
//! ## 为什么是 SSE
//!
//! 页面只需**单向**接收行情，SSE 复用既有 token 中间件与前端 `R.sse`（退避 + 抖动）底座，
//! 不新开端口、不引新依赖。帧约定沿用会话流：默认 `message` 事件 + JSON 帧体 + 按 `type` 分派。
//!
//! ## 订阅复用（本模块的核心）
//!
//! 上游币安连接按 **`(市场,标的,周期)`**（K 线）/ **`(市场,标的)`**（盘口）**共享** ——
//! 同键的 N 个页面共用一条上游，最后一个订阅者离开即中止上游（FR-6 ~ FR-8）。
//!
//! ## 两条实测依据（决定了本模块的形状，见 plan.md §二.0）
//!
//! 1. **不存在的标的**：币安照常完成 WS 握手、然后**一帧不发**、也不断开 —— 没有"错误帧"
//!    可解析（解析它是死代码）；真正需要的是**标的预校验**（同步 404）+ **就绪窗口**。
//! 2. **合约 K 线流实测零帧**（同轮合约盘口 305 帧、合约 REST 200，三次复现，原因未确定）
//!    —— 故"某一路没数据"必须**按路报错**，不能让整页看起来像在实时。

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path as UrlPath, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures::{Stream, StreamExt as _};
use ricow_core::{Kline, OrderBookUpdate};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use super::markets::{normalize_symbol, parse_interval, parse_market};
use super::WebError;
use super::WebState;

/// 广播容量: K 线 ~2s/帧、盘口 ~100ms/帧 → 256 约合盘口 25s 余量。
const BROADCAST_CAP: usize = 256;
/// 下发给页面的盘口档数 —— 与盘口面板展示档数一致（`markets.js` 用 depth=20）。
/// 上游累计盘口有 50 档，原样推给浏览器是纯浪费（D6）。
const DEPTH_LEVELS: usize = 20;
/// 单路**就绪窗口**: 上游连上后这么久仍无任何数据帧，就如实报该路"无数据"。
///
/// 依据实测节奏（建连 ~4s + K 线首帧 ≤2s / 盘口首帧 ~4s）：12s 既不误报，
/// 又能在"合约 K 线零帧"这类真问题上很快给用户说法，而不是让图默默冻住。
const READY_WINDOW: Duration = Duration::from_secs(12);

// ---- 下行帧（SSE 协议） ----

/// 下行帧: 统一走默认 `message` 事件，帧体据此 `type` 分派（与会话流同约定）。
/// 字段名偏短 —— 盘口 ~100ms 一帧，帧体大小直接决定浏览器解析开销。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    /// 握手: 告诉页面"实际订到了什么"（页面据此亮实时标识、清该路的旧错误）。
    Hello { market: &'static str, symbol: String, interval: String },
    /// K 线增量: 同一根未收盘的 K 线会以**同一个 `t`** 反复推送（前端原地更新）。
    Kline(KlineFrame),
    /// 盘口快照: 上游 `OrderBookUpdate` 已是**累计**语义 → 页面整体替换即可，无需自己合并。
    Depth(DepthFrame),
    /// 某一路没有数据。`scope` = `kline` / `depth`; `msg` 必须带**可核对的实情**。
    Error { scope: &'static str, msg: String },
}

/// K 线帧: 时间用 epoch **毫秒**（与 REST `Kline.open_time` 的 ISO 串在前端都落到同一 epoch 秒）。
#[derive(Debug, Clone, serde::Serialize)]
struct KlineFrame {
    /// 开盘时间 (epoch ms)。
    t: i64,
    /// 收盘时间 (epoch ms)。
    #[serde(rename = "T")]
    close_t: i64,
    o: String,
    h: String,
    l: String,
    c: String,
    v: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct DepthFrame {
    /// 上游帧时间 (epoch ms)。
    #[serde(rename = "E")]
    at: i64,
    bids: Vec<Level>,
    asks: Vec<Level>,
}

/// 盘口档位: 与 REST `orderbook` 端点**同字段名**（`price` / `size`），
/// 页面盘口表两条数据来源共用同一个渲染函数。
#[derive(Debug, Clone, serde::Serialize)]
struct Level {
    price: String,
    size: String,
}

impl Frame {
    fn encode(&self) -> String {
        // 帧结构为本模块独有、字段全由我们自己写 —— 序列化失败不可能发生。
        serde_json::to_string(self).unwrap_or_else(|e| {
            tracing::error!(target: "web.market", error = %e, "行情帧序列化失败");
            r#"{"type":"error","scope":"kline","msg":"帧序列化失败"}"#.to_string()
        })
    }
}

// ---- 订阅表 ----

/// 上游广播的消息: 数据帧或**该路不可用时的一句实情**。
///
/// 用同一个频道承载两类消息，页面就不需要"数据频道的错误侧信道"——
/// 这也是唯一能让页面区分"没数据"与"数据没变化"的做法。
#[derive(Debug, Clone)]
enum Tick<T> {
    Data(T),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct KlineKey {
    market: &'static str,
    symbol: String,
    interval: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DepthKey {
    market: &'static str,
    symbol: String,
}

/// 一份在跑的上游订阅。
struct Sub<T> {
    tx: broadcast::Sender<Tick<T>>,
    /// 活跃订阅者数。**显式计数**, 不用 `broadcast::Sender::receiver_count()` ——
    /// [`Lease`] 与 `Receiver` 属于同一次 drop、先后顺序无保证, 靠 `receiver_count()`
    /// 会在"先 drop 租约"时看到自己那条尚未释放的接收端, 于是永远不拆。
    leases: usize,
    /// 上游守护任务句柄: 计数归零即 `abort()`, 立刻释放币安连接（FR-7）。
    abort: AbortHandle,
}

#[derive(Default)]
struct HubState {
    klines: HashMap<KlineKey, Sub<Kline>>,
    depth: HashMap<DepthKey, Sub<OrderBookUpdate>>,
}

/// 槽位判定结果（**纯记账，不碰网络** —— 于是这部分可以无网单测）。
enum Slot<T> {
    /// 有活条目可复用。
    Reuse(broadcast::Receiver<Tick<T>>, Lease),
    /// 没有可复用的：条目不存在，或死条目已被中止并移除 → 调用方负责建上游。
    Rebuild,
}

/// 行情订阅中枢: 进程内**唯一**一份（挂在 [`WebState`] 上）。
pub(crate) struct MarketHub {
    state: Arc<Mutex<HubState>>,
}

impl MarketHub {
    pub(crate) fn new() -> Self {
        Self { state: Arc::new(Mutex::new(HubState::default())) }
    }

    /// 订阅 K 线；同键复用，首个订阅者拉起上游。
    ///
    /// 结构是**双检**: ① 锁内快路径复用（无 `await`，故可持锁）→ ② 锁外建上游
    /// （`subscribe_klines` 是 async，**绝不能持 `std::Mutex` 跨 await** —— 否则
    /// handler 的 future 不是 `Send`，axum 直接不收）→ ③ [`Self::install_kline`] 锁内双检后落表。
    async fn subscribe_klines(
        &self,
        market: &'static str,
        symbol: &str,
        interval: &str,
    ) -> Result<(broadcast::Receiver<Tick<Kline>>, Lease), String> {
        let key = KlineKey { market, symbol: symbol.to_string(), interval: interval.to_string() };
        if let Slot::Reuse(rx, lease) = self.slot_kline(&key) {
            return Ok((rx, lease));
        }
        let stream = match market {
            "futures" => {
                let c = ricow_binance::FuturesClient::new().map_err(|e| e.to_string())?;
                c.subscribe_klines(symbol, interval).await.map_err(|e| e.to_string())?
            }
            _ => {
                let c = ricow_binance::BinanceClient::new().map_err(|e| e.to_string())?;
                c.subscribe_klines(symbol, interval).await.map_err(|e| e.to_string())?
            }
        };
        Ok(self.install_kline(key, stream))
    }

    /// 订阅盘口；同键复用，首个订阅者拉起上游（与 [`Self::subscribe_klines`] 同构）。
    async fn subscribe_depth(
        &self,
        market: &'static str,
        symbol: &str,
    ) -> Result<(broadcast::Receiver<Tick<OrderBookUpdate>>, Lease), String> {
        let key = DepthKey { market, symbol: symbol.to_string() };
        if let Slot::Reuse(rx, lease) = self.slot_depth(&key) {
            return Ok((rx, lease));
        }
        let stream = match market {
            "futures" => {
                let c = ricow_binance::FuturesClient::new().map_err(|e| e.to_string())?;
                c.subscribe_depth(symbol).await.map_err(|e| e.to_string())?
            }
            _ => {
                let c = ricow_binance::BinanceClient::new().map_err(|e| e.to_string())?;
                c.subscribe_depth(symbol).await.map_err(|e| e.to_string())?
            }
        };
        Ok(self.install_depth(key, stream))
    }

    /// 锁内复用判定: 有活条目就 +1 并交出接收端；计数为 0 的**残留死条目**一律中止重建（FR-8）。
    fn slot_kline(&self, key: &KlineKey) -> Slot<Kline> {
        let mut st = self.state.lock().expect("行情订阅表锁未中毒");
        let Some(sub) = st.klines.get_mut(key) else { return Slot::Rebuild };
        if sub.leases == 0 {
            // 上游已死但条目未清: 绝不能把新订阅者挂到一条死管道上。
            sub.abort.abort();
            st.klines.remove(key);
            return Slot::Rebuild;
        }
        sub.leases += 1;
        let tx = sub.tx.clone();
        Slot::Reuse(
            tx.subscribe(),
            Lease {
                state: Arc::clone(&self.state),
                release: Some(Release::Kline { key: key.clone(), tx }),
            },
        )
    }

    fn slot_depth(&self, key: &DepthKey) -> Slot<OrderBookUpdate> {
        let mut st = self.state.lock().expect("行情订阅表锁未中毒");
        let Some(sub) = st.depth.get_mut(key) else { return Slot::Rebuild };
        if sub.leases == 0 {
            sub.abort.abort();
            st.depth.remove(key);
            return Slot::Rebuild;
        }
        sub.leases += 1;
        let tx = sub.tx.clone();
        Slot::Reuse(
            tx.subscribe(),
            Lease {
                state: Arc::clone(&self.state),
                release: Some(Release::Depth { key: key.clone(), tx }),
            },
        )
    }

    /// 双检后落表: 建上游期间别人可能已建好 → 用别人的（自己这条 stream 丢弃即结束）。
    fn install_kline(
        &self,
        key: KlineKey,
        stream: Pin<Box<dyn Stream<Item = Kline> + Send>>,
    ) -> (broadcast::Receiver<Tick<Kline>>, Lease) {
        let mut st = self.state.lock().expect("行情订阅表锁未中毒");
        if let Some(sub) = st.klines.get_mut(&key) {
            if sub.leases > 0 {
                sub.leases += 1;
                let tx = sub.tx.clone();
                return (
                    tx.subscribe(),
                    Lease {
                        state: Arc::clone(&self.state),
                        release: Some(Release::Kline { key, tx }),
                    },
                );
            }
            sub.abort.abort();
            st.klines.remove(&key);
        }
        let (tx, rx) = broadcast::channel(BROADCAST_CAP);
        let abort = spawn_forwarder(stream, tx.clone(), "kline");
        st.klines.insert(key.clone(), Sub { tx: tx.clone(), leases: 1, abort });
        (rx, Lease { state: Arc::clone(&self.state), release: Some(Release::Kline { key, tx }) })
    }

    fn install_depth(
        &self,
        key: DepthKey,
        stream: Pin<Box<dyn Stream<Item = OrderBookUpdate> + Send>>,
    ) -> (broadcast::Receiver<Tick<OrderBookUpdate>>, Lease) {
        let mut st = self.state.lock().expect("行情订阅表锁未中毒");
        if let Some(sub) = st.depth.get_mut(&key) {
            if sub.leases > 0 {
                sub.leases += 1;
                let tx = sub.tx.clone();
                return (
                    tx.subscribe(),
                    Lease {
                        state: Arc::clone(&self.state),
                        release: Some(Release::Depth { key, tx }),
                    },
                );
            }
            sub.abort.abort();
            st.depth.remove(&key);
        }
        let (tx, rx) = broadcast::channel(BROADCAST_CAP);
        let abort = spawn_forwarder(stream, tx.clone(), "depth");
        st.depth.insert(key.clone(), Sub { tx: tx.clone(), leases: 1, abort });
        (rx, Lease { state: Arc::clone(&self.state), release: Some(Release::Depth { key, tx }) })
    }
}

/// 把上游流转发进广播频道（上游任务的生命周期 = 该键的订阅生命周期）。
///
/// 退出条件:
/// - 上游流结束 → 推一帧"上游连接已结束"的错误帧（页面据此把该路标成不可用），然后退出；
/// - 广播无人接收（`send` 报错）→ 退出（与 [`Lease`] 的 `abort` 是双保险）。
fn spawn_forwarder<T>(
    stream: Pin<Box<dyn Stream<Item = T> + Send>>,
    tx: broadcast::Sender<Tick<T>>,
    scope: &'static str,
) -> AbortHandle
where
    T: Clone + Send + 'static,
{
    let handle = tokio::spawn(async move {
        use futures::StreamExt as _;
        let mut stream = stream;
        let mut ready = false;
        loop {
            let next = if ready {
                stream.next().await
            } else {
                match tokio::time::timeout(READY_WINDOW, stream.next()).await {
                    Ok(v) => v,
                    Err(_) => {
                        // 连上了但窗口内一帧未到 —— 实测这就是"标的不存在/该流不推数据"的表现。
                        // 如实报一次, **不退出**: 数据只是晚到的话, 到了就自然恢复。
                        let msg =
                            format!("上游已连接但 {} 秒内未推送任何数据", READY_WINDOW.as_secs());
                        tracing::warn!(
                            target: "web.market", scope, window_s = READY_WINDOW.as_secs(),
                            "上游订阅无数据帧"
                        );
                        ready = true;
                        if tx.send(Tick::Error(msg)).is_err() {
                            return;
                        }
                        continue;
                    }
                }
            };
            match next {
                Some(item) => {
                    ready = true;
                    if tx.send(Tick::Data(item)).is_err() {
                        return;
                    }
                }
                None => {
                    tracing::warn!(target: "web.market", scope, "上游流结束, 转发任务退出");
                    let _ = tx.send(Tick::Error("上游连接已结束".to_string()));
                    return;
                }
            }
        }
    });
    handle.abort_handle()
}

/// 订阅租约: 页面断开即归还；计数归零 → 中止上游 + 移除条目（FR-7）。
pub(crate) struct Lease {
    state: Arc<Mutex<HubState>>,
    /// `None` = 上游压根没建起来（订阅失败），没什么可还的。
    release: Option<Release>,
}

/// 归还动作: 带上"还回哪张表"与**身份**（`tx`）。
enum Release {
    Kline { key: KlineKey, tx: broadcast::Sender<Tick<Kline>> },
    Depth { key: DepthKey, tx: broadcast::Sender<Tick<OrderBookUpdate>> },
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(release) = self.release.take() else { return };
        let mut st = self.state.lock().expect("行情订阅表锁未中毒");
        match release {
            Release::Kline { key, tx } => {
                let Some(sub) = st.klines.get_mut(&key) else { return };
                // 身份校验: 表里可能已换成后来者新建的那条, 别误拆别人的。
                if !sub.tx.same_channel(&tx) {
                    return;
                }
                sub.leases = sub.leases.saturating_sub(1);
                if sub.leases == 0 {
                    let sub = st.klines.remove(&key).expect("刚查到的条目");
                    sub.abort.abort();
                }
            }
            Release::Depth { key, tx } => {
                let Some(sub) = st.depth.get_mut(&key) else { return };
                if !sub.tx.same_channel(&tx) {
                    return;
                }
                sub.leases = sub.leases.saturating_sub(1);
                if sub.leases == 0 {
                    let sub = st.depth.remove(&key).expect("刚查到的条目");
                    sub.abort.abort();
                }
            }
        }
    }
}

// ---- HTTP ----

pub(super) fn routes() -> Router<WebState> {
    Router::new().route("/api/markets/{symbol}/stream", get(market_stream))
}

#[derive(Debug, Deserialize)]
pub(super) struct StreamQuery {
    market: Option<String>,
    interval: Option<String>,
}

/// `GET /api/markets/{symbol}/stream?market=&interval=`: 一条 SSE 同时推 K 线与盘口。
///
/// 参数在**联网之前**全量校验：`market` / `interval` 必填（订阅是长承诺，静默默认会让
/// "我传了 4h、它订了 1h"只在界面上表现为"图不对"），标的不在视野内直接 404 ——
/// 实测币安对不存在的标的**照常握手然后一帧不发**，不预校验就只能靠超时兜（§二.0）。
pub(super) async fn market_stream(
    State(state): State<WebState>,
    UrlPath(symbol): UrlPath<String>,
    query: Result<Query<StreamQuery>, QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, WebError> {
    let Query(q) = query.map_err(|e| WebError::bad_request(format!("查询参数非法: {e}")))?;
    let market =
        parse_market(q.market.as_deref(), true).map_err(WebError::bad_request)?.unwrap_or("spot");
    let raw_interval = q.interval.as_deref().map(str::trim).unwrap_or("");
    if raw_interval.is_empty() {
        return Err(WebError::bad_request("interval 必填(如 1h), 实时订阅不接受缺省周期"));
    }
    let interval = parse_interval(Some(raw_interval)).map_err(WebError::bad_request)?;
    let symbol = normalize_symbol(&symbol);

    // 标的预校验: 必须在**所请求市场的全量交易对**里, 否则 404（而不是连上后默默无数据）。
    //
    // 用 `force_all = true`: 校验的是"交易所是否真有这个标的", 而不是用户的**当前视野开关**
    // —— 视野是页面的展示偏好, 不该决定"这个流能不能订"（否则用户一关视野开关, 已打开的
    // 详情页就会莫名断开）。这也是实测逼出来的：币安对不存在的标的照常握手然后一帧不发。
    let view = crate::commands::pairs::current_view(&state.root, true).await?;
    let universe = match market {
        "futures" => &view.futures,
        _ => &view.spot,
    };
    if !universe.iter().any(|s| s == &symbol) {
        let label = if market == "futures" { "合约" } else { "现货" };
        return Err(WebError::not_found(format!(
            "{label}市场没有交易对 {symbol} (实时订阅只接受行情视野内的交易对)"
        )));
    }

    let mut head =
        vec![Frame::Hello { market, symbol: symbol.clone(), interval: interval.to_string() }];
    let hub = &state.realtime;

    let kline_side: Pin<Box<dyn Stream<Item = Frame> + Send>> =
        match hub.subscribe_klines(market, &symbol, interval).await {
            Ok((rx, lease)) => Box::pin(tick_stream(rx, lease, "kline", |k: Kline| {
                Frame::Kline(KlineFrame {
                    t: k.open_time.timestamp_millis(),
                    close_t: k.close_time.timestamp_millis(),
                    o: k.open.to_string(),
                    h: k.high.to_string(),
                    l: k.low.to_string(),
                    c: k.close.to_string(),
                    v: k.volume.to_string(),
                })
            })),
            Err(msg) => {
                head.push(Frame::Error { scope: "kline", msg });
                Box::pin(futures::stream::empty())
            }
        };

    let depth_side: Pin<Box<dyn Stream<Item = Frame> + Send>> =
        match hub.subscribe_depth(market, &symbol).await {
            Ok((rx, lease)) => Box::pin(tick_stream(rx, lease, "depth", |u: OrderBookUpdate| {
                Frame::Depth(DepthFrame {
                    at: u.timestamp.timestamp_millis(),
                    bids: top_levels(&u.bids),
                    asks: top_levels(&u.asks),
                })
            })),
            Err(msg) => {
                head.push(Frame::Error { scope: "depth", msg });
                Box::pin(futures::stream::empty())
            }
        };

    let frames = futures::stream::iter(head).chain(futures::stream::select(kline_side, depth_side));
    let events = frames.map(|f| Ok(Event::default().data(f.encode())));
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// 广播接收端 → 帧流；租约随流同生共死（流被 drop = 页面断开 → 归还订阅）。
///
/// 分两步映射（`unfold` 只产出 [`Tick`]，再用 `map` 转成 [`Frame`]）—— 这样 `map` 闭包是
/// **被 move 进流里的自有值**，而不是被 `async` 块捕获的引用（后者不 `Send`，handler 收不了）。
///
/// `Lagged` 与会话流同口径: 跳过并 warn, **不补发**（实时视图宁缺勿错）。
fn tick_stream<T, F>(
    rx: broadcast::Receiver<Tick<T>>,
    lease: Lease,
    scope: &'static str,
    map: F,
) -> impl Stream<Item = Frame> + Send
where
    T: Clone + Send + 'static,
    F: Fn(T) -> Frame + Send + 'static,
{
    let ticks = futures::stream::unfold((rx, lease), move |(mut rx, lease)| async move {
        loop {
            match rx.recv().await {
                Ok(tick) => return Some((tick, (rx, lease))),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        target: "web.market", scope, skipped,
                        "行情 SSE 帧滞后, 已跳过 {skipped} 帧 (实时视图宁缺勿错)"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    ticks.map(move |tick| match tick {
        Tick::Data(item) => map(item),
        Tick::Error(msg) => Frame::Error { scope, msg },
    })
}

/// 截断到面板展示档数（上游累计盘口 50 档）。
fn top_levels(levels: &[ricow_core::PriceLevel]) -> Vec<Level> {
    levels
        .iter()
        .take(DEPTH_LEVELS)
        .map(|l| Level { price: l.price.to_string(), size: l.size.to_string() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(ms: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(ms).expect("合法时间戳")
    }

    fn kline(close_ms: i64) -> Kline {
        let d = rust_decimal::Decimal::ONE;
        Kline {
            open_time: ts(1_759_806_000_000),
            open: d,
            high: d,
            low: d,
            close: d,
            volume: d,
            close_time: ts(close_ms),
        }
    }

    /// 一条**永远沉默**的上游流（不是 Exchange 替身: 只用来测转发/超时记账, 不碰交易流程）。
    fn silent_kline() -> Pin<Box<dyn Stream<Item = Kline> + Send>> {
        Box::pin(futures::stream::pending())
    }

    /// 手搓一个"上游已拉起"的活条目（leases = 1 代表首位订阅者）。
    fn seed_live_kline(hub: &MarketHub, key: &KlineKey) -> tokio::task::JoinHandle<()> {
        let task = tokio::spawn(async { std::future::pending::<()>().await });
        let (tx, _) = broadcast::channel::<Tick<Kline>>(8);
        hub.state
            .lock()
            .unwrap()
            .klines
            .insert(key.clone(), Sub { tx, leases: 1, abort: task.abort_handle() });
        task
    }

    /// 断言任务**确实被中止**。
    ///
    /// `abort()` 只是排队取消, `is_finished()` 会滞后一两拍 —— 直接 await 到任务真正结束,
    /// 再加超时兜底（真没被中止 = 一直 pending → 超时即失败）。
    async fn assert_aborted(task: tokio::task::JoinHandle<()>) {
        match tokio::time::timeout(Duration::from_secs(1), task).await {
            Ok(Err(e)) => assert!(e.is_cancelled(), "任务应被 abort, 实际: {e:?}"),
            Ok(Ok(())) => panic!("任务应被 abort, 却正常结束了"),
            Err(_) => panic!("任务没被 abort (1s 内仍在运行)"),
        }
    }

    // ---- 帧口径 ----

    #[test]
    fn test_kline_frame_serializes_with_short_keys_and_string_numbers() {
        let f = Frame::Kline(KlineFrame {
            t: 1_759_806_000_000,
            close_t: 1_759_809_599_999,
            o: "2497.43".into(),
            h: "2510.00".into(),
            l: "2488.12".into(),
            c: "2501.09".into(),
            v: "12345.678".into(),
        });
        let v: serde_json::Value = serde_json::from_str(&f.encode()).unwrap();
        assert_eq!(v["type"], "kline");
        assert_eq!(v["t"], 1_759_806_000_000_i64);
        assert_eq!(v["T"], 1_759_809_599_999_i64);
        // 数值一律字符串 —— 与 REST klines 端点同口径, 前端 `Number(...)` 两路共用。
        assert!(v["o"].is_string() && v["c"].is_string() && v["v"].is_string());
    }

    /// 盘口帧必须**截断到面板档数**（D6）；字段名与 REST orderbook 一致，
    /// 页面盘口表才能两条来源共用一个渲染函数。
    #[test]
    fn test_depth_frame_truncates_and_matches_rest_field_names() {
        let levels: Vec<ricow_core::PriceLevel> = (0..50)
            .map(|i| ricow_core::PriceLevel {
                price: rust_decimal::Decimal::from(3000 + i),
                size: rust_decimal::Decimal::ONE,
            })
            .collect();
        let frame = Frame::Depth(DepthFrame {
            at: 1_791_341_760_000,
            bids: top_levels(&levels),
            asks: top_levels(&levels),
        });
        let v: serde_json::Value = serde_json::from_str(&frame.encode()).unwrap();
        assert_eq!(v["type"], "depth");
        assert_eq!(v["bids"].as_array().unwrap().len(), DEPTH_LEVELS, "超出展示档数应截断");
        assert_eq!(v["asks"].as_array().unwrap().len(), DEPTH_LEVELS);
        assert_eq!(v["bids"][0]["price"], "3000", "字段名须与 REST orderbook 一致");
        assert!(v["bids"][0]["size"].is_string());
    }

    #[test]
    fn test_hello_and_error_frame_shape() {
        let hello =
            Frame::Hello { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        let v: serde_json::Value = serde_json::from_str(&hello.encode()).unwrap();
        assert_eq!(v["type"], "hello");
        assert_eq!(v["market"], "spot");
        assert_eq!(v["interval"], "1h");

        let err = Frame::Error {
            scope: "kline", msg: "上游已连接但 12 秒内未推送任何数据".into()
        };
        let v: serde_json::Value = serde_json::from_str(&err.encode()).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["scope"], "kline");
        assert!(v["msg"].as_str().unwrap().contains("12 秒"), "错误帧必须带可核对的实情");
    }

    // ---- 订阅复用与生命周期 (FR-6 / FR-7 / FR-8) ----

    /// FR-6: 同键第二次订阅**复用**同一条上游, 计数递增, 不新建第二条。
    #[tokio::test]
    async fn test_slot_reuses_same_upstream_and_counts() {
        let hub = MarketHub::new();
        let key = KlineKey { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        seed_live_kline(&hub, &key);

        let Slot::Reuse(_rx, lease) = hub.slot_kline(&key) else { panic!("应复用") };
        assert_eq!(hub.state.lock().unwrap().klines.len(), 1, "同键不得出现第二条");
        assert_eq!(hub.state.lock().unwrap().klines[&key].leases, 2);
        // 再来一个: 仍是同一条。
        let Slot::Reuse(_rx2, lease2) = hub.slot_kline(&key) else { panic!("应复用") };
        assert_eq!(hub.state.lock().unwrap().klines[&key].leases, 3);
        drop((lease, lease2));
        assert_eq!(hub.state.lock().unwrap().klines[&key].leases, 1, "放掉两个应回到 1");
    }

    /// FR-7: 最后一个租约归还 → 条目移除 + 上游任务被中止（不许留着空转）。
    #[tokio::test]
    async fn test_last_lease_release_drops_entry_and_aborts_upstream() {
        let hub = MarketHub::new();
        let key = KlineKey { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        let task = seed_live_kline(&hub, &key);
        let tx = hub.state.lock().unwrap().klines[&key].tx.clone();

        // 首位订阅者的租约（与 seed 的 leases = 1 对应）。
        let first = Lease {
            state: Arc::clone(&hub.state),
            release: Some(Release::Kline { key: key.clone(), tx }),
        };
        drop(first);
        assert!(!hub.state.lock().unwrap().klines.contains_key(&key), "归零应移除条目");
        assert_aborted(task).await;
    }

    /// FR-8: 计数为 0 的**残留死条目**不得复用 —— 必须中止旧任务、清掉、重建。
    #[tokio::test]
    async fn test_slot_rebuilds_instead_of_reusing_dead_entry() {
        let hub = MarketHub::new();
        let key = KlineKey { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        let task = tokio::spawn(async { std::future::pending::<()>().await });
        let (tx, _) = broadcast::channel::<Tick<Kline>>(8);
        hub.state
            .lock()
            .unwrap()
            .klines
            .insert(key.clone(), Sub { tx, leases: 0, abort: task.abort_handle() });

        assert!(matches!(hub.slot_kline(&key), Slot::Rebuild), "死条目必须判为重建");
        assert_aborted(task).await;
        assert!(hub.state.lock().unwrap().klines.is_empty(), "死条目应被清掉");
    }

    /// 双检: 建上游期间别人已建好 → 用别人的, 自己这条 stream 丢弃（不产生第二条）。
    #[tokio::test]
    async fn test_install_reuses_upstream_created_meanwhile() {
        let hub = MarketHub::new();
        let key = KlineKey { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        seed_live_kline(&hub, &key);
        let before = hub.state.lock().unwrap().klines[&key].tx.clone();

        let (_rx, lease) = hub.install_kline(key.clone(), silent_kline());
        let st = hub.state.lock().unwrap();
        assert_eq!(st.klines.len(), 1, "不得插进第二条");
        assert!(st.klines[&key].tx.same_channel(&before), "应沿用先建好的那条");
        assert_eq!(st.klines[&key].leases, 2);
        drop(st);
        drop(lease);
    }

    /// 正常路径: 无条目 → 落表、计数从 1 起、上游真的在跑。
    #[tokio::test]
    async fn test_install_creates_entry_when_none_exists() {
        let hub = MarketHub::new();
        let key = KlineKey { market: "spot", symbol: "ETHUSDT".into(), interval: "1h".into() };
        let (_rx, lease) = hub.install_kline(key.clone(), silent_kline());
        assert_eq!(hub.state.lock().unwrap().klines[&key].leases, 1);
        drop(lease);
        assert!(hub.state.lock().unwrap().klines.is_empty(), "放完应清空");
    }

    /// 盘口走同一套记账（两个键互不干扰）。
    #[tokio::test]
    async fn test_depth_slot_is_independent_from_kline_slot() {
        let hub = MarketHub::new();
        let dkey = DepthKey { market: "spot", symbol: "ETHUSDT".into() };
        let (_rx, lease) = hub.install_depth(dkey.clone(), Box::pin(futures::stream::pending()));
        assert_eq!(hub.state.lock().unwrap().depth[&dkey].leases, 1);
        assert!(hub.state.lock().unwrap().klines.is_empty(), "盘口订阅不应污染 K 线表");
        drop(lease);
        assert!(hub.state.lock().unwrap().depth.is_empty());
    }

    // ---- 转发任务 ----

    /// 上游沉默 → 窗口到点必须报一次可核对的实情（"合约 K 线零帧"这类问题唯一的暴露路径）。
    #[tokio::test]
    async fn test_forwarder_reports_error_when_upstream_is_silent() {
        let (tx, mut rx) = broadcast::channel::<Tick<Kline>>(8);
        let abort = spawn_forwarder(silent_kline(), tx, "kline");
        assert!(
            tokio::time::timeout(READY_WINDOW / 2, rx.recv()).await.is_err(),
            "窗口未到之前不得凭空产生帧"
        );
        let msg = tokio::time::timeout(READY_WINDOW, rx.recv())
            .await
            .expect("窗口到点应报一次")
            .expect("应收到消息");
        match msg {
            Tick::Error(m) => assert!(m.contains("未推送任何数据"), "{m}"),
            other => panic!("应报无数据, 实际 {other:?}"),
        }
        abort.abort();
    }

    /// 上游流结束 → 推一帧"已结束"，页面据此把该路标成不可用（而不是安静地停更）。
    #[tokio::test]
    async fn test_forwarder_reports_error_when_upstream_ends() {
        let (tx, mut rx) = broadcast::channel::<Tick<Kline>>(8);
        let one: Pin<Box<dyn Stream<Item = Kline> + Send>> =
            Box::pin(futures::stream::once(async { kline(1_759_809_599_999) }));
        let abort = spawn_forwarder(one, tx, "kline");
        assert!(matches!(rx.recv().await.unwrap(), Tick::Data(_)), "先收到数据帧");
        match rx.recv().await.unwrap() {
            Tick::Error(m) => assert!(m.contains("已结束"), "{m}"),
            other => panic!("流结束后应报错, 实际 {other:?}"),
        }
        abort.abort();
    }

    /// 无人接收 → 转发任务在**第一次要发送时**自行退出（与租约的 `abort` 是双保险）。
    ///
    /// 注意这条退路的时机: 沉默上游要等满就绪窗口才会产生第一帧, 所以本用例必须用
    /// **立刻出帧**的上游 —— 用沉默流会误以为"没退出"（那只是还没到发送时机）。
    #[tokio::test]
    async fn test_forwarder_exits_when_nobody_listens() {
        let (tx, rx) = broadcast::channel::<Tick<Kline>>(8);
        drop(rx);
        let one: Pin<Box<dyn Stream<Item = Kline> + Send>> =
            Box::pin(futures::stream::once(async { kline(1_759_809_599_999) }));
        let abort = spawn_forwarder(one, tx, "kline");
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(abort.is_finished(), "无人接收时转发任务应自行退出");
    }

    /// 收到数据帧后**不再**误报"无数据"（真帧到了就该安静）。
    #[tokio::test]
    async fn test_forwarder_stays_quiet_after_first_data_frame() {
        let (tx, mut rx) = broadcast::channel::<Tick<Kline>>(8);
        let one: Pin<Box<dyn Stream<Item = Kline> + Send>> =
            Box::pin(futures::stream::once(async { kline(1_759_809_599_999) }));
        let abort = spawn_forwarder(one, tx, "kline");
        assert!(matches!(rx.recv().await.unwrap(), Tick::Data(_)));
        abort.abort();
    }

    // ---- 与页面/端点的对齐 ----

    /// `DEPTH_LEVELS` 必须与市场页盘口请求的档数一致 —— 两处数字不再各写一份。
    #[test]
    fn test_depth_levels_matches_market_page_request() {
        let js = include_str!("assets/markets.js");
        assert!(js.contains("depth=20"), "市场页盘口请求档数变了, 需同步 DEPTH_LEVELS");
        assert_eq!(DEPTH_LEVELS, 20);
    }
}
