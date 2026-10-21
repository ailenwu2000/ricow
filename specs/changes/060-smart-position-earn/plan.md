# 060 实施计划(摘要)

> 权威蓝本 = `.trae/documents/spot_smart_position_earn_plan.md`(含 2026-10-20 架构审核修正)。本文只留执行索引。

核心决策: D5 理财申赎走 `_RICOW_EARN_OPS` 队列(回调后 drain, 天然"下周期再看") · D9 **仅存 quote 现金腿**(base 不入理财, 保 size↔lots 不变量) · D4 权益 = 现货现金 + 持仓市值 + 理财市值, 回测四处聚合点逐一加腿(mark_to_market L1153 / mark_to_market_portfolio L751 / balance L2313 不混入 / lua fill_snapshot L600-614) · D7 demo 网 sapi 不支持 → LiveContext 吞错+WARN 降级 · D8 防循环调仓 = cycle_days 时间闸 + 2pp 阈值带 + min_notional。

## 执行顺序

1. `ricow_core/exchange.rs`: Exchange trait + earn_balance/subscribe/redeem 默认方法(默认不支持)
2. `ricow_binance`: 新 `earn.rs`(SAPI 四方法, 复用 signed_get/post) + client.rs signed_* 改 pub(crate) + lib.rs 挂 mod + spot.rs 覆写三方法
3. `ricow_strategy/context.rs`: Context trait 三方法(Option/bool 静默口径) + LiveContext 桥接(60s TTL + Err 收敛) + DryRunContext 虚拟钱包
4. `ricow_strategy/backtest.rs`: virtual_earn 划转(不碰 positions/lots) + 四处聚合点加理财腿
5. `ricow_strategy/lua.rs`: LuaCtxData.earn 快照 + ctx:earn_* 绑定 + drain_earn_ops(on_tick/on_fill/on_stop 同时机)
6. `strategies/spot/smart_position.{lua,toml}` + catalog.rs/builtin_tests.rs/.gitignore 注册
7. 测试: 单测(解析/划转守恒四锚点/队列/策略四件套) + 架构守卫 + clippy/fmt
8. 验证: SOLUSDT 1 年 1h 真实回测(backtest.md §〇 v1 全量) + demo 测试网 Dry Run
9. 文档: lua-api.md 理财小节 + 内置清单, roadmap, converge.md
