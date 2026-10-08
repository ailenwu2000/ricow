# 037 技术方案: uniswap_v2_grid 适配事件模型

**背景**: 034 落地引擎事件驱动能力（`on_fill` 可返回订单，settle_orders 递归，深度上限 8）；036 已完成 paired_grid_futures_long 策略侧适配（commit f2f1115）。本计划沿用同一模式，**只改 uniswap_v2_grid 一个策略**（用户工作规则：每次只改一个策略）。

## 一、现状诊断

策略当前决策路径（[uniswap_v2_grid.lua](../../../strategies/spot/uniswap_v2_grid.lua)）：

| 触发 | 行为 | 缺口 |
|---|---|---|
| `on_tick`（逐 bar，节流 `t.ts == last_bar_ts`） | 激活/建仓市价单、成本门槛校验、停摆计数；`need_rehang` 为真时全撤重挂两侧（334-398 行） | 重挂被 bar 节流 |
| `on_fill` | 建仓分支（405-429 行）与网格分支（431-448 行）均只记账（balance_price/模型账本/统计），置 `need_rehang = true`，**返回 nil** | 成交→重挂要等下一次 on_tick |

与 036 相同的缺口：bar 内 fill 先入账再走 on_tick，"本 bar 成交"的重挂尚能同 bar 完成；但 **on_tick 重挂出的单在本 bar 内再成交（链式穿价）后，第二次重挂拖到下一根 bar**。对 univ2 影响比 paired 更直接——其买卖单挂在 balance_price ± spacing（无 min_pair_profit 保底、间距可低至 0.4% 下限），1m 数据 + 0.4% 间距下同 bar 双向穿价概率高于 1% 间距的 paired 版，链式损失窗口更常出现。

## 二、方案

### R1 抽共享重挂函数 `do_rehang(ctx)`

把 `on_tick` 中"成本门槛之后、重挂产出订单"的逻辑（现 328-398 行：停摆恢复、C/Q 真实账本读取、1:1 恢复量公式、cap_buy/cap_sell、min_notional、两侧均挂不出时的停摆计数）原样抽成 `do_rehang(ctx)`，**逐行搬移不改语义**，返回订单表（含 `cancel_pending`）或 `{}`。成功挂出时 `stall_since/stall_warned_at 清零、need_rehang=false、rehang_count+=1、save_state` 等副作用随函数走。

### R2 `on_fill` 返回订单（核心改动）

- `on_fill` 记账完成后（**建仓分支与网格分支都包含**），置 `need_rehang = true` 后调用 `do_rehang(ctx)` 并 **return 其订单表**；无单可挂（小名义/单侧资金耗尽）返回 `{}`、保持 `need_rehang` 由后续 on_tick 重试——与现有停摆语义一致。
- 建仓成交（`pending_entry` 分支）同样立即重挂：建仓成交 → 同 bar 挂出平衡价 ± spacing 两侧限价单，不再等下一根 bar。
- 返回的重挂单价格 = balance_price ± spacing：买单在现价下方、卖单在现价上方（balance_price := 本笔成交价），**不会在本 bar 立即成交** → 引擎 drain 无二次派发，递归深度恒为 1，不触碰 034 深度上限。
- `halted == true` 时 `on_fill` 直接 return（防御）。

### R3 `on_tick` 保持骨架，只换重挂出口

- 节流、ATR 未就绪分支、成本门槛、未激活/建仓分支**全部不动**；
- 已激活分支末尾由 `if need_rehang then return do_rehang(ctx) end` 出口（替代原内联重挂块）；
- 激活/建仓市价单仍只在 on_tick 发出（行情驱动决策，与 036 D1 同口径）。

### R4 行为变化声明（本计划的目的，非回归）

- 改后：同 bar 内"重挂单成交 → 立即按新平衡价再重挂"成为可能，链式穿价段（震荡密集区）成交密度提升；
- 回测结果**必然相对基线漂移**，以复跑新结果作为新基线记录；
- 除 R1/R2/R3 外的任何语义（1:1 恢复公式含费修正、min_spacing_pct 下限、成本门槛、停摆 WARN、模型账本对账、期末不清仓）**零改动**。

## 三、决策点

| # | 决策 | 推荐 | 备选 |
|---|---|---|---|
| D1 | 建仓激活是否也搬进 on_fill | 否——激活判断是行情驱动，留在 on_tick；建仓**成交后**的立即重挂搬入 on_fill | 全搬（行为变化更大，否） |
| D2 | stall_bars 统计口径 | 不变：仍按 bar 计，on_fill 挂出成功即清零（副作用在 do_rehang 内，天然一致） | 按 fill 计 |
| D3 | paired_grid_futures_short 是否同期改 | 否——该策略尚不存在，且每次只改一个策略 | — |
| D4 | 同 bar 双向 ping-pong（实施中发现） | **待定**——见 §四"实施发现" | — |

## 四、涉及文件与测试

**文件**:
- `strategies/spot/uniswap_v2_grid.lua`（唯一代码改动）
- `specs/roadmap.md`（测试基线数字）
- 本 plan.md 落地验证记录

**测试**（builtin_tests.rs 增补，复用 run_event_full/EventRun）:
1. **同 bar 链式重挂**: 构造一根 bar 内网格成交，`on_fill` 返回的订单含 `cancel_pending` + 买/卖限价单，价格 = 新 balance_price ± spacing、量满足 1:1 恢复公式（含费修正）。
2. **建仓成交立即重挂**: 建仓市价单成交 → `on_fill` 返回两侧限价单，`balance_price == 建仓成交价`。
3. **无单可挂**: 资金不足时 `on_fill` 返回空表且 `need_rehang` 保持 true（后续 bar 重试）。
4. **1:1 平衡不变**: on_fill 链式重挂后，模型账本 C/Q 在新平衡价处价值比仍 ≈ 1:1（含费修正量）。
5. **深度安全**: on_fill 返回的限价单不在本 bar 成交 → 引擎 drain 无二次派发（无递归）。
6. **SOLUSDT 1m 6 个月复跑**: 门禁全绿后复跑（投入 10000 USDT，start_price 同基线窗口），与基线报告（/tmp/ricow-bt-univ2: 1928 笔 / 净 +374.68）对比成交数与净盈亏，按回测规范 v1 出概览（A1-A7 + B 区），结果作为新基线记录。

**门禁**: cargo fmt / clippy / test --workspace（583 passed 不倒退）。

## 五、风险

- 行为漂移超出预期 → 以第 6 项测试复核归因，异常即回滚重挂出口改动。
- on_fill 内调用 `ctx:atr_tf()`/`ctx:balance()` → 034 已验证 fill 派发点只读上下文就绪，036 已实测无新增风险。
- 0.4% 最小间距下链式成交显著增多 → 手续费磨损上升使净盈亏反而下降 → 归因说明并在新基线中记录，必要时讨论下限调参（不在本计划范围）。

## 六、实施发现与最终结论: 同 bar 双向 ping-pong → univ2 豁免 R2

**计划 §二 R2 曾假设**"重挂限价单不会在本 bar 立即成交 → 递归深度恒为 1"。实施中被合成 K 线测试与 180d A/B 回测**双重证伪**，最终裁决:**univ2 放弃 R2（on_fill 不返回订单），仅保留 R1 重构（do_rehang）与 R3 出口**。

### 病理机制（比计划预判更严重: 非仅乐观近似，而是静默停摆）

- 机制: univ2 网格 = `平衡价 ± spacing` **双侧**挂单（间距低至 0.4% 下限）。bar 范围同穿两侧时，
  引擎 OHLC 撮合器对同 bar 新挂单按整根 bar 重匹配 → 买 98 → 卖 100 → 买 98 → … 同 bar ping-pong，
  撞 034 `MAX_FILL_DECISION_DEPTH=8` 封顶后**整批静默丢弃**（含 cancel_pending，仅 WARN）。
- 后果链: 丢批后 `need_rehang=false` 但挂单簿空/旧 → 后续 bar 不再重挂 → **数周静默停摆**
  （0.4% 间距下 ping-pong 在波动时段常态出现）。036 (paired) 间距 ≥4% 未触发。

### 180d A/B 实测（SOLUSDT 1m, 2026-03-31~09-27, 投入 10000, start_price=82.63）

| 版本 | 成交笔数 | 净盈亏 | 停摆 |
|---|---|---|---|
| 基线（on_fill 不返回订单, 等价 033 v1） | **1906** | **+386.65**（费 46.89） | 零 |
| 首版 R2（on_fill 返回订单, 无自愈） | 828 | +305.03 | 6-7 月停摆 |
| 自愈版（深度封顶后保持 need_rehang=true） | 1666 | +159.85 | 8/19 后停摆 5 周 |

两个事件模型变体均**净劣于基线**（ping-pong 链被深度封顶截断后挂单簿失同步是主要损耗; 自愈版虽恢复挂单但链式截断已扭曲平衡价路径）。R2 对本策略为纯负收益。

### 最终落地面（v2 终态）

1. **R1 保留**: `do_rehang(ctx)` 抽取, on_tick 骨架不变、末尾 `return do_rehang(ctx)` 出口（纯重构, 语义零改动）;
2. **R2 豁免**: on_fill（建仓/网格分支）记账后直接 `return`（保留 need_rehang=true）, 头注释记录豁免理由与本病理;
3. 测试: 测试辅助 `settle_univ2` 保留（对齐引擎递归语义, 服务未来策略）; `chain_capped` 测试改锁"豁免后无 ping-pong 链"行为（宽 bar 双侧各成交 1 次, fill_count=3）; 索引断言回到基线口径（重挂在 fill 同 bar 的 on_tick 批次, orders[16]/orders[18]）;
4. **回测等价性验收（已通过）**: 同窗口对照（2026-03-30 17:29/17:32 起 180d 1m, v1 lua 从 git HEAD 提取经 `--script` 直跑）: 基线 1915 笔 / 净 +382.85 = 终态 1915 笔 / 净 +382.85; **fills 逐分全等**（仅随机 exchange_order_id UUID 不同）; equity 曲线数值一致（仅因两次运行窗口起点相差 3 分钟整体平移 3 bar）。此前 /tmp/ricow-bt-univ2-base（1906 笔/+386.65）为 09-26 窗口, 差异纯由数据窗口偏移导致, 非行为漂移。

### 038 候选（后续计划）

- 引擎撮合保真: 同 bar fill 触发新挂的限价单限制"单向穿越"匹配（允许 036 式单向链、封杀双向 ping-pong）;
- 深度封顶丢批的策略通知机制（on_order_update/错误回传）, 消除静默丢批类病理。
