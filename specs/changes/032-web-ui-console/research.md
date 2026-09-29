# Phase 0 调研记录: 032 Web UI 工作台化

所有"待澄清"在规划前已与用户确认(4 项关键决策),本文记录决策依据、被否方案与代码实证。

## D1 Web 渠道写操作确认模型

**决策**: 确认渠道由 2 个(对话/终端)扩为 3 个(+Web)。Web 渠道:普通写操作(保存策略、启停 dry/demo、存密钥)以页面显式交互(确认对话框/表单提交)为确认;live 保持双道逐字短语(风险披露一次性 + 每次 `确认实盘 <名>`)。

**依据**:
- 023-ai-chat-ux 已有"确认分渠道"先例,宪法安全要求章 2026-09-18 修订记录明确"真正的风险源是模型擅自动作,而非用户误回一个字"。
- Web 直控操作没有 LLM 中间人:动作由用户点击发起,不存在"模型擅自调工具"路径。
- 实盘资金风险不可降级:live 两道逐字短语与终端同口径(FR-023),且复用 `require_explicit_phrase` 的期望串构造,不自造校验。

**被否方案**:
- 全部写操作仍绕道对话流:用户明确否决(工作台诉求);且设置密钥/启停在对话流里需多轮交互,不可用。
- Web live 也放宽成单个口语词:资金安全降级,不可接受。

**落地约束**: 025 FR-016/FR-027 必须在 spec 显式修订(FR-027);宪法安全要求章同步加 Web 渠道条款(阶段 5 任务);对话渠道(025 FR-016)与终端渠道(FR-034)代码路径零改动。

## D2 前端不引构建链

**决策**: vanilla JS 按视图拆 7 文件 + hash 路由,`include_str!` 嵌入。

**依据**: 025 D4"编译期嵌入、运行期不依赖工作目录/外部 CDN";分发形态是单文件二进制;团队当前无 Node 工具链。

**被否**: Vite+React —— 引入 Node 构建链与产物管线,违背 YAGNI;收益(组件化)对 5 个视图规模不显著。

## D3 图表库 vendored

**决策**: lightweight-charts UMD standalone production(~45KB)存入 `web/assets/lightweight-charts.js`,经 1080 代理下载,随仓库分发。

**被否**: 自绘蜡烛图(缩放/十字线/周期切换成本高);CDN(违宪一)。
**许可**: Apache-2.0(与项目兼容),落库时在文件头保留许可注释。

## D4 策略直接落盘的引擎复用

**实证**: `ricow_engine::strategy::execute_strategy(db, preview_id, token, dir, allow_replace)` 与 preview/token 强耦合(`confirm::consume`),但其 L122-174 的文件内核(名校验→market 校验→路径拼接→存在性拒绝→备份→写 lua→写 toml 失败回滚)操作的是 `StrategyConfig`,可干净抽出。

**决策**: 抽 `pub async fn write_strategy_files(config: StrategyConfig, dir: &Path, allow_replace: bool) -> CoreResult<DeployedStrategy>`;`execute_strategy` 改为 consume 后调它(行为零变化,既有 5 个 strategy.rs 测试兜底)。
Web 保存链路:构造 `StrategyConfig`(`create_strategy(name, code, pair, params)` 过编译门禁 → set market)→ 前缀冲突(`validate_new_name`)+保留名(`is_builtin_id` 提 pub(crate))→ 运行中检查(`instances::views`)→ `write_strategy_files(allow_replace=true/false)`。

**被否**: Web 也走 preview 表 + approve token —— 多两张临时态与三步点击;preview 模型是为"AI 产出 → 用户事后批准"设计,亲手编辑场景无受益。

## D5 AI 修改不落盘

**决策**: `POST /api/strategies/{id}/ai-edit` 返回编译校验后的代码,前端填入编辑器;只有用户点保存才落盘。保持 019 R5"LLM 无写实工具面"。
LLM 调用:`ai::config::resolve(root)` + `ai::config::api_key(root, provider)`(均现成)→ `provider::build_client`/`connect(...).ask(prompt)`(现成非流式)→ 新增 `quick_ask` 仅为薄封装(组 prompt + 取 Answer 文本)。

## D6 回测作业表内存态

**决策**: `WebState` 增 `jobs: Arc<Mutex<HashMap<String, BacktestJob>>>`;状态 running/done/error;单并发(已有 running 作业时新请求 409);完成条目保留 5 分钟惰性清理。
**被否**: SQLite 持久化 —— 回测廉价可重跑,重启丢失无成本,YAGNI。
**内核复用**: `backtest.rs::run` 当前混合参数解析/打印;抽出 `run_backtest_core(args/spec) -> CoreResult<String 报告文本>` 供 CLI 与 web 共用(报告渲染仍唯一来源 `format_backtest_report`,满足 025 FR-025 同口径;首期 Web 先 `<pre>` 展示文本,结构化渲染沿用对话页既有能力,后续增强)。

## D7 daemon 自动拉起

**实证**: `commands/daemon.rs::start()`(L61-109)= 清陈旧台账→current_exe→spawn `daemon run`(detach、stdout 重定向 daemon.log)→轮询 daemon.json 5s。`Client::connect(root)` 可达性检测现成。

**决策**: 抽内核 `pub(crate) async fn ensure_daemon(root: &Path) -> CoreResult<()>`(无 println;陈旧清理的提示改为日志/tracing,web 侧不可见);`start()` 包一层保留全部终端打印(FR-034 零回归)。web start handler:connect 失败→ensure_daemon→再发 Request::Start。

## D8 市场列表入口

**实证(更正审核中的误判)**: `commands/pairs.rs` L17 已 `use ricow_engine::{build_view, filter_view, PairsView}`;`current_view(root, force_all)` 与 `lookup(root, market, q)` 均 pub(crate) 现成,带 600s 静态缓存。web 直接调 `pairs::current_view`+`filter_view`(或把这两个函数提 `pub(crate)` 可见性,web 模块本就在同一 crate)。

行情明细:订单簿走 `commands::bn_exchange()? -> Arc<dyn Exchange>::get_orderbook(pair, depth)`(market.rs 同款);K 线现货 `BinanceClient::get_klines(symbol, interval, limit)`、合约 `FuturesDataClient::get_klines(...)`(签名一致)。market 分支:前端显式传 `market=spot|futures`,服务端只认这两个值。

## D9 内置保留名

**实证**: `catalog::is_builtin_id(id)`(catalog.rs L139)私有;`catalog::all()` 扫描时对占用保留名已记 problem(L210)。提为 `pub(crate)` 供保存校验;不新写第二份保留名清单。

## 前端资源加载实证

`web/mod.rs` 静态资源三件套经 `include_str!` + 独立 GET handler + token query。新增 8 个文件照同模式注册;index.html 按依赖顺序引 `<script>`(common→router→chat/settings/markets/strategies/runs→app 入口;lightweight-charts 在 markets 之前)。所有文件共享同一全局命名空间,采用 `window.Ricow = {...}` 单命名空间挂载各视图模块,避免 IIFE 散乱。

## 无残留澄清项

四关键问题(live 支持/前端形态/K线方案/保存流程)已在规划前由用户拍板;其余按宪法与既有代码惯例取合理默认。
