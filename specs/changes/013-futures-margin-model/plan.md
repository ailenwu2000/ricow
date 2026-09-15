# 013 实施计划: 合约逐仓记账校准

## 一、决策

- **D1 钱包维度按侧**: `wallet: HashMap<String, Decimal>` 的键改为"逐仓钱包键" —— one-way = `pair_key`;
  hedge = `{pair_key}|long` / `{pair_key}|short`。所有按方向的资金操作(开仓保证金、平仓盈亏、手续费、资金费)
  改为经 `wallet_key(pair_key, side)` 寻址 → one-way 自动退化为 symbol 级, 现货路径不受影响。理由: 真实账户
  one-way 就是 symbol 级钱包, hedge 才分侧; 用"按方向寻址 + 模式决定键"这一个机制同时满足两者, 不新增模型分支。
- **D2 强平判定按模式分派**: `check_liquidation(pair_key, side: Option<OrderSide>, bar)` ——
  `None`(one-way)= 现状组合判定; `Some(side)`(hedge)= 单侧判定 `E_side ≤ MMR×名义_side`, 只清算该侧。
- **D3 资金费按侧结算**: hedge 逐侧扣/收(不抵消); one-way 保持净额(现状)。
- **D4 结算时点(L1)**: 新增纯函数 `funding_settlements_in_bar(open_ms, close_ms) -> u32`(严格落在 `(open, close]`
  内的 8h 边界个数), bar 内按次数结算(1d = 3 次)。理由: 与 interval 无关的通用解, 不需要"限定支持的 interval"这类约束型拍板。
- **D5 收尾补结算(L2)**: 新增 `finalize()`, 在报告生成前对最后一根已收盘 bar 补一次 `settle_closed_bar`;
  由 `report()` 内部保证只补一次(幂等标志), 调用方无感。
- **D6 报告口径**: 账户级 `funding_net` / `liquidation_count` 含义不变(改为 Σ 各侧), hedge 下新增按侧明细
  (`liquidation_count_long/short`、收盘 `wallet_long/short`), 便于与交易所账单对照。
- **D7 现货零改动**: 所有改动包在 `is_futures()` 分支内; 现货回归用例必须数值不变(SC-004)。

## 二、改动清单(按依赖序)

1. `crates/locus_strategy/src/backtest.rs`:
   - `hedge()` 判定 + `wallet_key(pair_key, side)` + `wallet_of(pair_key, side)` / `add_wallet(pair_key, side, delta)`
   - 开仓保证金/平仓盈亏/手续费按订单方向寻址钱包(改所有调用点)
   - `settle_closed_bar`: 资金费按 `funding_settlements_in_bar` 次数结算 + 按侧钱包; hedge 按侧强平
   - `check_liquidation(pair_key, side: Option<OrderSide>, bar)`: one-way 现状逻辑, hedge 单侧
   - `finalize()`: 尾 bar 补结算; `report()` 前自动调用(幂等)
   - 报告字段: hedge 按侧明细
2. 单测(`backtest.rs` 内 tests):
   - 结算点计数(1m/1h/4h/1d)
   - hedge 单侧强平: 一侧归零另一侧不变
   - hedge 资金费按侧(不抵消)
   - one-way / 现货回归不变
   - DASHUSDT 真实校准点(50x + MMR 1.5% → 强平距离 ≈0.53%)
3. CLI 报告渲染(`crates/locus_cli/src/commands/backtest.rs`): hedge 按侧明细(有则打)

## 三、风险

- **R1 既有测试依赖"按对共享钱包"数值**: hedge 相关既有用例的期望值需要按真实语义更新(允许, 但需逐条说明为何新值正确)。
- **R2 one-way 行为漂移**: 钱包寻址改造若在某处漏改按侧, one-way 下两笔不同方向的订单会落到同一键(正确), 但漏改会让某笔落到错误键 → 用既有 one-way 用例兜底。
- **R3 强平计数语义变化**: hedge 一侧触发一次即计数一次(现状按对至多一次) → 报告数字会变大, 属修正(真实是每侧独立)。
