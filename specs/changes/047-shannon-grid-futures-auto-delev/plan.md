# 047 — 合约香农网格: 新策略(去对冲) + 可选自动降杠杆

状态: 已实施(2026-10-01 用户口径, 计划批准 `.trae/documents/047-shannon-grid-futures-auto-delev-plan.md`)
范围: 新策略 `strategies/futures/shannon_grid_futures.{lua,toml}` + catalog/.gitignore 注册 +
`builtin_tests.rs` 047 测试段。零引擎改动; 母本 `shannon_hedge_grid_futures` 零改动。

## 用户口径

1. 参考合约香农对冲网格, 新增**合约香农网格**: 去掉对冲腿(纯多头, 不反向开空)。
2. 新增参数「自动降杠杆」选项, **默认不选**; 选中后交易过程中自动降低杠杆:
   1) 仅杠杆 ≥ 2 时该选项才真正生效;
   2) 建仓后按建仓价记录**建仓价值**(只可缩小、不增大)。价格上涨: 按卖出价计算卖量,
      保持建仓价值不变(仓位名义压回建仓价值), 卖出后消减虚拟资金, 使虚拟资金与仓位
      价值保持 1:1。价格降低: 保持原来处理。仓位价值本就 ≤ 建仓价值: 按原来方式处理。

## 算法(= 历史 041 v3 锚定建仓价值的 mult=1 特例, 以默认关闭开关复活)

- 生效判定: `auto_delev=1` 且 `leverage≥2` 且已建仓 且 `现价 ≥ entry_price`(非浮亏闸门, 同 041)。
- 锚定: `V0 = entry_size × entry_price`(建仓成交定格, 复用既有持久化键, 不新增锚变量、不 ratchet)。
- 卖侧(do_rehang, Q>0 分支): 仓位名义 `Q·sell_px > V0` 时
  `q_delev = (Q·sell_px − V0) / (sell_px·(1−f))`, 最终卖量 = max(1:1 量, q_delev); 买侧不动。
- 收口(model_apply 卖分支): 卖后 `v_cash − v_pos×px > 4×fee_side×名义` 时削 v_cash 回精确 1:1,
  削减额累计 `delev_trimmed`(持久化)。阈值 4× 的依据: 引擎 maker 实收 2bps < fee_side 假设 5bps
  的合法漂移每笔 ≤3bps×名义, 卖前累计 ≈6bps×格量名义 < 20bps×卖名义 —— 正常卖单永不触发,
  降杠杆超额(≈间距×仓位 ≈ 1.5%×名义)远超阈值必触发。⚠ 排除 `LIQ-` 强平(同 041)。
- 重建核对恒等式扩展: `v_cash = v_total − m_fee − buy_notional + sell_notional − delev_trimmed`。
- 可观测: `stat_delev_count / stat_delev_extra_notional / stat_delev_cash_trimmed / stat_auto_delev`
  + 降杠杆卖出/消减收口 INFO 日志 + on_stop 汇总行。

## 与 041 的异同

- 同: 锚定 V0、非浮亏闸门、卖量取 max、收口削 v_cash、恒等式扩展、排除 LIQ-、统计三件套。
- 异: ① 041 默认开启(mult=1, 负数禁用), 本策略 `auto_delev` 默认关闭(0/缺失=关闭);
  ② 本策略加杠杆≥2 生效闸门(041 无); ③ 本策略 mult 固定 1(V0 即上限, 无 delev_cap_mult 参数);
  ④ 041 在合约香农策略上实施, 本策略在**去对冲**的独立新策略上(母本对冲网格不受影响)。

## 决策记录

- 策略 id 复用空闲旧名 `shannon_grid_futures`(045/046 改名让位), 名「合约香农网格」。
- `position_mode` 保持 hedge(引擎双向持仓模式, 只下 long 侧), `default_leverage=2` 保留。
- 新策略不声明 `[backtest].margin_mode`(无对冲腿, 回归引擎默认 isolated; CLI 可覆盖)。
- 历史回测产物(backtests/ 等)保留旧名不追改(既有口径)。

## 测试(builtin_tests.rs 047 段, 7/7 过; 全量 223+3 过)

- `no_short_orders`: 全程零空头订单(对冲移除)。
- `delev_off_matches_hedge_grid`: 关闭态买/卖价量逐值 = 母本 1:1(买 97@1.5202/卖 103@1.4809)。
- `delev_on_caps_sell_to_entry_value`: 2x 首挂卖 103 卖量 = q_delev 2.91408 > 1:1 1.48095。
- `delev_fill_trims_to_one_to_one`: 成交后 v_cash==v_pos×103(差<1e-9)、仓位名义≈V0、trimmed>200。
- `delev_inactive_below_2x`: 1x+开关 → 纯 1:1、零削减。
- `delev_drawdown_no_trim`: 回撤(现价<建仓价)全程 trimmed=0。
- `delev_state_snapshot_and_recon`: delev_trimmed 持久化 + 恒等式核对 <0.01 + stat 导出。

## 验收

- 冒烟回测 SOLUSDT 30d on/off 对照(见本目录 smoke/ 或 /tmp/sol_delev/): 开组
  `stat_delev_count>0`、`stat_v_recon_diff<0.01`; 关组与母本对冲网格(hedge 关闭)逐笔一致。
