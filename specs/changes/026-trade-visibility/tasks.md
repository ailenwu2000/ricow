# 026 任务分解: 交易信息与日志可见性

**依据**: [spec.md](./spec.md)(FR-001..FR-028 / SC-001..SC-010) + [plan.md](./plan.md)(D1..D20) | **日期**: 2026-09-19

格式: `T编号 [P=可并行] [阶段] 描述(含精确文件路径)` —— 每条尾部标注溯源的 D / FR / SC。

**落地口径提醒(取自 spec, 非新增决策)**: FR-001/FR-002 的字段清单是"**至少含**" —— FR-005 要求 `orders` / `positions` 都带模式标识, 故两张表在清单之外**各增一列 `mode`**(∈ `dry_run` / `demo` / `live`); `fills` 表**不加列**(D5), 成交的 mode 由 `exchange_order_id` ⟕ `orders` 带出。

## 阶段 1 地基(数据层, 阻塞全部后续)

- [x] T001 [地基] `crates/ricow_strategy/src/db.rs`: `migrate()` 内以 `CREATE TABLE IF NOT EXISTS` **幂等追加** `orders` 表 —— 列: `strategy_id` / `exchange_order_id` / `client_order_id` / `pair` / `side` / `price` / `size` / `filled_size` / `status` / `mode` / `created_at` / `updated_at`, 主键 `(strategy_id, exchange_order_id)`(D7) —— FR-001 / FR-005 / FR-027
- [x] T002 [地基] `crates/ricow_strategy/src/db.rs`: 同法追加 `positions` 表 —— 列: `strategy_id` / `pair` / `size`(带方向) / `entry_price` / `mode` / `updated_at`, 主键 `(strategy_id, pair)`(D7); 为两表的查询路径补索引(strategy_id / updated_at) —— FR-002 / FR-005 / FR-027
- [x] T003 [P] [地基] `crates/ricow_strategy/src/db.rs`: 新增 `upsert_order()`(一单一行, 冲突则更新 `filled_size` / `status` / `updated_at`) —— D7 / FR-003
- [x] T004 [P] [地基] `crates/ricow_strategy/src/db.rs`: 新增 `upsert_position()`(每策略每对一行) —— D7 / FR-003
- [x] T005 [P] [地基] `crates/ricow_strategy/src/db.rs`: 新增 `recent_orders()` / `current_positions()`(支持按 `strategy_id` 过滤 + `limit`) —— FR-010
- [x] T006 [P] [地基] `crates/ricow_strategy/src/db.rs`: 新增 `insert_pnl_snapshot()` / `recent_pnl_snapshots()` —— **不改 `pnl_snapshots` 列结构**(FR-007), 只补写入与读取者 —— FR-007 / FR-009
- [x] T007 [地基] `crates/ricow_strategy/src/db.rs`: 新增 `recent_fills_with_mode()` —— `fills` ⟕ `orders` 按 `exchange_order_id` 取 `mode`; 关联不到**返回"未知"**(不猜) —— D5 / FR-005
- [x] T008 [P] [地基] `crates/ricow_strategy/src/db.rs` 单测: **幂等迁移** —— 用**不含新表**的旧库文件打开 → 自动补齐 `orders` / `positions`, 既有 8 张表**逐行不变**; 重复 `migrate()` 不报错、不重复建 —— FR-027 / SC-004
- [x] T009 [地基] `crates/ricow/src/ai/tools.rs` + `crates/ricow/src/web/mod.rs`: 抽出/复用 **三态判定** `source` ∈ {`ok`, `daemon_down`, `unreadable`} —— 判定口径**只**用 `supervisor::ledger::read_daemon_info` + TCP 连接(不新造第二套), 供新增工具与新增端点共用 —— D10 / FR-014 / FR-020

## 阶段 2 引擎落库(F0 / F1)

- [x] T010 [F0] `crates/ricow_engine/src/command.rs`: **时点① 下单提交** —— `place_order` 的 `Ok(ack)` 分支(live 两处 + dry run 一处)接 `upsert_order()` —— FR-003
- [x] T011 [F0] `crates/ricow_engine/src/command.rs`: **时点② 成交回报** —— 在既有 `insert_fill` 的**同一 5 个调用点**(`drain_user_events` / dry run 主循环 / dry run 停机清理 / live `UserEvent::Fill` / live 停机清理)**旁边**追加 `upsert_order()`(更新 `filled_size` / `status`)与 `insert_pnl_snapshot()`(取 `ctx.pnl()` 的 `realized_pnl` / `total_fees` / `net_pnl` / `trade_count`, **同源不重算**) —— D6 / FR-003 / FR-007 / FR-008
- [x] T012 [F0] `crates/ricow_engine/src/command.rs`: **时点③ 订单状态变化** —— **新增** `UserEvent::Order(upd)` match 分支 → `upsert_order()`(现状该分支被丢弃, 撤单/过期/部分成交不可见); **只在既有 `Fill` 分支旁加分支, 不改 `Fill` 分支与任何下单/撤单/风控逻辑** —— D8 / FR-003 / R7
- [x] T013 [F0] `crates/ricow_engine/src/command.rs`: **时点④ 持仓变化** —— `set_positions` 调用点(实盘新鲜切片)与 dry run 成交后(`ctx.position(pair)`)接 `upsert_position()` —— FR-003
- [x] T014 [F0] `crates/ricow_engine/src/command.rs`: 全部新增写库的**错误处置与既有 `insert_fill` 逐字同构** —— 只 `persist_errors += 1` + `tracing::error!` + 记 `last_error`, **不上抛、不阻塞、不改变执行结果** —— D6 / FR-004 / FR-028
- [x] T015 [P] [F0] `crates/ricow_engine/src/command.rs` 单测: 下单 → 成交 → 订单状态变化 → 持仓变化后, `orders` / `positions` / `fills` / `pnl_snapshots` 四表行数与数值符合预期; **注入写库错误后交易主流程照常完成**; 快照数值与 `fill_stats` 同源 —— SC-003 / FR-004 / FR-008

## 阶段 3 AI 会话回流(F4)

- [x] T016 [F4] `crates/ricow/src/commands/instances.rs`: 新增 `format_positions()` / `format_open_orders()` / `format_pnl()`, 与既有 `format_fills` **同风格**(AI 工具与 Web **同源**); **不新增 CLI 子命令、既有文案零改动** —— FR-012 / FR-025
- [x] T017 [F4] `crates/ricow/src/ai/tools.rs`: 新增只读工具 `positions` / `open_orders` / `pnl`(带策略名与条数上限), 并登记进 `READ_ONLY_TOOLS` 白名单; 同步更新白名单数量断言与"白名单 = 注册表"断言;**既有 `fills` / `logs_tail` / `instance_status` 行为与文案不动** —— D9 / FR-019 / R11
- [x] T018 [F4] `crates/ricow/src/ai/tools.rs`: 修 `tool_logs_tail` 假阴性 —— 区分"**从未启动**(文件不存在)"与"**读不到**(权限 / IO 错误)", 不再一律说"无日志文件: ... (该策略从未启动过?)" —— FR-020
- [x] T019 [F4] `crates/ricow/src/ai/tools.rs`: 三个新工具返回经 T009 的 `source` 三态 —— daemon 未运行 / 库不可读时如实说"无法判断", 不得假阴性说"没有" —— FR-020
- [x] T020 [F4] `crates/ricow/src/ai/session.rs`: `LineDisposition::Confirm` 分支的 `Ok(msg)` / `Err(e)` **两条**都把结果 push 进 `self.history`(`Ok` → assistant 标注为执行结果; `Err` → 如实告知失败), 内容经既有 `clip_for_history` 裁剪且与 `sink` **同源**; **不触碰** `confirm::consume_line` 状态机语义 —— FR-021 / FR-022 / FR-023 / FR-024
- [x] T021 [P] [F4] `crates/ricow/src/ai/session.rs` 单测: 写动作确认执行后 `history` 含该结果文案且与显示**同源**(SC-005); **失败场景同样注入**(FR-024); 断言确认状态机不受影响 —— SC-005 / FR-022 / FR-024
- [x] T022 [P] [F4] `crates/ricow/src/ai/tools.rs` 单测: daemon 未运行时查持仓 → "无法判断 / 此刻状态不可知"而非"没有持仓"; 确实无持仓时才说"无" —— SC-006 / FR-020

## 阶段 4 Web 交易端点(F2)

- [x] T023 [F2] `crates/ricow/src/web/mod.rs`: `WebState` 拿到**只读用** `Database` 句柄(从既有 `SessionStore` 暴露或克隆 —— 连接池克隆零成本), **与引擎同一库** —— D1 / FR-010
- [x] T024 [F2] `crates/ricow/src/web/mod.rs`: 新增只读端点(全部挂在**既有 `require_token` 同一 `.layer` 内**, 服务仍只绑 `127.0.0.1`): `GET /api/trades/fills|orders|positions|pnl?name=&limit=`; 响应带 D10 的 `source` 与 D11 的 `updated_at` —— D10 / D11 / D14 / FR-010 / FR-011 / FR-014
- [x] T025 [P] [F2] `crates/ricow/src/web/mod.rs` 单测: 对**每个**新端点发无 token / 错 token 请求 → `401` 且响应体**不含任何交易数据**; 断言监听地址仍为 `127.0.0.1` —— SC-007 / FR-011
- [x] T026 [P] [F2] `crates/ricow/src/web/mod.rs` 单测: 空状态**三态**文案分开 —— "无持仓" / "无成交" vs "daemon 未运行, 此刻状态不可知" vs "读不到(原因)"; 停机快照带"截至 <时间>"且**不**谎称"当前" —— D10 / D11 / FR-014

## 阶段 5 Web 日志流(F3)

- [x] T027 [F3] 新建 `crates/ricow/src/web/tail.rs`: **纯逻辑尾读** —— 给定 (文件路径, 上次 offset) → 返回 (完整新增行, 新 offset); 含**轮转检测**(读到长度 < 上次 offset → 视为轮转/截断 → offset 归零重读 + 提示); 不写、不改、不轮转该文件 —— D13 / FR-015 / FR-016
- [x] T028 [P] [F3] `crates/ricow/src/web/tail.rs` 单测: 首次读给最近 N 行; 增量读只给新增行; 请求超大 N → **夹到 200**(D12); 文件被截断/轮转后**不丢行、不重复** —— D12 / D13 / FR-015 / SC-008
- [x] T029 [F3] `crates/ricow/src/web/mod.rs`: 新增 `GET /api/logs`(策略日志清单) + `GET /api/logs/{name}/tail?lines=`(**策略未运行时仍可看历史日志**) —— FR-015 / FR-017
- [x] T030 [F3] `crates/ricow/src/web/mod.rs`: 新增 `GET /api/logs/{name}/stream`(SSE) —— 服务端尾读驱动, 新行实时推; SSE 轮询 500ms; **独立端点、独立广播通道, 不共用 025 会话 `Hub`** —— D2 / D12 / R5 / FR-016
- [x] T031 [F3] `crates/ricow/src/web/mod.rs`: 两个新日志端点同样挂在 T024 的 token 层内; 日志内容**原样**返回(不解析 / 不改写 / 不做着色推断) —— D14 / FR-018

## 阶段 6 前端面板

- [x] T032 [面板] `crates/ricow/src/web/assets/index.html` / `style.css`: 新增**交易面板**(当前持仓 / 挂单 / 最近成交)与**日志面板**(策略切换 + 实时追加)区域; 配色沿用既有深色系, **无构建链**(`include_str!` 内嵌) —— D16 / FR-012
- [x] T033 [面板] `crates/ricow/src/web/assets/app.js`: 交易面板渲染 —— 数据来自 T024 端点; **空状态三态分开呈现** + 停机快照标注"截至 <时间>"; **只读, 不提供任何直连交易所的按钮**(撤单/平仓/停机仍走既有对话确认) —— D11 / D17 / FR-012 / FR-013 / FR-014
- [x] T034 [P] [面板] `crates/ricow/src/web/assets/app.js`: 日志面板 —— 策略切换 + 打开先给最近 N 行 + SSE 实时追加; 页面刷新后重建 —— FR-016 / FR-017
- [x] T035 [P] [面板] `crates/ricow/src/web/mod.rs` 断言: 前端资源**不含**任何直连交易所的写按钮/写请求(只读红线); 025 既有前端断言全部保持通过 —— D17 / FR-013 / FR-026

## 阶段 7 打磨与验收

- [x] T036 [打磨] 三门禁: `cargo fmt --all -- --check` 零差异 + `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告 + `cargo test --workspace --locked` 全绿且 passed **≥ 480** / failed = 0 / ignored = 21(**不得倒退**) —— SC-001
- [x] T037 [打磨] 零回归走查: `git diff` 中既有 CLI 用户可见文案(`ricow fills` / `ricow info` / `ricow logs` / `ricow daemon *`)与 025 Web 端点契约(会话 CRUD / 会话 SSE / `/api/terms` / `/api/lang`)**无改动**, 输出逐字一致 —— D18 / FR-025 / FR-026 / SC-009
- [x] T038 [打磨] 端到端真实调用(宪法三, **禁 mock 交易所**): 跑一次 demo 测试网策略 → 产生**真实成交** → 在 Web 交易面板**页面上**看到该笔成交与持仓(带"截至"时间与 mode); 同时 `logs/<name>.log` 新行**实时**出现在日志面板; 全程无终端交互 —— SC-002
- [x] T039 [打磨] 已覆盖项端到端复验(**本次不改代码, 只验**): ① daemon 未运行时不再出现"已经停着"这类假阴性表述; ② demo 测试网停机不再印「实盘停机」文案 —— SC-002 / spec §六
- [x] T040 [打磨] 文档同步: `specs/architecture.md` 表清单 8 张 → 10 张(`orders` / `positions`) + 新增"交易可见性"章节(落库时点 / 数据源口径 / 端点); `specs/roadmap.md` 记录 026 —— D19 / SC-010

## 依赖与执行顺序

- 阶段 1 → 阶段 2 严格顺序(T001/T002 阻塞 T003–T007; T003–T007 阻塞 T010–T013)。
- T009 阻塞 T019(工具三态)与 T024(端点三态)。
- 阶段 2 内部: T010 与 T013 可并行; T011 依赖 T003/T006; T012 依赖 T003; T014 是 T010–T013 的收口(同一次改动内完成)。
- 阶段 3: T017 依赖 T016(格式化函数)与 T009; T019 依赖 T009/T017。
- 阶段 4: T024 依赖 T005/T007/T009/T023。
- 阶段 5: T029/T030 依赖 T027; T031 依赖 T029/T030。
- 阶段 6: 依赖 T024(交易端点)与 T030(日志 SSE); T032 可与阶段 3/4/5 并行开工(纯静态骨架)。
- 阶段 7 依赖全部前置。

## 并行示例

- 第一批可并行: T003 / T004 / T005 / T006 四组 `db.rs` 新方法(不同方法, 同表结构定稿后可并行); T008 单测可与 T009 并行。
- 第二批可并行: T010+T013(落库时点) 与 T016(`format_*`) 互不依赖。
- 第三批可并行: T024(交易端点) 与 T027(`tail.rs` 纯逻辑) — 两者互不依赖; T032(前端骨架) 此时可并行。
- 第四批可并行: T025/T026(端点单测) 与 T034/T035(前端日志面板与断言)。
- 阶段 7 的 T036 / T037 与 T040(文档)可并行。

## 实施策略

- **MVP 优先**: 阶段 1 + 阶段 2 + T024 + T032/T033 完成即为可交付 MVP —— "跑测试网 → 页面交易面板看得到成交与持仓"。
- **增量交付顺序**: F0 落库 → F1 PnL 快照 → F2 交易面板 → F3 日志流 → F4 AI 回流; F4 只依赖阶段 1, 可与 F2/F3 并行。
- **零回归红线**: T036 / T037 / T038 是三道闸, 任一不过即阻断交付。基线 **480 passed / 0 failed / 21 ignored**。
- **不改既有代码的边界**: 除 `db.rs`(仅幂等追加新表与新方法) / `command.rs`(仅新增 match 分支 + 既有落库调用点旁追加) / `tools.rs`(仅新增工具 + 修一处假阴性文案) / `session.rs`(仅 Confirm 分支两条 push history) / `instances.rs`(仅新增 `format_*`) / `web/mod.rs`(仅新增端点) 外, 其余既有文件不改。
