# 币安测试网环境 (demo.binance.com)

> 状态: 已配置(2026-09-04 用户提供凭据)
> 用途: ricow 现货 + 合约功能在币安测试网的真实联调与测试(下单/撤单/成交/持仓/门禁全闭环)。
> 凭据授权: ~~曾授权明文入库~~ → 2026-09-14 收敛 T072: 凭据值已移出仓库, 统一存于 `$RICOW_ROOT/ricow.toml`。

## 平台信息

- 平台地址: https://demo.binance.com/ (币安模拟交易 / demo trading, 覆盖现货与合约)
- 官方说明: https://www.binance.com/zh-CN/support/faq/detail/ab78f9a1b8824cf0a106b4229c76496d
- 开发者文档(接入 API 端点时参考): https://developers.binance.com/docs/binance-trading-api/demo-trading

## 实测 REST/WS 端点 (2026-09-04 实弹验证)

| 市场 | 类型 | 端点 | 实测 |
|:--|:--|:--|:--|
| 现货 | REST | `https://demo-api.binance.com` (`/api/v3/...`) | ✅ ping/exchangeInfo 200 |
| 现货 | WS | `wss://demo-stream.binance.com/ws` | ✅ TCP 通 |
| 合约 USDT-M | REST | `https://demo-fapi.binance.com` (`/fapi/v1...v2...`) | ✅ ping/klines 200 |
| 合约 USDT-M | REST 下单/账户 | 同上 demo-fapi (`/fapi/v1/order` 等) | ✅ 已联调 (2026-09-04, 见下文实测记录) |

> ⚠️ 勿用 `api.demo.binance.com` / `fapi.demo.binance.com`: 2026-09-04 本机 DNS 被污染(解析到 AWS ALB,
> 证书不匹配), 正确主机是 `demo-api.` / `demo-fapi.` 前缀。

## 凭据(测试网专用)

**本文件不再保存凭据值**(2026-09-14 收敛 T072)。凭据统一存放在用户的唯一配置文件里:

```toml
# $RICOW_ROOT/ricow.toml (Unix 权限 0600, 不入 git)
[exchange]
demo_key=***      # 币安模拟交易平台的 API Key
demo_secret=***   # 对应 Secret
```

- 用 `ricow run/start <策略> --demo` 即以该凭据调用 `demo-api` / `demo-fapi`; 缺失时命令会明确报"测试网(demo)凭据未填写"并给出文件路径。
- 本文件仅保留**端点**与**获取方式**说明; 密钥值一律不进仓库(与 spec FR-059 单一凭据文件一致)。

## 在 ricow 中使用

- `--demo` 会自动切换到 demo 端点并用 `[exchange].demo_*` 凭据, 无需再手工设环境变量。
- **demo key 与旧现货测试网 testnet.binance.vision 不互通**(代码 `BinanceClient::testnet()` 指向后者, 勿混用)。
- 现货/合约 REST 基址: 代码已支持环境变量覆盖主网域名
  (`RICOW_BN_BASE_URL` 现货 / `RICOW_FAPI_BASE_URL` USDT-M), 指向上表 demo 端点即可切换测试网。
- 密钥注入: 集成测试/联调读环境变量 `RICOW_BN_API_KEY` / `RICOW_BN_SECRET_KEY`(不落盘)。
  产品路径走**单一配置文件** `ricow.toml` `[exchange]` 段(Unix 0600 明文 / Windows 仅当前用户 ACL; OS Keyring 方案已于 2026-09-14 随 019 D31 移除)。

## 注意事项

- **时钟偏差必须先校准 (2026-09-12 实测踩坑, 每轮联调前必做)**: 币安对签名请求有硬限制 —— 本机 timestamp 比服务器超前 >1000ms 直接拒绝
  (`BN 400 ... Timestamp for this request was 1000ms ahead of the server's time`); **滞后**方向有 5s recvWindow 容差, 是安全侧。
  WSL2 时基病实测: 单调钟比服务器快 ~5.25%, 壁钟靠 systemd-timesyncd 每 ~32s 拽回一次, 偏差呈锯齿形, 峰值 >+1000ms 即被拒
  → 症状是"同一轮里时通时不通"。**2026-09-12 实测 `wsl --shutdown` 重启后该问题依旧**(单调钟仍 +5.25%), 不要指望它。
  可用做法 = 跑测试前用 `sudo date -s` 主动把本机时间压到**滞后**, 且**现货/合约分开跑、各自对齐自己的 demo 时间**:
  ```bash
  # 合约组: 对齐合约 demo 自身时间, 滞后 1s (fapi 与 spot 的服务器时间本身相差 ~1.9s, 混跑必撞)
  python3 -c "import json,subprocess,time,urllib.request as u; t=json.load(u.urlopen('https://demo-fapi.binance.com/fapi/v1/time'))['serverTime']/1000-1.0; subprocess.run(['sudo','date','-s',time.strftime('%Y-%m-%d %H:%M:%S',time.localtime(t))+'.000'])"
  # 现货组: 对齐现货 demo, 滞后 3s (现货交易节点时钟比其 /api/v3/time 快约 2s)
  python3 -c "import json,subprocess,time,urllib.request as u; t=json.load(u.urlopen('https://demo-api.binance.com/api/v3/time'))['serverTime']/1000-3.0; subprocess.run(['sudo','date','-s',time.strftime('%Y-%m-%d %H:%M:%S',time.localtime(t))+'.000'])"
  ```
  实测有效性: 2026-09-12 按上式对齐后, 现货 1 例 + 合约 2 例全部一次通过 (此前 8 次连续失败均为时钟)。
- **两个 demo 环境的服务器时间本身不一致 (2026-09-12 实测量化)**: 现货 `demo-api` 的 `/api/v3/time` 比合约 `demo-fapi` 的 `/fapi/v1/time`
  **晚 1.5~1.9s**; 且现货群集内部也不一致 —— 存在"`/api/v3/time` 说本机落后, 但 `/api/v3/openOrders` 仍报本机超前 1000ms"的情况,
  说明 `time` 端点与交易端点不是同一节点的时钟。独立 python 签名脚本对照(同一时钟、同一签名逻辑):
  合约 demo `15/15` 成功, 现货 demo `10/15` 成功(失败即 -1021), 证实**不是本仓客户端问题**。
  对策: 本地时钟留 2~3s 余量(落后方向有 5s recvWindow 容差), 且**每次跑联调前重新对齐**(WSL 漂移 +47ms/s, 约 1 分钟即吃掉 2.8s)。
- 本凭据仅用于测试网, 无真实资产; 即便泄露也不涉及主网资金, 仍按仓库约定提交入库。
- 交易流程测试纪律见 specs/constitution.md 第三节: testnet 真实调用, 禁 mock、禁假 token、禁主网下单测试。
- 合约(USDT-M)的强平/钱包语义(backtest.md §十一 遗留项)可在本环境实盘联调校准。

## 联调实测记录 (2026-09-12, 008 验收 A3 —— `crates/ricow_binance/tests/demo_open_orders_live.rs`)

跑法(每轮前先按 §注意事项 对齐对应市场的 demo 时间, 现货/合约分开跑):
`cargo test -p ricow_binance --test demo_open_orders_live <用例名> -- --ignored --nocapture`

- **现货挂单查询闭环 ✅**: BTCUSDT 挂远离市价限价单 → `Exchange::get_open_orders` 返回该单, side/pair/price/size/status/filled=0/orderId 逐项核对通过 → 撤单 → 再查已消失。
- **合约挂单查询闭环 ✅**: one-way + 1x, fapi 挂单 → `FuturesClient::get_open_orders` 返回该单, side/price/size 核对通过 → 撤单 → 再查已消失。
  坑: 价格须按 `PRICE_FILTER.tickSize` 对齐(合约 BTCUSDT tick=0.1), 否则 `BN 400 Price not increased by tick size`; 数量须按 LOT_SIZE.stepSize 对齐。
- **拒单如实上报 ✅**: 故意提交违反对齐的合约单 → `BN 400 ... Precision is over the maximum defined for this asset.` 作为 Err 抛出(带交易所原始原因、非超时), 交易所无残单。
- 结论: 三个用例按上式对齐时钟后**一次全绿**; 此前 8 次连续失败全部是时钟超前 >1000ms, 与客户端签名无关(已排除)。

## 联调实测记录 (2026-09-04)

- **现货闭环 ✅** (`crates/ricow_binance/tests/demo_spot_live.rs`, #[ignore]): 随机 3 对
  (首轮 API3USDT/DRAMBUSDT/STXBUSDT) 限价挂单→撤单→市价买→余额方向断言→市价卖平仓归零, 全程真实成交。
- **合约 one-way 闭环 ✅** (`crates/ricow_binance/tests/demo_futures_live.rs`): 随机 3 对 (ZEREBROUSDT/CUSDT/TRBUSDT)
  杠杆 1x + ISOLATED → 市价开多 → positionRisk 查仓 → reduce_only 平多归零 → 限价开空挂单 → 撤单 → 资金费率读取。
- **合约 hedge 双向 ✅**: 同对同时 LONG+SHORT 并存 (positionRisk 两条), 双向平仓归零, 可用余额变化符合逐仓语义。
- **实测语义要点** (回测模型校准依据, 对照 specs/backtest.md):
  1. one-way 模式持仓 positionSide=BOTH (非 LONG); hedge 模式才分 LONG/SHORT。
  2. hedge 模式平仓带 positionSide 即可, **不能带 reduceOnly** (fapi 报错 "reduceonly sent when not required");
     one-way 平仓必须 reduceOnly=true (否则反手开新仓)。
  3. set_margin_type / set_position_side_dual 幂等重复调用返回 400 "No need to change..." (非错误)。
  4. 合约 MIN_NOTIONAL ≈ 50 USDT (BTCUSDT), 下单名义须 ≥ 该值。
  5. 现货/合约手续费实扣 (spot 3 对来回 ΔUSDT ≈ -0.17; futures 3 对开平 Δ ≈ -0.78, 均含 10bps/2-5bps 量级)。
## 联调实测记录 (2026-09-13, 011 实盘运行器 T013-T015)

### ⚠️ 现货用户数据流: legacy listenKey 已被币安下线 (关键变更)

- 实测: `POST /api/v3/userDataStream` → **410 Gone**(nginx HTML), **demo 与主网都是**; 合约 `POST /fapi/v1/listenKey` 仍 **200 OK**(合约 listenKey 未下线)。
- 查证: 币安 **2026-02-20 07:00 UTC 永久下线** REST `POST/PUT/DELETE /api/v3/userDataStream` 与 WS-API `userDataStream.start/ping/stop`。
- 替代路径(已实测可用): 现货 **WebSocket API**
  - 端点: demo `wss://demo-ws-api.binance.com/ws-api/v3` / 主网 `wss://ws-api.binance.com/ws-api/v3`
  - 握手: 连接后发 `{"method":"userDataStream.subscribe.signature","params":{apiKey,timestamp,signature}}` —— **HMAC key 即可**(无需 Ed25519), 签名字符串 = 参数按字母序拼 `k=v&...` 后 HMAC-SHA256
  - 响应 `{"result":{"subscriptionId":0}}`; 事件以 `{"subscriptionId":N,"event":{...}}` 推送(事件体与旧 stream 相同: `executionReport` / `outboundAccountPosition`); 无 listenKey、无 keepalive, 连接 24h 上限, 断线重连后须重新订阅
- **订阅就绪语义(踩坑)**: 若在订阅确认前就下单, 成交事件会落在订阅建立之前 —— 实测出现"引擎 `fills=0` 但账户持仓已变"(引擎持仓/盈亏与交易所不一致)。现 `subscribe_user_events` **等订阅确认后才返回**(15s 超时即报错), 引擎在主循环前完成订阅。
- 兜底平仓的成交只能经用户流送达 → 停机清理后短暂吸干用户流(上限 5s, 无新事件提前结束)再落库。

### 实盘闭环实测 (011 T014/T015, demo 现货, `ricow run --live`)

跑法(每轮前按 §注意事项 对齐现货 demo 时钟; 探针策略与临时部署目录不入库):
```bash
export RICOW_ROOT=<临时目录> RICOW_BN_BASE_URL=https://demo-api.binance.com
cargo build && { sleep 12; echo stop; sleep 10; } | ./target/debug/ricow run probe-limit-011 --live
```

- **T013 用户流(第一验证目标)✅**: 订阅就绪后下一笔小额市价单 → 收到 `executionReport`(NEW→TRADE/FILLED, 含 `L`/`l`/`n`/`t` 字段)并可转 `OrderFill`; `crates/ricow_binance/tests/demo_user_stream_live.rs`(2 例, `#[ignore]`)。
- **T014 挂单 → 停机撤单兜底 → 零残留 ✅**: 限价挂单 0.004 ETH @ 市价 -10%; 停机输出 `已撤挂单=1 撤单失败=0 残留挂单=0`; 交易所侧 `GET /api/v3/openOrders` = `[]`。
- **T015 `stop --close-all` 兜底平仓 ✅**: 市价买入 0.004 → stdin 下发 `stop --close-all` → 平仓单 `probe-hold-011-close-<ts>` 真实成交; 吸干用户流 `drained=1`; `ricow fills` 可见 buy/sell 两笔(含手续费), `ricow info` 实时快照余额/持仓/挂单(查询成功, 无挂单)。
- **T016 时钟预检 ✅(自然复现)**: 首跑 T015 时 WSL 漂移使本机超前 **1142ms** → 程序**拒绝启动**并打印对齐步骤; 人为拨快复现**未执行**(需 `sudo date -s`, 按纪律由用户在场执行)。
- **参数对齐实况**: Lua 传出的数字经 f64 带入尾差(`0.0040000000000000000832667269` / `2245.230000000000018189894035`), 引擎按 step/tick 对齐后下发并 warn 记录“原值→对齐值”——这正是该机制的真实价值(否则交易所按精度规则拒单)。
- **遗留**: demo 账户剩微量 ETH 尘埃(往返手续费按 base 扣, 累计 0.000084 ETH ≈ 0.2 USDT, 低于 minNotional 5 USDT 无法再卖) —— 非未平仓, 是交易所规则下限。

- **强平观察 (T7)**: BTCUSDT 三轮 (100x/125x 单边 + 125x 双向对冲) 窗口内未触发 (波动不足); 换
  **DASHUSDT 50x 双向对冲开仓 177s 真实触发** (LONG 被清算, SHORT 原样保留 = hedge 每侧独立判定实锤);
  leverageBracket 签名 API 证实首档 MMR (BTCUSDT 0.4% / DASH 1.5%, exchangeInfo 2.5% 为深档误导值)。
  完整结论见 specs/backtest.md §十一 (2026-09-05)。

## 合约实盘实测 (012, demo USDT-M, ETHUSDT)

跑法(每轮前对齐**合约**时钟 `/fapi/v1/time` —— 与现货不同步, 实测差 1.5~1.9s):

```bash
export RICOW_ROOT=<临时部署目录>        # strategies/*.toml 里 market = "futures"
export RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com
{ sleep 16; echo "stop --close-all"; sleep 30; } | ricow run probe-fut-hold-012 --live
```

**端点(2026-09-13 实测)**
- REST `https://demo-fapi.binance.com`; 市场流 `wss://demo-fstream.binance.com/ws/<symbol>@depth@100ms`; 用户流 `POST /fapi/v1/listenKey` → `wss://demo-fstream.binance.com/ws/<listenKey>`(**事件直推, 无 subscriptionId 外壳**, 与现货 WS-API 不同)
- listenKey 未下线(现货 legacy listenKey 已于 2026-02-20 下线, 合约不受影响)

**事件类型**: `ORDER_TRADE_UPDATE`(成交: `x=TRADE`, `L/l/n/t` 字段) / `TRADE_LITE`(轻量成交, 与前者重复 → 忽略以免重复计) / `ACCOUNT_UPDATE`(余额·持仓) / `ACCOUNT_CONFIG_UPDATE`(杠杆)。
`positionSide/dual` 重复设置返回 400 `No need to change position side.` → 视为成功(幂等)。

**实测结果**
- 限价挂单 → 停机 `已撤挂单=1`; 交易所侧挂单/持仓均无 ✅
- 市价开多 0.08(含上轮残留 0.04)→ `stop --close-all` → 平仓单 `probe-fut-hold-012-c2912370` 成交 → `残留持仓=0`, `positionRisk` 归零 ✅
- hedge: 预配置自动切 dual=true → 同 tick 开 LONG+SHORT 各 0.04 → 停机**两笔**平仓(`positionSide` 分别 long/short, 不带 `reduceOnly`)→ 两侧归零 ✅; 收尾用 one-way 探针复核并把账户切回 one-way

**坑(均在实测中暴露并已修)**
1. `newClientOrderId` ≤ **36 字符**: "策略名前缀 + close + 毫秒时间戳 + 序号" 必超(实测 43 字符)被拒 `BN 400 Client order id length should be less than 36 chars` → 平仓失败并留下仓位(引擎如实上报)。现生成端用秒级短前缀 + 截断保护(保留可判归属的前缀头)。
2. 合约 `@depth` 是**增量 diff**(实测帧可能 `b:[]` 而 `a` 非空): 覆盖式替换会让盘口缺档 → `mid_price()` 返 None。现按价格合并、`size=0` 删档、深度上限; 现货流同源同改。
3. 过滤字段名不同市场不同: 合约 `MIN_NOTIONAL.notional`, 现货 `NOTIONAL|MIN_NOTIONAL.minNotional`。
4. 合约 demo 账户与现货 demo 账户资金独立(实测 USDT ≈4980 / ≈4998)。

## 实盘资金费与强平可见性实测 (014, demo 合约)

- **资金费账单端点(签名)**: `GET /fapi/v1/income?incomeType=FUNDING_FEE&startTime=<ms>&limit=1000` → demo 实测 **200 `[]`**
  (账户当前无跨 8h 结算点的持仓 → 无流水)。落库路径由单测覆盖(幂等/精确合计/水位), **未用真实流水验证过入库**
  —— 需持有仓位跨越 UTC 00/08/16 结算点才能产生真实流水(如实记录, 不假装验证过)。
- **距强平距离(真实数据)**: 50x 逐仓小仓(0.04 ETH)实测
  `WARN risk: 接近强平: 距离 1.48% (阈值 15.0%, 已穿越为负) side=Buy size=0.040 liq=2443.31445226 mark=2479.89409884`
  —— 交易所报告的 `liquidationPrice` 与公式 `(mark−liq)/mark = (2479.89−2443.31)/2479.89 = 1.475%` 一致。
- **`ricow info` 合约快照**: 显示"资金费: N 笔, 合计 X(账户口径; 负 = 净支付)"与持仓的"距强平 X%"(交易所未给 liq → 显示"未知")。
- 阈值可配: 策略 params `liq_warn_pct`(默认 15); 只提示不自动减仓(风控动作仍归交易所与用户)。

---

## 2026-09-13 P4 dogfood demo 预演实测记录(017 来源)

以**网格策略(shannon_grid)**在 demo 现货跑完整"启动 → 交易 → 停机 `--close-all`"链路(无真实资金), 三轮实测:

| 轮次 | 现象 | 判读 |
|:--|:--|:--|
| 1 | `平仓说明: 无持仓, 无需平仓`, 但账户实际持有 **2.016 ETH** | 🔴 `stop --close-all` 静默失效(017-A: 012 起清理用 `get_positions_directional`, 现货实现恒空) |
| 2 | 卖单 `1.0079 → 0.5039 → 0.252 → 0.126 → 0.063 → 0.0315 → 0.016`(同价), 合计≈全部持仓; `提交订单=216 拒单=209 成交=8` | 🔴 几何级数清仓(017-B: 现货只刷持仓不刷现金) + 参数不匹配(209 次拒单 = 再平衡量级 < `min_order_notional`) |
| 3(修复后) | `提交订单=1 拒单=0 成交=2`, `平仓单=shannon-demo-c3056810 平仓失败=0 残留持仓=0.000058`; 交易所侧 `ETH 0.000058 / openOrders=0 / 成交=买 1.008 → 卖 1.007` | ✅ 三项修复生效 |

**时钟窗口(GW: WSL 漂移)**: 本轮多次因本机超前 >1000ms 被预检/签名拒绝(实测检查时 -1911ms, 启动瞬间 +996ms 订阅即被拒
`Timestamp for this request is outside of the recvWindow`)。可用"检查滞后窗口→立刻开跑→启动失败即重试"的脚本抓窗口
(见 P4 runbook §七之四)。

**其他实测事实**: 现货 `minNotional` 5 USDT; 停机后残留 `0.000058 ETH` 尘埃(低于最小名义额, 卖不掉, 属正常);
用户流通知逐笔可达(本轮通知里完整留档了那一串几何级数卖单, 是缺陷定位的关键证据)。
