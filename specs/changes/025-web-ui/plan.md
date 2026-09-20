# 025 实施计划: Web UI 模式

**规格**: [spec.md](./spec.md) | **来源**: 用户需求九条 + 六项已拍板决策(子命令入口 / axum+SSE / 多会话持久化 / Web 专用启动脚本 / 恢复最近 20 轮 / 单页面双语切换) | **日期**: 2026-09-19 | **分支**: `025-web-ui`

## 一、决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | 入口 = 新增 `ricow web` 子命令(`main.rs` 的 `Command` 枚举 + 分发链各加一处) | 用户已拍板; 默认无参行为与全部既有子命令零影响 |
| D2 | 服务端 = **axum 0.8 + SSE **(单向流式, 不用 WebSocket) | 用户已拍板; 助手回复是单向增量流, SSE 语义更贴合且实现更少 |
| D3 | 只绑 `127.0.0.1` + 随机端口(默认 `--port 0`) + 一次性随机 token, 每请求校验, 比较用**常量时间**函数(照抄 `supervisor::server::ct_eq`) | 宪法「完全本地化 / 私钥不出本机」; 复用既有鉴权写法, 不新造 |
| D4 | 前端 = **手写静态资源 + `include_str!` 内嵌**, 无 npm / 无打包器 / 无 TS 编译 | 沿用 `templates.rs` / `prompt.rs` 既有做法; 满足 YAGNI, 且保证离线可构建 |
| D5 | 复用法 = **扩展 `SessionSink`**(补输入侧抽象 + 输出级别), 新写 `WebSink`; `ChatSession` 业务逻辑**零复制** | 兑现 `session.rs` 模块头预留的架构缝: "新写一个 WS/HTTP sink 并复用本文件" |
| D6 | `interactive` 判定从 `opts.interactive && stdin.is_terminal()` 改为**前端能力声明**(`Options` 增字段); 终端前端仍走 `is_terminal()`, Web 声明 `true` | Web 有输入通道但 stdin 非 tty; 不声明 true 则 `tools.rs` 的 `if !ctx.interactive` 会挡掉全部写操作(FR-005 是 FR-025 的前提) |
| D7 | `chat.rs` 的 `repl` 对 sink 的依赖由具体类型放宽为 `&mut dyn SessionSink` | Web 端要复用同一 REPL 循环; 仅放宽签名, 不改循环逻辑 |
| D8 | 输出分级 = **宿主侧显式标注**(`Severity::{Normal, Notice, Warn, Error}`), 前端按级别着色 | 用户要求"关键/警告/出错特殊颜色"; 关键字猜测会产生误报, 显式标注是唯一可靠口径 |
| D9 | 会话持久化 = 复用 `ricow_strategy::db::Database`, 在 `migrate()` 内**幂等追加** `web_sessions` / `web_messages` 两表 + 级联外键 | `crates/ricow` 无 sqlx 直接依赖; `Database` 已封装 sqlx 并开 WAL/外键 → **零新依赖** |
| D10 | 恢复上下文 = **最近 20 轮**往返, 单条按既有 `clip_for_history` 上限截断 | 用户已拍板; 防 token 撑爆; 复用既有截断函数, 不新增第二套口径 |
| D11 | 双语 = 单页面 + 语言切换按钮; 沿用 023 决策 D13(**就地双语字面量**, 无 i18n 框架 / 无资源文件) | 用户已拍板; 与既有机制一致 |
| D12 | 语言落 `ricow.toml` 的 `[ui].lang`(复用 `config_file::UiSection` + `set_values`, 保注释); 与 CLI **共用同一配置**; 切换后立即生效(复用 `rebuild_llm` 语义) | 用户已拍板"与 CLI 共用同一配置"; 不新增第二处语言状态 |
| D13 | 术语解释 = **静态内置数据**(中英各一份), 词条全集由 `format_backtest_report` 的指标文案反推 | 用户要求"鼠标点击弹出解释"; 离线可用、口径与回测报告同源、不依赖 AI 生成 |
| D14 | 启动脚本 = **新增**三平台 Web 专用脚本(复用同一套三级二进制查找 + 切工作目录), 只把启动行换成 `ricow web`; 既有三个脚本**逐字节不动** | 用户已拍板; 保证 CLI 用户零感知 |
| D15 | 确认语义**零改动**: 仍要求用户回当前语言的**单个口语词**, 不做前端按钮直确认 | 沿用 023 安全设计; 前端按钮直确认会绕开"人读过确认块"这道关 |
| D16 | CLI 零回归边界: `chat.rs` 的 `StdioSink` 输出字面量、`approve.rs`、`ctrl.rs`、`commands/ai.rs` 用户可见文案**不动** | 宪法「严格只改相关代码, 不顺手重构」 |
| D17 | 依赖新增仅限: workspace `[workspace.dependencies]` 加 `axum = "0.8"`; `crates/ricow/Cargo.toml` 加 `axum.workspace = true`; 如 `SessionSink` 输入侧需要 async 方法, 再加 `async-trait.workspace = true`(workspace 顶层已定义) | 最小增量; `tokio-stream` / `async-stream` / `futures` / `serde_json` 已在锁文件或已是直接依赖 |
| D18 | 不做: WebSocket / 公网访问 / 多用户 / 账号体系 / 图表可视化 / 会话导出 / 前端构建链 | YAGNI; 均未被要求 |
| D19 | 文档同步范围: `architecture.md`(接入点) + `product.md` / `README.md` / `README_zh.md`(启动方式) + `roadmap.md` | 宪法文档体系要求 |

## 二、改动清单

| 文件 | 改动 |
|---|---|
| `Cargo.toml`(workspace) | `[workspace.dependencies]` 新增 `axum = "0.8"`(与既有 `rig` 注释风格一致, 注明"Web UI 服务端") |
| `Cargo.lock` | 新增 axum 及其独有传递依赖条目(axum / axum-core / axum-macros / matchit / serde_path_to_error 等); **本机 registry 缓存已实测有 axum 0.8.9 全套, 离线可构建** |
| `crates/ricow/Cargo.toml` | 新增 `axum.workspace = true`; (按需) `async-trait.workspace = true` |
| `crates/ricow/src/main.rs` | `Command` 枚举新增 `Web(commands::web::WebArgs)`; `main()` 分发链新增 `Some(Command::Web(args)) => commands::web::run(args).await`; 默认无参分支**不动** |
| `crates/ricow/src/commands/mod.rs` | 新增 `pub mod web;`(与既有子模块并列, 不改动 `require_interactive_terminal` 等既有函数) |
| `crates/ricow/src/commands/web.rs`(新) | `WebArgs { port, no_open }` + `run()`: 复用 `onboard::run_if_needed` / `permission_warning` → 建 `Database` + `ChatSession` → 起 axum → 打印带 token 的 URL → (非 `--no-open`) 打开浏览器 |
| `crates/ricow/src/web/mod.rs`(新) | axum 路由与状态: token 中间件(常量时间校验)、静态资源、SSE 流、会话 CRUD API、语言读写 API; 只绑 `127.0.0.1` |
| `crates/ricow/src/web/sink.rs`(新) | `WebSink`: 实现扩展后的 `SessionSink` —— 助手增量 → SSE 帧; 宿主行 → 带 `Severity` 的 SSE 帧; 用户输入 → 从浏览器经 channel 进入(以闭包/oneshot 阻塞等); `secret` 走同一条不回显通道 |
| `crates/ricow/src/web/store.rs`(新) | 会话/消息读写封装(新建 / 列表 / 加载最近 20 轮 / 删除 / 追加消息); 只调 `Database` 新增方法, 不直接拼 SQL |
| `crates/ricow/src/web/terms.rs`(新) | 术语表: `&[(key, zh, en)]`, key 与 `format_backtest_report` 指标名对齐; 单测断言双向无遗漏 |
| `crates/ricow/src/web/assets/index.html` / `app.js` / `style.css`(新) | 单页三区布局(左会话列表 / 右上对话流 / 右下输入区)、SSE 消费、分级着色、术语点击弹窗、语言切换按钮; 配色沿用 `website/style.css` 深色系 |
| `crates/ricow/src/ai/session.rs` | ① `SessionSink` 补输入侧方法(读用户输入 / 读密钥), `line()` 增加级别参数; ② 新增 `Severity` 枚举; ③ `Options` 增加"前端能力"字段, `open()` 的 `interactive` 判定改为读该字段(终端路径仍走 `is_terminal()`, **行为不变**); ④ `reply()` / 错误分支 / 用量行等宿主输出显式标注级别; ⑤ 模块头"将来接网页端"表述更新为已接入 |
| `crates/ricow/src/commands/chat.rs` | `StdioSink` 实现新增的 trait 方法(**输出字面量与现有行为逐字不变**); `repl` 签名放宽为 `&mut dyn SessionSink`(D7) |
| `crates/ricow_strategy/src/db.rs` | `migrate()` 幂等追加 `web_sessions` / `web_messages`(级联外键 + 时间索引); 新增会话/消息 CRUD 方法; **既有表与既有方法零改动** |
| `packaging/` 新增 3 个 Web 启动脚本 | Windows `.cmd` / macOS `.command` / Linux `.sh`: 复用既有三级二进制查找与切工作目录逻辑, 启动行 = `ricow web`; 既有三个脚本**逐字节不动** |
| `specs/architecture.md` / `product.md` / `roadmap.md` / `README.md` / `README_zh.md` | 补 Web 模式接入点与启动方式 |

## 三、风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | `Cargo.lock` **不含 axum**, 需新增 axum 及其独有传递依赖, 增量中等 | 本机 registry 缓存已实测存在 `axum-0.8.9` / `axum-core-0.5.6` / `axum-macros-0.5.1` / `matchit-0.8.4` / `serde_path_to_error-0.1.20`; 先离线加锁再 `cargo build --locked` 验证(SC-012) |
| R2 | `crates/ricow` **无 sqlx / async-trait 直接依赖** | 会话存储走 `ricow_strategy::db` 封装 → 零新依赖(D9); 仅当 `SessionSink` 输入侧必须 async 时才追加 `async-trait.workspace = true`(workspace 顶层已有定义) |
| R3 | `SessionSink::secret` 是**同步阻塞**签名, 与异步 SSE 服务形态冲突 | 保持签名不变, `WebSink` 内部用 `tokio::sync::oneshot` + 阻塞等待(或 `spawn_blocking`)拿到浏览器回传; 仍保证"`None` = 放弃本次修改且不写文件"(FR-015) |
| R4 | Web 端拿到 `interactive = true` 即获得完整写能力(含实盘) | 三重约束: ① 只绑 `127.0.0.1`; ② 一次性 token 常量时间校验, 错则 `401` 且不回任何内容; ③ 写操作确认**仍要求回口语词**, 不做前端按钮直确认(D15) |
| R5 | 复用同一 SQLite 文件与既有行情/回测写并发, 可能锁竞争 | `Database::open` 已开 WAL + 外键 + 连接池 4; 会话写为低频; 遇 `SQLITE_BUSY` 有界重试; **不改既有表结构**, 只新增两张表 |
| R6 | 破坏既有 CLI 行为(最高优先级风险) | `StdioSink` 输出字面量与 `approve.rs` / `ctrl.rs` 零改动(D16); `cargo test --workspace` 基线 403 passed 不得倒退(SC-001); 打包脚本逐字节比对(SC-002) |
| R7 | 术语表遗漏指标, 或中英不对称 | 单测双向 grep 断言 `format_backtest_report` 指标名与术语表 key 一一对应, 且每 key 中英均非空(SC-009) |
| R8 | 双语遗漏硬编码中文 | 宿主新增文案一律走 `t(lang, zh, en)`; 前端文案集中在单一字典对象; 切到 `en` 做一次全页面走查(SC-010) |
| R9 | 无构建链前端(裸 HTML/JS)在功能增长后难维护 | 接受(YAGNI, D4); 控制资源文件规模, 不为"将来可能"引入打包器 |
| R10 | token 出现在 URL, 理论上可被本机其它进程读取 | 一次性随机 token + 只绑 loopback; token **不写盘、不进日志**; 服务退出即失效 |
| R11 | 流式输出期间用户误以为可继续输入 | 输入区在回复期禁用并显示"正在思考"; 不排队、不并发多轮 |

## 四、验收动作

1. 离线加锁与构建: `cargo build --locked` 成功(断网), 证明 R1 处置成立。
2. `cargo fmt --all -- --check` 零差异。
3. `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告。
4. `cargo test --workspace --locked` 全绿, 且 passed 数 **≥ 403**、ignored 为 21(SC-001)。
5. `git diff --stat` + 逐字节比对: `packaging/` 三个既有脚本无改动; `chat.rs` 的 `StdioSink` 输出字面量无改动(SC-002)。
6. 双击 Web 启动脚本: 浏览器自动打开且页面可用(见 SC-003)。
7. 浏览器内真实提问 → 流式回答; 全程无终端交互(SC-003)。
8. 浏览器内完成一次写操作(部署策略或启动测试网), 回口语词后动作**真实执行**; 确认为 testnet/币安 demo 真实调用, **禁 mock**(宪法三, SC-004)。
9. 无 token / 错 token 请求返回 `401` 且响应体为空; 断言监听地址为 `127.0.0.1`(SC-005)。
10. 新建两会话各发数轮 → 关页重开 → 完整恢复; 删除其中一个 → 其消息行数为 0(SC-006)。
11. 30 轮旧会话恢复上下文数为 20 且超长条被截断(SC-007)。
12. 术语点击弹窗: 中英各验一条, 内容与 `format_backtest_report` 口径一致(SC-009)。
13. 切到 `en` 走查全页面, 再切回 `zh`; 核对 `ricow.toml` 的 `[ui].lang` 已落盘, 且终端 `ricow ai` 语言一致(SC-010)。
14. `/keys` 在 Web 端录入一次, 断言明文密钥不出现在对话流与 localStorage(SC-011)。
15. 终端 CLI 回归: 默认 `ricow` 进对话、`ricow ai "..."` 单次模式、`ricow approve` 逐字短语门禁, 行为与改动前一致(FR-032/FR-034)。
16. 文档同步核对: `architecture.md` / `product.md` / `roadmap.md` / `README.md` / `README_zh.md` 均已更新(SC-013)。
