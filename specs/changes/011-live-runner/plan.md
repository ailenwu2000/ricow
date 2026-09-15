# 011 实施计划: 实盘运行器 (live_enabled 生效)

**分支**: `master`(当前工作树) | **日期**: 2026-09-13 | **规格**: [spec.md](spec.md)

## 摘要

把"`live_enabled=true`"从空承诺变成能跑: ① 显式门禁(配置声明 **且** 命令行开关)下以**真实账户**启动 —— `LiveContext` 首次被真正构造(现状全仓无构造点);
② 启动期账户/过滤器装配 + 下单前时钟预检; ③ 真实成交回写策略并落库(现货用户数据流); ④ 停机清理: 撤单兜底(按订单号归属)+ 可选平仓 + 残留如实上报;
⑤ `info` 实盘持仓/挂单/余额快照 + 台账 `mode=live`。

**范围**: **现货首批**(Phase 1)。合约实盘后置为 Phase 2 —— 复核发现 `Exchange` trait 的**唯一实现是现货**(`crates/locus_binance/src/spot.rs:99`), 合约只有裸 REST 的 `FuturesClient`,
fapi 的 WS 盘口/用户流**未实现**, 故合约实盘需**新建适配层**(属新子系统, 不是接线)。是否并入 011 由用户复核拍板, 可拆为 012。

## 技术上下文

**语言/依赖**: Rust workspace(声明 1.83), 本变更**零新增依赖**; 用到 tokio(`select!` 与既有的 runtime `Handle`)、sqlx(成交落库)、tracing、`rust_decimal`、chrono。

**现货链路现状(本变更复用)**: `BinanceClient` + `BnSpotExchange: Exchange` —— 已具备 `place_order` / `cancel_order` / `get_open_orders` / `get_balance` / `get_markets`(解析 PRICE_FILTER.tickSize + LOT_SIZE)/ `subscribe_orderbook` / `subscribe_user_events`(listenKey + 30min 续期)。

**实测结论 (2026-09-13, T013)**: demo 用户数据流已真实验证 —— legacy listenKey 已被币安下线(410 Gone, 主网/demo 均实测), 现货改走 **WebSocket API** `userDataStream.subscribe.signature`(HMAC key 即可), 真实下单后收到 `executionReport` 并转 `OrderFill`; 订阅须**就绪后**才可下单(否则启动瞬间漏成交); 合约 listenKey 仍可用。

**时间语义**: 启动前时钟预检(交易所服务器时间 vs 本机); 规则时间仍走 `Context::now_utc()`(004 已给 LiveContext 实现真实 UTC)。

**存储**: 不新增表、不新增文件 —— 台账 `run/<name>.json` 的 `mode` 字段(008 已备, `live` → "实盘" 映射已在 `instances.rs::mode_text`)直接复用; 成交仍落既有 `fills`。

**测试**: 纯逻辑用单测(对齐/预检判定/归属匹配/清理顺序); 交易流程走 demo 测试网真实调用(`#[ignore]`, 现货/合约分开跑, 每轮前按 specs/testnet.md 对齐时钟)。

## 宪法检查

- [x] 原则一(完全本地化): 仅交易所 API, 无遥测、无自动更新
- [x] 原则二(策略层统一 Lua): 不新增 Rust 策略本体; 实盘跑**同一份** Lua 脚本, 只换下单/账户事实源
- [x] 原则三(测试纪律): 实盘链路 demo 真实调用, 禁 mock 与假 token; 未配 key 时 `#[ignore]` 跳过而非 mock
- [x] 原则四(产物一律中文)
- [x] 原则五(少而精): 零新增依赖; 不新造表/通道(归属用订单号天然键、模式用既有台账字段、对齐用既有 `Market` 过滤器); 不做市场门禁、不改钱包模型

## 项目结构

```text
specs/changes/011-live-runner/
├── spec.md      # 功能规格(2026-09-13)
├── plan.md      # 本文件
├── tasks.md     # 任务分解
└── checklists/requirements.md
```

### 源码改动 (Phase 1 现货)

```text
新增 (实施期产生)
  crates/locus_strategy/src/align.rs         # 参数对齐 + 归属前缀 + 实盘下单前处理 (prepare_live_order)
  crates/locus_engine/src/live.rs            # 时钟预检判定 / 实盘门禁 / 停机清理编排 (纯逻辑可单测)
  crates/locus_binance/tests/demo_user_stream_live.rs  # 现货用户流真实调用用例 (#[ignore])

修改
  crates/locus_core/src/types.rs             # Market 增 min_notional: Option<Decimal> (serde default, 同 step_size 模式)
  crates/locus_binance/src/client.rs         # 解析 MIN_NOTIONAL; 新增 server_time() (现货 /api/v3/time)
  crates/locus_binance/src/futures_client.rs # F1: 用传入 client_order_id (缺省生成带前缀 id); 解析 MIN_NOTIONAL
  crates/locus_binance/src/ws.rs             # 现货用户流改 WebSocket API 订阅 (legacy listenKey 已下线) + demo host 映射 + executionReport 解析
  crates/locus_binance/src/spot.rs           # (无改动, 仅经 client 透传)
  crates/locus_strategy/src/context.rs       # LiveContext: 过滤器缓存 + 下单前对齐 + position_directional + 真实成交回灌
  crates/locus_engine/src/live.rs      (新)  # 停机清理(撤单兜底/平仓/残留核对) + 归属前缀匹配 + 时钟预检判定 (纯逻辑可单测)
  crates/locus_engine/src/command.rs         # 新增 Engine::run_live(): 装配 → 行情流+用户流 select → 下单/回写/落库 → 停机清理
  crates/locus_cli/src/commands/run.rs       # --live 门禁 + 时钟预检 + 实盘分支(如实输出)
  crates/locus_cli/src/commands/ctrl.rs      # start --live / stop --close-all 转发
  crates/locus_cli/src/supervisor/proto.rs   # Request::Start 增 live 开关; Request::Stop 增 close_all
  crates/locus_cli/src/supervisor/procs.rs   # spawn 参数透传 + 台账 mode 写 live
  crates/locus_cli/src/commands/instances.rs # info: 实盘账户快照; 去掉"当前仅 Dry Run"提示(缺口关闭)
文档同步
  specs/architecture.md (§三 未实施项 / §七 安全模型 / §十一 B、C 缺口关闭)
  specs/testnet.md (实盘联调实测记录) · specs/roadmap.md (011 状态与测试基线) · specs/lua-api.md (实盘停机时 on_stop 与兜底的顺序)
```

## 决策

| # | 决策 | 结论 |
|:--|:--|:--|
| D1 | 运行路径 | 新增 `Engine::run_live` 与 `run_dry_run` **并列**; 不把 Dry Run 改造出实盘分支 —— 两者的账户事实源与停机清理语义不同, 合并会让模式判定散落到循环里 |
| D2 | 启用门禁 | 双条件: TOML `live_enabled = true` **且** CLI `--live`; 缺一即按 Dry Run 运行并**打印原因**(spec 拍板 1) |
| D3 | 账户事实源 | 启动期从交易所拉 `get_balance` / `get_position` / `get_open_orders` + 过滤器(`get_markets`)注入 `LiveContext`; 实盘不生成虚拟资金 |
| D4 | 成交来源 | 现货 `subscribe_user_events` → 转 `OrderFill` → `ctx.record_fill` + `db.insert_fill`; **不用**下单 ack 的累计成交量当成交(ack 无成交价)。<br>**实现修正 (2026-09-13 实测)**: 底层走现货 **WebSocket API** 订阅 `userDataStream.subscribe.signature`(legacy listenKey 已被币安 2026-02-20 下线); 事件包在 `{"subscriptionId","event"}`; 接口契约不变, 语义(实时、非轮询)与 D4 一致 |
| D5 | 停机清理顺序 | 停消费行情 → 策略 `on_stop` → 撤单兜底 → (可选)平仓 → 残留核对(`get_open_orders`/`get_position` 复查) → 如实输出; **幂等**(重复停机只执行一次) |
| D6 | 归属判定 | `clientOrderId` 统一带 `<策略名前缀>-`; 兜底只撤前缀匹配的单, 非匹配只上报(spec 拍板 4); 顺带修 F1 |
| D7 | 参数对齐 | 引擎侧(该 pair 的 `Market` 过滤器); 数量按 `step_size` 向下取整, 价格取到"不劣于原始意图"的一侧(买单向下/卖单向上); 不足 `min_qty`/`min_notional` → 拒单并如实报错(spec 拍板 3) |
| D8 | 时钟预检 | 仅实盘启动前生效(不影响 Dry Run/回测): 取现货服务器时间, 超前 >1000ms 或滞后 >4000ms → 拒绝启动, 打印 specs/testnet.md 的对齐命令 |
| D9 | 合约实盘 | Phase 2, 前置 = 新建 `BnFuturesExchange`(`Exchange` 适配 + fapi WS 盘口/用户流) + hedge 定向持仓 + one-way/hedge 参数装配 + MIN_NOTIONAL; 可拆 012 |
| D10 | FR-014 落地 | **不做引擎注入**: 策略若需区分语境, 用现有 `ctx:config_bool` + TOML params 通道(`lua.rs` 已有 config_bool_map, 零改动); 在 lua-api.md 写清 |
| D11 | `dry_run_started_at` 时长门禁 | 不属本变更(002 范畴), 维持现状(architecture §十一 A 不变) |

### 实现决策

| # | 议题 | 结论与依据 |
|:--|:--|:--|
| P1 | 用户流断线 | 视为**异常停机**(不静默继续): 与 008 "行情流中断 → 非零退出" 同口径; 断线后把成交当"没有"会误判仓位 |
| P2 | 双流并行 | `tokio::select!` 同时消费盘口与用户流; 用户流事件**不驱动 tick**(只回写成交), 避免策略被成交事件额外触发(与 Dry Run 的 tick 语义一致) |
| P3 | 头寸刷新 | 成交后与停机前刷新 `get_position`; 不做高频轮询(避免打交易所限频) |
| P4 | 撤单兜底实现 | `get_open_orders(pair)` → 前缀过滤 → 逐个 `cancel_order`; 单个失败不中断其余, 失败如实累计 |
| P5 | 平仓实现 | 现货 = 市价卖出该 pair 的 base 持仓(数量按 `step_size` 对齐); 合约(Phase 2)= one-way 用 `reduce_only`、hedge 用 `position_side`(实测语义见 specs/testnet.md) |
| P6 | 台账 mode | 由 supervisor 按启动参数写(`live` / `dry_run`), 不新增字段; `list`/`status`/`info` 三处一致 |
| P7 | 预检位置 | CLI 层(门禁之后、构造引擎之前): 失败即退出, 错误信息面向用户, 不带进引擎路径 |
| P8 | `min_notional` 缺省 | `None` 时不拦截(兼容旧缓存数据), 与 `step_size` 同策略; 对齐后不足则交交易所拒并如实回传 |
| P9 | `StrategyScheduler` | 004 P8 遗留: 它至今无消费者, 且职责与 `Engine` 主循环重叠 → 本变更内**建议删除**(由收敛阶段核对无引用后执行), 与"死代码必删"一致 |

## 改动清单(Phase 1, 按依赖顺序)

1. `Market.min_notional` + 现货/合约 exchangeInfo 解析 + 单测
2. F1 修复(合约使用传入 `client_order_id`) + 单测
3. 对齐纯逻辑 `align_order(req, market)` + 单测(取整方向/不足拒单/本已合规不动)
4. 时钟预检判定纯逻辑 + 服务器时间获取, 单测(超前/滞后/正常三态)
5. 归属前缀匹配 + 清理顺序与幂等纯逻辑 + 单测
6. `LiveContext`: 过滤器缓存、下单前对齐接线、现货 `position_directional`、真实成交回灌入口
7. `Engine::run_live`: 启动装配 → 双流 `select!` → 下单/成交回写/落库 → 停机清理接线(含 on_stop)
8. CLI: `run --live` 门禁 + 预检; `start --live` 经 proto/procs + 台账 `mode`; `stop --close-all`
9. `info` 实盘快照(持仓/挂单/余额; 查询失败如实标注)
10. 文档同步 + 档案(验收实测表、踩坑记录)

## 验证

- **单测**: 对齐 4 例(向下取整/价格不吃亏侧/不足 min_qty 拒/合规不动) + 预检 3 态 + 归属匹配 3 例(前缀匹配/不匹配不撤/大小写与分隔符) + 清理顺序与幂等 3 例 + 门禁 2 例; 基线 **233 passed / 0 failed / 9 ignored** 不回归
- **demo 现货真实链路**(`#[ignore]`, 每轮前按 specs/testnet.md 对齐时钟, 现货单独跑):
  a) **T013 用户数据流成交回写**(第一验证目标, 先单独验) b) **T014 实盘闭环**: 下单 → 成交回写 → `fills` 落库 → 停机撤单兜底 → 交易所零残留(SC-002)
  c) **T015** 平仓开关分支真实成交、持仓归零(SC-004) d) **T016** 时钟预检: 人为拨快 >1s → 拒绝启动且给出可执行对齐步骤(SC-003)
- **不回归**: `cargo test --workspace` 全绿; `locus backtest --strategy shannon_grid --pair ETHUSDT --days 20` 报告与基线逐位一致(SC-005)
- **诚实边界**: demo 用户流与实盘停机清理的真实验证结果如实记录; **主网小资金 dogfood 由用户执行**(agent 不代跑, spec 假设 A6); 合约 Phase 2 的 WS 适配未验证前不得声称合约实盘可用

## 风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | demo 用户数据流未实测(仅 TCP 通) | ✅ **已结案 (2026-09-13)**: 实测确认 legacy listenKey 已被币安下线(410 Gone) → 改走现货 WebSocket API 订阅(已实测可用); **回退轮询方案未启用**(端点 `GET /api/v3/order?origClientOrderId=` 实测可用, 仅作为备选记录) |
| R2 | WSL 时钟漂移(+47ms/s) | 每轮验证前对齐; 预检拦下失控情形(D8) |
| R3 | 实盘误触 | 三道: 默认 Dry Run + 双条件门禁 + 时钟预检; 主网执行由用户主导 |
| R4 | 范围膨胀(合约需新适配层) | Phase 2 明确可拆 012, 现货交付不受影响 |
