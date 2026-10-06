# 任务分解: 037 实盘资金安全加固

**功能目录**: `specs/changes/037-live-safety-hardening`

**创建日期**: 2026-10-06

**状态**: 已收敛 (2026-10-06; 见 [converge.md](converge.md))

## 批次 A: P0-A 启动接管遗留挂单

- [x] **T001** (FR-1) `ricow_strategy/src/events.rs`: 加 `KIND_ORPHAN_CANCELED`
- [x] **T002** (FR-1) `ricow_engine/src/live.rs`: `split_owned(orders, prefix) -> (owned, foreign)`
- [x] **T003** (FR-1) `ricow_engine/src/live.rs`: `plan_cleanup` 内部改用 `split_owned`(零行为变化)
- [x] **T004** (FR-1.3) `ricow_engine/src/live.rs`: `must_refuse_start(is_live, residual_owned, enumeration_ok) -> bool`
- [x] **T005** (FR-1) `live.rs` 单测: 归属切分(含空前缀/空列表) / 拒绝启动判定(live vs demo / 枚举失败) / 两条拒绝文案
- [x] **T006** (FR-1) `command.rs`: 启动接管接线(替换"只 warn"块): 撤 → 复查 → 残留 → live 拒绝启动
- [x] **T007** (FR-1.4) `command.rs`: `RunOutcome.orphan` + 事件流逐笔 `orphan_canceled` + warn 日志; `commands/run.rs` 收尾打印

## 批次 B: P0-B 引擎级最小订单登记

- [x] **T008** (FR-2.1) 新建 `ricow_engine/src/oms.rs`: `OrderState` / `OrderEntry` / `UnknownSubmission` / `RecordVerdict` / `OmsCounts` / `OrderRegistry`
- [x] **T009** (FR-2.1) `oms.rs`: 状态转移 `record_ack` / `apply_user_order` / `apply_fill`(增量) / `mark_canceled` / `mark_all_canceled`
- [x] **T010** (FR-2.2) `oms.rs`: `record_unknown` + `unknowns()` / `unresolved_ids()`
- [x] **T011** (FR-2.3) `oms.rs`: 非终态重复单号检测(`RecordVerdict::DuplicateLive`; 终态后复用为 `DuplicateTerminal` 不告警)
- [x] **T012** (FR-2) `oms.rs` 单测: 状态机 / 重复 / Unknown / 部分成交累计 / 终态锁定 / 插入顺序 / 空登记表
- [x] **T013** (FR-2.4) `command.rs`: 主循环接线(下单 ack / 传输失败 / 用户流回报) + 停机清理 + `RunOutcome.oms`
- [x] **T014** (FR-2.2) `command.rs` + `commands/run.rs`: 停机时 Unknown 非空 → 醒目提示(逐条 error 日志 + CLI 警告)

## 批次 C: P0-C 组合敞口只读视图

- [x] **T015** (FR-3) `ricow_engine/src/exposure.rs`: 只读聚合(本地库 `positions`/`orders` → 各标的净头寸 + 挂单数 + 总名义估算); 聚合键 = `(标的, 模式)`, 绝不跨模式相加; `is_open_status` 作未终结判据唯一来源
- [x] **T016** (FR-3) `commands/exposure.rs` + `main.rs`: `ricow exposure [--pair] [--mode] [--detail] [--limit]` 接线 + 单测; 无写路径、无新确认面

## 批次 D: 收敛与门禁

- [x] **T017** `cargo test --workspace --no-fail-fast` 全绿: **761 passed / 0 failed / 22 ignored**(起点 736, +25, 基线不减)
- [x] **T018** `cargo fmt --all -- --check` 0 差异 / `cargo clippy --workspace --all-targets -D warnings` exit 0 / `cargo deny --locked check` 全绿 / `scripts/ci_grep_gates.sh` **5 条**红线全绿
- [x] **T019** `architecture_guard` 三条用例全绿
- [x] **T020** 同步 `specs/architecture.md`(§五 / §六 / §七 / §九)与 `specs/roadmap.md`(测试基线 / 变更档案表 / 变更说明 / 下一步); 写 [converge.md](converge.md)

## 未纳入(待拍板)

- [ ] **P0-D** 单笔名义上限 / 异常速率熔断 —— 需**可配置风险参数面**, 与宪法 D15/D17 冲突, 待用户拍板(见 spec §三/§六 与 roadmap "下一步" 第 8 项的三个候选方案)

## 明确不做(本轮, 非待办)

- 订单状态机完整 FSA / 改单 / 批量下单(P2 能力边界)
- tick 级对账 / 交易所侧成交回补(依赖数据资产)
- `exposure` 的 Web 面板与 AI 结构化只读工具(P1; 本轮 AI 侧只在提示词命令速查里提到了这条命令)
- 回测限价单"bar 触及即全成"的撮合偏差修正(属 🟡 P1, 会改变既有回测数字, 需单独立项 + 逐位比对)
