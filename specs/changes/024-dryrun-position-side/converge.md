# 024 收敛记录: Dry Run 虚拟持仓方向记账修复

**变更**: [spec.md](./spec.md) · [plan.md](./plan.md) · [tasks.md](./tasks.md)

**来源**: [023-ai-chat-ux/converge.md](../023-ai-chat-ux/converge.md) §五 **F5(HIGH)** / §七 活体证据

**日期**: 2026-09-19 · **状态**: 已收敛(实现 + 取证完成, 未提交)

**代码改动**(`git diff --numstat` 口径): `crates/ricow_strategy/src/context.rs`(+185/-52, 含纯函数抽取/修复/7 条单测)、`crates/ricow_strategy/src/lua.rs`(+37/-9, 含 `position_side_label` 与 1 条单测); **`backtest.rs`(BacktestContext)与实盘 `LiveContext` 一行未改**(D5)。

## 一、决策落实(D1–D9)

| # | 决策 | 落实 |
|:--|:--|:--|
| D1 | 两个 `else` 分支各补一行 `entry.side` | 已落: [context.rs:292-294](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L292-L294) / [:319-320](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L319-L320); 未改 `Position` 结构、未引第三态 |
| D2 | 抽纯函数 `apply_fill_to_net_position` | 已落: [context.rs:267-330](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L267-L330); `apply_position_change` 改为调用([:672](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L672)) |
| D3 | 分两步回退验证(先抽取+测试跑红, 再修复跑绿) | 已落, 原始输出见 §三 |
| D4 | 抽 `position_side_label` | 已落: [lua.rs:290-302](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/lua.rs#L290-L302), 调用点 [:359](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/lua.rs#L359) |
| D5 | 只改 Dry Run | 已落: `git diff --stat` 中 `ricow_strategy` 仅 `context.rs` / `lua.rs` |
| D6 | 不加 CLI/配置/依赖; 不动余额口径 | 已落: 无新增参数与依赖; `apply_virtual_balance_change` 未改 |
| D7 | SC-004 用同一条回测命令前后比对 | 已落, 见 §四 |
| D8 | SC-003 复用 `e2e_ob` 夹具与 `%TEMP%\ricow-e2e` 沙箱 | 已落, 见 §五 |
| D9 | 不实现 Dry Run 方向仓 | 已落: `position_directional` 仍走 trait 默认 |

## 二、验收标准逐条结论

| SC | 结论 | 证据 |
|:--|:--|:--|
| SC-001 门禁全绿零回归 | ✅ | `cargo test -p ricow_strategy` → `149 passed; 0 failed`(EXIT=0); `cargo test -p ricow` → bin `165 passed / 0 failed / 1 ignored` + integration `3 passed / 0 failed / 9 ignored`(EXIT=0); `cargo clippy --workspace --all-targets -- -D warnings` EXIT=0; `cargo fmt --all -- --check` EXIT=0 |
| SC-002 新增单测 ≥5 条 + 回退验证(先红后绿) | ✅ | 新增 8 条(context.rs 7 + lua.rs 1); 红: `146 passed; 3 failed`(EXIT=101); 绿: `149 passed; 0 failed`。原始输出见 §三 |
| SC-003 真实 Dry Run 复测 | ✅ | 同夹具同沙箱: `fills` 买卖交替、`side` `none`↔`long` 交替(各 118 次)、`size` 恒 0.0/0.27 无放大。见 §五 |
| SC-004 回测口径零变化 | ✅ | `shannon_grid` 42 笔 / +2662.5808 / 胜率 100%; `dca` 2160 笔 / -43.8505 —— 与 023 §七 基线逐项一致。见 §四 |
| SC-005 文档同步 | ✅ | 023 `converge.md` §五 F5 行改"已修复"并指向本变更 + §七 新增 §七.1 对照表; `specs/lua-api.md` 净仓语义段补注 Dry Run 不变式 |

## 三、红→绿回退验证原始输出(SC-002 / T3 / T5)

**红**(临时移除两行 `entry.side = …`, 仅留 D2 纯抽取 + 测试):

```
test context::tests::test_reopen_short_after_closing_short ... FAILED
test context::tests::test_reopen_long_after_closing_long ... FAILED
test context::tests::test_repeated_open_close_never_leaves_stale_side ... FAILED
thread '…test_reopen_short_after_closing_short' panicked at crates\ricow_strategy\src\context.rs:977:9:
left: Buy / right: Sell
thread '…test_reopen_long_after_closing_long' panicked at crates\ricow_strategy\src\context.rs:964:9:
left: Sell / right: Buy
thread '…test_repeated_open_close_never_leaves_stale_side' panicked at crates\ricow_strategy\src\context.rs:1027:13:
left: Sell / right: Buy
test result: FAILED. 146 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
RED_EXIT=101
```

(上表行号为"移除两行"后的位置; 恢复修复后对应 [context.rs:966](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L966) / [:979](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L979) / [:1029](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L1029)。)

**绿**(恢复 D1 两行后):

```
test result: ok. 149 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
STRATEGY_EXIT=0
FMT_EXIT=0
CLIPPY_EXIT=0
test result: ok. 165 passed; 0 failed; 1 ignored   (bin ricow)
test result: ok. 3 passed; 0 failed; 9 ignored     (integration)
RICOW_TEST_EXIT=0
```

**新增单测清单**(FR-007 覆盖): `test_open_short_after_closing_long`(①)、`test_open_long_after_closing_short`(②)、`test_reopen_long_after_closing_long`(③)、`test_reopen_short_after_closing_short`(③对称面)、`test_partial_close_keeps_side_and_entry`(④)、`test_reversal_larger_than_position`(⑤)、`test_repeated_open_close_never_leaves_stale_side`(023 现象 A 回归)、`test_position_side_label_contract`(FR-005 下游契约: `(size=0,*)->"none"` / `(size>0,Buy)->"long"` / `(size>0,Sell)->"short"`)。

## 四、回测零变化取证(SC-004 / D7 / T8)

同一条命令口径, 与 023 §七 基线**逐项一致**:

| 策略 | 区间 | 成交 | 净盈亏 | 其他 | EXIT |
|:--|:--|:--|:--|:--|:--|
| `shannon_grid` ETHUSDT | 90d 1h(2160 根) | 42 笔 | `2662.5808561554223298571714144` | 已实现盈亏 `2751.8595…` / 手续费 `89.2786…` / 最大回撤 7.33% / 胜率 100% / 拒单 0 | 0 |
| `dca` ETHUSDT | 90d 1h(2160 根) | 2160 笔 | `-43.850472500000000912821328853` | 最大回撤 4.33% / 拒单 0 | 0 |

结论: 本修复走 `DryRunContext`, 回测(`BacktestContext`)关键指标零变化 → 满足 FR-006/SC-004。

## 五、真实 Dry Run 复测(SC-003 / D8 / T9)

沙箱 `%TEMP%\ricow-e2e`(含 `strategies/e2e_ob.lua` + `e2e_ob.toml`, 形状 = 无仓买入 / 有仓全平, 只用 `ctx:position_size`/`ctx:position_side` 判仓), `ricow daemon start` → 运行中(pid=21904, 端口 55432); `ricow start e2e_ob` → Dry Run 实例运行中(pid 7360, 重启后 26144)。

| 观测项 | 修复前(023 §七) | 修复后(本次) |
|:--|:--|:--|
| `ricow fills e2e_ob` 首 12 行 | 全部 sell | **buy/sell 交替**: buy 2604.55000000 ×0.27 / sell 2604.59000000 ×0.27 / buy 2604.60000000 ×0.27 … |
| 策略诊断 `side` | `short` 恒真 | `none` ↔ `long` 交替 |
| 策略诊断 `size` | 单调放大 0.27 → 1.37 | 恒 `0.0` / `0.27`, 无放大 |
| 逐 tick 分布 | — | `none|0.0` = 118 次, `long|0.27` = 118 次(计数相等 → 每次都走完"开仓→平仓归零"闭环) |
| 虚拟余额 `usdt` | 99999.9968 冻结 | `99999.87`(空仓)↔ `99296.515`(持仓)摆动, 手续费正常递减 |
| `status`/`info` | 成交 542/578、手续费 14.06 | 成交 412 / 460 / 885 笔, 手续费累计 289.77 / 323.52 / 622.35 |

夹具自检(同脚本走回测): `ricow backtest --strategy e2e_ob --days 3 --interval 1h` → 72 根 K 线 / 72 笔 / 净盈亏 `-15.975400500000001051037479259`(EXIT=0)。

## 六、文档同步(T10 / SC-005)

- [023-ai-chat-ux/converge.md](../023-ai-chat-ux/converge.md): §五 F5 行 → 类型标 `defect(已修复)`、证据列按实测收窄(缺陷只在非平仓 `else` 分支暴露)、处置列指向本变更; §五 汇总行改为"已由 024 修复"; §七 处置段补修复说明, 新增 **§七.1 F5 修复后对照证据**表。
- [specs/lua-api.md](../lua-api.md): 持仓语义段(`ctx:position_side/size` 说明下方)补注 Dry Run 虚拟净仓遵循同一不变式 —— `position_size > 0` 时 `position_side` = 建仓方向, 归零报 `"none"`, 平仓归零后按原持仓方向再开不会残留旧方向/不累加。

## 七、偏差与遗留(如实)

1. **T3 预期与实际不一致(已修正记录)**: `tasks.md`/`plan.md` 预期"①②③⑤ 红 / ④ 绿", **实测 3 红 4 绿**。原因: "平仓/反手"分支本身**已带** `entry.side` 赋值, 未修复也正确; 缺陷只落在外层 `else`(开仓/加仓)分支, 仅"**平仓归零后按原持仓方向再开**"会踩中。故 ①(平多后反向开空)、②(平空后反向开多)、⑤(反手)在未修复代码上即绿。已在 `tasks.md` T3 备注与 023 §五 F5 证据列按实测修正(plan R1 的原始表述以本节为准)。
2. **真实 Dry Run 的行情驱动特性**: 按 R5 只断言结构性事实(买卖交替 / `size` 归零 / `side` 不再恒单边), 不断言具体价格与笔数; 计数(各 118 次)与成交笔数(412/460/885)随运行时长增长, 属预期。
3. **未跑项**: 无(plan 验收动作 1–6 全部执行)。
4. **环境收尾**: `ricow stop e2e_ob` → `exit=0`; `ricow daemon stop` → `pid=21904 已停止`; 沙箱 `%TEMP%\ricow-e2e` 已删除(`SANDBOX_EXISTS=False`, 无 `ricow-e2e*` 残留目录)。
5. **未提交**: 代码与文档改动均在工作区, 未 `git commit`(宪法提交纪律: 用户明确说"提交"才提交)。
