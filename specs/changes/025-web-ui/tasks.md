# 025 任务分解: Web UI 模式

**依据**: [spec.md](./spec.md)(FR-001..FR-034 / SC-001..SC-013) + [plan.md](./plan.md)(D1..D19) | **日期**: 2026-09-19

格式: `T编号 [P=可并行] [阶段] 描述(含精确文件路径)` —— 每条尾部标注溯源的 D / FR / SC。

## 阶段 1 搭建

- [x] T001 [搭建] workspace `Cargo.toml` 的 `[workspace.dependencies]` 新增 `axum = "0.8"`; `crates/ricow/Cargo.toml` 新增 `axum.workspace = true`; 离线加锁后 `cargo build --locked` 必须成功 —— D17 / SC-012
- [x] T002 [搭建] `crates/ricow/src/main.rs`: `Command` 枚举新增 `Web(commands::web::WebArgs)`(枚举区)并新增分发分支(默认无参分支不动); `crates/ricow/src/commands/mod.rs` 新增 `pub mod web;`; 新建 `crates/ricow/src/commands/web.rs` 定义 `WebArgs { port: u16, no_open: bool }` 与 `pub async fn run(args: WebArgs) -> CoreResult<()>` 骨架 —— D1 / FR-001
- [x] T003 [P] [搭建] 新建 `crates/ricow/src/web/assets/index.html` / `style.css` / `app.js` 三区布局骨架(左会话列表 / 右上对话流 / 右下输入区), 配色沿用 `website/style.css` 深色系 —— D4 / FR-006

## 阶段 2 地基(阻塞性前置, 后续全部故事依赖)

- [x] T004 [地基] `crates/ricow/src/ai/session.rs`: 新增 `pub enum Severity { Normal, Notice, Warn, Error }` —— D8 / FR-013
- [x] T005 [地基] `crates/ricow/src/ai/session.rs`: 扩展 `SessionSink`(第 37-46 行) —— `line()` 增级别参数(或新增带级别方法), 并补输入侧方法(读用户一行 / 读密钥); 同步更新 `session.rs` 内的 `FakeSink` 测试实现 —— D5 / FR-012 / FR-013
- [x] T006 [地基] `crates/ricow/src/commands/chat.rs`: `StdioSink`(第 18-37 行)实现新增 trait 方法, **输出字面量与现有行为逐字不变**; `repl`(第 75-90 行)签名由具体 sink 类型放宽为 `&mut dyn SessionSink` —— D7 / D16 / FR-014
- [x] T007 [地基] `crates/ricow/src/ai/session.rs`: `Options`(第 57-69 行)新增前端能力字段(声明是否有输入通道 / 是否交互), `open()`(第 93-152 行)的 `interactive` 判定(第 124 行 `stdin().is_terminal()`)改为读该字段; 同步改两处 `Options` 构造点 `chat.rs:58` 与 `commands/ai.rs:41`(终端语义保持等价) —— D6 / FR-005
- [x] T008 [地基] `crates/ricow/src/ai/session.rs`: `reply()`(第 599-626 行)与错误分支、`[用量]` 行、`flush_menu()`(第 406-417 行)等宿主输出**显式标注** `Severity` —— D8 / FR-013
- [x] T009 [地基] `crates/ricow_strategy/src/db.rs`: `migrate()`(第 56-157 行)内以 `CREATE TABLE IF NOT EXISTS` **幂等追加** `web_sessions` / `web_messages`(会话外键级联删除 + 时间索引); 新增会话/消息 CRUD 方法(新建 / 列表 / 取最近 N 轮 / 删除 / 追加); 既有表与既有方法零改动 —— D9 / FR-017 / FR-018 / FR-019
- [x] T010 [地基] 新建 `crates/ricow/src/web/store.rs`: 会话/消息读写封装(只调 `Database` 新方法, 不直接拼 SQL); 会话标题取首条用户消息截断生成 —— D9 / FR-007 / FR-020 / FR-022
- [x] T011 [地基] 新建 `crates/ricow/src/web/mod.rs`: axum 服务骨架, 只绑 `127.0.0.1`(默认 `--port 0` 自动选端口); 一次性随机 token 中间件, 校验用常量时间比较(照抄 `supervisor/server.rs` 的 `ct_eq`); 失败一律 `401` 且响应体不含任何会话内容 —— D3 / FR-002
- [x] T012 [地基] 新建 `crates/ricow/src/web/sink.rs`: `WebSink` 实现扩展后的 `SessionSink` —— 助手增量 → SSE 帧, 宿主行 → 带 `Severity` 的 SSE 帧; 用户输入经 channel 进入; `secret` 用 `tokio::sync::oneshot` + 阻塞等待, 保持"`None` = 放弃本次修改且不写文件" —— D5 / R3 / FR-012 / FR-015
- [x] T013 [地基] `crates/ricow/src/web/mod.rs`: SSE 端点 —— 复用 `provider::ask_stream`(第 137-182 行)的流式出口, 不新写 LLM 调用路径; 输出帧结构含增量文本与级别 —— D2 / D5 / FR-008 / FR-011
- [x] T014 [地基] `crates/ricow/src/web/mod.rs`: 用户输入端点(把浏览器一行送入 REPL 循环) + 会话 CRUD API + 语言读写 API —— FR-007 / FR-011 / FR-029

## 阶段 3 用户故事 P1: 浏览器内完成对话(MVP)

- [x] T015 [P1] `crates/ricow/src/commands/web.rs` 的 `run()`: 复用 `onboard::run_if_needed`(首次向导, 与 `chat.rs:48` 同一路径)与 `config_file::permission_warning`; 建 `Database` + `ChatSession`(以前端能力声明打开); 起 axum; 输出可点击 URL(含 token); 非 `--no-open` 时打开系统默认浏览器; 无 AI 密钥时在浏览器内给出可操作提示 —— FR-003 / FR-004
- [x] T016 [P1] `crates/ricow/src/web/assets/app.js`: SSE 消费 + **流式增量**渲染 + 自动滚底(用户手动上滚时不强制拉回) —— D2 / FR-008
- [x] T017 [P1] `crates/ricow/src/web/assets/{index.html,app.js}`: 输入区 `Enter` 发送 / `Shift+Enter` 换行; 空输入不发送; 回复期间输入区禁用并显示"正在思考" —— FR-009 / R11
- [x] T018 [P] [P1] `crates/ricow/src/web/assets/app.js`: 斜杠命令(`/history` `/lang` `/keys`)在输入区可用; 宿主菜单渲染为**可点击列表**, 点击等价输入对应序号 —— FR-010 / FR-011

## 阶段 4 用户故事 P2: 多会话历史

- [x] T019 [P2] `crates/ricow/src/web/assets/{index.html,app.js}`: 左侧会话列表(标题 + 最后活动时间) + 新建 / 切换 / 删除(删除需二次确认) —— FR-007
- [x] T020 [P2] `crates/ricow/src/web/assets/app.js` + `crates/ricow/src/web/mod.rs`: 切到某会话时加载并渲染其全部消息(顺序与内容一致) —— FR-021
- [x] T021 [P2] `crates/ricow/src/web/store.rs` + `crates/ricow/src/ai/session.rs`: 恢复旧会话时装入**最近 20 轮**往返作为 `ChatSession.history`, 单条按既有 `clip_for_history`(第 789-800 行)上限截断 —— D10 / FR-020

## 阶段 5 用户故事 P3: 分级着色与术语解释

- [x] T022 [P3] `crates/ricow/src/web/assets/{app.js,style.css}`: 按 `Severity` 四种级别给不同颜色(普通 / 提示 / 警告 / 错误) —— D8 / FR-023
- [x] T023 [P3] 新建 `crates/ricow/src/web/terms.rs`: 术语表 `&[(key, zh, en)]`, key 与 `commands/mod.rs` 的 `format_backtest_report`(第 913-1006 行)指标文案对齐; 解释为一句通俗说明 + 必要口径 —— D13 / FR-024 / FR-026
- [x] T024 [P] [P3] `crates/ricow/src/web/terms.rs` 单测: 双向核对 —— `format_backtest_report` 的每个指标名在表中存在, 且每 key 的中英文均非空 —— R7 / SC-009
- [x] T025 [P3] `crates/ricow/src/web/assets/app.js`: 术语/指标名可点击 → 弹出**当前语言**的解释浮层 —— FR-024
- [x] T026 [P] [P3] `crates/ricow/src/web/assets/app.js`: 回测报告结构化渲染(指标名可点击, 口径与 `format_backtest_report` 同源); 体验增强(策略列表查询等)必须只读 —— FR-025 / FR-027

## 阶段 6 用户故事 P4: 双语

- [x] T027 [P4] `crates/ricow/src/web/assets/{index.html,app.js,style.css}`: 右上角语言切换控件 + 前端单一文案字典; 文案不允许中英叠排 —— D11 / FR-028 / FR-031
  > 实机走查(2026-09-19)修: `applyLang` 取 `dataset` 键名用了大写开头(`"En"` / `"Zh"`), 而 `dataset` 键名是**首字母小写**驼峰(`data-zh` → `dataset.zh`)→ 全部取到 `undefined`, 静态标签变空串(按钮整片空白)、`placeholder` 变字面量 `"undefined"`; 改为 `const suffix = state.lang;`。`web/mod.rs` 补断言 `APP_JS.contains("const suffix = state.lang;")`。
- [x] T028 [P4] `crates/ricow/src/web/mod.rs`: 语言读写走 `config_file::UiSection`(第 65 行)与 `config_file::set_values`(第 381 行, 保注释); 与 CLI 共用同一 `[ui].lang`; 非法值按既有规则硬失败 —— D12 / FR-029 / FR-031
- [x] T029 [P4] `crates/ricow/src/web/mod.rs` + `crates/ricow/src/ai/session.rs`: 切语言后**立即生效**(复用 `rebuild_llm` 第 418-440 行语义重建 LLM 与提示词语言), 不重启服务 —— FR-030
- [x] T030 [P] [P4] grep 走查: 新增宿主文案零硬编码中文裸串(一律走 `i18n::t` / `tf`); 前端文案全部来自字典 —— R8 / FR-031

## 阶段 7 用户故事 P5: 启动脚本

- [x] T031 [P5] `packaging/` 新增 Windows Web 启动脚本: 复用既有 `启动-ricow-AI助手.cmd` 的三级二进制查找与 `cd /d "%RICOW_CWD%"`, 只把启动行换成 `ricow web` —— D14 / FR-033
- [x] T032 [P] [P5] `packaging/` 新增 macOS Web 启动脚本(`.command`): 同上, 复用既有 `.command` 查找逻辑 —— D14 / FR-033
- [x] T033 [P] [P5] `packaging/` 新增 Linux Web 启动脚本(`.sh`): 同上, 复用既有 `.sh` 查找逻辑 —— D14 / FR-033
- [x] T034 [P5] 逐字节比对既有三个启动脚本零改动并记录 —— D14 / D16 / SC-002

## 阶段 8 打磨与验收

- [x] T035 [打磨] 集成/单测: 无 token 与错 token 返回 `401` 且响应体无会话内容 + 监听地址断言 `127.0.0.1`(SC-005); 会话往返持久化与级联删除(SC-006); 30 轮旧会话恢复数为 20 且超长条截断(SC-007); `Severity` 显式标注(SC-008)
- [x] T036 [打磨] `/keys` 在 Web 端可录入, 且明文密钥不出现在对话流与浏览器 localStorage —— FR-012 / SC-011
  > 实机走查(2026-09-19)修: 密钥遮蔽原写 `els.input.type = "password"`, 但输入区是**多行 `<textarea>`**, 其 `type` 是只读 getter → `TypeError: Cannot set property type of #<HTMLTextAreaElement> which has only a getter`, 在 `"use strict"` 下打断调用方, 页面一打开(`openSession`)即报「请求失败」; 改用 CSS 类(`setMasked` + `#input.masked { -webkit-text-security: disc }`)。同时修掉密钥期空回车被 `!value.trim()` 挡下 —— 提示语承诺"回车放弃"、服务端 `read_secret` 也按空值不改配置, 守卫改为 `(!value.trim() && !state.secret)`。`web/mod.rs` 补反向断言 `!APP_JS.contains("input.type =")`(原断言把 bug 当成了期望值, 全绿而运行期必崩)。
- [x] T037 [打磨] 三门禁: `cargo fmt --all -- --check` 零差异 + `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告 + `cargo test --workspace --locked` 全绿且 passed **≥ 403** / ignored 21(不得倒退) —— SC-001
- [x] T038 [打磨] 终端 CLI 回归走查: 默认 `ricow` 进终端对话; `ricow ai "..."` 单次模式; `ricow approve` 逐字短语门禁不变 —— FR-032 / FR-034
- [x] T039 [打磨] 文档同步: `specs/architecture.md`(Web 接入点, 并更新 `session.rs` 模块头表述) / `specs/product.md` / `specs/roadmap.md` / `README.md` / `README_zh.md` —— D19 / SC-013
  > 已落笔: `architecture.md` §二 Workspace 表 `ricow` 行 + §五 新增「Web UI 接入点(025)」块(token 门禁 / 与 CLI 同源 / 第二个 sink / 输入与密钥通道 / 历史持久化 / 前端三件套与双语 / 不做清单); `session.rs` 模块头「将来接网页端」改为「网页端已落地(025)」; `product.md` §二 主用户入口与差异注 + §五 标题/图/要点; `roadmap.md` 当前基线 470/0/21 与 025 备注块; `README.md` / `README_zh.md` 各补 Web 启动方式(手动解压段 + 源码检出段)与独立小节(序号 1-8 连续, 已 grep 校验)。
- [ ] T040 [打磨] 端到端真实调用(宪法三, **禁 mock 交易所**): 双击 Web 启动脚本 → 浏览器自动打开 → 真实提问得流式回答(SC-003); 浏览器内完成一次写操作(部署或启动测试网), 回口语词后**真实执行**(SC-004)
  > 实机走查(2026-09-19)修: 浏览器内对已部署策略发起 `start_dry_run` 并回口语词「确认」后, 执行阶段报 `invalid argument: daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start` —— 该次确认已被消耗, 用户白输一次。根因是 `prepare_*` 在渲染确认块**之前**没有校验 daemon 可达: `commands/instances.rs` 的 `views()` 在 daemon 不可达时回退本地台账并把 `running` 置 `false`, 于是「当前未运行」这项校验误判通过 → 确认块照发, 直到 `execute_confirmed` 才撞墙(破坏「确认块只发给能执行的动作」这一既有原则, 同 `prepare_start_demo` 的凭据前置)。
  > 修: `crates/ricow/src/ai/tools.rs` 新增私有 `ensure_daemon_running`(探活口径与 `supervisor::client::Client::connect` 一致: 读 `run/daemon.json` + TCP 连接), 在 `prepare_start_dry_run` / `prepare_start_demo` / `prepare_start_live` 的**各校验最前**调用; 不可达时明确给出「本次**没有登记任何待确认动作, 你的确认词没有被消耗**」与恢复动作(`ricow daemon start`)。
  > 边界(刻意不改): `prepare_stop` / `prepare_restart_live` 要求「正在运行」(daemon 不在时 `views()` 已如实拒绝); `prepare_update_params` / `prepare_delete_strategy` 仅在「原模式且运行中」才经 daemon, 而「正在运行」蕴含 daemon 在线。
  > 回归: 新增 `ai::tools::tests::daemon_down_blocks_start_actions_before_confirmation`(无 `run/daemon.json` 与「陈旧 `daemon.json` 指向无人监听端口」两种, 三个启动动作均须在发确认块前被拒且 slot 保持 `None`); 既有 `r3s6_start_demo_gates_in_order` / `fr024_start_dry_run_prepare_mentions_gate_clock` 补「在线 daemon」替身(须按协议应答 `List`, 只 bind 不 accept 会让 `views()` 挂住)。
  > 门禁: `cargo fmt --all -- --check` 零差异; `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告; `cargo test --workspace --locked` 472 passed / 0 failed / 21 ignored。

## 依赖与执行顺序

- 阶段 1 → 阶段 2 严格顺序(T002 阻塞 T011/T015; T001 阻塞 T011)。
- 阶段 2 内部: T004 → T005 → {T006, T008}; T007 依赖 T005; T009 → T010; T011/T012/T013/T014 依赖 T005/T007。
- 阶段 3 依赖阶段 2 全部(T013/T014/T015); 阶段 4 依赖 T010/T014; 阶段 5 依赖 T008(级别)与 T013(帧结构); 阶段 6 依赖 T014(语言 API)与 T028; 阶段 7 仅依赖 T015(启动行 `ricow web` 存在即可并行开工)。
- 阶段 8 依赖全部前置。

## 并行示例

- 第一批可并行: T003 与 T001/T002(前端骨架不依赖后端)。
- 阶段 2 可并行: T009+T010(数据层) 与 T011(服务骨架) 互不依赖; T004/T005 与 T009 可并行。
- 阶段 7 三条脚本 T031/T032/T033 完全并行。
- 阶段 5 的 T024(术语表单测)与 T025/T026(前端)可并行。

## 实施策略

- **MVP 优先**: 阶段 1-3 完成即为可交付 MVP —— "点击启动 → 浏览器里流式对话 → 全部能力可达(含写操作确认)"。
- **增量交付顺序**: P1(MVP) → P2(多会话) → P3(着色与术语) → P4(双语) → P5(启动脚本)。
- **零回归红线**: T006 / T034 / T037 / T038 是四道回归闸, 任一不过即阻断交付。
- **不改既有代码的边界**: 除 `session.rs`(trait 扩展, 本变更核心接缝) / `chat.rs`(仅签名放宽 + trait 新方法实现) / `main.rs`(仅新增枚举变体与分支) / `db.rs`(仅幂等追加新表与新方法) 外, 其余既有文件不改。
