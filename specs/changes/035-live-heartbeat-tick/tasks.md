# 035 任务清单: 实盘行情静默期兜底心跳 tick

- [x] **T001** `command.rs`: 新增常量 `QUOTE_HEARTBEAT_SECS` (30) 与 `LiveEvent::Heartbeat` 变体; 实盘主循环 select! 加心跳分支 (与 funding_tick 同构)。
- [x] **T002** `command.rs`: 抽取 Quote 分支的"on_tick 产单 → 下单 → 计数/落库/拒单回传 → 即成交刷仓"为共用辅助 `live_decision_tick`; Quote 与 Heartbeat 共用 (FR-002)。
- [x] **T003** 心跳处理: 空仓保真 —— orderbook 缺失跳过、不 tick_kline、不计 ticks (FR-003/FR-004)。
- [x] **T004** 引擎测试 `test_live_decision_tick_calls_on_tick_and_submits`: 共用出口调 on_tick + 订单真实提交 (RecordingExchange 记录) + 不制造行情事实 (SC-002 部分落地; 说明: 完整 30s 主循环静默时序不做单测 —— tokio interval 常量固定 + 宪法禁 mock 实盘流, 由心跳分支守卫为纯内联检查覆盖)。
- [x] **T005** 文档: architecture.md §四新增"实盘静默期兜底心跳"小节 + roadmap.md 基线 579/0/22 + 034 plan §五 #10 标注已落实 (SC-003)。
- [x] **T006** 门禁: cargo fmt / clippy / test 全绿 **579 passed / 0 failed / 22 ignored** (SC-001); 附带修复 034 遗留的 backtest_runner.rs 未使用导入 (OrderSide 移至 tests mod)。
