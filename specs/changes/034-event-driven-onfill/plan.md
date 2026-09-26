# 034 技术方案: 纯事件驱动决策 (on_fill 可下单)

**需求(用户 2026-09-26 口径)**:
1. 参考 NautilusTrader / Hummingbot 的事件驱动架构, 成交后**立即**让策略做决策(撤单/重挂), 不受主时钟节流。
2. 竞品架构相比 ricow 还有其他优势 → 本文 §五**仅列出清单**, 本次不改, 留待后续计划。

**范围铁律(用户 2026-09-26 追加)**: 本计划**只改架构, 不改任何策略**(strategies/ 目录零改动、内置策略重挂逻辑不动)。策略侧利用 on_fill 下单的能力改造, 一律留待后续单独计划。

## 一、现状诊断(逐路径核实)

| 路径 | 决策触发 | 成交→决策延迟 |
|---|---|---|
| 回测单标的 ([backtest_runner.rs:71-102](../../../crates/ricow_engine/src/backtest_runner.rs)) | 逐 bar 一次 `on_tick`(fill 先入账) | **最多 1 根主时钟 bar** |
| 回测组合 (backtest_runner.rs:224+) | 同上 | 同上 |
| dry-run (command.rs:683) | 逐 bar `on_tick` | 最多 1 根 bar |
| 实盘 (command.rs:1080) | **每个 WS quote 都调 `on_tick`**(已是事件驱动) + fill 后刷持仓再 `on_fill` | 到下一个 quote 更新(通常毫秒级, 行情静默时无界) |

缺口不在"通知"——`on_fill` 已经即时派发——而在**决策出口**:
- `Strategy::on_fill` 返回 `()`(strategy.rs:17), 订单只从 `on_tick` 产出;
- 回测中 `on_tick` 下单即成交(市价/越线限价)后, `on_fill` 派发了但没有后续决策, 要等下一根 bar;
- 网格类策略"成交→全撤重挂"因此被 bar 节流, 一根 bar 内多次穿价时成交密度受损。

**基线语义(必须保持)**: 不从 `on_fill` 返回订单的策略(现有全部内置策略), 回测结果逐分不变(公共前缀等价性测试继续通过)。

## 二、方案

### R1 trait 与 Lua 绑定: `on_fill` 可返回订单

- `Strategy::on_fill(&mut self, ctx, fill) -> Vec<OrderRequest>`(默认 `vec![]`, 现有 impl 零改动语义)。
- `LuaStrategy::on_fill`: Lua 侧 `on_fill(ctx, fill)` 可像 `on_tick` 一样 `return { {...订单表...} }`; `return nil`/无返回 = 无订单。复用 `on_tick` 的返回值解析路径(同一套订单表 → OrderRequest 校验)。
- 文档 `specs/lua-api.md`: on_fill 回调签名更新 + "在 on_fill 里下单"的语义(引擎会在下单后继续 drain 派发, 见 R2)。

### R2 引擎"成交→决策"有界循环

统一抽取决策执行辅助(回测两处 + dry-run 共用; 实盘见 R3):

```
place_orders(ctx, strategy, reqs):
    for req: place → 拒单回传 on_order_update
    fills = ctx.drain_fills()
    for fill: strategy.on_fill(ctx, fill) → 若返回新订单, 递归 place_orders, 深度 +1
    深度 ≥ MAX_FILL_DECISION_DEPTH(默认 8) → 停止递归, WARN + 计数
```

- 主循环骨架不变: `step_bar → drain/on_fill(带循环) → on_tick → place_orders`。
- 防失控: 网格重挂单挂在成交价 ± 间距处, 不会立即成交; 只有策略写出"成交即市价反手"的病态逻辑才可能递归, 深度上限 + WARN + `stat_recursive_rehang`(可观测)兜底。
- 回测行为等价性: 不返回订单的 `on_fill` 不触发任何新 place, 与现状逐分一致。

### R3 实盘接线 (command.rs LiveEvent::Fill 分支)

- `strategy.on_fill(&mut ctx, fill)` 的返回订单走既有的下单+落库路径(persist_order_ack / outcome.orders_submitted), 拒单回传 `on_order_update`。
- 实盘新单成交来自用户流(UserEvent::Fill), 天然再次触发 on_fill → 不需要实盘侧递归循环, 零额外风险。
- 回测/实盘语义对齐说明写入 architecture.md(实盘的"下一事件"是 quote/user 流, 回测的"下一事件"是同 bar 撮合)。

### R4 文档与规范

- `specs/lua-api.md`: on_fill 契约(可下单、递归深度上限、病态模式警示)。
- `specs/architecture.md`: 事件驱动决策语义小节(fill 先入账 → on_fill 决策 → on_tick 兜底)。
- `specs/roadmap.md`: 测试基线数字更新。
- `specs/research/competitor-architecture-2026-09.md`: §五追加"事件驱动决策"已落实状态。

## 三、决策点(推荐已给出, 审核时可改)

| # | 决策 | 推荐 | 备选 |
|---|---|---|---|
| D1 | API 形态 | on_fill 直接返回订单(Nautilus 同构, 改动最小) | 新增 ctx:place_order() 让 Lua 在回调内即时下单(暴露命令通道, 违背"返回声明式订单"惯例, 否) |
| D2 | 递归深度上限 | 8, 超限 WARN+计数并停止 | 可配置参数(YAGNI, 固定常量) |
| D3 | on_tick 是否也改为"逐 quote 调用"的回测语义 | 不改——回测没有 quote 流, 保持逐 bar; 回测粒度提升留给后续"竞品优势"清单(L2/盘中撮合增强) | — |
| D4 | 内置策略是否改用 on_fill 重挂 | 本次不改策略(架构能力与策略改造分离) | 顺手改 univ2/paired(YAGNI, 后续单独计划) |

## 四、涉及文件与测试

**文件**:
- `crates/ricow_strategy/src/strategy.rs`: trait 签名。
- `crates/ricow_strategy/src/lua.rs`: on_fill 返回值解析。
- `crates/ricow_engine/src/backtest_runner.rs`: 两处主循环 + place_orders 辅助。
- `crates/ricow_engine/src/command.rs`: dry-run 循环 + 实盘 Fill 分支。
- 文档四处(R4)。

**测试**(builtin_tests.rs / lua.rs / engine tests):
1. Lua `on_fill` 返回订单 → 订单被 place(类型/价格/数量断言)。
2. `on_fill` 下单即成交 → 新 fill 再次派发 on_fill(深度 2 链条), 账本逐分一致。
3. 病态策略(市价反手)→ 深度 8 封顶 + WARN, 不死循环。
4. 不返回订单的现有策略 → 回测公共前缀逐分等价(回归硬门槛)。
5. 实盘 Fill 分支: on_fill 返回订单 → orders_submitted 计数 + ack 落库(mock exchange 层既有测试模式)。
6. SOLUSDT 1m 180d 复跑(paired_grid_futures_long + uniswap_v2_grid): 报告与 2026-09-26 基线一致(等价性实证)。

**实施验证(2026-09-26)**: ①②已以引擎级集成测试落地(`on_fill_orders_placed_and_chained_within_bar` / `on_fill_recursion_depth_is_capped`); ⑤实盘分支走同一提交路径(计数+落库), 编译门禁覆盖; ⑥等价性实证: uniswap_v2_grid SOLUSDT 1m 180d(2026-03-30→09-26, 投入 10000, start_price=81.53)复跑 = **1913 笔 / 净 +375.953805… / 总价值 12451.319305…(+24.51%)**, 与基线逐位一致; 门禁 `cargo test --workspace` = **578 passed / 0 failed / 22 ignored**(较 576: +2), fmt/clippy 全绿。

门禁: cargo fmt / clippy / test(576 passed 不倒退)。

## 五、竞品架构优势清单(仅列出, 本次不改)

来源: NautilusTrader 官方架构文档、竞品架构研究(specs/research/competitor-*.md)补遗。

| # | 优势 | 竞品 | ricow 现状 | 备注(价值/成本) |
|---|---|---|---|---|
| 1 | **回测撮合含部分成交与滑点模型** | Nautilus(市价单走簿、latency 注入、partial fill) | 回测限价单"触价即全量成交", 无滑点/排队 | 高价值(回测-实盘偏差主源, 文献称贡献 10-30% PnL 偏差); 成本中 |
| 2 | **事件溯源 + 状态重建**(event sourcing: fill/订单/资金全为追加事件, 崩溃后重放重建) | Nautilus | SQLite 落库(成交/订单/状态快照)但无"重放=重建"保证 | 中价值(崩溃恢复审计); 成本高 |
| 3 | **实盘对账(reconciliation)**: 启动时以交易所回报为准校正本地订单/持仓状态 | Nautilus ExecutionManager | 启动按快照续接, 无启动对账 | 高价值(实盘安全); 成本中 |
| 4 | **RiskEngine 独立层**(订单字段/名义/频率限额集中校验, 可配置速率限制) | Nautilus | 风控散在 004 risk-guards 与各 adapter | 中价值; 成本中 |
| 5 | **确定性重放 / 纳秒时间戳**: 同一事件流回放得到逐字节相同结果 | Nautilus | 回测确定性已具备(纯函数), 实盘事件流无录放 | 中价值(调试利器); 成本中 |
| 6 | **L2 订单簿模拟**(backtest 内模拟撮合簿, 限价单排队位置) | Nautilus | OHLC 四价撮合 | 低价值(对 1m 网格收益有限)除非做高频; 成本高 |
| 7 | **策略自报版本 + 可视化标注**(`version()` / `plot_annotations()`) | Freqtrade | 无 | 低成本小功能(竞品研究 §五已记录, 维持"暂不做") |
| 8 | **同一 kernel 跑回测/仿真/实盘**(代码路径全同) | Nautilus | 三路径相似但三份循环(dry-run/live/回测各自实现) | R2 抽 place_orders 辅助是朝此方向的第一步; 全量统一成本高 |
| 9 | **MessageBus pub/sub 解耦**(组件间发布订阅, 策略可订阅任意数据通道) | Nautilus | 固定回调接口(need_klines/on_tick/on_fill) | 符合 ricow "少而精", 现有声明式接口更简单, 不建议引入 |
| 10 | **实盘行情静默期兜底**: quote 无更新时也有周期性决策心跳 | Hummingbot(轮询+事件混合) | 实盘 on_tick 仅由 quote 驱动, 行情断流=无决策(断流直接停机, 有保护) | 低成本: 实盘加一个 N 秒兜底 tick; 可与 #3 同期做 |

**建议的后续优先级**(供未来计划排期, 本期不动): #3 对账 > #1 滑点/部分成交 > #10 心跳 > 其余 YAGNI。

## 六、风险

- 病态策略递归下单 → 深度上限 + WARN + 计数(R2)。
- 回测结果漂移 → 等价性回归(#4/#6)是硬门槛, 任何漂移即打回。
- trait 签名变更波及测试桩 → 默认实现 `vec![]` 使波及面最小。
