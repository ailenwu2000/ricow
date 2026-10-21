# 061 任务清单

- [x] T1 spec.md 落盘
- [x] T2 smart_position.lua 组合化重写(universe/weights/逐币阈值/滚动预算/理财静默降级) + .toml 参数面
- [x] T3 BacktestContext `earn_supported_assets` 配置键(默认 USDT; 不支持→None/false)
- [x] T4 run_portfolio_backtest 收尾补 on_stop + strategy_stats 透传
- [x] T5 CLI 组合路径接线(逐 pair 取数/统一时间轴/组合报告/导出四件套/敏感性硬拒)
- [x] T6 测试: 组合六件套 + 白名单单测 + engine e2e + 架构守卫; cargo test/clippy/fmt 全绿
- [x] T7 修复实施中发现的 Lua 缺陷: 卖单同周期申购理财会被拒(划转 drain 先于成交) → 卖出腿发出后本周期不申购, 下周期自愈; 账本漂移 244.76 归零, 断言收紧回 <0.01
- [x] T8 4 资产(BTC/ETH/TSLAB/NVDAB)132 天真实回测(§〇 v1 全量汇报)
- [x] T9 文档收敛: backtest.md 4c / roadmap / 060 档案指针 / converge.md
