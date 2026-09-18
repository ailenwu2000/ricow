# 024 实施计划

**规格**: [spec.md](./spec.md) · **来源**: [023-ai-chat-ux/converge.md](../023-ai-chat-ux/converge.md) §五 F5(HIGH)

## 一、决策

| # | 决策 | 理由 |
|:--|:--|:--|
| D1 | 修复点 = `apply_position_change` 的两个 `else`(加仓)分支各补一行 `entry.side = req.side`; **不改 `Position` 结构、不引 "flat" 第三态** | 根因即"平仓残留 `side` + 加仓不置位"。`size == 0` 时 `old_notional = 0`, 开仓价本身会自动重算为 `fill_price` —— 唯一缺失的量就是 `side` |
| D2 | 先把记账逻辑抽成**纯函数** `apply_fill_to_net_position(&mut Position, side, size, fill_price) -> Decimal`(返回该笔已实现盈亏), 由 `apply_position_change` 调用 | `apply_position_change` 需 `DryRunContext` 实例(依赖 `Arc<dyn Exchange>`), 仓库内**无**可复用测试交易所替身(已 grep 确认); 抽成纯函数后可像既有 `net_position`/`side_tag` 一样零依赖单测, 且不改变任何运行时行为 |
| D3 | 回退验证**分两步**落地: 第一步只做 D2 纯抽取(零行为变化)+ 加测试 → 跑出**红**; 第二步落 D1 一行 → 跑**绿** | SC-002 要求"未修复代码上必须红"。一次改完无法自证测试真的盯住了缺陷 |
| D4 | `lua.rs` 的方向标签抽成纯函数 `position_side_label(&Position) -> &'static str`(`size<=0` → `none`) | FR-005 的契约(`ctx:position_side` 归零返 `none`)实现在 `lua.rs:344-353`; 抽取后可在单测直接断言 `(size=0, side=Sell) → "none"`, 零行为变化 |
| D5 | 只改 Dry Run; `BacktestContext` / `LiveContext` **一行不改** | 回测是多空分离实现(已正确), 实盘持仓来自交易所; 扩大范围无收益且有回归风险 |
| D6 | 不新增 CLI 参数 / 配置项 / 依赖; 不动余额与手续费口径(`apply_virtual_balance_change`) | 防止把"记账修复"扩大成"行为变更"; SC-004 要求回测口径零变化 |
| D7 | SC-004 用**修复前后同一条回测命令输出比对**取证(成交笔数 / 净盈亏), 不靠推断 | 回测走 `BacktestContext`, 本不受影响; 但需实测证据闭环 |
| D8 | SC-003 复用 023 复测的 `e2e_ob` 夹具与隔离沙箱 `%TEMP%\ricow-e2e`, 真实行情 Dry Run | 同夹具同口径才能与 023 §七 的 F5 活体证据逐项对照, 使"修复前 vs 修复后"可比 |
| D9 | 不实现 Dry Run 的方向仓(`position_directional` 仍走 trait 默认 `None`) | 超出本次范围, 见 spec §二"不做" |

## 二、改动清单

| 文件 | 改动 |
|:--|:--|
| `crates/ricow_strategy/src/context.rs` | 抽出纯函数 `apply_fill_to_net_position`(含 D1 的一行修复); `apply_position_change` 改为构造/复用 `Position` 后调用它并按返回值记盈亏; `mod tests` 新增 ≥5 条单测覆盖 FR-007 五条路径 |
| `crates/ricow_strategy/src/lua.rs` | 抽出 `position_side_label` 并在净仓快照处调用(替换原地 if/else 链); `mod tests` 新增 FR-005 断言 |
| `specs/changes/023-ai-chat-ux/converge.md` | §五 F5 行状态由"未修复"改为"已修复(024)", 并指向本变更; §七 追加修复后复测对照证据 |
| `specs/lua-api.md` | `ctx:position_side/size/entry` 段补一句: Dry Run 净仓的 `side` 由成交方向决定, 归零(净 0)时返 `none` —— 与文档既有契约一致(SC-005 文档同步) |
| `specs/changes/024-dryrun-position-side/` | `tasks.md`(任务分解) + `converge.md`(收敛证据) |

## 三、风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | "同向加仓时 `side` 已是同向, 补赋值是空操作"被误当成修复无效 | D3 的两步回退验证直接证明: 只抽取不修复时, "平仓后反向开仓"用例必须失败 |
| R2 | 误伤既有正确路径(同向加权均价 / 部分平仓保留均价) | FR-003/FR-004 各有专门单测; SC-001 全量测试零回归 + SC-004 回测零变化双闸 |
| R3 | 抽纯函数时改动 `record_pnl` 调用点导致盈亏统计漂移 | `record_pnl(0)` 对 `PnlTracker` 是空操作(不加权、不计胜负场次, 已核实 `pnl.rs:36-45`); 仍保留"仅非零才记"的调用条件以完全等价 |
| R4 | `size == 0` 与 `reduce_only` / 挂单撮合路径的耦合被忽略 | `reduce_only` 走撮合前置判断, 与记账无关; 计划内不改撮合, 由 SC-001 全量测试兜底 |
| R5 | 真实 Dry Run 复测不可复现(行情驱动, 非确定性) | 按 SC-003 用**时间盒 + tick 数上限**并只断言结构性事实(出现买卖交替 / `size` 回落至 0 / `side` 不再恒单边), 不断言具体价格与笔数 |

## 四、验收动作

1. **红→绿回退验证(SC-002)**: 完成 D2 纯抽取后先跑 `cargo test -p ricow_strategy` 记录失败输出(≥5 条新增用例中"平仓后反向开仓"类必须红), 再落 D1 修复跑绿; 两段输出留档到 `converge.md`。
2. **门禁全绿(SC-001)**: `cargo test -p ricow_strategy` + `cargo test -p ricow` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt --all -- --check` 全部 EXIT=0。
3. **真实回测零变化(SC-004)**: 用同一配置、同一交易对与区间跑 `ricow backtest`, 比对修复前后的成交笔数与净盈亏一致。
4. **真实 Dry Run 复测(SC-003)**: 重建隔离沙箱 `%TEMP%\ricow-e2e`, 用 `e2e_ob` 夹具在真实行情下跑 one-way 净仓策略, 观测: `side` 不再恒为单边、`fills` 出现买/卖交替、`size` 能回到 `0` 后再次开仓。
5. **文档同步(SC-005)**: 回填 023 `converge.md` §五 F5 状态 + §七 对照; 核对 `specs/lua-api.md` 净仓口径描述与修复后行为一致。
6. **收尾**: `tasks.md` 全项勾选 + 写 `converge.md`; 清理沙箱与已停实例。
