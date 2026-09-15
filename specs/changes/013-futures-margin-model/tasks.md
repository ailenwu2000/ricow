# 013 任务分解

> 状态: **全部完成**(2026-09-13)。
> 基线: 282 → **286 passed / 0 failed / 11 ignored**。
> 规则: 记账为纯逻辑 → 单测(用 2026-09-05 真实清算实测数据校准); 现货路径必须零回归。

## 阶段 1 — 钱包维度(FR-001/FR-003)

- [x] **T001 钱包键按侧**: `wallet_key(pair_key, side)` + `wallet_of`/`add_wallet` 改造(one-way 退化 symbol 级); 判据: 既有 one-way/现货用例全绿
- [x] **T002 资金操作按方向寻址**: 开仓保证金/平仓盈亏/手续费结算全量改调用点; 判据: 单测(hedge 两侧钱包独立)+ one-way 数值不变

## 阶段 2 — 强平按侧(FR-002)

- [x] **T003 hedge 单侧强平判定**: `check_liquidation(pair_key, Some(side), bar)`; 判据: 单测(一侧穿价 → 该侧归零, 另一侧持仓/钱包/强平价不变)
- [x] **T004 报告按侧明细**: hedge 的 `liquidation_count_long/short` 与收盘钱包余额; 判据: 单测 + CLI 输出

## 阶段 3 — 结算时点(FR-004/FR-005, 关 L1/L2)

- [x] **T005 结算点计数纯函数**: `funding_settlements_in_bar`; 判据: 单测(1m/5m/15m/1h/4h = 1 次, 1d = 3 次, 跨日边界/不跨界 = 0)
- [x] **T006 按次数结算 + 按侧钱包**: `settle_closed_bar` 改造; 判据: 单测(1d 结算 3 次; hedge 多头付/空头收不抵消)
- [x] **T007 尾 bar 补结算**: `finalize()` + `report()` 前幂等调用; 判据: 单测(结束于 8h 边界 bar 时末段进报告)

## 阶段 4 — 验证与文档

- [x] **T008 真实数据校准断言**: DASHUSDT 50x + MMR 1.5% → 强平距离 ≈0.53%(SC-005)
- [x] **T009 全量回归**: `cargo test --workspace` 全绿; 现货回测命令数值逐位不变(SC-001/SC-004)
- [x] **T010 文档同步**: backtest.md(§十 L1/L2 关闭 + §十一 校准入模型依据)、architecture/roadmap、本档案 converge

## 收尾待办(不阻塞)
1. 实盘侧资金费/强平记账(fapi `/fapi/v1/income`)—— 另立变更
2. `StrategyScheduler` 去留(004 P8 遗留)

---

## 实施记录 (2026-09-13)

**核心改动(`crates/locus_strategy/src/backtest.rs`)**:
- `wallet_key_of(pair, side)`:one-way = `pair`(symbol 级单钱包),hedge = `pair|long` / `pair|short`;开仓保证金/平仓盈亏/手续费/资金费全量改为按**订单方向**寻址钱包(T001/T002)
- `check_liquidation_side(pair, side, bar)`(新):hedge 单侧判定 `E_side(p) ≤ MMR×名义_side`,只清该侧;`check_liquidation`(按对整体)仅 one-way 使用(T003)
- `settle_funding(price)`(新):one-way 按净仓(同对多空自然抵消);hedge 按侧独立 —— 多头付、空头收,**不跨侧抵消**(T006)
- `funding_settlements_in_bar(open_ms, close_ms)`(新, 纯函数):结算点 = bar 覆盖时段 `[open, close)` 内的 8h 边界;1m~4h 恰 1 次、1d 恰 3 次(L1 关闭)(T005)
- `finalize()`(新, 幂等):报告前补结算**最后一根未收盘 bar**;`backtest_runner` 已接线(L2 关闭)(T007)
- 报告新增 `hedge_sides: Option<HedgeSides>`(各侧强平次数 + 收盘各侧钱包余额),CLI 打印按侧明细(T004)
- 按侧强平计数 `liquidation_long` / `liquidation_short`(账户级 `liquidation_count` 仍为两侧之和)

**单测(新增 4 例, 全量 282 → 286)**:
- `test_funding_settlements_in_bar_is_interval_independent`:1m/1h/4h/1d/零长/倒置(1d = 3 次;1m 只有覆盖边界的那根命中)
- `test_hedge_funding_flows_between_independent_side_wallets`:两侧钱包各自扣/收,差值 = 2×资金费(不抵消)
- `test_finalize_settles_last_bar`:停在 8h 边界 bar 时末段资金费进报告,finalize 幂等
- `test_futures_hedge_side_independent_liquidation`(重写旧 `..._long_first_keep_short_then_liq`):跌 bar 只清 LONG(独立强平价 ≈92.31),SHORT 侧持仓 200 原样保留;涨过 SHORT 独立强平价(≈107.32)后该侧才被清算 —— 旧用例编码的是"按对共享钱包"的旧语义(第二侧被提前清算), 已按真实实测重写
- `test_futures_liquidation_distance_matches_real_calibration`(SC-005):50x + MMR 1.5% → 强平距离 ≈0.51%(纯保证金公式),未穿越不清算/穿越即清算

**真实数据冒烟(demo 无法重放清算, 用真实 fapi K 线 + CLI)**:
- 合约 one-way 20d(shannon_grid ETHUSDT):跑通,资金费净额 −294.51(多头净付,量级 = 20d×3 次×名义 ≈48k×0.01% ≈ 288 ✓);强平 0;one-way 行为与改造前一致(SC-004)
- 合约 hedge 20d(临时探针):**按侧明细显示 LONG 钱包 24481.54 / SHORT 钱包 24778.41** —— 两侧独立记账(差 = 2×资金费 ≈296.9),账户净额仍 ≈0;名义敞口按 Σ|size| = +101.6%(多空对冲正确)
- 现货 20d(shannon_grid ETHUSDT):正常出报告(净盈亏 65.53 / 最大回撤 3.17%),现货路径未改动

**未做(另立变更)**:实盘侧资金费/强平记账(fapi `/fapi/v1/income?incomeType=FUNDING_FEE` 与持仓 `liquidationPrice` 监控)。
