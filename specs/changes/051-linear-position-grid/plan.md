# 051 现货线性仓位网格（linear_position_grid）实施计划

## 一、需求理解

现货内置策略, 三阶段生命周期:

1. **逐步建仓**(独立过程): 目标总量 `build_target = T(start_price)` 均分 `build_steps`(默认 10)批; 价格**低于 start_price** 且距上批 ≥ `build_interval_hours`(默认 1h)才市价买一批(高价到点顺延不烧步); 建仓期间不挂任何限价单; 建满 → `ref := 末笔成交价`, `peak := ref`, 进入网格。
2. **线性网格**: `w(p)` 在 `[p_low,p_high]` 线性(p_low→pos_low_pct 默认 0.7, p_high→pos_high_pct 默认 0.3, 区间外钳制), `T(p)=w(p)×C0/p`; ref±Δ 两侧限价单(Δ=max(atr_mult×ATR, min_spacing_pct×价格), 钳进区间), 买量=T(买价)−Q、卖量=Q−T(卖价); 任一成交 ref:=成交价全撤重挂(039 事件模型, 038 R1 链内限价次 bar 生效)。
3. **动态止盈**(可选): 网格阶段 peak 只升不降; `peak−price ≥ tp_dd_atr_mult×ATR`(默认 3) **且** `(equity−C0)/C0 ≥ tp_min_profit_pct`(默认 0.10) → 撤光+市价清仓+停机; `tp_min_profit_pct=0` 禁用。

出界处理 `out_of_range`: "exit"(默认)撤单停机不清仓 / "wait"暂停等回界内(ref 重锚现价)。盈利沉淀: T(p) 恒用固定 C0, 差价转现金不再投入。

## 二、关键设计决策

### 1. 账本模型(真实账本, 同 shannon_grid 口径)
- `m_cash/m_pos/m_fee` 按 fill 独立推进, on_stop 与引擎交叉核对(误差 <0.01 → WARN + `stat_ledger_diff_*`)。
- C0 = invest_cash(>0 启动定格; =0 首次建仓时余额定格并持久化 `C0` 键)。

### 2. 状态机
`phase = "build"|"grid"` + `halted`(exit 出界/止盈/参数/成本门槛) + `tp_closed`(止盈清仓, 终态) + `paused`(wait 出界)。
- 出界检查两阶段通用(建仓期无挂单, exit 即停/wait 即暂停)。
- halt/止盈跳变当 tick 显式返回 `{cancel_pending}`(引擎 pending_orders 与策略状态无关, on_fill 链内撤单次 bar 才生效 —— backtest.rs L1000-1003 教训)。
- 止盈批次 = `{cancel_pending, 市价卖 cap_sell(Q)}`; 清仓成交在 on_fill 的 halted 分支**只入账不产单**(账本逐分同步)。

### 3. 穿越守卫(do_rehang)
`buy_px ≥ 现价` 不挂买腿、`sell_px ≤ 现价` 不挂卖腿 —— 引擎对"开盘已穿越的限价单按开盘价成交"(try_match_ohlc), wait 恢复/ref 陈旧时防被动扫仓。

### 4. ATR 读取(性能铁律)
同周期走 `ctx:atr`(primary 1000 尾窗), 跨周期才走 `ctx:atr_tf`(TfCache) —— 同 `shannon_virtual_grid.lua` L380-382 口径。

### 5. 参数读取陷阱
`pos_high_pct`(0=顶部清仓合法)与 `tp_min_profit_pct`(0=禁用)直读 `ctx:config_f64`, 缺省由清单 default 注入; 其余用 `num/cfg_str`。

## 三、文件清单

| 文件 | 动作 |
|---|---|
| `strategies/spot/linear_position_grid.lua` | 新建(~560 行) |
| `strategies/spot/linear_position_grid.toml` | 新建(19 参数, 止盈提前下车警示入 description/unsuitable) |
| `crates/ricow/src/strategies/catalog.rs` | BUILTIN 追加(id/toml/lua 三元组) |
| `.gitignore` | 现货白名单追加两行 |
| `crates/ricow_strategy/src/builtin_tests.rs` | 新增 15 测试区块(参数 FATAL×3 / 成本门槛 / 建仓门槛×3 / 重挂价格数量 / 成交重挂 / exit / wait / 止盈×3 / 账本快照) |

## 四、验证

- 门禁: `cargo test -p ricow_strategy` / `--test architecture_guard` / `-p ricow --bin ricow` / `cargo fmt --check` / `clippy -D warnings`。
- 回测: SOLUSDT 1m 一年(exit/wait 两组) + 对照组, 四件套 + 规范 v1 报告。
- 实盘: 按宪法走币安 demo testnet 短周期(实施后单独执行)。
