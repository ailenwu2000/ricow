# 040 技术方案（plan）

## 一、决策清单（拍板项，供审）

| # | 决策 | 取舍与依据 |
| :-- | :-- | :-- |
| **D1** | 上游按 `(市场,标的,周期)`（K 线）/ `(市场,标的)`（盘口）**共享**，一个进程内同键只开**一条**币安 WS | 每标签各开一条会把币安连接配额（300/5min）当计数游戏；共享是唯一不浪费的做法。代价 = 需引用计数与生命周期管理（FR-6 ~ FR-8） |
| **D2** | **一条 SSE 同时推 K 线 + 盘口**，不拆两个端点 | 详情页本来就同时要两路；一条连接省一半前端状态机（一个 `R.sse` 句柄、一个状态标识） |
| **D3** | 帧格式沿用**会话流**的约定：只用默认 `message` 事件，帧体是 JSON，靠 `type` 分派 | 与 `web/sessions.rs` 的 `frame.encode()` 同构；`R.sse` 已有（退避 + 抖动）无需改公共层 |
| **D4** | **不做** Last-Event-ID 续传；重连后 REST 快照重对齐 | 行情无补发语义：断线期间的价格变动不存在，重放旧帧只会把图拉回过去（FR-11） |
| **D5** | 上游"订上了但没数据"用**每路就绪窗口**（12s）检测并下发错误帧，且**按路隔离**（`scope: kline / depth`），不整条连接打死；数据后来到了则**自行恢复** | 实测（见 §二.0）合约 K 线流会"连上但一帧不给" —— 不检测就会表现为"图冻住了但界面说实时"。分路隔离的依据同样是实测：同轮合约盘口正常而 K 线为零 |
| **D6** | 盘口服务端**截断到 20 档**再下发 | 与面板展示档数一致；50 档原样推是纯浪费（FR-5） |
| **D7** | 流端点 `interval` **必填**（缺 → 400），不继承兄弟端点的 `1h` 默认 | 订阅是长承诺：静默默认会让"我传了 4h、它订了 1h"这种错在界面上表现为"图不对"而非报错 |
| **D8** | 复用既有 `ws::run_depth_ws` 的写法；把 `backoff_delay` 提为 `pub(crate)` 并**删掉 `futures_ws.rs` 里的第二份** | 本变更会引入第三个使用者（K 线守护，现货/合约各一）；留三份退避实现是自找不一致 |
| **D9** | 前端连接状态 **三态 + 最后更新时刻**，`onfail` 给手动重试而非静默降级轮询 | 静默降级成轮询 = 用户以为在看实时；显式"已断开 + 重试"是诚实且更少代码（FR-9） |
| **D10** | **不做**订阅并发上限；**不做**运行页实时化；**不引**新依赖 | 见 spec §五 非目标（回环 + token + 单用户；运行页数据源不是行情） |

## 二、改动面

### 0. 实测事实（本方案的两条硬依据，2026-10-07 真机）

1. **不存在的标的**：币安对 `zzzusdt@kline_1h`（现货/合约都一样）**照常完成 WS 握手，然后一帧不发** ——
   既不回 error 帧也不断开。故"解析错误帧"是**死代码**（不做，YAGNI）；
   真正需要的是**标的预校验**（同步 404）与**就绪窗口**。
2. **合约主机的非订单簿行情整体不下发**（初版只记为"合约 K 线零帧"，2026-10-07 补齐取证后订正）：
   同轮 35s 窗口内 `fstream` 盘口 305 帧、合约 REST 200，而 `btcusdt@kline_1m` / `kline_1h` / `kline_1d`
   与 `/stream?streams=` 组合**全部 0 帧**（三次独立复现）。**逐类对照后确认范围更大** ——
   同一 20s 窗口内 `bookTicker` 9 105 帧 / `depth@100ms` 153 帧，而 `aggTrade` / `trade` / `kline_*` /
   `ticker` / `miniTicker` / `markPrice@1s` **全部 0 帧**（含与服务端定时器绑定的 `markPrice@1s`，故与"有没有成交"无关）。
   已排除：我方 URL/参数（现货同形 URL 正常）、客户端库（Node 内置 / 自写裸 WS / ricow 的 tokio-tungstenite 一致）、
   单节点（8 个 IP 全同）、预热（90s 仍 0）、走错主机与中间人（真币安证书 + 真实 AWS 东京 IP）、
   "合约没行情"（REST 当前 1h K 线含 123 320 笔成交、aggTrades 距本机 ~7s）。
   **平台且正常受理了订阅**：`SUBSCRIBE` 回 `{"result":null,"id":1}`、`LIST_SUBSCRIPTIONS` 里 kline 已登记，
   之后仍只有 bookTicker。→ **推断**为币安侧按地区/出口 IP 的行情权限限制（**不臆造其内部原因**），
   完整证据链见 `converge.md` §3。
   → 后果：合约详情页的 K 线必须如实显示"无实时数据（已降级为快照）"，
   而**同一页面的盘口仍应正常实时** —— 这是"按路隔离"的实证依据（且它是按**数据来源**隔离，不只是按面板分路）。

推送节奏实测（定阈值用）：K 线 1h 首帧 ≈5.3s（含 ~4s 建连）后 **~2s/帧**；盘口首帧 ≈4s 后 **~100ms/帧**。

### 1. `crates/ricow_binance/src/ws.rs`（现货 + 共用）

- 新增 `const KLINE_READ_IDLE: Duration = 300s`（服务器 Ping ≤ 3min，5min 无消息必为半开）。
- 新增 `pub(crate) async fn run_kline_ws(ws_url, symbol, tx, interval)`：与 `run_depth_ws` 同构
  （退避重连 + `tx.is_closed()` 退出），但**无需快照同步**（K 线每帧自带完整 OHLCV）。
- 新增 `pub(crate) fn parse_kline_frame(v: &Value) -> Option<Kline>`：只认 `e == "kline"`，
  从 `k` 取 `t/T/o/h/l/c/v`，时间 `from_timestamp_millis`；字段缺失/类型不符 → `None`（不猜）。
- `BinanceClient::subscribe_klines(&self, symbol, interval) -> CoreResult<Stream<Item = Kline>>`：
  流名 `{sym}@kline_{interval}`，拼 `ws_base()`，spawn 守护，返回 `ReceiverStream`。

### 2. `crates/ricow_binance/src/futures_ws.rs`（合约）

- `FuturesClient::subscribe_klines(...)`：同签名同语义，走合约 `ws_base()`（`fstream`）。
- 删掉本文件私有 `backoff_delay`，改 `use crate::ws::backoff_delay`（D8）。

### 3. `crates/ricow/src/web/realtime.rs`（新文件，核心）

- `MarketHub`：`Arc<Mutex<HubState>>`；`HubState { klines: HashMap<KlineKey, Sub<Kline>>, depth: HashMap<DepthKey, Sub<OrderBookUpdate>> }`。
- `Sub<T> { tx: broadcast::Sender<T>, lease_count: usize, abort: AbortHandle }`。
- `subscribe_klines / subscribe_depth`：
  锁内取条目；**存在且 `lease_count > 0`** → 复用（`tx.subscribe()` + `lease_count += 1`）；
  **存在但计数为 0**（上游已死的残留条目）→ `abort()` 旧任务 + 移除 + 重建（FR-8）；
  不存在 → 建 broadcast（容量 256）+ 调 `subscribe_klines`/`subscribe_depth` + spawn 转发任务。
  上游订阅**同步失败**（如非法标的握手失败）→ 返回 `Err`，不建条目。
- `Lease`：持有 `Arc<Mutex<HubState>>` + key + `tx`（`same_channel` 做身份校验）；
  `Drop` 时锁内 `lease_count -= 1`，归零 → 移除条目 + `abort()`（FR-7）。
  计数用**显式租约**而非 `receiver_count()`：Lease 与 Receiver 的 drop 顺序无保证，
  靠 `receiver_count()` 会时而漏拆（真·踩坑点，注释里写明）。
- `market_stream` handler：
  1. 复用 `markets.rs` 的 `parse_market(required)` / `parse_interval` / `normalize_symbol`
     （把这三个改为 `pub(super)`，**不复制**）；
  2. 首帧 `hello`（回显 market/symbol/interval，让前端知道订到了什么）；
  3. 分路订阅：成功 → `unfold(rx, lease)` 适配成流；失败 → 该路退化为**单帧错误流**；
  4. `stream::iter(head).chain(select(kline_side, depth_side))`，两侧统一 `Box::pin`；
  5. `Sse::new(..).keep_alive(KeepAlive::default())`（同 `session_events`）。
- 帧类型（`serde::Serialize`）：`hello` / `kline`（`t,o,h,l,c,v,T`，Decimal 走工作区 `serde-str`）/
  `depth`（`bids/asks` 各 ≤20 档 + `timestamp`）/ `error`（`scope` + `msg`）。
- Lagged：跳过 + `tracing::warn`（与 `sessions.rs` 同口径，不补发）。

### 4. `crates/ricow/src/web/mod.rs`

- `WebState` 增 `realtime: Arc<realtime::MarketHub>`，在 `WebState::new` 内默认构造
  （既有装配方与全部测试调用点**签名不变**，沿用 `jobs` 的既有做法）。
- 路由 `.merge(realtime::routes())`（挂在 token 中间件之前，自动获得同一道门）。

### 5. `crates/ricow/src/web/assets/markets.js`

- `openStream(symbol, market, interval)`：用 `R.sse` 打开；`onopen` → 状态"实时"+ 记时刻；
  `onmessage` → 按 `type` 分派；`onfail` → 状态"已断开"+ 重试按钮。
- `kline` 帧：`series.update()`（同 `open_time` 原地更新、更大的 `open_time` 自动长出新根）；
  成交量柱同步 `update`；MA 尾点重算（只需末尾 `N` 个收盘价，不整列重算）；图例/末价同步。
- `depth` 帧：直接整体重绘盘口两表（`OrderBookUpdate` 已是累计语义）+ 由最优档重算现价。
- 状态标识：新增一个小 chip（`实时 12:34:56` / `重连中…` / `已断开` + 重试），放在详情页头部。
- 生命周期：`destroyChart()` / 换标的 / 切周期 / 离开视图 → `closeStream()`；用既有 `gen`
  代际令牌防串台（异步回来的帧属于上一代一律丢弃）。

### 6. 文档

- `specs/architecture.md`：Web 层新增 `realtime.rs` 与 SSE 第二处用法的现状描述。
- `specs/roadmap.md`：变更表加 040 行 + 测试基线数字更新。
- `specs/changes/040-realtime-market/`：spec / plan / tasks / converge。

## 三、测试策略

| 层 | 方式 |
| :-- | :-- |
| K 线帧解析 | 单测：**真实抓取的币安帧**（现货/合约各一份）+ 缺字段 / 非 kline 事件 / 类型错 → `None` |
| 帧序列化 | 单测：kline 帧字段名与 `serde-str` 口径；盘口 20 档截断；错误帧带 `scope` |
| 订阅复用 / 生命周期 | 单测：把"上游来源"做成可注入的函数 —— 注入的是**普通 broadcast 频道**（纯连接记账，非 Exchange 替身，不涉下单/成交/风控，故不违反宪法原则三）。断言：同键两次订阅只建一条上游、计数正确、全 drop 后条目被移除且任务被 abort、死条目不被复用 |
| 参数校验 | 复用 `markets.rs` 既有单测；新增流端点 `interval` 必填 / `market` 必填的断言 |
| 前端 | `node --check` 九份脚本 |
| 真机 | `curl -N` 读 SSE 断言真的在跳（数值随时间变化）+ 与 REST 交叉核对 + 真实非法标的的错误帧 + 官方冒烟零回归 |

## 四、风险与对策

- **`series.update()` 时间不单调会抛错**（LWC 硬约束）：WS `t`(ms) 与 REST `open_time`(ISO)
  必须落到**同一个 epoch 秒**。对策：两条路径都用 `Math.floor(ms/1000)`；`update` 前对该根
  做 `time >= last.time` 检查，不满足则**丢弃该帧**并记一次计数日志（不塞进去炸掉整图）。
- **重连窗口内的数据空档**：对策 = `onopen` 重拉 REST 快照重对齐（FR-11），并在状态 chip 上
  暴露最后更新时刻。
- **上游订阅同步失败阻塞首帧**：对策 = 连接**先建**（先发 `hello`），分路结果随后按帧报错，
  页面不会因为某一路上游握手慢而白屏。
