# 004 任务分解

> 顺序即依赖顺序; 每项完成须有可验证判据。
> 基线: 实施前 `cargo test --workspace` = **216 passed / 0 failed / 9 ignored**(2026-09-12, 008 提交后); 实施后 **233 passed / 0 failed / 9 ignored**。
> 规则属纯逻辑(不产生交易流程) → 单元测试; 真实链路验证走真实行情 Dry Run(禁 mock, 宪法原则三)。
> 状态: 2026-09-12 实施完成 (T001-T012 全部完成)。

## 阶段 1 — 引擎骨架(规则接口与构造)

- [x] **T001 规则接口与错误类型**
  内容: `RiskRule::check` 改 `&mut self`(状态规则需要) + 新增 `name()`; `RiskEngine::check` 同步 + 拒单统计(`rejection_count` / `rejection_stats` / `last_reason`); `RiskError` 增 `OrderRateLimited` / `ConsecutiveLossHalt` / `PeakDrawdownHalt`(Display 含关键数值)。
  判据: `cargo test -p locus_strategy` 既有 risk 用例改签名后全绿; 统计可断言。
  完成: 三处新变体均有单测断言(含消息文案); `test_engine_stops_at_first_reject_and_counts` 覆盖"首拒即停 + 计入统计"。

- [x] **T002 风控引擎构造与配置**
  内容: `RiskEngine::from_config(&StrategyConfig)` —— 优先级 `--param risk_*` **>** `[risk]` TOML **>** 默认; 装配既有 4 条静态护栏(配置了才启用) + 新的 2 条默认护栏; `RiskConfig` 增 6 个可选字段; 绕过校验时防御性收敛。
  判据: 单测覆盖装配条数、参数优先级、默认值、防御性收敛。
  完成: `risk.rs` 单测 `test_from_config_defaults_include_new_guards`(默认装配 2 条) / `test_from_config_static_rules_and_param_priority`(--param 覆盖 TOML) / `test_rate_limit_zero_clamped_to_one`。

## 阶段 2 — 新规则

- [x] **T003 下单频率上限(滑动窗口)**
  内容: `OrderRateLimit { max_per_sec, window: VecDeque<DateTime<Utc>> }`; 时间源 `ctx.now_utc()`(None → 真实 UTC); 窗口 1s, 推入后窗口内计数 > 上限即拒。
  判据: 单测 —— N=1 时第 2 次同秒拒; 窗口过期后恢复; 默认上限不误伤; 0 收敛为 1。
  完成: 4 个用例全绿(`test_rate_limit_rejects_past_limit_within_window` / `_window_expires` / `_default_is_generous` / `_zero_clamped_to_one`)。

- [x] **T004 两级亏损熔断**
  内容: `LossCircuitBreaker`(一级惰性周期结算 / 二级峰值回撤 / 平仓豁免 / 重启清零)。
  判据: 单测 —— 连续 N 周期亏损停开仓、平仓放行、转盈清零自动恢复、无平仓周期不改变计数、二级触发、峰值 ≤0 不触发二级、重启清零、小时粒度。
  完成: 7 个用例全绿。**测试实现要点(踩坑记录)**: 惰性结算的正确时序是"周期开始建基线 → 本周期成交 → 下一周期开始结算上一周期"; 首版测试把结算写在同一次调用里, 导致计数差 1 个周期(5 例失败), 已修正为 `trade_in_day` 辅助函数。

- [x] **T005 规则级回归与不误伤**
  内容: 默认参数下常规节奏不触发任何新规则。
  判据: 单测 —— 默认上限内多次下单 `Ok`; 未触发时统计为空。
  完成: `test_risk_defaults_do_not_misfire`(引擎级, 5 单全成交 + 拒单数为 0) + `test_rate_limit_default_is_generous`(规则级)。

## 阶段 3 — 接线(三条下单路径)

- [x] **T006 BacktestContext 接入**
  内容: 加 `risk: RefCell<RiskEngine>`(构造期 `from_config`), `place_order` 顶部 `risk_reject`; 被拒 → `Rejected` ack + `rejected_count += 1` + `tracing::warn!(target: "risk")`。
  判据: 单测 —— 极小上限下拒单被如实计数, 策略循环不中断; 默认参数与原行为逐位一致。
  完成: `test_risk_rate_limit_wired_into_backtest`(limit=1 → 第 2 单 Rejected、拒单计数 1、持仓不增长); 真实回测对照见 T010。

- [x] **T007 DryRunContext / LiveContext 接入 + now_utc**
  内容: 两 context 加 `risk` 字段与前置检查; 实现 `now_utc()`(真实 UTC)。
  判据: 共享同一 `RiskEngine::from_config`; 拒单返回 `Rejected` 而非 `Err`; `now_utc()` 返回 Some。
  完成: `context.rs` 两处 `risk_reject` + `now_utc` 实现; 真实 Dry Run 冒烟见 T011(715 次拒单且进程不中断)。

- [x] **T008 配置校验边界**
  内容: `StrategyConfig::validate_risk()`(频率 ≤0 / N ≤0 / loss_period 非法 / 回撤 ∉ (0,100] / 静态护栏非正 → `CoreError::InvalidArgument`, 消息含字段名与取值); 在 `backtest.rs::resolve_config`(TOML 与直跑两条分支)与 `run.rs` 装配处调用。
  判据: 单测 4 类非法值均被拒且消息明确; 合法值通过。
  完成: `test_settings_validate_rejects_illegal_values`(5 例) + 3 处 CLI 调用点。

## 阶段 4 — 验证

- [x] **T009 默认值实测定标**
  内容: 真实数据回测测量内置策略单 tick 最大下单数, 作为"默认 100/s 不误伤"的依据。
  判据: 实测数字(命令 + 结果)记录。
  完成: `shannon_grid` / `ladder`(num_levels=10) / 其余内置脚本在 1m 粒度下 `--param risk_max_orders_per_sec=1` 均 **零拒单** → 单 tick 下单数 ≤1;
  默认 **100/s** 的余量用于覆盖"网格类一次挂多档"(单 tick 数十单)与真实行情短时突发, 同时远低于下单风暴量级(≥10³/s)。已写入 `specs/backtest.md` §二.6 与 `risk.rs` 常量注释。

- [x] **T010 基线不回归**
  内容: `cargo test --workspace` 全绿; 真实数据回测与"护栏关闭"对照一致。
  判据: 报告对照 + 测试数统计。
  完成: 233 passed / 0 failed / 9 ignored(216 + 17 新增);
  真实回测 `locus backtest --strategy shannon_grid --pair ETHUSDT --days 20` 默认护栏 vs `--param risk_max_orders_per_sec=1000000 --param risk_level1_enabled=0 --param risk_level2_enabled=0`
  → 成交笔数 7 / 净盈亏 `52.847085278004757216285536250` / 拒单 0 / 最大回撤 3.21% **逐位一致**(零误伤)。

- [x] **T011 真实链路冒烟(SC-002)**
  内容: 极小频率上限 + 真实行情 Dry Run 下观察超限拒绝且进程不中断。
  判据: 真实跑通(禁 mock)。
  完成: `LOCUS_ROOT=/tmp/locus-004-smoke`(内含 `strategies/burst.toml`: 每 tick 下 2 笔 0.001 ETH 市价单 + `[risk] max_orders_per_sec = 1`)
  → `locus run burst` 真实公共行情 Dry Run 40s: **715 次 `风控拒单: 下单频率超限: N 单 / 1000ms > 上限 1 单/秒`**(target=risk), 进程持续运行至超时结束(策略循环不中断)。
  诚实边界: Dry Run 走公共行情 + 虚拟撮合, **不使用 demo 密钥、不签名**(护栏是下单前置拦截, 与签名/时钟无关), 因此本项不是 testnet 下单链路验证 —— 下单/撤单的真实链路验证属 011 实盘运行器。

- [x] **T012 文档同步**
  内容: `specs/backtest.md`(§二.6 风控前置检查: 语义/默认值/拒单口径/时间源/实测定标)、`specs/architecture.md`(§七 安全模型: 三路径接入 + "此前从未生效"修正记录)、`specs/roadmap.md`(004 状态 + 测试基线 233)。
  判据: 文档描述与代码一致(逐条对照)。
  完成: 三份文档已更新; 本档案 spec/plan/tasks 齐备。

## 收尾待办(不阻塞归档)

1. `StrategyScheduler`(唯一旧调用点, 无消费者)是否删除 —— 随 011 实盘运行器一并决定(见 plan P8)。
2. 熔断告警事件对接 003-notifications(现为 `tracing::warn!` 日志; 003 未上线)。
3. 频率上限默认值在 dogfood 期按真实成交节奏复核(当前依据 = 内置脚本实测 + NautilusTrader 参照)。
