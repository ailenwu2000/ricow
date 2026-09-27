# 036 技术方案: paired_grid_futures_long 适配事件模型

**背景**: 034 已落地引擎事件驱动能力（`on_fill` 可返回订单，引擎 place 后 drain 再派发，深度上限 8）。034 范围铁律明确"策略侧改造留待单独计划"——本计划即该单独计划，**只改 paired_grid_futures_long 一个策略**（uniswap_v2_grid 等留待后续，每次一个策略）。

## 一、现状诊断

策略当前决策路径（[paired_grid_futures_long.lua](../../../strategies/futures/paired_grid_futures_long.lua)）：

| 触发 | 行为 | 缺口 |
|---|---|---|
| `on_tick`（逐 bar，节流 `t.ts == last_bar_ts`） | 激活/追踪上移/爆仓监控/成本门槛；`need_rehang` 为真时全撤重挂 | 重挂被 bar 节流 |
| `on_fill` | 只做记账（lots/flag/ref/统计），置 `need_rehang = true`，**返回 nil** | 成交→重挂要等下一次 `on_tick` |

回测主循环为 `step_bar → drain/on_fill → on_tick → place_orders`，bar 内 fill 先入账再走 on_tick，所以"本 bar 触发成交"的重挂尚能同 bar 完成；真正的缺口是：**on_tick 重挂出的单在本 bar 内再成交（穿价链式成交）后，第二次重挂要拖到下一根 bar**。1m 主时钟下最多丢 1 分钟成交窗口，链式穿价密集段成交密度受损。这正是 034 plan.md §一"成交→决策延迟 = 最多 1 根主时钟 bar"的策略侧实例。

## 二、方案

### R1 抽共享重挂函数 `do_rehang(ctx)`

把 `on_tick` 中"成本门槛之后、重挂产出订单"的逻辑（现 383-454 行：卖价锚定栈顶成本、min_pair_profit 保底、cap_open/cap_close、min_notional、停摆计数）原样抽成 `do_rehang(ctx)`，**逐行搬移不改语义**，返回订单表（含 `cancel_pending`）或 `{}`。成功挂出时 `stall_bars=0; need_rehang=false; rehang_count+=1; save_state` 等副作用随函数走。

### R2 `on_fill` 返回订单（核心改动）

- `on_fill` 记账完成后（建仓拆格分支与网格成交分支都包含），置 `need_rehang = true` 后调用 `do_rehang(ctx)` 并 **return 其订单表**；无单可挂（小名义/资金不足）返回 `{}`、保持 `need_rehang` 由后续 on_tick 重试——与现有停摆语义一致。
- 建仓成交（`pending_entry` 分支）同样立即拆格重挂：建仓成交 → 同 bar 挂出全部配对单，不再等下一根 bar。
- 返回的重挂单是 ref±spacing 的限价单，**不会在本 bar 立即成交** → 引擎 drain 不会再派发，递归深度恒为 1，不触碰 034 的深度上限。
- `halted == true` 时 `on_fill` 直接 return（防御，正常不会发生）。

### R3 `on_tick` 保持骨架，只换重挂出口

- 节流、激活/建仓、追踪上移 while 循环、爆仓监控、成本门槛**全部不动**；
- 追踪上移/续接/拒单等置 `need_rehang = true` 的地方，末尾由 `if need_rehang then return do_rehang(ctx) end` 出口（替代原内联重挂块）。
- **追踪上移仍只在主时钟 bar 触发**（决策点 D1）：成交本身已把 ref 置为成交价（即"成交价锚定"），追踪是行情驱动的行为，交给 bar 时钟语义更清晰，也让行为变化最小化。

### R4 行为变化声明（本计划的**目的**，非回归）

- 改后：同 bar 内"重挂单成交 → 立即再重挂"成为可能，链式穿价段成交密度提升；
- 180d 回测结果**必然相对基线漂移**（不再要求与 bar 节流基线逐分一致），以复跑新结果作为新基线记录；
- 除 R1/R2/R3 外的任何语义（间距、flag、建仓不计数、卖价锚栈顶成本、min_pair_profit、coin 模式、期末强平）**零改动**。

## 三、决策点

| # | 决策 | 推荐 | 备选 |
|---|---|---|---|
| D1 | 追踪上移是否也搬进 on_fill | 否——仍在 on_tick（bar 驱动），成交只做 ref:=成交价+重挂 | 搬入（栈空卖出后立即追踪，行为变化更大，否） |
| D2 | stall_bars 统计口径 | 不变：仍按 bar 计（on_tick 侧累计），on_fill 挂出成功即清零 | 按 fill 计（口径变化，无必要） |
| D3 | uniswap_v2_grid 是否同期改 | 否——每次只改一个策略（用户工作规则） | 同期改 |

## 四、涉及文件与测试

**文件**:
- `strategies/futures/paired_grid_futures_long.lua`（唯一代码改动）
- `specs/roadmap.md`（测试基线数字）
- 本 plan.md 落地验证记录

**测试**（builtin_tests.rs 增补）:
1. **同 bar 链式重挂**: 构造一根 bar 内 fill 后，`on_fill` 返回的订单含 `cancel_pending` + 买/卖限价单，且价格/数量与记账后的 lots/flag/ref 一致。
2. **建仓成交立即拆格**: 建仓市价单成交 → `on_fill` 返回 n 笔配对仓对应的限价卖单（栈顶）+ 买单，`ref_price == 建仓成交价`。
3. **无单可挂**: 资金不足时 `on_fill` 返回空表且 `need_rehang` 保持 true（后续 bar 重试）。
4. **卖价锚定不变**: on_fill 重挂的卖价 == 栈顶成本×(1+up)，min_pair_profit 保底仍生效。
5. **深度安全**: on_fill 返回的限价单不在本 bar 成交 → 引擎 drain 无二次派发（无递归）。
6. **SOLUSDT 1m 180d 复跑**: 门禁全绿后复跑，与 bar 节流基线（v2: 324 笔 / +171.99）对比成交数与净盈亏，确认链式成交密度变化方向合理，结果作为新基线记录。

**门禁**: cargo fmt / clippy / test（578 passed 不倒退）。

**实施验证(2026-09-27)**: 全部落地。①-⑤以 5 个引擎级集成测试覆盖(`on_fill_rehangs_immediately` / `on_fill_build_splits_immediately` / `on_fill_no_order_keeps_rehang` / `on_fill_min_pair_profit_floor` / `grid_pair_cycle`), 测试辅助 `run_event_full` 复刻引擎 settle_orders 递归(034)。⑥同窗同源 A/B: fapi 主网/本地代理当日不可达, 双方均用 `--klines-market spot`(镜像 data-api.binance.vision)同一序列: **基线 v2 = 246 笔 / 净 +144.57(+7.23%) / 权益 2122.73** vs **事件模型 = 242 笔 / 净 +142.65(+7.13%) / 权益 2120.75**(start_price=83.28, cash 2000, initial/order 100, --close-at-end)。差异 −4 笔/−0.1pp: 1m 主时钟+1% 间距下同 bar 链式穿价极罕见, 消除节流缺口后表现持平; 链式语义本身由 `grid_pair_cycle` 实测(同 bar 双卖)。导出四件套 `/tmp/ricow-bt-sol-036/`(基线对照 `/tmp/ricow-bt-sol-036-base/`)。门禁 `cargo test --workspace` = **583 passed / 0 failed / 22 ignored**, fmt/clippy 全绿。

**实施修正记录**: 计划 §一诊断的"缺口"在实测中弱于预期 —— bar 节流的重挂本就发生在"fill 先入账后的同一 on_tick", 真正损失仅在"重挂单本 bar 内再成交"的二次链场景(1m 罕见); 事件模型的价值在低周期/小间距/高波动场景更显著, 且实盘侧原就毫秒级。测试中发现 `num()` 把 `direction_offset=0` 当缺省回退 0.2(既有口径, 未改), 测试断言按 0.2 实际值编写。

## 五、风险

- 行为漂移超出预期（如成交暴涨/暴跌）→ 以第 6 项测试复核归因，异常即回滚重挂出口改动。
- on_fill 内调用 `ctx:price()` 等只读接口 → 引擎在 fill 派发点行情数据已就绪（034 已验证 on_fill 上下文），无新增风险。
