---
description: "032 Web UI 工作台化任务清单"
---

# 任务: 032 Web UI 工作台化

**输入**: `specs/changes/032-web-ui-console/`(spec/plan/research/data-model/contracts/quickstart)

**测试纪律**: 纯逻辑写 `cargo test` 单测;交易链路(demo 启停/成交)必须 testnet 真实调用,禁 mock(宪法原则三)。

## 路径约定

- 后端: `crates/ricow/src/web/*.rs`、`crates/ricow/src/commands/*.rs`、`crates/ricow_engine/src/strategy.rs`、`crates/ricow/src/strategies/catalog.rs`
- 前端: `crates/ricow/src/web/assets/*`
- 全部新端点注册在 `crates/ricow/src/web/mod.rs` 的 `router()` 内、`require_token` 层之前

---

## 阶段 1: 搭建(共享前端地基)

**目的**: 图表库落地 + app.js 拆分 + hash 导航骨架;完成后对话视图行为零变化。

- [X] T001 经本机 1080 http 代理下载 lightweight-charts UMD standalone production,存入 `crates/ricow/src/web/assets/lightweight-charts.js`(实际直连 unpkg 下载成功,代理端口未监听);保留文件头 Apache-2.0 许可注释;在 `crates/ricow/src/web/mod.rs` 加 `LWC_JS` 常量、`lwc_js` handler 与 `/lightweight-charts.js` 路由(token 层内)
- [X] T002 从 `crates/ricow/src/web/assets/app.js` 抽出公共部分到 `crates/ricow/src/web/assets/common.js`(`api()` 带 Bearer 封装、TEXT 中英字典、DOM/i18n 工具),全部挂 `window.Ricow` 单命名空间
- [X] T003 新建 `crates/ricow/src/web/assets/router.js`:监听 hashchange,解析 `#chat|#markets|#strategies|#runs|#settings` 及二级 `#markets/{symbol}`、`#strategies/{id}`,按 `.view.active` 切换视图,提供各视图 activate/refresh 钩子;刷新恢复原视图
- [X] T004 [P] 改造 `crates/ricow/src/web/assets/index.html`:`#sidebar` 增 `#nav` 五项导航(对话/市场/策略/运行/设置,带 data-zh/data-en),`#main` 内建五个视图容器;现有三区 DOM 迁入 `#view-chat`;按序引入 common→router→lightweight-charts→chat→(后续视图 js)→app
- [X] T005 [P] 扩充 `crates/ricow/src/web/assets/style.css`:导航项与高亮、`.view`/`.view.active`、通用表格/表单/模态/卡片样式(深色系,沿用 025 FR-023)
- [X] T006 把现有对话/面板逻辑整体迁入 `crates/ricow/src/web/assets/chat.js`(会话列表/SSE/输入/三个只读面板/语言切换,逻辑零改写);`app.js` 瘦身为入口(初始化+首跳);`web/mod.rs` 为 common/router/chat/app.js 各注册静态路由
- [X] T007 `cargo test` 与手工验证:对话视图全部既有行为(新建/切换/删除会话、流式回复、斜杠命令、面板、日志流、中英切换)零回归;错误 token 取新静态资源全部 401(代码层 39 个 web 测试全绿;浏览器手工回归留待 T037)

**检查点**: 五导航可切换(新视图为占位),对话与改版前一致。

---

## 阶段 2: 用户故事 1 - 页面配置密钥(优先级: P1)🎯 MVP

**目标**: 设置页配置币安/demo/AI 三组凭据,脱敏回显,与终端同一配置文件。

**独立测试**: quickstart 场景 1;保存后查 ricow.toml、刷新只见尾号、对话 AI 立即可用。

- [X] T008 [P] [US1] 新建 `crates/ricow/src/web/keys.rs`
- [X] T009 [US1] 在 `crates/ricow/src/web/keys.rs` 实现 `POST /api/config/keys`(204,空密钥不改、bool 容错、白名单原子落盘)并注册路由;WRITABLE 提 pub(crate) 共用
- [X] T010 [US1] keys.rs 单测 7 个(hint 三态/过滤转换/白名单/不泄明文/空密钥不覆盖)+401 矩阵(GET/POST 空体不回显)
- [X] T011 [P] [US1] 新建 `crates/ricow/src/web/assets/settings.js` 并注册 R.views.settings:三组密钥卡片+AI 文本字段+show_all_pairs 复选,脱敏占位,保存行内反馈
- [X] T012 [US1] settings 双语文案、深色样式、R.api 兼容 204;构建/clippy/235 测试全绿(浏览器手工验证留待 T037)

**检查点**: 不碰终端即可配好全部密钥,对话即时可用。

---

## 阶段 3: 用户故事 2 - 浏览市场与 K 线(优先级: P1)

**目标**: bStock 默认过滤的现货/合约列表、搜索、视野开关;详情页订单簿 + K 线。

**独立测试**: quickstart 场景 2;列表与 `ricow pairs` 一致,K 线渲染。

- [X] T013 [P] [US2] 新建 `crates/ricow/src/web/markets.rs`:`GET /api/markets`(query all/q/market),调 `commands::pairs::current_view(root, all)` + `ricow_engine::filter_view`(现成,见 research D8),返回 `{spot,futures,filtered}`
- [X] T014 [US2] 在 `crates/ricow/src/web/markets.rs` 实现 `GET /api/markets/{symbol}/orderbook?market=&depth=`(market 必填仅 spot|futures;走 `commands::bn_exchange()?.get_orderbook`,depth 1..50 默认 20)与 `GET /api/markets/{symbol}/klines?market=&interval=&limit=`(现货 `BinanceClient::get_klines`、合约 `FuturesDataClient::get_klines`;interval 仅 1m/5m/15m/1h/4h/1d;limit 1..500 默认 200;Decimal 序列化为字符串);注册三路由
- [X] T015 [US2] 单测:interval/market/depth/limit 参数校验的纯解析函数(非法值 400);行情上游错误映射为统一错误体;端点 401 保护
- [X] T016 [P] [US2] `crates/ricow/src/web/assets/markets.js` 列表视图:现货/合约两组表、视野标注与计数、搜索框(两组同步子串过滤)、"显示全部"开关(调 US1 配置端点持久化后重取)
- [X] T017 [US2] `markets.js` 详情视图:面包屑返回、depth=1 取 mid 现价、买卖盘渲染、`window.LightweightCharts` 蜡烛图(周期按钮 15m/1h/4h/1d,默认 1h 200 根);加载态/失败卡片+重试,不展示伪数据;index.html 引入
- [X] T018 [US2] 手工验证 quickstart 场景 2(公开接口免 key;视野与 CLI 一致;断网错误态) — 浏览器实测通过(2026-09-29): 默认 bStock 视野「过滤视野 · 现货 78 · 合约 168」、显示全部「全部视野 · 现货 1367 · 合约 776」双向切换、搜索 AAPL 两组同步过滤(现货 1 AAPLBUSDT/合约 1 AAPLUSDT)、BTCUSDT 详情现价=买卖一中间价 82970.855、买卖盘各 20 档、TradingView 蜡烛图画布渲染、15m 切换重载重建; 断网错误态未实测(失败卡片+重试以 markets.js 实现与代码为准), 证据见 quickstart.md 执行记录

**检查点**: 选标的与看盘闭环可用。

---

## 阶段 4: 用户故事 3 - 策略管理(复制/新建/编辑/AI/回测)(优先级: P1)

**目标**: 浏览器内完成策略产生→校验→回测→保存闭环;AI 不直接落盘。

**独立测试**: quickstart 场景 3;磁盘文件、编译错误、AI 草稿、回测报告。

- [X] T019 [US3] 在 `crates/ricow_engine/src/strategy.rs` 把 `execute_strategy` L122-174 的文件内核抽为新 `pub async fn write_strategy_files(config: StrategyConfig, dir: &Path, allow_replace: bool) -> CoreResult<DeployedStrategy>`(含名校验/market 校验/备份/lua+toml 写入/失败回滚);`execute_strategy` 改为 consume 后委托调用,行为不变;跑通该文件既有全部测试
- [X] T020 [P] [US3] `crates/ricow/src/strategies/catalog.rs` 将 `is_builtin_id` 提升为 `pub(crate)`(不新写第二份保留名清单)
- [X] T021 [US3] 新建 `crates/ricow/src/web/strategy_io.rs`:`GET /api/strategies/{id}/source` 返回 `{id,market,lua,instance_toml}`(内置 instance_toml=null,未知 404);`POST /api/strategies` 按 data-model §5 执行校验链:`validate_strategy_name`→`create::validate_new_name`(前缀)→`is_builtin_id`(保留名)→`instances::views` 运行中 409→`create_strategy` 编译门禁(set market)→引擎 `write_strategy_files`(新建 allow_replace=false/保存 true);错误码 reserved/prefix/running/compile(尽量带行号);注册路由
- [X] T022 [US3] 为 T021 的可纯测部分(参数/命名/保留名/覆盖判定)在 `strategy_io.rs` 抽纯函数并补单测;编译失败返回结构测试;端点 401
- [X] T023 [US3] 在 `crates/ricow/src/ai/provider.rs` 新增 `quick_ask(root, instruction, lua_code, manifest_summary) -> CoreResult<String>` 薄封装:`ai::config::resolve`+`ai::config::api_key` 取配置(未配 key 返回特定错误)→`build_client`+`connect().ask()`(非流式,max_tokens 充足);prompt 组装为独立纯函数 `build_edit_prompt`(含 manifest 参数说明+完整代码+指令,要求只回完整 Lua 无围栏)
- [X] T024 [P] [US3] 在 `crates/ricow/src/web/strategy_io.rs` 实现 `POST /api/strategies/{id}/ai-edit`:取源码与清单→`quick_ask`→剥代码围栏(`ricow_engine::extract_code`)→编译门禁校验→200 `{code}`(**不落盘**);403 need_keys(未配 AI key)、400(超时/编译失败带原因);单测 `build_edit_prompt` 含代码与指令且不含任何密钥
- [X] T025 [US3] 在 `crates/ricow/src/commands/backtest.rs` 抽出 `pub(crate) async fn run_backtest_core(spec: BacktestRunSpec) -> CoreResult<String>`(参数解析后的执行内核,返回 `format_backtest_report` 文本);CLI `run()` 改为组装 spec 后调用,输出逐字不变;既有 backtest 相关测试全绿
- [X] T026 [P] [US3] 新建 `crates/ricow/src/web/backtest_jobs.rs`:`BacktestJob{status,report,error,created_at}` + `JobStore`(Arc<Mutex<HashMap>>,running/done/error 单向转移,已有 running → 409 busy,done/error 保留 5 分钟惰性清理);`POST /api/backtest`(spawn_blocking 跑 run_backtest_core,回测参数覆盖写 StrategyConfig.params)与 `GET /api/backtest/{job_id}`(404 规则);注册路由;JobStore 单测(状态转移/busy/过期)
- [X] T027 [P] [US3] 新建 `crates/ricow/src/web/assets/strategies.js` 列表与编辑:现货/合约×内置/用户分组列表;内置[复制]向导读取 source→输新名(实时保留名/前缀校验反馈)→落盘;[新建空白](名+市场+最小 lua 模板);详情页 manifest 参数表单(f64/i64/string/bool/enum)、标的、textarea 代码编辑(Tab 插两空格);保存(冲突/编译错误行内展示,运行中禁用)
- [X] T028 [US3] `strategies.js` AI 修改条(loading→草稿替换+"AI 草稿未保存"标记+撤销还原→手动保存)与回测区(参数/标的/周期/天数表单→发起→1s 轮询→报告 `<pre>`,离开返回后仍可取结果);index.html 引入
- [X] T029 [US3] 手工验证 quickstart 场景 3(落盘文件、拒绝向量、坏 Lua、AI 草稿、回测与 CLI 同口径、运行中 409) — 浏览器实测通过, 证据见 quickstart.md 执行记录

**检查点**: 策略生产闭环(含 AI 辅助与回测)不离开浏览器。

---

## 阶段 5: 用户故事 4 - 运行与监控策略(优先级: P1)

**目标**: Dry Run/demo/live 三模式启停、状态/收益/日志;live 双道逐字短语;daemon 自动拉起。

**独立测试**: quickstart 场景 4;demo 必须 testnet 真实调用,日志域名可核对。

- [X] T030 [US4] 在 `crates/ricow/src/commands/daemon.rs` 抽出 `pub(crate) async fn ensure_daemon(root: &Path) -> CoreResult<()>`(L61-109 内核:陈旧台账清理、spawn `daemon run`、轮询 daemon.json 5s;**无 println**,提示走 tracing/返回错误);`start()` 保留全部原打印包一层,终端输出逐字不变(FR-034)
- [X] T031 [P] [US4] 新建 `crates/ricow/src/web/runs.rs`:`GET /api/runs`(`instances::snapshot` 出实例数组)、`GET /api/strategies/{name}/status`(InstanceView + `db.recent_pnl_snapshots(Some(name),1)`,source 三态如实,Decimal 字符串)、`POST /api/strategies/{name}/stop`(body close_all,复用 `ctrl::stop_daemon`,未运行 400);注册路由
- [X] T032 [US3] T031 的状态组装纯函数补单测(snapshot+pnl 合并、三态 source、缺 pnl 时不伪造);端点 401
- [X] T033 [US4] 在 `crates/ricow/src/web/runs.rs` 实现 `POST /api/strategies/{name}/start`:`Client::connect` 失败先 `ensure_daemon`;mode=dry_run→`ctrl::start_daemon(live=false,demo=false)`;demo→缺 demo 凭据 400 need_keys 否则 start_daemon(demo=true);live→phrase 必须逐字等于 `确认实盘 {name}`(比较用常量时间,同 `supervisor::server::ct_eq` 口径),空/错一律 400 无副作用→`ctrl::live_preflight`(risk 未确认 → 403 `{need_risk_ack,disclosure:ricow_engine::RISK_DISCLOSURE}`;TOML live_enabled 缺失 → 400)→start_daemon(live=true);已运行 409
- [X] T034 [P] [US4] 在 `crates/ricow/src/web/runs.rs` 实现 `POST /api/risk-ack`:`{phrase}` 逐字等于 `确认风险` 才调 `commands::write_risk_ack()` 落盘,不符 400 无副作用;抽期望短语比对为纯函数并补单测(空/错拒、正对受、risk_ack.json 不落脏记录)
- [X] T035 [P] [US4] 新建 `crates/ricow/src/web/assets/runs.js`:实例表(名称/模式/状态/运行时长/标的/净收益/成交数,5s 轮询,页面隐藏暂停);启动对话框(选策略→manifest 参数表单→模式单选);dry/demo 确认按钮(demo 缺凭据置灰+设置链接);live 两步模态(披露全文滚底→输 `确认风险`(已 ack 跳过)→输 `确认实盘 <名>`,旁显期望串);停止确认(可选 close_all);行内日志面板复用 `/api/logs/{name}/stream` SSE;三态文案中英双语;index.html 引入
- [X] T036 [US4] **testnet 真机验证(禁 mock)**:demo 启动后 `logs/<name>.log` 出现 `demo-api.binance.com`/`demo-fapi.binance.com` 真实请求与成交,页面 PnL/成交数更新,停止(含平仓勾选项)成功;live 拒绝路径全验(错短语/未 ack/live_enabled=false);杀 daemon 后页面自动拉起;结果记录进 quickstart 执行记录

**检查点**: demo 全生命周期页面闭环;live 门禁与终端同强度。

---

## 阶段 6: 用户故事 5 - 对话工作流零回归(优先级: P2)

**目标**: 新导航下对话能力与改版前完全一致。

**独立测试**: quickstart 场景 5。

- [X] T037 [US5] 全量回归 `#chat` 视图:会话增删切、流式着色、斜杠命令(/help /history /lang /keys)、对话渠道口语词写确认(025 FR-016 对话侧不变)、三面板与日志 SSE;发现回归就地修复(主要落在 `chat.js`/`router.js`) — 浏览器实测通过(含 /lang 修复、mk/sg id 修复)
- [X] T038 [US5] 视图切换健壮性:各视图 activate 时刷新数据、不可见时停轮询;对话 SSE 跨视图保持、Closed 后重开;刷新恢复 hash(quickstart 场景 5 第 2 条) — 浏览器实测通过(runs 行内日志 SSE、市场视野切换、跨视图往返)

---

## 阶段 7: 打磨与收敛

- [X] T039 [P] 中英文案全量核对(无中英叠排、data-zh/data-en 齐全,025 FR-031);错误态文案(daemon 拉起失败/LLM 超时/磁盘 IO/行情失败)齐 — 脚本核对 8 个 js 的 zh/en 词典逐键一致、全部 t() 调用有词条、index.html data 配对齐全;四类错误文案均有中文前缀(daemon「启动失败: {e}」/AI「AI 调用失败(超时或上游错误)…」/IO 透传 os error/行情「加载失败: 」+重试),多数已浏览器实测
- [X] T040 [P] 扩展既有"前端不存密钥"扫描测试到 common/settings/markets/strategies/runs.js 全部新文件;新静态资源 401 矩阵测试 — 实现期已建齐(扫描 9 个 js + 10 路径×无/错 token),`cargo test --bin ricow web::tests` 20 通过
- [X] T041 治理同步(spec FR-027~029):`specs/constitution.md` 安全要求章增"Web 渠道:页面显式交互确认;live 双道逐字短语"条款与修订记录;`specs/changes/025-web-ui/spec.md` 顶部加修订注记指向 032;`specs/architecture.md` Web 章节更新(视图/端点/前端拆分) — 宪法已升 1.2.0(确认渠道 2→3),025 spec 注记与 architecture §五 032 段(端点描述按真实路由核准)均已落地
- [X] T042 `cargo test` 全绿、`cargo clippy --all-targets -- -D warnings` 干净;按 quickstart 全量走查;更新 `specs/roadmap.md` 测试基线与 032 条目;写 `specs/changes/032-web-ui-console/converge.md` 后 `/speckit-converge` — 全部完成(589/0/22 + fmt/clippy 0 差异; 场景 1-5 全量走查含场景 2 补验; roadmap 589 基线+032 注记; converge.md 已写, 收敛结论「已收敛, 零 actionable findings」)

---

## 依赖与执行顺序

- 阶段 1 是全部故事的地基(拆分/路由/资源管线),必须最先完成且保证对话零回归
- US1(T008-012)→ US2(T013-018):市场视野开关复用 US1 配置端点,故 US1 先
- US3(T019-029)依赖阶段 1;其中 T019(引擎抽函数)→ T021(保存)→ T027-028(前端);T023-024(AI)与 T025-026(回测)可并行
- US4(T030-036)依赖阶段 1;逻辑上用已部署策略(US3 产物)做真机验证,但代码仅依赖实例 TOML,可与 US3 后半并行
- US5(T037-038)在所有视图接入后做
- 阶段 7 最后;T041 治理同步可在阶段 4 完成后即开始

## 并行机会

- T004/T005(前端 HTML/CSS)与 T002/T003 同批不同文件
- 每个故事的后端端点与其 js 文件可前后脚并行(契约已定,见 contracts/)
- US3 内:T020、T023/T024、T025/T026 三条线互不阻塞
- US4 内:T031/T032 与 T033/T034 可并行;T035 前端依赖端点契约定稿即可开工

## 实施策略

MVP = 阶段 1+US1(设置页可用即构成可演示增量);随后按 US2→US3→US4 增量交付,每个检查点跑对应 quickstart 场景并保证 `cargo test` 全绿;US5 与阶段 7 收口。交易类验收严格走 testnet,任何 demo/live 路径禁止 mock Exchange。
