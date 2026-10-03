# 049 合约做空香农网格（shannon_short_grid_futures）实施计划

## 一、需求理解

用**现货虚拟香农网格（shannon_virtual_grid）的事件模型**管理一个**纯做空**的 USDT-M 合约网格：

- 杠杆固定 1 倍；只下 SHORT 侧订单（卖出开空 / 买入平空），绝不碰 LONG 侧。
- **一开始就建仓**：回测启动后第一个决策 bar 立即按现价市价卖出做空一半资金（不等任何信号）；成交价 = 第一平衡价，同时建立虚拟仓位：按开始价格"一半做多 + 一半资金"的 1:1 虚拟账本，以后所有交易量都按虚拟仓位计算。
- 信号与间距全部用 1 小时 K 线：ATR(14) 作最小间距基准，EMA2/EMA3 金叉/死叉作交易闸门 —— **闸门只管建仓后的再平衡成交，不管建仓**。
- 金叉且价格偏离平衡价 ≥ 一个间距 → 按虚拟账本算买入量 → 实际执行**买入平空**；成交后平衡价 := 成交价，虚拟账本按目标量更新。
- 镜像语义：死叉且偏离 ≥ 一个间距 → 按虚拟账本算卖出量 → 实际执行**卖出开空**。

即：现货网格的"买"映射为"平空"，现货网格的"卖"映射为"开空"。

## 二、关键设计决策

### 1. 账本模型（双账本，照搬现货虚拟网格）

- 虚拟账本 `v_cash / v_pos`：决策引擎，恒 1:1（`v_cash = v_pos × 平衡价`），`v_total = invest_cash × 1`（杠杆固定 1）。
- 真实账本 `m_cash / m_pos`：按实际成交推进，`m_pos` = 实际空头数量（正数表示空仓量），on_stop 与引擎 `pos_size(pair,"short")` / 现金交叉核对（误差 < 0.01）。
- 建仓成交时：`balance_price = 成交价`，`v_pos = v_total/(2×成交价)`，`v_cash = v_total/2`；真实侧卖出开空 `v_total/2` 名义。

### 2. 交易量公式（与现货虚拟网格逐字同构）

设现价 P、单边费率 f：

- **金叉**（平空侧）且 `balance_price − P ≥ buy_spacing`：
  `q_v = (v_cash − v_pos×P) / (P×(2+f))`，虚拟账本按 q_v 推进（v_cash −= q_v·P·(1+f)，v_pos += q_v，flag −1）。
  实际平空量 cap 到引擎空头持仓（不自砍、不反向开多）；若 `q_v > 实际空头持仓` → **重置**（见 4）。
- **死叉**（开空侧）且 `P − balance_price ≥ sell_spacing`：
  `q_v = (v_pos×P − v_cash) / (P×(2−f))`，虚拟账本按 q_v 推进（v_cash += q_v·P·(1−f)，v_pos −= q_v，flag +1）。
  实际开空量受**可用保证金**截断：1x 下可开名义 ≤ 可用现金（预扣费+滑点余量，同 `cap_open` 口径），截断计 `margin_limited_sells`，虚拟仍按目标推进。

### 3. 信号与间距

- 主时钟默认 `1h`；EMA(2)/EMA(3) 与 ATR(14) 均默认 `1h` 周期。
- 与主时钟同周期时走 `ctx:ema` / `ctx:atr`（primary 尾窗，避免 042 记录的 TfCache O(n²) 性能坑）；跨周期才用 `ctx:ema_cross` / `ctx:atr_tf`。
- 生效间距 = `max(atr_mult × ATR, min_spacing_pct × 价格)`；`atr_mult` 默认 1（用户口径"ATR 作为最小间距"），`min_spacing_pct` 默认 0.002（合约 4×费率下限，防磨损）。
- 保留 `flag_spacing_mult`（默认 1.2）趋势侧间距指数放大，符号口径与母本一致：平空（买）−1 / 开空（卖）+1，建仓与强平不计 flag。
- 成本门槛启动校验一次：生效间距 < 4×fee_side×价格 → FATAL 停机。

### 4. 重置（镜像现货"死叉卖量超持仓→重置"）

做空版中"仓位受限侧"是平空：**金叉平空量 q_v > 实际空头持仓** → 不部分平空，按 `v_total` 在当前价重建 1:1 虚拟仓位（`v_pos = v_total/(2P)`，`v_cash = v_total/2`，平衡价 := 当前价），本次不发单、不计 flag，`reset_count++`。

### 5. 合约侧要点

- hedge 模式，所有订单带 `position_side = "short"`；on_fill 防御性忽略非 short 侧成交。
- 引擎强平 fill（`LIQ-` 前缀，`LIQ-{pair}-short`）→ 记 `liq_count/liq_pnl` → halted 停机不再交易（同 shannon_grid_futures 口径；halt 后仍记账）。
- 爆仓监控：每决策 bar 读 `pos_liq(pair,"short")`，距离 < `liq_warn_ratio` WARN；追踪 `stat_liq_dist_min/ts/price`。
- `margin_mode`：默认 isolated（1x 半仓做空，价格翻倍才爆，逐仓语义直观且与"投入即保证金"一致）；`[backtest] margin_mode = "isolated"` 显式声明，可 `--param margin_mode=cross` 切换。
- CLOSE- 期末强平 fill 按普通平空推进账本、盈亏单列 `stat_close_pnl`、不计 flag。
- 停摆可观测：连续 1 天无成交 WARN + `stat_stall_bars` / `stat_cash_final`。

### 6. 不做的事（YAGNI）

- 不加 enable_build/build_price 开关——做空版建仓即"开始时卖出做空一半资金"，无等待触发价语义。
- 不加 virtual_mult——杠杆固定 1，`v_total = invest_cash`。
- 不挂任何限价单——纯市价事件驱动再平衡（同现货虚拟网格）。

## 三、交付物清单

| 文件 | 动作 |
|---|---|
| `strategies/futures/shannon_short_grid_futures.lua` | 新建：母本 = shannon_virtual_grid.lua 做空镜像（双账本、市价、EMA 闸门、flag 间距放大、重置、LIQ 停机、爆仓距离追踪） |
| `strategies/futures/shannon_short_grid_futures.toml` | 新建：manifest（market=futures, position_mode=hedge, default_leverage=1, [backtest] margin_mode=isolated）；注释纪律 = 只留策略语义，无变更编号/日期叙事 |
| `crates/ricow/src/strategies/catalog.rs` | BUILTIN 表追加一条 |
| `crates/ricow_strategy/src/builtin_tests.rs` | 新增单测组（见下） |
| `specs/changes/049-shannon-short-grid-futures/plan.md` | 本文件 |

参数 schema（[[params]]）：`pair`(必填) / `invest_cash`(默认 0=余额全额) / `interval`(1h) / `ema_interval`(1h) / `ema_fast`(2) / `ema_slow`(3) / `atr_interval`(1h) / `atr_period`(14) / `atr_mult`(1) / `min_spacing_pct`(0.002) / `flag_spacing_mult`(1.2) / `min_notional`(5) / `fee_side`(0.0005) / `liq_warn_ratio`(0.1)。

## 四、测试计划

单元测试（builtin_tests.rs，纯逻辑）：

1. 启动只产 SHORT 侧订单、无 LONG 侧成交路径（防御断言）。
2. 首 bar 立即建仓（不等信号）：市价卖出开空 ≈ 投入一半，成交价 = 第一平衡价，虚拟账本 1:1 建立，建仓不计 flag。
3. 金叉平空：偏离 ≥ 间距才发单；量 = 1:1 恢复公式；成交后平衡价 := 成交价、账本推进、flag −1。
4. 死叉开空：量公式正确；开空名义超可用现金 → 截断 + margin_limited 计数，虚拟按目标推进。
5. 平空量超实际空头持仓 → 重置（不发单、reset_count++、1:1 重建）。
6. flag 间距放大对称性（flag<0 放大平空间距 / flag>0 放大开空间距 / mult=1 禁用）。
7. LIQ- 强平 fill → 停机 + liq_count/liq_pnl + 账本以引擎为真值。
8. 状态快照续接（balance_price/v_cash/v_pos/flag/reset_count）。
9. 成本门槛 FATAL、ATR 未就绪跳过。
10. 账本恒等式核对：`v_cash = v_total − Σfee − Σ买名义 + Σ卖名义` 误差 < 0.01。

回归验证（按回测规范 v1）：

- SOL/USDT 合约 demo/testnet 数据一年回测（1h 主时钟 + 1m 撮合两档），`--export-dir` 四件套，报告含 A1-A7 概览 + B 区成交统计，策略账本与引擎交叉核对 < 0.01。

## 五、风险与待确认点

1. **1x isolated 开空保证金**：引擎 isolated 下开空按名义/杠杆静态划转钱包保证金，可用现金截断口径 = `balance(quote)`；若引擎划转后余额语义与预期不符，测试阶段以实测为准调整 cap 公式。
2. 平空 cap 到实际持仓后若产生残差（目标 q_v > 持仓但差距小），走重置分支（与现货"不部分卖出"同口径），不引入部分成交特判。
