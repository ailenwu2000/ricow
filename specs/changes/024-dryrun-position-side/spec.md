# 024 功能规格: Dry Run 虚拟持仓方向记账修复

**功能目录**: `specs/changes/024-dryrun-position-side`

**创建日期**: 2026-09-19

**状态**: 草稿

**触发**: [023-ai-chat-ux/converge.md](../023-ai-chat-ux/converge.md) §五 **F5(HIGH)** —— 023 复测时用当前构建跑 Dry Run 实测暴露

**输入**: 用户描述: "按 SDD 单开支线修复 F5"(Dry Run 虚拟持仓 side 记账错误)

## 一、为什么

023 复测 Dry Run 时, 一个只用 `ctx:position_size` 判仓的简单策略(无仓买入 / 有仓卖出)在真实行情下出现**反常且可复现**的表现:

| # | 现象(实测, 隔离沙箱 + 真实行情) | 后果 |
|:--|:--|:--|
| **A** | 策略诊断行 `side=short` **恒真**, `size` 单调增长 `0.27 → 1.37` | 仓位被凭空放大, 试跑结论完全不可信 |
| **B** | `ricow fills` 首 12 行**全部为 sell**, 无一笔平仓性质成交 | 用户看到的成交流向全部错误 |
| **C** | `usdt` 冻结在 `99999.9968`、`eth=0.0` | 权益曲线失真 |

日志侧对照显示**撮合与回灌正常**(`strategy.dryrun: dry run order placed ... side=Buy ... status=Filled`), 即**错在记账, 不在行情与撮合**。

根因(定位到代码):

- Dry Run 用**单条净仓**记录一个交易对的方向: `DryRunContext` 的 `virtual_positions`(`crates/ricow_strategy/src/context.rs:395`)。
- 平仓完成后, 该记录被置为 `size = 0` 但 `side` 保留为**平仓方向**(`context.rs:639-641`: `entry.side = OrderSide::Sell`)。
- 之后**反向**成交进入"加仓"分支时, **只累加 `size`、不翻转 `side`**(`context.rs:618-624` 的 `else` 分支缺 side 赋值) →
  形成 `(side = Sell, size > 0)` 的**伪空头**, 且每次"卖出"都落进同一分支继续累加 → `size` 单调放大。
- 下游按 `pos.side` 呈现方向(`crates/ricow_strategy/src/lua.rs:344-353`, `size<=0` → `none`, 否则 `side==Buy` → `long`, 否则 `short`) →
  策略侧永远看到"有空头", 于是永远不平仓。
- `BacktestContext`(`crates/ricow_strategy/src/backtest.rs::close_position`)是**多空分离**实现, 无此问题 —— 缺陷**仅存在于 Dry Run**。

**业务影响**: 任何以 `ctx:position_size` / `ctx:position_side` 判仓的策略, 在 Dry Run("试跑")下**永不平仓、仓位单调放大** →
023 FR-024 `StartDryRun` 的实证价值失效; 不涉真实资金, 但会误导用户对策略的判断, 进而误导其是否可以上实盘。

## 二、范围

**做**:

- 修正 Dry Run 净仓记账的**方向一致性**: 成交后 `(side, size)` 必须始终自洽 —— `size > 0` 时 `side` = 建仓方向, `size == 0` 时无方向。
- 覆盖**同向加仓 / 部分平仓 / 平仓归零后反向再开 / 反手(成交量 > 当前持仓量)**四类路径, 保证多空双向对称。
- 新增**纯逻辑单测**(Dry Run 上下文, 无网络、无 mock 替身), 覆盖上述路径, 且修复前必须失败。
- 真实 Dry Run 复测: 用同一夹具在真实行情下确认 `side` 不再恒为单边、`size` 能回到 0 并重新开仓。

**不做**:

- 不改 `BacktestContext`(多空分离已正确)、不改实盘 `LiveContext`(持仓来自交易所)。
- 不改 Dry Run 的余额与手续费口径(`apply_virtual_balance_change`)、不改撮合与滑点。
- **不引入 "flat" 第三态**: 不改 `Position` 结构、不改 `position_side` 的 `size<=0 → none` 判定, 避免波及 hedge 方向仓语义。
- 不实现 Dry Run 的方向仓(`position_directional`, 当前走 trait 默认返回 `None`), 该能力不在本次范围。
- 不改任何 CLI 参数、文案与配置项; 不引入新依赖。

## 三、功能需求

- **FR-001**: 净仓 `size == 0` 时**不保留方向含义** —— 任何方向的新成交都必须把该交易对的持仓 `side` 置为该成交方向。
- **FR-002**: 与当前持仓 `side` **相反**的成交, 在平掉现有持仓后, 若仍有剩余量, 必须按**新方向**建立持仓(反手), 不得累加到旧方向。
- **FR-003**: 与当前持仓 `side` **相同**的成交必须加权平均开仓价, 且 `side` 保持不变(既有正确行为, 不得回退)。
- **FR-004**: 部分平仓必须记已实现盈亏, 并保留剩余持仓的 `side` 与开仓价(既有行为, 不得回退)。
- **FR-005**: 平仓归零后, 策略侧 `ctx:position_side` 必须返回 `none`、`ctx:position_size` 必须返回 `0`。
- **FR-006**: 修复仅作用于 Dry Run 虚拟持仓; 回测与实盘的撮合、盈亏与持仓口径**零变化**。
- **FR-007**: 必须提供**纯逻辑单测**(不联网、不打桩)覆盖: ① 平多后反向开空; ② 平空后反向开多; ③ 平多后同向再开多(开仓价重算); ④ 部分平仓保留剩余方向与开仓价; ⑤ 反手(成交量大于当前持仓量)后的 `side` 与 `size`。
- **FR-008**: 真实 Dry Run 复测必须可观测到: `side` 不再恒为单边、成交方向出现买/卖交替、`size` 能回到 `0` 后再次开仓。

## 四、验收标准

- **SC-001**: 门禁全绿零回归 —— `cargo test -p ricow_strategy` 与 workspace 测试全部通过, 无失败、无新增忽略。
- **SC-002**: 新增单测 ≥5 条覆盖 FR-007 五条路径; 且经**回退验证**: 在未修复代码上这批测试必须红(证明测试真的盯住了缺陷), 修复后转绿。
- **SC-003**: 真实 Dry Run 复测(真实行情, 隔离沙箱): 修复前复现 `side` 恒为单边 + `size` 单调增长; 修复后同一夹具在相同 tick 数内出现买/卖交替且 `size` 回落至 `0`。
- **SC-004**: 回测口径零变化 —— 同一策略、同一交易对与区间的回测关键指标(成交笔数、净盈亏)与修复前一致。
- **SC-005**: 文档同步 —— 023 的 `converge.md` §五 F5 标记为已修复并指向本变更; `specs/backtest.md` 或 `specs/architecture.md` 中涉及 Dry Run 虚拟持仓的现状描述与修复后行为一致。

## 五、假设

- 平仓后残留的 `(side=平仓方向, size=0)` 本身无害: 只要**开仓时按成交方向置位**, `(side, size)` 即可恢复自洽 —— 因此不引入第三态。
- Dry Run 采用**净仓**模型(一对交易对一条记录), 与 `BacktestContext` 的净仓口径一致; 方向仓不在本次范围。
- `position_side` 的 `none` 判定(`size<=0`)是下游契约, 保持不变。
- 缺陷为**纯逻辑**问题, 依宪法测试纪律用单元测试即可充分验证; 另按 SC-003 补一次真实行情 Dry Run 复测作为端到端佐证。
- 修复不改变磁盘格式与配置, 不影响已部署策略的兼容性。
