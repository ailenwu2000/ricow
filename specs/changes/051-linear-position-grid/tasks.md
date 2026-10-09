# 051 任务清单

- [x] Task 1 `strategies/spot/linear_position_grid.lua`: 三阶段状态机(逐步建仓/线性网格/动态止盈) + do_rehang 事件模型 + 穿越守卫 + 出界 exit/wait + 守卫与 stat 导出
- [x] Task 2 `strategies/spot/linear_position_grid.toml`: 19 参数清单(default 全注入; 止盈提前下车警示写 description/unsuitable)
- [x] Task 3 catalog.rs BUILTIN 注册 + .gitignore 白名单
- [x] Task 4 builtin_tests.rs 15 条集成测试(建仓分批/顺延/门槛、重挂价格数量精确值、ref 步进、exit/wait、止盈触发/门槛/禁用、FATAL×4、账本核对+快照)
- [x] Task 5 门禁: architecture_guard / ricow_strategy 297 通过 / ricow bin 444 通过 / fmt / clippy -D warnings
- [x] Task 6 SOLUSDT 1m 一年回测(exit/wait 两组) + 规范 v1 报告核对 + converge
- [x] Task 7 币安 demo 测试网实单验证(建仓 3 批成交、网格挂出、停机撤单兜底、持仓保留)
