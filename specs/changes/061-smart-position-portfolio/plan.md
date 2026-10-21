# 061 实施计划(摘要)

> 权威蓝本 = `.trae/documents/smart_position_portfolio_plan.md`(已批准, 含 D1-D4 全部设计决策)。本文只留执行索引。

核心决策: D1 策略组合化为唯一模式(universe + weights, 缺省等权; target_pct = 币腿合计占比; 阈值闸逐币独立; 资金池共享、先卖后买滚动预算) · D2 理财腿只存 USDT, `earn_supported_assets` 配置键(默认 USDT)让"bStock 无赚息"在回测中真实演练, 不支持→None→策略静默关闭理财、零报错 · D3 CLI `ricow backtest` 接 `run_portfolio_backtest`(universe≥2 → 组合路径; 单标的路径与 Web 路径零改动) · D4 TOML/架构守卫同步。

## 执行顺序

1. `strategies/spot/smart_position.lua` 组合化重写 + `.toml` 增 universe/weights
2. `ricow_strategy/backtest.rs`: `earn_supported_assets` 解析(大写集合) + earn 三方法按支持面返回 None/false
3. `ricow_engine/backtest_runner.rs`: `run_portfolio_backtest` 收尾补 `on_stop` + `strategy_stats` 透传(对齐 run_backtest)
4. `ricow/commands/backtest.rs`: `run_backtest` 分流 → `run_portfolio_backtest_cli`(逐 pair 取数/缓存/缺口/预热硬错 → `build_interval_ticks` → 组合报告文本 + 导出四件套; 敏感性组合不支持)
5. 测试: builtin_tests 组合六件套 + earn 白名单单测 + engine e2e(真实 Lua) + 架构守卫白名单(universe/earn_supported_assets)
6. 验证: 4 资产 132 天真实回测(backtest.md §〇 v1 全量)
7. 文档: backtest.md 4c + roadmap + 060 档案指向 061 + converge.md
