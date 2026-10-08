# 041 — 合约香农策略: 卖出侧降杠杆 v2 (锚定建仓价值)

状态: 已实施(v2, 2026-09-29 用户口径定稿), 待回测对照/testnet 验收
范围: 仅 `strategies/futures/shannon_grid_futures.lua` + `.toml` + 测试。零引擎改动。

## v1 → v2 演进

- v1(λ=2 目标杠杆式): 有效杠杆 λ=仓位名义/投入 > 2 时消减超额 → **回撤中也持续消减、提前锁亏**, 用户否决("消减太快")。
- v2(锚定建仓价值, 用户口径): **V0 = entry_size × entry_price**(建仓定格, state 已持久化, 零新增状态)。

## v2 规则

挂卖单时, 若 **现价 ≥ 建仓价(非浮亏闸门)** 且 **卖单价下可实现仓位名义 Q·sell_px > V0 × delev_cap_mult**:

```
q_delev = (Q·sell_px − V0×mult) / (sell_px·(1−f))
最终卖量 = max(正常 1:1 量, q_delev)     -- 永不低于正常再平衡卖量
```

- 下跌/回撤(现价 < 建仓价): **完全不消减**, 按原 1:1 处理, 不提前锁亏。
- 买侧公式不动(买=回补、卖=降风险)。
- 参数 `delev_cap_mult`(f64, 默认 1 = 封顶在建仓价值; 0/缺失 = 默认 1 —— config_f64 缺失/CLI 浮点可能返回 0, 不能做禁用哨兵; **负数禁用**)。
- 效果: 非浮亏时仓位可实现名义封顶 V0×mult, 利润沉淀为现金, 有效杠杆随利润累积棘轮式下行。
- ⚠ 建仓后首格卖单即会消减一个间距的可实现超额(卖价=平衡价+spacing, 按可实现价计已 > V0); 消减卖成交后账本暂现金偏重, 下一格买单按 1:1 买回一部分, 仓位价值在 V0 下方震荡、不越顶 —— 属预期。
- 可观测: `stat_delev_count` / `stat_delev_extra_notional`(相对 1:1 量超额) + 每次消减 INFO 日志 + on_stop 汇总行。

## 测试(builtin_tests.rs, 12/12 过)

- `delev_sell_caps_to_initial_value`: 5x 首格卖量 = 降杠杆量, 一步压回 V0。
- `delev_drawdown_no_trim`: 深跌全程计数/超额定格在首格值, 不新增消减。
- `delev_disabled_keeps_one_to_one`: -1 禁用 → 纯 1:1。
- `delev_default_caps_at_2x_too`: 默认开启对 2x 同样生效。
- 原 1:1 回归用例(build/buy/sell restore)以 `delev_cap_mult=-1` 隔离降杠杆, 专测 1:1 公式。
- 账本恒等式 `v_cash = v_total − m_fee − buy_notional + sell_notional` 不受影响(model_apply 仍按真实成交推进)。

## 待办

- [ ] 回测开关对照(delev_cap_mult=1 vs -1, 同窗口同参): 回测规范 v1 全项, 看 stat_liq_dist_min 改善 vs 净盈亏代价。
- [ ] testnet demo 实跑验证挂单量分支与日志。
