# 040: 合约香农网格 (shannon_grid_futures)

状态: 已实现（2026-09-28），等待用户提交。

## Why
现货香农网格(shannon_grid)已有稳定实现; 用户需要合约版: 杠杆 1~5 倍(默认 2), 总资金 = 投入 × 杠杆,
虚拟账本(虚拟现金 + 虚拟仓位)做网格计算, 爆仓价实时监控。引擎已具备全部基础能力(逐仓保证金/pos_liq/
LiqWarn/清单 position_mode+default_leverage), 无需改引擎。

## What
- 新增 `strategies/futures/shannon_grid_futures.lua` + `.toml`, 注册 `catalog.rs` BUILTIN。
- 逐行承袭现货 shannon_grid(do_rehang / 038 defer_new_limits 事件模型 / 4×fee 成本门槛 / 停摆可观测)。
- 合约化 6 点(头注释显式声明):
  1. leverage 1~5 校验(越界 FATAL); 杠杆唯一权威 = 清单 default_leverage; v_total = 投入 × 杠杆,
     激活用 v_total/2 名义开多(cap_open 预检真实可用)。
  2. 虚拟账本 v_cash/v_pos: v_pos 恒等于引擎实际多头仓位; v_cash 隔离保证金波动, 挂单量按
     现货同款 1:1 恢复公式以虚拟账本计算。
  3. 全部订单 position_side="long"; 买=开多/卖=平多, 绝不反向开空; 平多 cap_close 无折扣
     (严禁 1e-9 削裁, 032 根因 A)。
  4. 爆仓价监控双通道: 策略侧 ctx:pos_liq + liq_warn_ratio WARN(去重); 实盘 = 引擎 LiqWarn。
     on_stop 导出 stat_liq_price_final + 开仓口径估算免责。
  5. 虚拟买单超真实可用: 不砍量, WARN + stat_underfunded_buys/notional。
  6. on_stop 合约口径对账: v_pos vs 引擎持仓 <0.01 WARN; 虚拟账本重建核对
     (v_cash = v_total − Σ费 − Σ买名义 + Σ卖名义, 差 >0.01 WARN, stat_v_recon_diff);
     v_cash 与引擎现金合法分叉只留档(保证金释放口径 + 资金费)。
- CLOSE- 强平 fill 按普通卖出推进虚拟账本, 盈亏单列 stat_close_pnl(均值成本口径)。
- 参数: atr_mult 默认 1.5 / min_spacing_pct 0.002(=4×合约费率) / min_notional 5(fapi 最小名义;
  初版 50 实测导致期末小名义双侧挂不出停摆 9421 bar, 已改 5) / fee_side 0.0005 / liq_warn_ratio 0.1。

## 验证
- 单测 7 项(builtin_tests.rs): 杠杆越界停机 / 激活建仓(名义=v_total/2, position_side=long,
  无 short 侧, v 账本初值) / 建仓重挂(±1.5×ATR, 1:1 量) / 买成交 1:1 / 卖成交 1:1 /
  underfunded 记录 / 状态快照。workspace 581 项全绿。
- 回测(SOLUSDT 1m 60d, 2x, --close-at-end, backtests/20260928-shannon-fut-60d/sol):
  净盈亏 +5346.14 (+53.46%), 291 笔成交, 零停摆, 零拒单, 零强平;
  v_cash − v_total = 5346.14 与引擎净盈亏逐分一致; 四件套齐备。
- 纪律: testnet 真实开平多 + 爆仓价展示闭环待用户指令(demo-fapi, 禁 mock)。

## 已知口径说明
- 1:1 公式假设费 = fee_side×名义; 引擎限价单收 maker 2bps < 5bps → v_cash 小额合法富余
  (方向恒正, 单测容差 0.1)。
- 期末 --close-at-end 后 v_pos=0/v_cash=全部落袋: 正常(CLOSE fill 按卖出推进虚拟账本)。
