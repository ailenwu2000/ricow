# 011 任务分解

> 顺序即依赖顺序; 每项完成须有可验证判据。
> 基线: 实施前 `cargo test --workspace` = **233 passed / 0 failed / 9 ignored**(2026-09-12, 004 实施后)。
> 规则: 纯逻辑(对齐/预检判定/归属匹配/清理顺序) → 单元测试; 交易流程 → **demo 测试网真实调用**(禁 mock、禁假 token, 宪法原则三), 每轮前按 specs/testnet.md 对齐时钟、现货/合约分开跑。
> 状态: **Phase 1 已实施并真实验证**(2026-09-13)。Phase 2(合约实盘)未做, 见「阶段 5」。
> 实施结果: `cargo test --workspace` = **270 passed / 0 failed / 11 ignored**(基线 233 → +37)。
> 计划外修正(有实测依据): **D4 现货成交来源改为现货 WebSocket API `userDataStream.subscribe.signature`**
> —— 币安已于 2026-02-20 永久下线 legacy listenKey(主网/demo 实测 410 Gone), 详见文末「实施记录」。

## 阶段 1 — 前置修正与纯逻辑(不碰真实链路, 全部可单测)

- [x] **T001 `min_notional` 数据与解析**
  内容: `Market` 增 `min_notional: Option<Decimal>`(serde default, 与 `step_size` 同模式); 现货 `MIN_NOTIONAL` 与合约 `MIN_NOTIONAL` 过滤器解析补齐。
  判据: 单测断言解析到的值; 旧缓存(无该字段)反序列化不报错。
  依据: FR-010 "不足最小名义即拒单"; 现状 `Market` 只有 `min_size`(LOT_SIZE.minQty)无最小名义。

- [x] **T002 F1 修复: 合约客户端丢弃 `client_order_id`**
  内容: `futures_client.rs:203` 改为使用调用方传入的 `client_order_id`(为空时才生成带前缀 id); 与现货 `client.rs:130` 行为对齐。
  判据: 单测断言请求体里 `newClientOrderId` = 传入值; 缺省时生成值含策略名前缀。
  依据: spec 起点事实 ⑧ + FR-*/口径拍板 4(归属判定与幂等键都依赖它)。

- [x] **T003 下单参数对齐纯逻辑**
  内容: `align_order(req, market)` —— 数量按 `step_size` 向下取整; 价格取到"不劣于原始意图"的一侧(买单向下/卖单向上)按 `tick_size` 对齐; 结果不足 `min_size`/`min_notional` → 明确错误(含交易所数值)。
  判据: 4 例单测(向下取整生效 / 价格不吃亏侧 / 不足最小值拒单 / 本已合规时逐字段不变)。
  依据: FR-010 + 口径拍板 3(复用既有 `tick_size`/`step_size`, 零新机制)。

- [x] **T004 时钟预检判定纯逻辑**
  内容: 纯函数判定 `skew_ms`: 超前 >1000ms 或滞后 >4000ms → 拒绝并给出对齐命令文本; 否则放行并回显实际偏差。
  判据: 3 例单测(超前拒 / 滞后过大拒 / 正常放行); 拒绝文案含 specs/testnet.md 的对齐步骤。
  依据: FR-008 + testnet.md 实测(超前 >1000ms 硬拒; 滞后侧有 5s recvWindow 容差)。

- [x] **T005 归属匹配 + 停机清理顺序/幂等**
  内容: 订单归属前缀匹配函数; 清理流程的编排(停消费 → on_stop → 撤单兜底 → 可选平仓 → 残留核对 → 输出)抽成可测纯逻辑(交易所调用以 trait 注入或结果收集方式测试)。
  判据: 归属 3 例(匹配才撤 / 不匹配不撤只上报 / 前缀含分隔符与大小写边界) + 顺序与幂等 3 例(重复停机只清一次 / 单个撤单失败不中断 / 残留如实输出)。
  依据: FR-005/FR-006 + 口径拍板 4。

## 阶段 2 — 实盘路径接线

- [x] **T006 `LiveContext` 实盘能力**
  内容: 过滤器缓存(`get_markets` 结果)→ `place_order` 前置对齐; 现货 `position_directional` 实现; 真实成交回灌入口(`record_fill` 由引擎调用)。
  判据: 单测(对齐接线生效: 不合规模单被拒、合规单原样下发); `position_directional` 返回真实持仓。
  依据: FR-004/FR-010。

- [x] **T007 `Engine::run_live`**
  内容: 装配(账户快照 + 过滤器 → `LiveContext`)→ 主循环 `select!`(盘口 + 用户流)→ 下单 → 成交回写 `on_fill` + 落库 → 停机清理接线(含 `on_stop`)。
  判据: 编译通过 + 单测覆盖"用户流事件不驱动 tick"; 真实链路判据在 T013/T014。
  依据: FR-001~FR-003/FR-016, D1/D4/P2。

- [x] **T008 停机清理接线(撤单兜底 / 平仓 / 残留核对)**
  内容: 停机路径调用 T005 编排: 前缀撤单 → 可选市价平仓 → 复查 `get_open_orders`/`get_position` → 如实输出; 平仓数量按 `step_size` 对齐。
  判据: 单测 + T014/T015 真实链路判据。
  依据: FR-005~FR-007, D5/P4/P5。

- [x] **T009 CLI 门禁与预检(`run --live`)**
  内容: `--live` 解析; 门禁判定(TOML `live_enabled=true` **且** `--live`); 缺一即 Dry Run 并打印原因; 通过则先做时钟预检再构造引擎。
  判据: 2 例单测(单条件 → Dry Run + 原因文本 / 双条件 → 进入实盘分支); 真实判据 T016。
  依据: FR-013/FR-008, D2/D8/P7。

## 阶段 3 — 管理器与可见性

- [x] **T010 `start --live` 控制通道 + 台账 mode**
  内容: `proto::Request::Start` 增 `live` 开关; `procs::spawn_strategy` 透传 `--live` 并写台账 `mode="live"`; `ctrl.rs` 输出如实(不再写死"当前运行器为 Dry Run")。
  判据: 单测(proto 往返含 live 字段; 台账 mode 落盘); `list`/`status`/`info` 显示"实盘"。
  依据: FR-012, P6。

- [x] **T011 `stop --close-all`**
  内容: `Request::Stop` 增 `close_all`; 停机指令随 stdin 下发到子进程(复用 008 管道停机通道)。
  判据: 单测(proto 往返; 停机指令文本含平仓意图); 真实判据 T015。
  依据: FR-007, D5/假设 A9。

- [x] **T012 `info` 实盘快照**
  内容: 实盘实例输出真实持仓/挂单/余额(账户接口); 查询失败如实标注; 删除"配置声明实盘但仅 Dry Run"的提示(缺口关闭)。
  判据: 单测(格式化输出含模式/快照; 查询失败路径输出"查询失败"而非陈旧值)。
  依据: FR-011/FR-012, architecture §十一 B。

## 阶段 4 — 验证(真实链路)

- [x] **T013 demo 用户数据流实测(第一验证目标)**
  内容: 现货 demo 下订阅用户数据流, 真实下一笔小额单, 断言收到成交/订单事件并可转 `OrderFill`。
  实际实现: 走现货 **WebSocket API** 订阅(legacy listenKey 已下线, 见文末); 用例 `crates/locus_binance/tests/demo_user_stream_live.rs`(2 例)。
  判据: 真实跑通并记录原始事件样例; 失败则触发 R1 回退方案(需重新拍板)。
  依据: D4 + plan R1(现状只验证过 TCP 连通)。

- [x] **T014 demo 现货实盘闭环(SC-002)**
  内容: `LOCUS_ROOT` 临时目录部署小额实盘策略 + `--live` → 下单 → 成交回写 → `locus fills` 查得成交 → 停机 → 撤单兜底 → 交易所复查零残留。
  判据: 命令与输出留存; 交易所侧查询证实无本策略残留挂单。
  依据: SC-002/U S1/U S2。

- [x] **T015 平仓开关真实成交(SC-004)**
  内容: 带 `--close-all` 停机, 真实平掉策略持仓。
  判据: 停机后持仓项为 0(交易所查询)。
  依据: SC-004/FR-007。

- [x] **T016 时钟预检复现拒绝(SC-003)**
  内容: 人为把本机时钟拨快 >1s 后启动实盘, 断言拒绝且提示可执行对齐步骤。
  判据: 拒绝输出 + 提示文本; 恢复时钟后正常启动。
  注意: `sudo date -s` 由用户在场批准执行(agent 不擅自改系统时钟)。

- [x] **T017 基线不回归(SC-005)**
  内容: `cargo test --workspace` 全绿; `locus backtest --strategy shannon_grid --pair ETHUSDT --days 20` 与基线报告逐位一致。
  判据: 测试统计 + 报告字段对比。

- [x] **T018 文档同步与档案**
  内容: `specs/architecture.md`(§三 未实施项、§七 安全模型、§十一 B/C 关闭)、`specs/testnet.md`(实盘联调实测记录)、`specs/roadmap.md`(011 状态与测试基线)、`specs/lua-api.md`(实盘停机时 `on_stop` 与兜底顺序、策略区分语境的现有通道)、本档案补验收实测表与踩坑。
  判据: 文档描述与代码/实测逐条对照一致。
  完成 (2026-09-13): architecture(§三/§七/§九/§十一 B-F) + roadmap(基线 270 + 011 行) + testnet(2026-09-13 实测记录) + lua-api(on_stop 实盘顺序/FR-014 通道) + 本档案(spec/plan/tasks/converge), 对照清单见 converge.md §六。

## 阶段 5 — Phase 2 合约实盘(是否并入 011 待用户复核, 可拆 012)

- [ ] **T019 `BnFuturesExchange` 适配层**
  内容: 用既有 `FuturesClient`(REST 已实测)实现 `Exchange`: place_order/cancel/get_open_orders/get_balance/get_position/get_markets/get_klines + **fapi WS 盘口订阅**。
  判据: 编译 + 真实(demo)盘口订阅跑通; 无 mock。
  依据: 口径拍板 2(fapi WS 现不存在)。

- [ ] **T020 合约装配与 hedge 定向持仓**
  内容: one-way/hedge 参数装配(杠杆/保证金类型/双向持仓, 幂等调用处理)、`position_directional`(positionSide 语义)、`reduceOnly` vs `positionSide` 差异落地。
  判据: demo 合约单手测/联调(真实开平仓)通过。
  依据: specs/testnet.md 实测语义要点 ①②③④。

- [ ] **T021 合约停机兜底与验收**
  内容: 合约撤单兜底 + 平仓(one-way/hedge 两种语义) + 残留核对; demo 合约真实闭环验收。
  判据: 合约侧零残留、持仓按开关归零。

## 收尾待办(不阻塞归档)

1. `StrategyScheduler` 去留(004 P8 遗留): 本变更内核对无引用后**建议删除**(P9)。
2. 实盘告警对接 003-notifications(未上线, 先落日志)。
3. `dry_run_started_at` 时长门禁随 002 落地(architecture §十一 A 维持现状)。

## 实施记录 (2026-09-13, Phase 1 收口)

### 一、测试与真实链路证据

| 项 | 证据 |
|:--|:--|
| 单测基线 | `cargo test --workspace` = **270 passed / 0 failed / 11 ignored**(基线 233) |
| T013 用户流 | demo 现货订阅就绪 → 真实下单 → 收到 `executionReport`(TRADE/FILLED, 含价格/数量/手续费/tradeId)→ 转 `OrderFill`; 用例见 `demo_user_stream_live.rs` |
| T014 闭环 | 限价挂单 0.004 ETH → 停机 → `已撤挂单=1 撤单失败=0 残留挂单=0`; 交易所 `openOrders` = `[]` |
| T015 平仓开关 | 市价买入 → stdin `stop --close-all` → 平仓单成交(`drained=1` 吸干回写)→ `locus fills` 两笔(buy/sell)、`locus info` 实时快照无挂单 |
| T016 时钟预检 | **自然复现**: WSL 漂移使本机超前 1142ms → 拒绝启动并打印对齐步骤; 人为拨快(`sudo date -s`)**未执行**(按纪律由用户在场执行) |
| T017 不回归 | 全部单测全绿; 回测 `shannon_grid ETHUSDT 20d` 两次运行关键指标逐位一致(K线 480 / 成交 9 / 净盈亏 71.911606658838271030103339567 / 回撤 3.17% / 拒单 0); 注: 与 004 档案记录的数值**不可逐位比对**(数据窗口随日期滑动), 故改用"路径零改动 + 同命令两次一致"作判据 |

### 二、实施中发现并修掉的真实缺陷(计划外)

1. **现货成交来源已被币安替换**(触发 plan R1): legacy listenKey(`POST /api/v3/userDataStream`)2026-02-20 起 410 Gone(demo/主网均实测)→ 改走 WebSocket API `userDataStream.subscribe.signature`(HMAC key 即可); 合约 listenKey 仍可用(实测 200), Phase 2 不受影响。
2. **demo WS host 映射缺失**(`ws.rs::ws_base` 只认 testnet): demo 环境会连到主网 stream → 已按 demo/testnet/主网三分支映射。
3. **现货用户流事件名不匹配**: 原实现只解析合约 `ORDER_TRADE_UPDATE`, 现货 `executionReport` 永远收不到 → 已补现货解析(成交/订单/均价/手续费/tradeId)。
4. **订阅就绪竞态**(真实链路才暴露): 首跑 T015 出现"引擎 `fills=0` 但持仓已变" —— 下单发生在订阅确认之前 → `subscribe_user_events` 改为**订阅确认后才返回**(15s 超时即报错), 并在停机清理后加 ≤5s 用户流吸干, 保证兜底平仓成交入账。
5. **合约下单丢弃 `client_order_id`**(计划内 F1): 已修, 与现货一致。
6. 顺带删除已失效的 `create_listen_key`/`keepalive_listen_key` 与仅为它服务的 `http_client`/`api_key_opt`(死代码, 端点已下线)。

### 三、与计划的偏差(均有实测依据)

- **D4 成交来源**: 由 `subscribe_user_events`(listenKey 实现)改为同一 trait 方法的 **WS-API 实现** —— 接口契约不变, 仅替换握手; 语义(实时成交回写、非轮询)与 D4 一致。
- **新增下单参数对齐的边界处理**: 对齐后数量为 0 亦视为拒单(错误信息含原值/step_size)。
- **`stop --close-all` 的语义前移**: 既可由 `start/run --close-all` 指定, 也可由停机指令 `stop --close-all` 动态触发(二者取或), 便于运行中决定是否平仓。
- **回测不回归判据**: 见上表 T017 说明。

### 四、未完成 / 待拍板

- **T016 人为拨快复现**: 需用户执行 `sudo date -s`(agent 不改系统时钟); 自然漂移已等效复现一次。
- **阶段 5(合约 Phase 2, T019-T021)未开始**: 需新建 `BnFuturesExchange`(fapi WS 盘口/用户流)+ hedge 定向持仓 + one-way/hedge 参数装配; 合约 listenKey 仍可用(已实测)。
- 收尾待办(不阻塞归档): `StrategyScheduler` 去留(004 P8)、实盘告警对接 003-notifications(未上线, 先落日志)、`dry_run_started_at` 时长门禁随 002。
