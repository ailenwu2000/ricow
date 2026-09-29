# 实施计划: 032 Web UI 工作台化

**分支**: `032-web-ui-console` | **日期**: 2026-09-26 | **规格**: [spec.md](spec.md)

**输入**: `specs/changes/032-web-ui-console/spec.md` 的功能规格

## 摘要

把 Web UI 从"对话 + 只读面板"扩展为五视图工作台:对话(保留)/ 市场 / 策略 / 运行 / 设置。后端在现有 axum 服务上新增三组接口(密钥配置、市场行情、策略管理与运行控制),前端把单文件 `app.js` 按视图拆为多文件 + hash 路由,内嵌 lightweight-charts 渲染 K 线。策略 Web 保存复用引擎落盘内核(抽函数),免去 preview/approve 链但保留编译门禁;策略启停复用 daemon 控制链,live 保持双道逐字短语。

## 技术上下文

**语言/版本**: Rust(workspace,edition 见 Cargo.toml);前端 vanilla JS(无构建、无 npm)

**主要依赖**: axum/tokio(既有)、sqlx(既有 SQLite)、mlua(策略编译门禁,既有)、rig(LLM,既有 `ai::provider`);新增第三方前端文件 lightweight-charts UMD(本地 vendored,约 45KB,经 1080 代理下载)

**存储**: 本机 `ricow.toml`(密钥/视野配置,0600)、SQLite(会话/成交/PnL,既有)、`strategies/` 目录(lua+toml)、`run/`+`logs/`(daemon 台账与日志,既有);回测任务仅内存态(不建表)

**测试**: `cargo test`(纯逻辑单测,含架构守卫);demo/live 交易路径 testnet 真实调用(宪法原则三,禁 mock)

**目标平台**: Windows / macOS / Linux 桌面本机;服务只绑 127.0.0.1

**项目类型**: 本地 CLI + 内嵌 Web 服务单二进制

**性能目标**: 市场列表走既有 600s 缓存;K 线单次 ≤200 根;回测异步(单并发),60s 内出常规报告

**约束**: 完全本地化无 CDN;全部端点 token 门禁;终端渠道行为零回归(025 FR-032/FR-034)

**规模/范围**: 约 14 个新 HTTP 端点;前端 1 拆 7 + 5 视图;约 5 个阶段

## 宪法检查

*门禁: Phase 0 前初判;Phase 1 设计后复查(见文末)。*

| 宪法条目 | 结论 | 说明 |
|---|---|---|
| 一、完全本地化 | ✅ | 行情走交易所公开 API;图表库本地嵌入;无遥测/CDN |
| 二、Lua 策略 + 架构铁律 | ✅ | 引擎/绑定层不读 manifest;Web 只做编辑器与表单;参数仍由 Lua 自读;不改架构守卫 |
| 三、testnet 真实调用 | ✅ | demo 启停与成交验收在 demo-api/demo-fapi 实测;纯逻辑(脱敏/作业表/命名/视野过滤)走单测 |
| 四、产物中文 | ✅ | 全部文档与注释中文 |
| 五、YAGNI | ✅ | K 线首期只读;编辑器纯 textarea;回测任务不落库;不引前端构建链 |
| 安全要求:写操作确认 | ⚠️→修订 | **本变更显式修订**:确认渠道 2→3(+Web)。依据写入 spec FR-027/028,同步改宪法"安全要求"章与 025 档案注记;live 双道逐字短语不放宽 |
| 安全要求:部署链 | ⚠️→修订 | Web 亲手编辑路径免 preview/approve(用户本人即作者),保留编译门禁;AI 产物须用户显式保存;终端/对话链零改动 |
| SDD 流程 | ✅ | 本档案即 032 变更档;收敛时复核 architecture/roadmap 测试基线 |

## 已拍板决策

- **D1(确认渠道扩 3 渠道)**: Web 渠道写操作(保存策略、启停 dry/demo、存密钥)确认 = 页面显式交互(提交按钮+确认对话框),不绕道对话;live 仍逐字短语两道(风险披露一次性 + 每次启动)。理由:023 已有"确认分渠道"先例;真正风险源是模型擅自动作,而 Web 直控没有 LLM 中间人。修订登记见 spec FR-027~029。
- **D2(前端不引构建链)**: 继续 vanilla JS,按视图拆 7 文件 + hash 路由,全部 `include_str!` 嵌入。否决 Vite/React:违背 D4 无构建、新增 Node 工具链,收益不抵成本。
- **D3(图表库 vendored)**: lightweight-charts UMD 存 `assets/` 编译期嵌入。否决自绘:蜡烛图交互(缩放/十字线)自绘成本高;否决 CDN:违宪。
- **D4(策略直接落盘)**: Web 保存调引擎新抽的 `write_strategy_files`(从 `execute_strategy` 抽出的纯文件内核,含备份/回滚/路径安全);保留 `create_strategy` 编译门禁;免去 preview/approve。运行中策略拒绝保存(409)。
- **D5(AI 修改不落盘)**: `/ai-edit` 只返回编译校验后的代码;用户点保存才写盘。保持 019 R5"LLM 无写实工具面"边界。
- **D6(回测内存作业表)**: `Arc<Mutex<HashMap<job_id,..>>>`,单并发,结果保留 5 分钟;不建 SQLite 表(重启丢失可接受,回测可廉价重跑)。
- **D7(daemon 自动拉起)**: 抽 `daemon.rs::start()` 内核为 `pub(crate) async fn ensure_daemon(root) -> CoreResult<()>`(无 println,错误返回值);`ricow daemon start` 终端输出逐字不变。
- **D8(市场列表直连既有入口)**: Web 调 `pairs::current_view(root, force_all)` + `ricow_engine::filter_view(view, market, q)`(两者均现成;注:规划审核中一度以为 filter_view 不存在,实测 `commands/pairs.rs:17` 已 re-export)。
- **D9(内置保留名)**: `catalog::is_builtin_id` 当前私有,提升为 `pub(crate)`;Web 保存命名校验 = `validate_new_name`(前缀冲突)+ `is_builtin_id`(保留名)。

## 项目结构

### 文档(本次功能)

```text
specs/changes/032-web-ui-console/
├── spec.md / plan.md / tasks.md
├── research.md            # 决策依据(D1~D9 详述)
├── data-model.md          # 接口数据结构与状态
├── contracts/
│   ├── http-api.md        # 新增 HTTP 端点契约
│   └── frontend-views.md  # 前端视图/导航契约
├── quickstart.md          # 端到端验证手册(含 testnet)
└── checklists/
```

### 源代码(仓库根)

```text
crates/ricow/src/
├── web/
│   ├── mod.rs                 # 路由注册 + handler(按组拆子模块)
│   ├── keys.rs          (新)  # /api/config/keys
│   ├── markets.rs       (新)  # /api/markets*
│   ├── strategy_io.rs   (新)  # 策略保存/读取/ai-edit
│   ├── runs.rs          (新)  # 启停/状态/risk-ack
│   ├── backtest_jobs.rs (新)  # 内存作业表 + 异步执行
│   └── assets/
│       ├── index.html  (改)   # 导航 + 视图容器
│       ├── style.css   (改)
│       ├── app.js      (薄)   # 入口
│       ├── common.js / router.js / chat.js
│       ├── settings.js / markets.js / strategies.js / runs.js (新)
│       └── lightweight-charts.js (新,vendored UMD)
├── commands/
│   ├── daemon.rs (改)  # 抽 ensure_daemon
│   ├── backtest.rs(改) # 抽 run_backtest_core(CLI/web 共用报告内核)
│   └── ctrl.rs   (改)  # start_daemon/stop_daemon/live_preflight 改 pub(crate) 供 web 调
├── ai/provider.rs (改) # 新增 quick_ask 薄封装(connect+ask)
└── strategies/catalog.rs (改) # is_builtin_id 提 pub(crate)

crates/ricow_engine/src/strategy.rs (改)
└── execute_strategy 抽出 write_strategy_files(config, dir, allow_replace)
```

**结构决策**: 不新增 crate;Web handler 按业务组拆同目录子模块(与 sink/store/tail/terms 同风格);引擎只抽函数不加接口。

## 实施阶段(与 tasks.md 对齐)

1. **阶段 1 框架+设置页**: vendored 图表文件;前端 1 拆 7 + hash 路由 + index.html 布局(对话视图零回归);`keys.rs` 两端点;settings 页
2. **阶段 2 市场**: `markets.rs`(列表/详情/orderbook/klines);markets 页 + K 线
3. **阶段 3 策略管理**: 引擎抽 `write_strategy_files`;`strategy_io.rs`(列表沿用旧端点、读源码、保存、ai-edit);strategies 页(复制/新建/编辑/AI 改写)
4. **阶段 4 回测+运行**: backtest.rs 抽内核 + `backtest_jobs.rs`;`ensure_daemon`;`runs.rs`;runs 页(双道 live 短语)
5. **阶段 5 打磨收敛**: 错误态/i18n/单测;宪法与 architecture/roadmap 同步;converge

## 复杂度追踪

| 违规项 | 为何需要 | 被否的更简替代方案 |
|--------|----------|--------------------|
| 修订"写操作必须对话确认"(025 FR-016/027) | 用户明确要页面直控工作台;密钥/启停/保存每次回对话流不可用 | 全部仍走对话 —— 不满足需求,且 Vibe-Trading 对照方案即直控 |
| 前端 1 拆 7 | 单文件承载 5 视图将超 1500 行难维护 | 保持单文件 —— 可读性与回归风险不可接受 |
| Web 免 preview 链直存策略 | 用户亲手编辑即作者兼确认者,preview 链为防"AI 擅自落盘"而设 | 沿用三步链 —— 页面操作繁琐且 preview 存储模型面向对话 |

## Phase 1 后宪法复查

- 架构守卫 `architecture_guard.rs` 不被触及(manifest 仍只在表现层消费) ✅
- 终端确认/部署链代码路径不改(只新增 web 调用方;抽函数保持原行为,由既有单测兜底) ✅
- 修订落地物:宪法安全要求章增补"Web 渠道"条款;025 spec 顶部加修订注记指向 032;architecture.md Web 章节更新 —— 均列入阶段 5 任务,收敛时逐条核对
