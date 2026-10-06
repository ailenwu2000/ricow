# 任务分解: 工程健壮性加固(038)

**功能目录**: `specs/changes/038-p1-robustness-hardening`

**状态**: 已收敛 (2026-10-06)

**输入**: [spec.md](spec.md) / [plan.md](plan.md)

## 批次 A — P1-A 回测限价成交模型去乐观

- [x] **T001** `config.rs`: `BacktestToml` / `BacktestParams` 加 `limit_fill_penetration_bps`(默认 0.0), 接进 `resolve` / `apply_overrides`
- [x] **T002** `backtest.rs`: `BacktestContext` 加字段 + `new()` 读取回写; `try_match_ohlc` 限价分支加穿透判定
- [x] **T003** `backtest.rs`: 限价/市价成交笔数分列统计, 进 `BacktestReport`
- [x] **T004** `ricow/src/commands/backtest.rs`: CLI `--limit-fill-penetration-bps` + 报告单列限价成交与假设
- [x] **T005** 单测: 默认 0 时行为与旧实现逐位一致; 穿透 > 0 时不足穿透不成交; 买/卖两侧对称; 报告计数正确

## 批次 B — P1-D 数据缺口检测

- [x] **T006** 绑定层新增纯函数 `find_gaps` / `gap_error_message`(+ 单测: 连续无缺口 / 单缺口 / 多缺口 / 空与单元素 / 乱序容忍)
- [x] **T007** CLI 回测取数后接入(有缺口硬报错, 信息含明细)
- [x] **T008** Web 回测取数路径同样接入

## 批次 C — P1-F 下单延迟度量

- [x] **T009** 新 `ricow_engine/src/latency.rs`: `LatencyStats` / `summarize`(+ 单测: 空 / 单样本 / 分位取整 / 全同值)
- [x] **T010** `command.rs`: 三处下单点计时累积; 收尾写入 `RunOutcome`
- [x] **T011** `commands/run.rs`: 收尾打印(无样本不打印)

## 批次 D — P1-B Prometheus `/metrics`

- [x] **T012** 新 `web/metrics.rs`: 暴露格式渲染 + 标签转义(+ 单测)
- [x] **T013** 挂 `GET /metrics` 进既有路由(走同一 token 门); 单测: 无 token 401 空体 / 有 token 200 且含关键指标名

## 批次 E — P1-C 崩溃自动重启

- [x] **T014** `config_file.rs`: `[supervisor]` 段(解析 + 白名单校验 + 模板 + `SECTION_LIST`)
- [x] **T015** `supervisor/server.rs`: `State` 加策略与重试计数; `monitor_loop` 收 `Server`; 纯函数 `restart_decision` + 单测
- [x] **T016** 接线: 子进程非主动退出 → 判定 → 重启 / 放弃(台账如实注明); 重试计数重置策略

## 批次 F — P1-E 实盘/回测对齐工具

- [x] **T017** run card 结构补 `Deserialize` + `read_run_cards`
- [x] **T018** 新 `commands/align.rs`: `LiveFacts` 汇总(纯函数)+ 对照表渲染(纯函数)+ 命令入口 + `main.rs` 注册
- [x] **T019** 单测: 事实汇总 / 样本不足如实说 / 期货不当盈亏 / 渲染含口径说明

## 批次 G — 收敛与门禁

- [x] **T020** `cargo test --workspace --no-fail-fast` 全绿(基线 761 不减)
- [x] **T021** `fmt --check` / `clippy -D warnings` / `cargo deny --locked check` / `ci_grep_gates.sh` 五红线 / `architecture_guard` 三条
- [x] **T022** 端到端模拟: 临时 root 跑 `ricow align` / `GET /metrics`
- [x] **T023** 文档同步: `specs/architecture.md` / `specs/roadmap.md` / `specs/research/framework-vs-commercial-2026-10.md`(P1 状态) / `README` / AI 提示词
- [x] **T024** 写 `converge.md`

## 未纳入(待拍板)

| 项 | 状态 |
|:--|:--|
| **P0-D 单笔名义上限 / 异常速率熔断** | 仍待用户拍板(需可配置风险参数面, 与宪法 D15/D17 冲突) — 见 `specs/roadmap.md` "下一步" |
| P1-A 的 tick 级撮合 / 成交概率模型 | 与"不做 tick 级回测"定位冲突, 不做 |
| P1-C per-instance 重启策略 | YAGNI, daemon 级足够 |
