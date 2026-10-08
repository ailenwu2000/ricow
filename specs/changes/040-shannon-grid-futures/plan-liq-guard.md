# 040-L: 合约香农网格 爆仓风险记录与停机 (2026-09-29)

状态: 已实现 + demo 实跑验证通过 (2026-09-29)。

## demo 实跑发现与修复(计划外, 有问题即修)
1. **实盘杠杆脱节(已修, run.rs)**: 交易所预配置杠杆只读 `params.leverage`(实例 TOML 无此键 → 1x),
   而清单 `default_leverage` 只回填 `[backtest].leverage` → 交易所侧 1x 与策略 2x 口径脱节。
   修复: build_exchange 杠杆来源 = params.leverage > [backtest].leverage > 1x。demo 复跑确认 2x 生效。
2. **小额资金静默停摆(口径确认)**: invest_cash=100 时 1:1 恢复量 ≈0.7 USDT < min_notional 5,
   两侧都挂不出且 1 天内无日志(停摆 WARN 按自然日节流)。demo 改 invest_cash=2000 正常。
   属既有口径, 未改代码。
3. **demo 验证结果**: 建仓 17.07@117.16(2x) / 网格重挂双侧挂单(买 115.48 / 卖 118.84)在案 /
   距强平 49.40% 展示 / 虚拟账本初值正确 / 续接(v_cash 恢复)正常 / 零错误。

## Why
杠杆高时运行有爆仓风险。用户要求:
1) 运行时记录"距爆仓最近"的时刻(最小距离 + 当时价格/时间);
2) 真发生爆仓时如何记录、并停止交易;
3) demo(币安 testnet fapi)实际运行验证。

## 调研结论(引擎既有能力, 无需改引擎)
- 引擎爆仓 fill: `client_order_id = "LIQ-{pair}-{side}"`(hedge) / `"LIQ-{pair}"`(one-way),
  `exchange_order_id` 带 `LIQ-` 前缀, `fill_price` = 触发价, side=卖出平仓;
  经 `drain_fills()` 正常派发到策略 `on_fill`(backtest.rs:861-921 / backtest_runner.rs:79-82)。
- `ctx:pos_liq(pair,"long")` 是**开仓口径估算**(仅显示/告警); 真实强平按钱包权益穿越维持保证金
  阈值触发(backtest.rs:191-196) —— 二者口径不同, 记录与告警用估算价, 判定爆仓只认 `LIQ-` fill。
- `--close-at-end` 期末强平走 `CLOSE-` 前缀, 与 `LIQ-` 不冲突。

## What (只改 strategies/futures/shannon_grid_futures.lua + toml 描述 + 单测)
1. **最近爆仓距离追踪**: on_tick 每决策 bar, 有仓且 `ctx:pos_liq` 有效时计算
   `dist = (price − liq) / price`; 维护 `liq_dist_min / liq_dist_min_ts / liq_dist_min_price`
   (只收紧不回退); on_stop 导出 `stat_liq_dist_min`(百分比) / `stat_liq_dist_min_ts`(UTC) /
   `stat_liq_dist_min_price`。现价距爆仓告警沿用现有 liq_warn_ratio WARN。
2. **爆仓判定与停机**: on_fill 识别 `client_order_id` 以 `"LIQ-"` 开头 →
   - `liq_count + 1`, 爆仓盈亏单列 `stat_liq_pnl`(成交额口径);
   - 虚拟账本按该 fill 照常推进(v_pos 归 0, v_cash += 释放保证金口径按真实 fill 记账);
   - `halted = true, fatal = 1` 停机(全撤在途挂单、save_state 持久化 halted, 重启不复活);
   - 日志 [FATAL] 明示爆仓时间/价格/爆仓价/损失, 停机摘要单列爆仓事件。
3. **文档**: toml description 与 lua 头注释补"爆仓即停机 + 最近距离可观测"两点。
4. **单测**(builtin_tests.rs, 复用 040 测试时序):
   - 构造宽幅下跌 bar 序列使钱包穿越维持保证金 → 断言收到 LIQ- fill 后 halted=1、
     不再挂单、`stat_liq_dist_min` > 0 且 < liq_warn_ratio、重启续接不复活。
5. **demo 实跑验证**(交易纪律: testnet 真实调用, 禁 mock):
   - `ricow run shannon_grid_futures --demo`(币安 demo fapi), 小额 invest_cash,
     参数同回测口径; 观察建仓/挂单/爆仓价告警展示/停机闭环; 有问题即修。

## 验证
- workspace 单测全绿; 回测冒烟 1 组(SOLUSDT 2x 60d)结果与 040-L 修改前一致(纯增量, 不改网格语义)。
- demo 实跑日志与成交记录留存。

## 不做
- 不引入自动减仓/加保证金动作(用户口径: 爆仓只记录+停机, 不动作)。
- 不改引擎强平模型与 `LIQ-` fill 语义。
