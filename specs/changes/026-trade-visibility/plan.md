# 026 实施计划: 交易信息与日志可见性

**规格**: [spec.md](./spec.md) | **来源**: 用户四条质疑的取证结论 + 四项已拍板决策(**本地库为唯一来源** / **尾读 + SSE 增量推送** / **PnL 每笔成交后一条 + 永久保留** / **只读工具 + 执行结果注入 history**) | **日期**: 2026-09-19 | **分支**: `026-trade-visibility`

## 一、决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | 数据源 = **本地库为唯一来源**(用户拍板): 面板与新增 AI 工具一律读本地 SQLite, **不直连交易所**, 不引入第二套行情/账户查询路径 | 离线可看; 与策略进程解耦; 查询快。代价(与交易所瞬时状态有延迟差)由用户接受(§六) |
| D2 | 日志呈现 = **尾读 + SSE 增量推送**(用户拍板): 打开先给最近 N 行, 之后新行实时推给页面 | 复用 025 既有 axum + SSE 装配; 对金融交易, 成交/撤单/风控行**当场可见**比手动刷新有用 |
| D3 | PnL 快照 = **每笔成交后一条 + 永久保留**(用户拍板): 写入点紧跟成交落库; 不做 TTL、不做行数淘汰 | 成交是唯一让 PnL 变化的事件; 永久保留便于回看整条曲线 |
| D4 | AI 回流 = **只读工具 + 执行结果注入 history**(用户拍板); **不做**运行中主动推送 | 先修"模型失明"这条断链; 主动推送需跨进程事件通道, 留后续变更 |
| D5 | `mode`(运行模式)只落**新增表** `orders`/`positions`; **不在 `fills` 表加列** —— 成交的 mode 由 `fills.exchange_order_id` ⟕ `orders.exchange_order_id` **关联带出** | 守住 FR-001「既有 8 张表结构零改动」, 避免在既有表上 `ALTER`(SQLite 无 `ADD COLUMN IF NOT EXISTS`, 幂等探测有风险)。**代价**: 026 之前的历史成交在 `orders` 里无对应行 → 面板**如实**显示 mode 为"未知", 不猜 |
| D6 | 落库位置 = **沿用既有 `insert_fill` 的同一批调用点**, 在旁边追加 upsert; 失败**只** `persist_errors += 1` + `tracing::error!` + 记 `last_error`, **不上抛** | FR-004 / FR-028: 写库不得改变执行结果, 不得阻塞下单/风控关键路径。与既有落库错误处置逐字同构 |
| D7 | `orders` 主键 = `(strategy_id, exchange_order_id)`, **一单一行 upsert 最新状态**; `positions` 主键 = `(strategy_id, pair)`, **每策略每对一行** | "挂单/持仓"是**当前状态**视图, 不是流水; 字段与既有 `OrderAck`/`OrderUpdate`/`Position` 对齐, 不新造形状 |
| D8 | 订单状态变化(`UserEvent::Order(OrderUpdate)`) **接入 live 主循环** → upsert `orders` | 现状: live 循环只 `if let UserEvent::Fill` 匹配, **撤单/过期/部分成交的状态变化被丢弃** → 挂单视图必然陈旧; 这正是 FR-003 的"订单状态变化"时点 |
| D9 | AI 只读工具 = **复用既有 `fills` / `logs_tail` / `instance_status`(行为不动)** + **新增 `positions` / `open_orders` / `pnl` 三个**; 另修既有 `logs_tail` 的假阴性文案 | 现状核实: 三个只读工具**已存在**(`READ_ONLY_TOOLS` 12 项), spec FR-019 的"查成交 / 读日志"已覆盖; **真实缺口是持仓 / 挂单 / PnL**。不重复造 |
| D10 | 空状态**三态**(FR-014 / FR-020): 端点与工具返回显式 `source` 字段 ∈ {`ok`, `daemon_down`, `unreadable`}; 分别呈现"无 X"、"daemon 未运行, 此刻状态不可知"、"读不到(原因)" | 修掉本轮用户遇到的同类假阴性; **不允许**把"读不到"说成"没有"。判定口径复用 `supervisor::ledger::read_daemon_info` + TCP 连接(唯一口径), 不新造 |
| D11 | 停机时的持仓/挂单 = **最后一次写入的快照** → 一律带 `updated_at`, 面板显式标注"截至 <时间>"; daemon 未运行时**不**把快照说成"当前" | D1 的代价必须在界面上说清; 本地库口径下"当前状态不可知"是诚实答案, 不是缺陷 |
| D12 | 日志尾读上限 N = **200 行**(与既有 AI `logs_tail` 同口径, 请求超限被夹取); 单行按既有输出上限裁剪; SSE 轮询间隔 **500ms** | FR-015 硬上限防一次拉整个文件; 与既有工具一致, 不新造第二套口径 |
| D13 | 日志轮转检测 = 读到的文件长度**小于**上次 offset → 视为轮转/截断 → offset 归零重读, 并推一行系统提示 | `procs.rs:137-144` 的 10MB 轮转(`.log` → `.log.1`)会让 tail 的 offset 失效; 静默跳读会丢日志 |
| D14 | 新增端点全部挂在既有 `require_token` 层**之内**(与既有路由同一 `.layer`); 服务仍只绑 `127.0.0.1` | FR-011; 不新开鉴权分支, 不新增泄露面 |
| D15 | 零新依赖: 复用 sqlx(经 `ricow_strategy::db::Database`)、axum、tokio、serde_json | YAGNI; 与 025 D9/D17 一致 |
| D16 | 前端 = 既有单页 `assets/` 内**新增**交易面板与日志面板区域, 手写 HTML/JS/CSS, **不引入构建链** | 沿用 025 D4(`include_str!` 内嵌) |
| D17 | 面板**只读**: 不提供任何直连交易所的按钮; 撤单/平仓/停机**仍必须**走既有对话确认流程 | FR-013; 不绕开 023/025 的"人读过确认块"门禁 |
| D18 | 025 既有 Web 契约零改动: 会话 CRUD / 会话 SSE / `/api/terms` / `/api/lang` 与页面既有行为**逐字不动** | FR-026 |
| D19 | 文档同步: `specs/architecture.md`(表清单 8 → 10 + 交易可见性章节) + `specs/roadmap.md` | SC-010; 宪法文档体系 |
| D20 | **不做**: 直连交易所 / 运行中主动推送 / 日志结构化或着色推断 / 图表可视化 / `.log.1` 历史轮转文件 / 日志云同步 / 前端构建链 / 改任何执行语义 / 改既有 CLI 用户可见文案 | YAGNI + 宪法「严格只改相关代码」 |

## 二、改动清单

| 文件 | 改动 |
|---|---|
| `crates/ricow_strategy/src/db.rs` | `migrate()` 幂等追加 `orders` / `positions` 两表(D5/D7) + 查询索引; 新增 `upsert_order()` / `upsert_position()` / `recent_orders()` / `current_positions()` / `insert_pnl_snapshot()` / `recent_pnl_snapshots()` / `recent_fills_with_mode()`(fills ⟕ orders 取 mode, D5)。**既有 8 张表与既有方法零改动** |
| `crates/ricow_engine/src/command.rs` | 四个时点接落库(F0/D6/D8): ① `place_order` 的 `Ok(ack)` 分支(live 两处 + dry run 一处) → `upsert_order`; ② 成交落库处(`drain_user_events` / dry run 主循环 / `UserEvent::Fill` / 两处停机清理) 在旁边追加 `upsert_order`(更新 `filled_size`/`status`) + `insert_pnl_snapshot`(取 `ctx.pnl()` 的 `realized_pnl`/`total_fees`/`net_pnl`/`trade_count`, 同源同口径, FR-008); ③ **新增 `UserEvent::Order(upd)` 分支** → `upsert_order`; ④ `set_positions` 调用点(实盘有新鲜切片)与 dry run 成交后(`ctx.position(pair)`) → `upsert_position`。全部错误处置与既有 `insert_fill` 逐字同构 |
| `crates/ricow/src/commands/instances.rs` | 新增 `format_positions()` / `format_open_orders()` / `format_pnl()`(与既有 `format_fills` 同风格, 供 AI 工具用, 保证 AI 与 Web **同源**)。**不新增 CLI 子命令**, 既有文案零改动 |
| `crates/ricow/src/ai/tools.rs` | 新增只读工具 `positions` / `open_orders` / `pnl`(D9), 并登记进 `READ_ONLY_TOOLS` 白名单(数量断言测试随之更新); 修 `logs_tail` 的"无日志文件"假阴性 → 区分"从未启动(文件不存在)"与"读不到(权限/IO 错误)"(FR-020)。**既有三个工具的行为与文案不动** |
| `crates/ricow/src/ai/session.rs` | FR-021~FR-024: `LineDisposition::Confirm` 分支的 `Ok(msg)` / `Err(e)` **两条**都把结果 push 进 `self.history`(`Ok` → `Message::assistant` 标注为执行结果; `Err` → 如实告知失败), 内容经既有 `clip_for_history` 裁剪且与 `sink` 显示**同源**; **不触碰** `confirm::consume_line` 的状态机语义(确认词仍只认真实用户输入行) |
| `crates/ricow/src/web/mod.rs` | 新增只读端点(全部在 D14 的同一 `.layer` 内): `GET /api/trades/fills|orders|positions|pnl?name=&limit=`; `GET /api/logs`(策略日志清单); `GET /api/logs/{name}/tail?lines=`; `GET /api/logs/{name}/stream`(SSE)。响应带 D10 的 `source` 状态与 D11 的 `updated_at`。`WebState` 拿到只读用的 `Database` 句柄(从既有 `SessionStore` 暴露或克隆 `Database`, **连接池克隆, 零成本**) |
| `crates/ricow/src/web/tail.rs`(新) | 纯逻辑尾读: 给定 (文件路径, 上次 offset) → 返回 (完整新增行, 新 offset) + 轮转检测(D13, 可单测); SSE handler 只做定时驱动与广播 |
| `crates/ricow/src/web/assets/index.html` / `app.js` / `style.css` | 新增交易面板(持仓 / 挂单 / 最近成交 + 空状态三态 + "截至 <时间>")与日志面板(策略切换 + 实时追加); 配色与既有深色系一致; 无构建链 |
| `specs/architecture.md` | 表清单 8 张 → 10 张(`orders` / `positions`); 新增"交易可见性"章节(落库时点 / 数据源口径 / 端点); `specs/roadmap.md` 记录本变更 |

## 三、风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | 表结构变更影响既有数据库文件 | 只用 `CREATE TABLE IF NOT EXISTS` **幂等追加**; 既有 8 张表零改动; 单测用**不含新表**的旧库文件打开 → 自动补齐且既有行逐行不变(SC-004) |
| R2 | 落库进入交易关键路径, 拖慢或拖挂下单 | 低频写(每单/每笔成交一行); 失败只计数 + 记日志, **不上抛**(D6); 复用既有 WAL + 连接池 4; 单测断言注入写库错误后主流程照常继续(SC-003) |
| R3 | 停机后的持仓快照被误读成"当前持仓"(假阴性/假阳性) | 一律带 `updated_at` + 面板/AI 显式标注"截至 <时间>"; daemon 未运行时**不**把快照说成"当前"(D10/D11); 单测断言三态文案(SC-006) |
| R4 | 尾读与策略进程同写一个日志文件 | 只读打开 + 按 offset 增量读; 不写、不改、不轮转该文件; 轮转/截断由 D13 检测 |
| R5 | 日志 SSE 长连接与既有会话 SSE 并存互相干扰 | **独立端点、独立广播通道**, 不共用 025 的会话 `Hub`; 会话链路一行不动(FR-026) |
| R6 | 新端点漏挂鉴权 → 交易数据外泄 | 全部新路由挂在同一 `require_token` 层内(D14); 单测断言无/错 token → `401` 且响应体不含交易数据(SC-007) |
| R7 | `UserEvent::Order` 接入后改变既有执行流程 | 只**新增一个 match 分支**做落库, 不改 `Fill` 分支、不改任何下单/撤单/止损逻辑; 既有测试基线不得倒退(SC-009) |
| R8 | PnL 快照口径与策略侧不一致, 出现两个"净盈亏" | 只从 `ctx.pnl()` 取值(FR-008), Web/AI **不重算**; 单测断言快照数值与 `fill_stats` 同源(SC-003) |
| R9 | 破坏既有 CLI 行为(最高优先级) | `fills` / `info` / `logs` / `daemon *` 文案逐字不动(D20); **不新增 CLI 子命令**; 基线 **480 passed / 0 failed / 21 ignored** 不得倒退(SC-001/SC-009) |
| R10 | 前端面板膨胀后难维护 | 接受(YAGNI, D16); 控制资源文件规模, 不为"将来可能"引入打包器 |
| R11 | 新增白名单工具导致既有权限断言测试失败 | `READ_ONLY_TOOLS` 数量断言与"白名单 = 注册表"断言**同步更新**; 断言"写工具名不得进白名单"的测试保持不变(仍必须通过) |

## 四、验收动作

1. `cargo fmt --all -- --check` 零差异。
2. `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告。
3. `cargo test --workspace --locked` 全绿, 且 passed 数 **≥ 480**、failed = 0、ignored = 21(SC-001, 基线不得倒退)。
4. 落库单测(SC-003): 下单 → 成交 → 订单状态变化 → 持仓变化后, `orders` / `positions` / `fills` / `pnl_snapshots` 四表行数与数值符合预期; 注入写库错误后**交易主流程照常完成**(FR-004)。
5. 幂等迁移(SC-004): 用**不含新表**的旧库文件打开 → 自动补齐 `orders` / `positions`; 既有 8 张表逐行不变。
6. 端到端(SC-002, 真实调用, **禁 mock**): 跑一次 demo 测试网策略 → 产生**真实成交** → 在 Web 交易面板**页面上**看到该笔成交与持仓(带"截至"时间与 mode); 同时 `logs/<name>.log` 新行**实时**出现在日志面板。全程无终端交互。
7. AI 回流(SC-005): 对话内确认执行一次写动作 → 断言 `history` 含该结果文案且与显示**同源**; 再验一次**失败**场景(同样注入 history)。
8. 只读工具无假阴性(SC-006): daemon 未运行时查持仓 → 返回"无法判断 / 此刻状态不可知", 而非"没有持仓"; 确实无持仓时才说"无"。
9. 鉴权(SC-007): 对**每个**新端点发无 token / 错 token 请求 → `401` 且响应体不含任何交易数据; 断言监听地址仍为 `127.0.0.1`。
10. 日志上限(SC-008): 请求超大 N → 响应行数被夹到 200; 轮转日志(`.log` 被截断)后 tail 不丢行、不重复。
11. 零回归(SC-009): `git diff` 中既有 CLI 用户可见文案与 025 的 Web 端点契约**无改动**; `ricow fills` / `ricow info` / `ricow logs` 输出与改动前逐字一致。
12. 已覆盖项端到端复验(本变更开工前已修, 本次只验): ① daemon 未运行时不再出现"已经停着"这类假阴性表述; ② demo 测试网停机不再印「实盘停机」文案。
13. 文档同步核对(SC-010): `specs/architecture.md` 表清单为 10 张且含新章节; `specs/roadmap.md` 已记录 026。
