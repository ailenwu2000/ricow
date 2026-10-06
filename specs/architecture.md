# ricow 架构文档 v4.1(现状, 纯 CLI 客户端)

> 状态: ✅ 已实施(P1-P3 完成; P4 dogfood 实盘待开始, 进度见 specs/roadmap.md)
> 职责: 本文描述**当前实际架构**(与代码逐份复核, 2026-09-16)。产品定位见 specs/product.md, 里程碑见 specs/roadmap.md。
> 策略 API 唯一权威见 specs/lua-api.md; 回测口径唯一权威见 specs/backtest.md。

---

## 一、目标架构

CLI 单入口直连同一引擎与同一下单路径, 安全模型一致(confirm + 固定下单频率护栏 + Dry Run 默认):

```
ricow CLI (clap)
      │
      ▼
ricow_engine (headless 核心: 命令分发 / 回测 / 确认通道)
      │
      ▼
ricow_strategy (Lua 沙箱 + ctx.* API + exec.* 组件 + 指标 / 回测 / SQLite / 固定 100/s 护栏)
      │
      ▼
ricow_core: Exchange trait ─► ricow_binance
                                    └─ 美股数据源: Nasdaq 官方 API(免 key; 仅日线, 供信号轨/交易日历)
```

## 二、Workspace(5 crate)

| Crate | 角色 |
|:--|:--|
| ricow_core | 核心类型(Order/Position/Balance/Kline…)+ Exchange trait |
| ricow_binance | Binance 现货 REST/HMAC/WS (place_order/cancel/account) + USDT-M 公共数据源 (`FuturesDataClient`: fapi K 线 / 首档 MMR 表) + **fapi 签名交易客户端 `FuturesClient`** (下单/账户/持仓/杠杆/双向持仓, 2026-09-04 testnet 联调新增; 域名 RICOW_BN_BASE_URL / RICOW_FAPI_BASE_URL 可配 demo 测试网) |
| ricow_strategy | 策略引擎: Lua 沙箱 / ctx 与 exec 注册 / 指标(ta)/ 回测 / PnL / SQLite / **固定 100 单·秒⁻¹ 护栏 `order_guard`(2026-09-16 019-R5: 原 `risk.rs` 四条静态限额与装配器已删除)** |
| ricow_engine | headless 核心: Engine 命令分发 / backtest_runner(单标的 + 组合) / confirm(preview+approve)/ loader / market / **美股层 `nasdaq`(Nasdaq 日线客户端) / `us_tickers`(bStock↔美股映射, 70 只快照) / `market_class`(bStock 现货池识别, 通用能力保留: 当前无内置消费者)** |
| ricow | 二进制 `ricow`: clap 子命令分发 + **AI 助手 `ai/`(019: 提示词、工具白名单 L0 只读 + L1 虚拟、审批门 `ToolGuard`; 会话缝 `ai/session.rs`(`ChatSession` + `SessionSink`, 零 stdio); R3/R4 对话内确认状态机 `ai/confirm.rs` 7 动作)** + **Web UI `web/`(025: axum + SSE, 只绑 `127.0.0.1`, 一次性 token; 036 起按端点族拆文件 —— 骨架与装配在 `mod.rs`(服务 `bind`/`router`/`serve`、`Hub`/`WebState`/`WebError`), 端点族各归 `assets`/`auth`(token+来源门)/`sessions`/`settings`/`trades`/`logs`/`strategies` 各自带 `routes()` 与测试, 共享测试脚手架 `test_support.rs`(仅 cfg(test)), `strategy_io`/`runs` 目录化测试体下沉 `tests.rs`)** + 首次向导 `commands/onboard.rs` + 单一配置文件读写 `commands/config_file.rs`(含 `set_values` 白名单 9 键) |

> **内置 AI 助手已落地**(019-ai-assistant): `ricow ai` 调用用户自配的 LLM(`ricow.toml [ai]` provider+api_key)。
> **LLM 接口统一为 rig 0.42 的 OpenAI 兼容通道**(2026-09-16 重审): 7 个预设(deepseek 首项, 默认模型 `deepseek-flash`; 其余为 OpenAI 兼容的主流厂商)+ custom 自定义 base_url, 供应商差异只收敛在 `ai/provider.rs` 一个文件, 无需 Anthropic 等第二通道; 不支持工具调用的模型如实报错, 不自动降级。
> 写实动作**不作工具注册**(L2), 工具面只有只读(L0, 12 个)与虚拟(L1, 4 个)。**R3/R4(2026-09-16)**: L1 的 `request_write_confirmation` 自身不落盘不起进程, 只渲染确认块并在会话内存登记一条 pending(`Arc<Mutex<Option<PendingAction>>>`, TTL 15min); 用户在**同一交互式 tty REPL** 逐字输入短语(裸 y/yes/ok 不认)后, 由**宿主** `ChatSession` 进程内直调既有引擎内核执行 —— **7 动作**: Deploy / StartDemo / StartLive(仍须原样跑 `ctrl::live_preflight` 三判据) / AckRisk / StopDemo / StopLive / CloseLive, 完整对照表见 019 spec §七 R4。模型输出永远不进入执行分支; 改开关/改参数仍须本人终端; 单次模式/管道/外部 agent 只回终端命令。详见 `specs/changes/019-ai-assistant/spec.md` §七 R3/R4。
> **MCP 不做**(2026-09-15 定案, 修订 product.md D12 修订 2): 单机程序, 用户已有的 agent 可直接调用本机 CLI, 生态入口 = `ricow agent-kit` 手册(019 T045–T047)。
> 原 2026-08-24 决策"无 MCP / 内置 LLM"中**内置 LLM 一项已由 019 修订**; MCP 一项维持不做(GUI 与 Telegram 仍不做)。
> Hyperliquid 适配 crate(`locus_hl`)已于 2026-09-12 全量移除(从未落地, 暂不考虑) —— 见 `specs/changes/009-remove-hyperliquid/`。

## 三、进程模型 (008 已实施)

常驻进程管理器 `ricow daemon`(pm2 模型, 零新增依赖; 见 `specs/changes/008-platform-process-model/`):

```
ricow CLI ──本机 TCP(127.0.0.1:随机端口 + token)──▶ ricow daemon (持 N 个子进程)
                                                        │
            stdin 管道(stop 指令 / EOF) ────────────────┼──▶ ricow run <name> (stdout/err → logs/<name>.log)
            try_wait(存活) / 台账 run/<name>.json ───────┘
```

- daemon 退出即中止全部子进程; daemon 被强杀时子进程靠 stdin 管道 EOF **自愈退出**(不依赖 OS 带走子进程)
- 停机 = 向子进程 stdin 写 `stop`(跨平台一致, 不依赖信号语义); 等待上限 30s, 超时如实报错且**不静默强杀**
- 实例视图 = daemon 运行中实例 ∪ 台账(`run/<name>.json`, 含上次退出码/原因) ∪ 已部署清单(`strategies/*.toml`)
- 启动方式: `std::process`(实测 `tokio::process` 在启动器退出时会带走子进程); daemon 自后台化用 `process_group(0)`(unix) / `creation_flags(DETACHED_PROCESS|CREATE_NO_WINDOW)`(windows), 均在 `forbid(unsafe_code)` 下可用
- 不再注册 OS 服务(systemd/launchd/SCM), 无开机自启与自动重启(`packaging/locus@.service` 已删除)
- 运行状态与数据落本地 SQLite(`ricow.db`); 日志落 `logs/<name>.log`(启动时 >10MB 轮转 `.log.1`)
- 实盘运行器已落地(011, 2026-09-13): `Engine::run_live` 与 `run_dry_run` **并列**(账户事实源与停机语义不同, 不合并); 启动装配账户快照 + 交易所过滤器 + 时钟预检 → 双流(盘口 + 用户流) → 成交回写落库 → 停机清理(停消费 → `on_stop` → 撤单兜底 → 可选平仓 → 残留复查, 幂等)
- 合约实盘已落地(012, 2026-09-13): `BnFuturesExchange` 实现 `Exchange`(REST + `fstream` WS 盘口/用户流), 引擎按 `market` 分派装配与持仓语义(one-way / hedge), 停机兜底平仓按市场传 `reduceOnly` / `positionSide`; 时钟预检按市场取数(现货/合约服务器时间不同步, 实测差 1.5~1.9s)
- 盘口流为**增量 diff**(合约与现货 `@depth` 一致): 按价格合并、`size=0` 删档, 严禁覆盖式替换(会缺档致 `price()` 为 None)

## 四、策略层(Lua)

- 策略统一 Lua 5.4(mlua 嵌入式沙箱): 4 回调 `on_init/on_tick/on_fill/on_stop`, `on_tick` 返回订单数组; ctx 为只读快照
- **ctx.\*** API(冒号调用): 行情 / 持仓余额 / 配置参数(config_f64 等)/ 指标(基于已收盘 K 线, 无前视)/ 时间 `now()`(2026-09-11 新增, 供"每日固定时刻动作"的盘中策略)
- **数据供给与分层纪律(030, 2026-09-24 策略/引擎分层收敛)**:
  - **分层铁律(最高约束)**: 引擎(`ricow_engine`)/ CLI(`ricow/src/commands`)/ 绑定层(`ricow_strategy` 的 `context.rs`/`lua.rs`)**零策略参数名** —— 策略参数(如 `atr_interval`/`regime_ema_period`/`atr_mult`)只能由 Lua 策略自己读; 上述各层不得出现策略参数名字面量, 由 `crates/ricow_strategy/tests/architecture_guard.rs` 机械锁死(注入即红, 反向验证通过)。
  - **声明式取数**: 策略在 `on_init` 用 `ctx:need_klines(role, tf, min_bars)` 声明数据需求(`primary` = 主时钟驱动逐 bar / `aux` = 辅助周期供指标), 引擎按声明拉取/重采样供给 —— 对标 QuantConnect `AddEquity` / Freqtrade `informative_pairs`; 引擎不猜 tf、不猜根数。
  - **Time Frontier(无前视)**: 策略永远读不到当前时间点之后的数据 —— 回测由引擎逐 bar 推进 + 可见前缀裁剪, 实盘由引擎按声明预装后逐 tick 供给; 可见性由引擎单方控制, 策略无法绕过。
  - **回测/实盘对称**: 同一份策略代码在回测与实盘零改动可运行, 两路径都走「声明 → 供给 → 读」, 不允许为任一路径在引擎里硬编码。
  - **on_init 幂等约定**: 声明阶段(`on_init` 里的 `need_klines`)只依赖 config, 不依赖 balance/K 线; 回测装配会先跑一次 `on_init` 收集声明再拉数, 故 `on_init` 必须可重复调用。
- **exec.\*** 执行组件(引擎内置 Rust 实现, 加载时注册全局表, 脚本内可覆盖): `levels` / `pullback_triggered` / `detect_quote` / `ticks_per` / `slice_due` / `side_order`
- 内置示例(**编译期 include_str! 嵌入二进制**, 登记表 = `crates/ricow/src/strategies/catalog.rs`; 031 起内置与用户策略统一由 catalog 管理, 现货/合约分 `spot/`/`futures/` 目录):
  - `strategies/spot/shannon_spot_grid.lua` — 香农现货网格(030): 虚拟账本(本金 × 杠杆 1~5, `v_cap` 动态 = 币×现价+现金)决定目标持币量; `start_price` 触发激活(可选 `initial_buy_amount` 建初始仓); 平衡价 ± `atr_mult×ATR` 双边限价网格, 成交即以该价为新平衡价并重挂两侧; 挂单量 = 使账本在该价回到 `target_ratio` 权重; 卖量受真实持仓兜底; 趋势门控可选(`trend_gate`, 默认关); 成本门槛硬校验(R7) + `strategy_state` 断点续接; 仓位清空即结束
  - `strategies/spot/paired_grid.lua` — 现货动态非对称网格(2026-09-24): 固定金额(`order_amount`)配对网格; 价格低于 `start_price` 激活; 以最近成交价为参考价上下各挂一单(下方固定金额买单、上方配对卖单, 配对 = LIFO 保证卖价恒 > 买价); 方向标志(买 −1 / 卖 +1)驱动上下间距不对称放大 `1+|flag|×direction_offset`, 抑制单向成交; 可选建仓(不计 flag)与 `accumulate_mode`(u 积累 U / coin 积累币); 成交全撤重挂 + 追踪(栈空时买单跟随上涨价格); 成本门槛(间距 > 2×单边费) + 断点续接; 仓位清空即结束
  - ~~`strategies/builtin/bs_momentum.lua`~~ — **已于 2026-09-11 删除** (真实 bStock 成交轨期望 ≈0:
    spot 91 天 每 bar −0.0198% / futures 220 天 +0.0367%; 七年 R1 数字含幸存者偏误不作证据);
    同批删除的还有 `bs_intraday_top5.lua`(日内 Top5, 成本算术否决)。证据见 specs/research/。
    **其引擎机制保留为通用能力**: 组合回测路径 `run_portfolio_backtest` + 组合信号模式
    (`universe` 键 / `signal_klines` / `SIGNAL_TAIL` 尾窗) + 任意 interval tick 对齐 `build_interval_ticks`,
    暂无内置消费者; 组合回测 CLI 入口随策略一并删除。
- 用户策略: `<项目根>/strategies/<name>.toml`(实例) + `strategies/{spot,futures}/<id>.{lua,toml}`(源码 + 清单; git 忽略默认私有, 内置示例例外)
- **单一配置文件**(019 D31, R4 修订): `$RICOW_ROOT/ricow.toml`(权限: **Unix 0600 / Windows 无 POSIX 权限位**, 写入时尽力收紧为仅当前用户 ACL —— 对外展示口径统一取 `commands/config_file.rs::permission_summary`, 不得无条件写"0600"; 进 `.gitignore`)—— `[ai]`(provider / model / base_url / max_turns / api_key)、
  `[exchange]`(demo_key / demo_secret / binance_key / binance_secret)与 `[market]`(show_all_pairs)同文件; 该文件**即界面**(无 `ricow keyring` / `ricow setup` / 写凭据命令), 未知键硬失败。
  **R4 起文件可写**: `commands/config_file.rs::set_values` 按行外科替换/缺键插入(**保留注释**、原子写 + 0600), 白名单 9 键, 白名单外一律拒绝 —— 入口是首次向导 `commands/onboard.rs` 与对话内 `/keys` `/market`, 仍无独立"写凭据命令"。
  目录顺序: `RICOW_ROOT` env > 当前目录(含 `ricow.db`/`strategies/`)> 平台数据目录。

## 五、CLI

命令集(**以 `ricow --help` 实测为准**, 2026-10-06):
`start [--demo]` / `stop [--close-all]` / `restart` / `list` / `status [name]` / `info` / `fills` / **`exposure`**(037) / `logs` / `run [--live|--demo]` /
`backtest` / `ticker` / `orderbook` / `pairs [--market] [--all]` / `create` / `approve` / `deploy` / `db` / `daemon {start|stop|status|run}` /
**`ai`**(019: 内置 AI 助手, 交互 / 单次 / `--plain`)/ **`agent-kit`**(019: `ricow agent-kit [--install [目录]]` 生成给外部 agent 的手册 —— AGENTS.md / SKILL.md / CLAUDE.md / lua-api.md, 与内置 AI 同源)/ **`web`**(025: Web UI 模式 —— 启动内置网页, 浏览器里完成全部对话与操作); `mcp` **不做**(2026-09-15 定案)。

**裸入口(019 R4)**: `ricow` 不带子命令 → 直接进对话(`commands::chat`); 缺 AI key 且非本地 ollama 时先走首次向导(`commands::onboard`: 供应商选择 → 静默录入密钥 → 可选连通校验 → 外科式写回 `ricow.toml`), 币安凭据可跳过后用 `/keys demo` 补录。非 tty 一律双语报错 + 打印配置路径, exit 1(不静默降级)。原 clap 子命令全部保留, 变成同一 `ChatSession` 的薄壳。对话内斜杠命令: `/keys [ai|demo|live]` 查看/静默录入密钥(只回显尾 4 位)、`/market [bstock|all]` 查看/切换交易对视野(`[market] show_all_pairs`)。

**Web UI 接入点(025)**: `ricow web` 在 `127.0.0.1` 上起内置 axum 服务(端口 0 = 系统分配空闲端口), 启动时生成**一次性 token**(`uuid` v4, 不落盘、不进日志, 进程退出即失效)并把带 token 的 URL 打印到终端, 同时尝试打开浏览器(Windows `cmd /C start` / macOS `open` / Linux `xdg-open`, 打开失败只 warn 不影响服务)。**全部端点(含静态资源)都在 token 中间件之后** —— 无 token 或错 token 一律 `401` 且**响应体不含任何会话内容**(D3 / FR-002); token 走 `Authorization: Bearer` 或 `?token=`, 页面里 `style.css` / `app.js` 的 URL 由服务端按本次请求的 token 回填。**来源门(036)**: 同一道 `require_token` 中间件内, token 校验之后对**写方法**(`POST/PUT/PATCH/DELETE`)加 `Origin`(缺失退 `Referer`)回环白名单 —— 非回环来源一律 `403` 空体(理由: token 因 `EventSource` 限制出现在 URL 查询串, 可能经 Referer 外流, 来源门挡住"别的站点拿着 token 发写请求"); 读方法不设限(curl/脚本可用), 两者都没有 → 放行(非浏览器请求)。

- **与 CLI 同源(D2 / D5)**: 每个会话线程里跑的仍是 `commands::chat::repl`, 助手增量仍由 `provider::ask_stream` 逐段产出 —— 本层只做 HTTP 骨架、线程登记与帧转发, 不复制任何会话/LLM 路径。
- **会话缝的第二个 sink**: `web/sink.rs` 的 `WebSink` 实现 `SessionSink`, 把宿主输出转成 SSE 帧 `delta{text}` / `line{text,sev}` / `secret_prompt{prompt}` / `turn_end` / `closed`; `Severity` 由宿主显式标注, 前端只按级别着色(不做关键字猜测)。`turn_end` 是唯一轮次分界线, `closed` 在 sink 析构时必发。
- **输入侧**: 浏览器一行 → `POST /api/sessions/{id}/input` → 入站通道 → REPL。密钥走**独立通道**(`InputChannel` 的 `awaiting_secret` 标志 + 专用队列), 绕开普通输入路由; 明文密钥**不落库、不进对话流、不写浏览器存储**。
- **会话历史持久化**: `web/store.rs` 把流水落 `ricow.db`(左侧列表可新建/切换/删除), 重开旧会话按写入顺序整屏回放并恢复最近 **20 轮**作 AI 上下文, 单条超 2000 字截断并标注原文长度。
- **前端三件套编译期嵌入**(D4): `index.html` / `app.js` / `style.css` 经 `include_str!` 进二进制, 运行期不依赖工作目录与外部 CDN; 界面**中英双语**, 语言与 CLI 共用 `ricow.toml [ui].lang`; 量化术语点击弹解释由 `web/terms.rs` 静态表提供。
- **不做(D18)**: WebSocket / 公网访问 / 多用户 / 账号体系 / 图表可视化 / 会话导出 / 前端构建链。

**Web UI 工作台化(032, 2026-09-29)**: `ricow web` 从「浏览器内对话工作台」升级为**多视图工作台** —— 左侧导航 + 五视图(对话 / 市场 / 策略 / 运行 / 设置), 视图间用 **hash 路由**(`#chat` / `#markets` / `#strategies[/{id}]` / `#runs` / `#settings`, 刷新恢复)。上列 D18 中的「图表可视化 / 前端构建链」由本变更推翻并按新口径执行, 其余(WSS / 公网 / 多用户 / 会话导出)仍不做。

- **前端脚本拆分(仍无构建链)**: 三件套拆为 `common.js`(API/双语/工具) + `router.js`(hash 路由) + 五视图模块(`chat.js` / `markets.js` / `strategies.js` / `runs.js` / `settings.js`) + `app.js`(装配), 全部 `include_str!` 编译期嵌入; K 线图用 **lightweight-charts UMD 内嵌**(无 CDN 依赖); 每份脚本 URL 仍由服务端按 token 回填。浏览器持久存储依旧零写入(SC-011 静态扫描锁死)。
- **新增只读/写端点**(全部仍在 token 中间件之后): 密钥与配置 `GET/POST /api/config/keys`(密钥静默写入 ricow.toml 只回显尾 4 位; 通用配置走 `updates` 数组 —— 市场视野开关 `market.show_all_pairs` 同端点); 行情 `GET /api/markets*`(免 key 公共行情 + 盘口 + K 线); 策略 `GET/POST /api/strategies`(POST = 创建/覆盖保存: 编译门禁 + 运行中 409 + 保留名/互前缀拒绝)、`GET .../source`(Lua/TOML 原文)、`POST .../ai-edit`(LLM 草稿, 编译失败 400 带行号)、`POST /api/backtest`(202 拿 job_id 后台跑 CLI 同一内核)+ `GET /api/backtest/{job_id}` 轮询; 运行 `GET /api/runs`、`POST /api/strategies/{id}/{status|start|stop}`(启停复用 CLI 同一内核 `commands::ctrl`, daemon 自举幂等)、`GET /api/logs/{name}/stream`(SSE 行内日志, 与对话视图各一条独立流)。
- **Web 渠道确认规则(宪法 1.2.0, FR-027~029)**: 页面直控写操作的确认 = 页面显式交互(确认对话框/表单提交); live 不降级 —— 启动模态逐字输 `确认实盘 <策略名>`, 首次 live 另须风险披露确认(逐字输 `确认风险`, 落盘 `risk_ack.json`), daemon 侧门禁复用不变。

**密钥管理页(033, 2026-10-02)**: 密钥由「唯一一份生效值」升级为**带别名的密钥环(vault)** —— 左侧导航新增第 6 个一级视图「密钥」(`#keys`, 位于「运行」与「设置」之间), 视图脚本 `web/assets/keys.js`(仍 `include_str!` 编译期嵌入, 无构建链)。

- **数据模型(唯一配置文件 `ricow.toml`)**: 生效值仍是 `[ai]` / `[exchange]` 两段(**字段一字未改**); 新增两个数组表存密钥环 —— `[[ai_key]]`(别名 `alias` · `provider` · `model` · `base_url` · `api_key`)、`[[exchange_key]]`(别名 `alias` · 环境 `env ∈ {live,demo}` · `key` · `secret`)。块级**行式编辑**(不整文件重写): 插入/替换/删除只动目标块, 其余内容(含注释)逐字保留; 复用既有原子写 + 0600 / icacls。
- **语义**: **选用**是显式动作 —— AI 条把四元组写入 `[ai]`, 币安条按 `env` 写入 `[exchange].binance_*`(live)或 `demo_*`(demo); 编辑"使用中"的条目 → 生效值同步跟随(不留旧值); 删除"使用中"的条目 → **同时清空**对应生效字段(幽灵凭据防护); 别名同类内唯一(空/超长 24/控制字符/重复 → 400 带机器码, 未知别名 404)。
- **新增 8 条端点**(全部在 token 中间件之内): `GET /api/keys`(总览: 条目 + `{configured,hint}` 脱敏 + 生效来源标记 `from_entry`/`live_from`/`demo_from` + 服务商预设表)、`POST /api/keys/ai/{save,use,delete}`、`POST /api/keys/exchange/{save,use,delete}`、`POST /api/keys/clear`(按 `target` 清空生效凭据)。legacy `GET/POST /api/config/keys` **保持可用**(对话通道与向导仍用它)。
- **脱敏契约不变(032 FR-007 / FR-008)**: 读接口只回 `***` + 末 4 位, 任何响应与页面资源都不含密钥全文; 密钥框**留空 = 不修改**; 浏览器持久存储零写入。
- **设置页职责收敛**: 不再重复承载密钥配置(只留市场视野 + 指向密钥页的入口), 避免两处配置漂移; 运行页与策略页的 `need_keys` 引导改指密钥页。
- **范围外**: 密钥加密存储 / 轮转与到期提醒 / 多交易所(见 `specs/changes/033-key-manager/spec.md` §一)。

**UI 主题 + 现代字体 + 历史消息折叠(034, 2026-10-03)**: 纯前端 + 一个只读配置项, 无新表、无新确认渠道。

- **主题三选一**: 顶栏(语言钮旁) `#theme-select` 下拉, `dark`(默认)/`light`(白色)/`red`(红色);
  值存 `ricow.toml` 的 `[ui].theme`(`UiSection.theme`, 白名单 `UI_KEYS`/`WRITABLE` 各加一项, 非法值硬失败),
  新端点 `GET/POST /api/theme`(镜像 `/api/lang`, 但**不**给会话线程送控制行 —— 纯外观);
  `common.js` 的 `R.applyTheme` 切 `<html data-theme>` 并广播 `ricow:theme` 事件。
- **CSS 全面变量化**: `:root` = 深色兜底, `[data-theme="light"]` / `[data-theme="red"]` 各覆盖一份变量;
  034 收敛了此前写死的 20 处颜色(`#1d232b`×16 / `#06231b`×4 / 7 处 rgba)为新变量
  `--raised / --on-accent / --accent-soft(-2) / --danger-border / --overlay / --raise-1/2`,
  组件层零硬编码; K 线图(`markets.js`)创建时从 CSS 变量取色, 主题事件触发用缓存 K 线就地重绘。
- **现代字体**: 正文 15px/1.65 → **14px/1.6**, 字体栈前置 Inter / Segoe UI Variable,
  中文回退苹方 / 微软雅黑 UI; 等宽栈前置 Cascadia Code / JetBrains Mono; 开抗锯齿。
- **历史消息两行折叠**: 对话流(`#stream`)里除最新一块外, 更早的 `.msg`/`.line` 默认收成两行
  (`-webkit-line-clamp: 2`, 按行盒计数, 不受气泡内边距与全局 border-box 干扰), 淡出遮罩 +
  "··· 点击展开"(`data-fold-hint`, 随语言); 点击展开/再点收起。例外: `.menu` 不折、流式中气泡不折、
  手动展开过的块(`data-keep-open`)不被自动收起、点术语先展开所在块、选文字不触发。
  入口钩子在 `chat.js` 的 `append` / `appendDelta`(最新一块始终完整可见)。

~~`keyring`~~ / ~~`setup`~~ / ~~`credentials`~~ / ~~`config`~~ —— 2026-09-14 随单一配置文件方案**全部删除**(019 D31: 文件即界面)。

- 建策略(002, 写操作不得一步落盘): `ricow create --name <n> --pair <p> [--script <file|->] [--param k=v] [--days N] [--interval] [--market spot|futures]`
  —— 编译门禁 → 真实 K 线沙箱回测 → 打印报告 + `preview_id`(**不写任何策略文件**);
  `ricow approve <preview_id>`(人工批准: 打印**确认块**(动作/目标/关键参数/后果), 要求**逐字输入** `确认部署 <策略名>` —— 裸 `y` 不接受, **且必须来自交互终端**(stdin 非终端即拒绝: 管道/脚本/agent 工具调用喂入的短语一律无效, 019 spec §七 R2 已实现); 通过后发一次性 token, 15 分钟有效)→ `ricow deploy <preview_id> --token <t>`
  —— 落盘 `strategies/<name>.toml`(`params.script_path` 指向)+ `strategies/<name>.lua`; 同名策略存在即拒绝(不覆盖)
  > **确认渠道(023)**: 上述逐字短语**仅终端渠道**; 对话渠道(`ricow` REPL / `ricow ai`)的确认词是**当前语言的单个口语词**(`确认`/`确定`/`同意` · `confirm`/`confirmed`, 只认当前语言, 刻意不含 `y`/`yes`/`ok`/空)。只读动作(回测 / 查询 / 预览)不需确认; 写操作(13 类)统一收敛到唯一入口 `request_write_confirmation`, 由用户当场确认后交宿主进程执行 —— LLM 的写实工具面仍为空。
- 首次使用风险确认(018, product.md §十): 实盘启动前一次性确认 —— 未确认时**拒绝启动**并打印披露要点(仅供学习/无止损与选品责任/先 Dry Run + 子账号小额/不代管资金密钥)与确认方式; `--accept-risk` 确认一次后记入 `$RICOW_ROOT/risk_ack.json`(带 schema 版本, 披露实质变更可递增触发重新确认)。判定顺序: **风险确认 → Dry Run 时长门禁 → 时钟预检**(未确认时零交易所往返); Dry Run/回测不受影响
- 交易对视野(019 R4/D5): `ricow pairs [--market spot|futures] [--all]` —— 默认视野 = bStock 现货(`<base>BUSDT`, XxxB × EQUITY 白名单交叉)+ 股票永续(`<base>USDT`, TRADIFI_PERPETUAL); `--all` 见全量。免 key 公共端点, 进程内 TTL 缓存 + 输出截断并报告总数; 同一视野由对话内 `/market` 与 L0 工具 `list_pairs` 共用
- Dry Run 虚拟本金可配(016): `params.initial_cash`(缺省 100000) —— 用小资金同口径预演, Dry Run 的权益口径才与实盘可比; 非法值报错不静默回落; 启动打印本金额
- Dry Run 起点与实盘时长门禁(002): 首次 Dry Run 启动时把 `dry_run_started_at`(ISO8601)写入策略 TOML;
  实盘启动要求 Dry Run 累计 ≥ `params.min_dry_run_hours`(默认 24 小时, 设 0 关闭), 不足则**拒绝启动**(不降级)

- 进程管理: `daemon start`(自后台化) / `daemon stop`(优雅停全部策略) / `daemon status`;
  `start|stop|restart <name>` 经 daemon 控制通道; `list|status` 为总览(daemon ∪ 台账 ∪ 部署清单), `status <name>` = 详情;
  `info <name>` 运行信息 + 成交统计; `fills [name] [--limit N]` 按 `strategy_id` 过滤; `logs <name> [-f] [--lines N]`
- 组合敞口(037 P0-C): `ricow exposure [--pair P] [--mode live|demo|dry_run] [--detail] [--limit N]` —— **只读**聚合本地库
  `positions` / `orders`, 逐 `(标的 × 模式)` 给出净头寸 / 多空合计 / 总名义(**按开仓均价估算**)/ 未终结挂单数 / 活跃策略数;
  `--detail` 下钻到"策略 × 标的 × 模式"。**只展示不拦截 · 不直连交易所(不需要密钥) · 绝不跨模式相加**; 无写路径、无新确认面。
- 前台调试: `ricow run <name>`(Dry Run; 进程内监听 stdin `stop` / 管道 EOF / Ctrl-C 优雅停机, 不被 daemon 管理)
- 回测: `ricow backtest --strategy <名|类型> [--pair] [--days] [--interval] [--script] [--param k=v] [--market spot|futures] [--position-mode one-way|hedge] [--fee/--fee-maker/--fee-taker/--slippage-bps/--cash/--leverage/--max-leverage/--mmr-pct/--funding-rate]`(杠杆默认上限 10x,超限须 --max-leverage 显式放宽;MMR 默认按 symbol 内置首档表, 表外 1.0%)
  - 数据源按市场分支: 现货走交易所 REST; 合约 (futures) 走 fapi 公共数据源 (K 线; MMR 按 symbol 内置首档表,表外回落 1.0%)
  - 直跑模式: `--strategy {shannon_spot_grid|paired_grid|lua}` — 内置脚本经 BUILTIN_SCRIPTS 常量表注入(需 --pair)
  - 部署模式: `--strategy <name>` 命中 `strategies/<name>.toml` 加载(`script_path` 引用文件或内嵌 `script`)
- 启动: `ricow start <name>`(经 daemon 后台运行) / `ricow run <name>`(前台调试; 默认 Dry Run, TOML `enabled=false` 拒绝启动)
- ~~选币: `ricow scan …`~~ —— **已于 2026-09-15 删除**(020-platform-scope-trim: 选币/研究入口与"运行策略的平台"定位正交; 用户拍板删除, 不留废弃代码)

## 六、数据布局

- `RICOW_ROOT`(数据目录, 决策 D4): 显式覆盖 > 当前目录已有 `ricow.db`/`strategies/` 时沿用现状 > 平台标准目录
  (Windows `%APPDATA%\ricow` / macOS `~/Library/Application Support/ricow` / Linux `$XDG_DATA_HOME|~/.local/share`+`/ricow`)
- `RICOW_DB`(默认 `RICOW_ROOT/ricow.db`): SQLite, **共 10 张表** — `klines`(交易所 K 线缓存, **主键含 `market` 维度**隔离现货/合约)/ `fills`(成交, 含 `strategy_id`)/ `pnl_snapshots`(盈亏快照)/ `previews`(写操作预览)/ `us_klines`(美股 Nasdaq 日线, 信号轨与交易日历; **缓存不回源, 需手工增量补最后若干天**)/ `funding_fees`(资金费流水, `tran_id` 幂等)/ `web_sessions` + `web_messages`(025: 会话与对话流水, 外键级联删除)/ `orders` + `positions`(026: 订单与当前持仓, 各带 `mode`)
  > **schema 版本化(035, 2026-10-03)**: 迁移由 `PRAGMA user_version` 驱动(`SCHEMA_VERSION` 常量)。
  > `migrate()` 分两步: ① **幂等基线建表**(全新库直接建出最新结构); ② **版本迁移**(仅 `v < SCHEMA_VERSION` 时执行并推进版本号)。
  > **版本已最新时零写事务直接返回** —— 这消除了旧实现"每次 `open` 都重跑全量建表"在多连接池同库并发下的偶发 `SQLITE_BUSY`;
  > `Database::open` 另加**锁错误**(`SQLITE_BUSY=5` / `SQLITE_LOCKED=6`)线性退避重试(用 `tokio::time::sleep`, 阻塞式 sleep 会卡死 current_thread 运行时把上一池的 `close()` 收尾一起阻塞)。
  > v0 → v1: `klines` 主键加 `market`, 旧行标 `spot`, 走"建新表 → 搬数据 → 删旧表 → 改名"单事务且幂等可重入。后续"给表加列"这类演进有了正规通道。
- `RICOW_ROOT/run/`: `daemon.json`(daemon pid/端口/token, Unix 0600 / Windows 仅当前用户 ACL)/ `<name>.json`(实例台账: pid/启动时间/模式/上次退出码与原因)/ `<name>/events.jsonl`(035: 该实例的**结构化运行事件流**, 一行一事件、写完即 flush、坏行读取时跳过)/ `backtest/<ts>-<策略>-<pair>.json`(035: 回测 run card, 见 `specs/backtest.md` §六.6)
  > ⚠️ 多进程共享同一 `ricow.db` 的并发写依赖 WAL + `busy_timeout`(sqlx 默认 5s): 实测 3 进程 × 200 事务在 busy_timeout=5s 下全部成功, =0 时失败 83%(`.hermes`→已归档 `specs/research/process-model-probe-2026-09.md` §四)。**不得把 `busy_timeout` 设为 0, 也不得把数据目录放在网络盘/云同步盘**(SQLite WAL 明确不支持网络文件系统)
- `RICOW_ROOT/logs/`: `<name>.log`(策略进程 stdout/stderr 追加日志; 启动时 >10MB 轮转 `.log.1`)与 `daemon.log`
- 密钥: 单一明文配置文件 `ricow.toml`(Unix 0600 / Windows 仅当前用户 ACL; 019 D31/R4; OS Keyring 与 headless 加密文件 fallback 已于 2026-09-14 移除)

### 交易可见性(026, 2026-09-19)

- **落库时点**(引擎侧, 与下单/回报同一处调用): `orders` 在下单提交 / 订单状态变化 / 成交回报时 `upsert_order`; `positions` 在成交后刷新持仓时 `upsert_position`; `pnl_snapshots` 在**每笔成交后**写一条(`insert_pnl_snapshot`), 永久保留 —— 面板与 AI 都由这些行重建, 不做二次推断。
- **数据源口径(D1)**: **本地库是唯一来源**。`/api/trades/*` 只读 `ricow.db`, 不直连交易所; 运行中与已停机一视同仁(停机后仍能看最后状态, 并标注"截至 <时间>")。
- **三态如实(D10)**: 每条回复带 `source` ∈ `ok` / `daemon_down` / `unreadable`(读库报错优先, 其次 daemon 不在)。后两者**不把"连不上 daemon"说成"没有交易"**, `reason` 给出原因, 前端按三态分开呈现。
- **mode 关联(D5)**: `fills` 表**不加列**(守住既有 8 张表结构零改动); 成交的 `mode`(∈ `dry_run` / `demo` / `live`)由 `fills.exchange_order_id` ⟕ `orders.exchange_order_id` 带出。026 之前的历史成交在 `orders` 里无对应行 → 面板与 AI **如实**显示"未知", 不猜。
- **端点**(全部**只读**, 挂在同一道 token 中间件之内): `GET /api/trades/{fills,orders,positions,pnl}`(查询串 `strategy_id` / `limit`) + `GET /api/logs`(策略日志清单) / `GET /api/logs/{name}/tail?lines=`(尾读, 缺省 50、一律夹到 200) / `GET /api/logs/{name}/stream`(SSE, 服务端 500ms 尾读驱动, **独立通道**不共用会话 SSE; **断线续传(036)**: 事件 id = 文件字节 offset, 重连按 `Last-Event-ID` 头(前端统一打开器重开新对象时以 `last_event_id` 查询参数兜底, 头优先)从断点续读, 不重发已给过的行)。日志**原样返回** —— 不解析、不改写、不做着色推断; 策略未运行也能看历史日志。前端消费侧(036): `common.js` 的 `R.sse` 统一打开器 —— 指数退避(1s→2s→4s→8s 封顶, ±20% 抖动)接管浏览器原生重连(不退避也不封顶), 连续失败 5 次放弃回调 `onfail`; `new EventSource` 全前端只此一处。
- **前端只读(D17)**: 交易面板与日志面板只做展示 + 轮询/订阅, **没有任何直连交易所的写按钮** —— 撤单 / 平仓 / 停机仍走既有的对话确认。
- **AI 回流(FR-019 ~ FR-024)**: 新增三个只读工具(`positions` / `open_orders` / `pnl`); 写操作执行结果注入对话 history, 使下一轮 LLM 看得到"刚才那步真做了什么"。

### 组合敞口只读视图(037, 2026-10-06)

- **只读聚合**: 算法在 `ricow_engine/src/exposure.rs::aggregate_with`(纯函数, 无 IO), 展示在 `commands/exposure.rs`。
  回答的是多策略场景下的盲区: "合起来我到底押了多少" —— 单看任一策略都正常, 合起来可能是自相对冲(白付两遍手续费)。
- **聚合键 = `(标的, 模式)`**: `dry_run` / `demo` / `live` **分开成行, 绝不相加** —— 模拟持仓混进实盘敞口会造出一个
  看似精确、实则错误的"总敞口", 那比不给数字更危险。
- **口径**: 净头寸 = Σ 带符号 `size`(多 − 空, 跨策略反向持仓相互抵消; `size` 由 `command::position_row_of` 落库时算好);
  总名义 = `Σ|头寸| × 开仓均价` —— 是**规模估算**, 本地库没有实时价; 有头寸但 `entry_price = 0` 时该行名义标 `*`
  并注脚**被低估**(不假装数字完整); **刻意不跨交易对汇总**(不同交易对报价资产未必一致, 相加无意义 —— 宁缺勿错)。
- **未终结判据单一来源** = `ricow_engine::is_open_status`(`open` / `partially_filled`) —— 挂单视图与敞口视图共用。
  此前各写一份白名单, 漂移的后果是"敞口视图说有挂单、挂单视图说没有"。
- **只展示, 不拦截**(守宪法"平台不做投资判断"): 无写路径、无新确认面、不直连交易所(因此不需要密钥, 也不会因网络问题给不出答案)。
  挂单数只覆盖传入的那段订单 —— 触到 `--limit` 时如实打印"更早的挂单可能未计入", 不把"没看到"说成"没有"。
- **已无敞口的组合不进表**(头寸全 0 且无挂单答不了"押了多少"), 但**不静默丢弃**: 如实报出漏了多少个, `--detail` 可见。

### 网络请求重试与限流(035, 2026-10-03)

- 单点定义在 `ricow_binance/src/retry.rs`(`send_with_retry` / `status_error`), 现货 / 合约 / fapi 公共数据三处客户端共用。
- **按请求语义分档**是安全红线: `Read`(K 线/行情/查询)可重试传输错误 + 429/418/408/5xx;
  `Write`(下单/撤单/改杠杆等改账户状态)**仅连接层失败**(`is_connect` = 请求确定未送达)可重试 ——
  响应超时/5xx 可能"其实已成交", 重放 = 重复下单。
- 退避 = 指数 + ±20% 抖动(默认 300ms 起、上限 8s); 429/418 优先尊重响应 `Retry-After`(仅秒形式, 上限 30s 防挂起)。
- 429/418 最终仍失败时由 `status_error` 归一为 **`CoreError::RateLimit`**(此前是**死变体**, 全仓无一处构造)。

### 运行可观测性与事件流(035, 2026-10-03)

- 结构: `ricow_strategy/src/events.rs` —— `RunEvent`(一行一 JSON)+ `EventWriter`(`create(root, name)` → `run/<name>/events.jsonl`)+ `read_events`(坏行跳过)。
- 事件种类: 实例级 `started` / `stopped`(引擎埋点); 订单级 `order_placed` / `order_rejected` / `order_canceled`(上下文埋点, Live 与 Dry Run 都有)。
- **不反压主循环**: `emit` **永不返回错误** —— 锁毒化用 `into_inner`, 写失败只提醒一次; 观测能力缺失不该阻断交易。
- 与 run card 同源(2.3): 两者都是"旁路产物", 落盘失败只 warn, 不回滚主流程。

### 错误归一(035, 2026-10-03)

- 新增 `CoreError::Db(String)`(与 `Exchange` 分开: 数据库故障不是交易所故障, 上层可按变体区分)。
- `SqlxResultExt::core()` 在存储边界把 `sqlx::Error` 一次归一到 `CoreError`; 上层不再手工 `map_err(|e| CoreError::Exchange(e.to_string()))` 猜错误种类(约 16 处已替换)。
- 长跑服务的锁毒化面(`multiframe.rs` / `web/backtest_jobs.rs`)改 `lock_or_recover` —— 单点 panic 不再级联成"后续请求全失败"。

## 七、安全模型

- 写操作强制确认: `create_strategy` 提交 → Lua 编译门禁 → 沙箱回测 → preview → `ricow approve`(一次性 token 重放)→ 部署运行
  > ✅ 现状(2026-09-13, 002 落地): `ricow create`(门禁 → 真实 K 线沙箱回测 → preview)→ `ricow approve`(人工批准, 一次性 token)
  > → `ricow deploy`(落盘 `<name>.toml` + `<name>.lua`, 同名拒绝覆盖)。引擎侧 `Engine::create_strategy_preview` /
  > `backtest_and_preview` / `ricow_engine::execute_strategy` 与 CLI 同源; AI 客户端只读 `specs/lua-api.md` 写代码, **无法自行批准**。
- Dry Run 时长门禁(002): 实盘启动额外要求该策略 Dry Run 累计 ≥ `params.min_dry_run_hours`(默认 24 小时 = 覆盖三个交易时段的一个完整日周期;
  设 0 关闭)。起点由首次 Dry Run 启动写入 TOML `dry_run_started_at`(ISO8601); 不足即拒绝启动, 并给出已运行时长与解除方式。
  Dry Run / 回测 / 沙箱不受此门禁影响。
- **平台不做投资判断**(2026-09-16, 019-R5; 宪法 §安全要求已同步修订) —— 赚赔政策属于策略, 平台只做执行 + 数据 + 门禁 + 状态:
  - **已删除**: `risk.rs` 的四条静态限额规则(最大持仓 / 单日最大亏损 / 最小订单 / 最大滑点)、`RiskSettings` / `RiskEngine` 装配器、`config.rs` 的 `RiskConfig` 与 `validate_risk()`、死模块 `scheduler.rs`; 老策略 TOML 残留的 `[risk]` 段与 `risk_*` 参数被 serde 忽略(**不再生效**, 装载不报错), 不做迁移脚本。
  - 策略自管: 用只读 `ctx:net_pnl()` / `ctx:equity()` 实现止损/回撤/仓位政策(内置 `shannon_grid` 的 `dd_stop_pct` 为参考写法, 默认关闭); ~~平台级两级亏损熔断~~ 已于 2026-09-15 删除(020)。
  - **保留**: 固定工程护栏 `ricow_strategy::order_guard` —— 三条 `place_order` 顶部统一拦截**下单频率上限**(滑动窗口 1s, 固定 100/s, 回测 / Dry Run / 实盘同一装配, 计数对全部 pair 共享), 防 bug 风暴下单被交易所限流封禁; **固定常量、不读任何配置、无配置面**。
  - 被拒请求返回 `Rejected` ack(与资金不足同形)并计入回测报告"拒单次数", 同时 `tracing::warn!(target: "order_guard")` 输出关键数值; 策略循环不中断。详见 `specs/backtest.md` §二.6 与 `specs/changes/019-ai-assistant/spec.md` §七 R5。
  - > 修正记录(2026-09-12): 004 之前 RiskEngine 的唯一调用点是无消费者的 `StrategyScheduler`, 三条真实下单路径**均未过风控** —— 即四条静态规则当时实际从未生效; 已随 004 接入, 又随 2026-09-16 R5 整体删除。
- Dry Run 默认, 确认后切实盘(011 落地): 门禁**双条件** = TOML `live_enabled=true` **且** 命令行 `--live`(缺一即按 Dry Run 运行并打印原因; `ricow start` 同口径, 台账 `mode` 与实际运行器一致)
- **实盘二次分离**(019 D4/T029-T030): 双条件之外, 每次实盘启动必须在**交互终端逐字输入** `确认实盘 <策略名>`(裸 `y`/空/EOF 一律拒绝, 零副作用);
  确认只发生在父进程 CLI, daemon 协议 `Request::Start.confirmed` 缺失时**明确拒绝**(不静默降级), 子进程由 daemon 注入内部 `--live-confirmed` 不再索要 stdin。
  > **内部通道双条件(019 R4 收敛)**: 子进程认账要求 `--live-confirmed` 标志 **且** 环境变量 `RICOW_DAEMON_SPAWNED`(由 daemon spawn 时经 `cmd.env` 注入)同时成立 —— 单靠标志可被手工构造, 加环境变量后"用户自己敲命令加 flag"不成立, 只剩 daemon 派生链内部可信(`supervisor/procs.rs::DAEMON_SPAWN_ENV` / `spawned_by_daemon()`, 判定与单测在 `commands/run.rs::daemon_confirmation_accepted`)。
  实盘三判据(018 风险披露确认 → 002 Dry Run 时长门禁 → 008 时钟预检)顺序与判据**未改动**。
  > **R4(2026-09-16)**: 该短语现在也可在**对话内**输入(`ricow` REPL / `ricow ai`, 仍须交互式 tty); 执行由宿主进程内直调 `ctrl::live_preflight` → `start_daemon(live, confirmed)`, **三判据一条不少、顺序一字未改**。停 demo/实盘与 `--close-all` 平仓同理(短语见 019 spec §七 R4 表)。
  > **R5-023(2026-09-18)**: 对话渠道的确认词由"逐字长短语"改为**当前语言的口语词**(`确认`/`确定`/`同意` · `confirm`/`confirmed`, 只认当前语言); 本节**终端门禁**的逐字短语与 `live_preflight` 三判据**一行不改**。执行链路不变: 宿主进程内直调 `ctrl::live_preflight` → `start_daemon(live, confirmed)`。
- **demo 运行模式**(019 T062): `ricow run|start <name> --demo` —— 真实调用币安**模拟交易(demo)**平台下单接口(无真实资金),
  因此**不适用**实盘三判据, 但仍需 demo 凭据、并按 **demo 服务器**做时钟预检; `--demo --live` 同时给直接拒绝; 台账与 CLI 文案均标注 `测试网模拟盘(demo)`。
- 实盘启动前**时钟预检**: 本机超前交易所服务器 >1000ms 或滞后 >4000ms → 拒绝启动并打印对齐步骤(币安对签名请求超前 >1s 直接拒绝, 见 `specs/testnet.md`); 取数走公共 `/api/v3/time`
- 实盘/demo 凭据(019 D31): 来自单一配置文件 `$RICOW_ROOT/ricow.toml` 的 `[exchange]` 段 —— `binance_key/binance_secret`(实盘)与
  `demo_key/demo_secret`(币安模拟交易 demo)**两套独立槽位, 永不跨套回落**; 缺失即拒绝启动并**点名键名与文件路径**, 不静默用空凭据发请求。
  旧方案(env `RICOW_BN_*` / OS Keyring / `ricow keyring` / `credentials.toml`)已于 2026-09-14 全部移除 —— 不保留回退(keyring 在本机不可用)。
- 实盘下单参数由引擎按交易所过滤器**自动对齐**(数量按 `step_size` 向下取整 / 限价取"不劣于意图"的一侧 / 不足 `min_qty`·`min_notional` 拒单并如实报错); 订单号统一带 `<策略名>-` 前缀, 停机撤单**只撤本实例归属**的单, 非归属单只上报不撤
- **启动接管遗留挂单**(037 P0-A, 资金安全): 实盘/demo 启动时先按**归属前缀**切分交易所该交易对的挂单(`live::split_owned`) ——
  本实例归属逐个**撤销**(逐笔落事件流 `orphan_canceled` + 写 `outcome.orphan`, 不静默), 非归属单(用户手工/其他实例)**只上报不撤**(沿用既有 `plan_cleanup` 口径);
  撤销后**复查**: 仍有本实例残留 **或** 枚举本身失败 → **live 拒绝启动**(错误带残留单号 + 手工撤单指引 + "撤销后重启"), demo 只警告继续。
  理由与"时钟预检拒绝启动"同口径 —— **无法建立安全前置时不进实盘**。**不提供 `orphan_policy` 配置面**(固定工程不变量, 与 `order_guard` 同口径:
  安全不变量不接受配置)。修的是"进程崩溃/强杀/断电后重启, 遗留挂单仍挂在交易所而策略 `on_init` 再下一张同样的单 → 敞口翻倍"。
- **引擎级最小订单登记**(037 P0-B, 资金安全): `ricow_engine/src/oms.rs` 的 `OrderRegistry` 记录**本次会话**提交的每张单及其生命周期
  (`open` / `partially_filled` / `filled` / `canceled` / `rejected` / **`unknown`**)。三条口径:
  ① **传输失败 = `Unknown`** —— 交易所可能已收到并挂单, 也可能没有; 单独入账、收尾**醒目提示用户手工核对交易所**, 不谎报结论;
  ② **重复单号检测** —— 同一 `client_order_id` 在**非终态**时被再次提交 = 策略 bug 信号(计数 + warn), 终态后的复用不算(交易所允许);
  ③ 会话账目(`submitted` / `live` / `filled` / `canceled` / `rejected` / `unknown` / `duplicates` / `unresolved`)计入 `RunOutcome`, CLI 收尾打印。
  纯内存、无 IO、无 await; 一切失败**如实降级不抛出**(与事件流同一旁路哲学 —— 交易比观测重要)。**策略侧看不到它**(策略仍只经 `ctx` 的既有只读查询)。
  增量/累计两个成交口径分开: `OrderFill.fill_size` 是**增量**(用加), `OrderUpdate.filled_size` 是**累计**(用取大), 不可互相套用。
- Lua 沙箱: 无 os/io/require/loadstring/pcall, 指令预算 1M/tick, 内存 64MB
- 网络: 仅交易所 API + 美股行情(Nasdaq 官方); 无遥测、无自动更新

## 八、~~选币 scan~~(已于 2026-09-15 删除)

- **删除范围**(020-platform-scope-trim): `ricow scan` 命令 + `ricow_engine::scan` 实现 + 仅它使用的 `ricow_engine::indicators`(`ema` / `adx` 及其测试)。
- **保留**(用户明示的通用能力, 见 roadmap"已终止的探索"): 美股数据层 `nasdaq` / `us_tickers` / `market_class` / `us_klines` 缓存、
  组合回测路径 `run_portfolio_backtest` + 组合信号模式、`build_interval_ticks`、`ctx:now()`。
- 历史设计口径(7d×0.4 + 30d×0.6 打分 / EMA20+ADX14 趋势 / 前 10 后 10 方向一致性排除)见 `specs/changes/005-market-filter/`。

## 九、测试策略

- 交易流程: BN testnet(demo 环境)真实调用, 禁 mock Exchange 替身、禁假 token、禁主网下单(requirements 第七节硬性纪律)
- 纯逻辑(指标 / 打分 / 参数校验 / 撮合记账): 单元测试, 已知向量
- **基线(2026-10-06, 037 实盘资金安全加固后实跑)**: `cargo test --workspace --no-fail-fast` = **761 passed / 0 failed / 22 ignored**(全目标零失败)。
  增量 = **+25**, 与本变更同源: `ricow` bin **370 → 382**(+12: `commands::exposure` 聚合视图的渲染/过滤/显示宽度对齐) + `ricow_engine` lib **99 → 112**(+13: `exposure.rs` 的组合敞口聚合, 含跨策略对冲/模式隔离/名义低估标记/顺序稳定); 其余 target 一字未变
  (ricow_binance 59 / ricow_core 18 / ricow_strategy 184 / `architecture_guard` 3 / `ai_live_smoke` 3)。
  > **如实**: 同口径上次记为 670(2026-10-05)。670 → 736 的差额来自 **2026-10-06 的四笔审计加固提交**(三梯队 + 中危 ×2 + 低危收尾, 见 `git log 8bbecd3` 起), 那几轮只写了提交说明、**未单独记基线**, 故此处以 736 为 037 的起点。
  门禁五件: `fmt --all -- --check` 0 差异 / `clippy --workspace --all-targets -- -D warnings` exit 0 / `cargo deny --locked check` 全绿 / `bash scripts/ci_grep_gates.sh` 五条安全红线全绿 / `architecture_guard` 三条用例全绿。
- **前基线(2026-10-03, 035 工程底座加固后实跑)**: `cargo test --workspace --no-fail-fast` = **642 passed / 22 ignored**(另有 2 例 `ai_live_smoke` **本机沙箱环境性失败**: `ERROR_PIPE_BUSY(231)`, 非代码缺陷; 真机终端为全绿)。
  门禁: `fmt --check` 0 差异 / `clippy --workspace --all-targets -D warnings` 0 / `cargo deny --locked check` 全绿(许可证白名单 + 已知漏洞 + 重复版本 warn + 来源禁未知) / `bash scripts/ci_grep_gates.sh` 五条安全红线全绿(AI 工具层零落盘 · 明文密钥不进日志 · 无调试残留 · `execute_strategy` 调用点白名单 · **生产代码不得对 `Instant` 做裸减法**〔红线 5, 2026-10-06 加: Windows 单调时钟锚在开机时刻, 往回减大 Duration 必 panic, windows-latest 曾因此固定红〕; 按 `#[cfg(test)]` 配平跳过测试代码)。
  CI 矩阵 ubuntu + windows + **macOS**(发布了 macOS 产物就必须测); `release.yml` 去 PR 触发(dist 持久开关在 `dist-workspace.toml` 的 `pr-run-mode`)。
- 前基线(2026-09-24, paired_grid 新增后实跑): `cargo test --workspace` = **544 passed / 0 failed / 22 ignored**(ignored 仍为需真实外部环境的联调用例, 不 mock 替代; paired_grid 新增 4 条集成测试: ATR 未就绪不动 / 激活建仓与上下单结构 / 配对卖价恒>买价 / 连续下跌 flag 为负)
- 历史基线(2026-09-15, 021 clippy 清零后): 339 passed / 0 failed / 12 ignored; 更早(2026-09-13, 018 实施后): 308 passed / 0 failed / 11 ignored(ignored = 需真实外部环境的联调用例, 不 mock 替代; 构成: BN demo 现货 4 + 合约 5 + Nasdaq 冒烟 2)
- 实盘链路真实验证(011 demo 现货 / 012 demo 合约): 用户流订阅 → 真实下单 → 成交回写落库 → 停机撤单兜底/平仓 → 交易所侧零残留(合约含 one-way 与 hedge 双向); 记录见 `specs/testnet.md`
- 账目类数字(SQLite `SUM`)须在 Rust 侧用 `Decimal` 聚合: SQL 的 INTEGER 兜底可击穿 f64 解码(崩溃), REAL 往返会污染小数(012 实测)

## 十、技术选型

| 组件 | 选型 |
|:--|:--|
| CLI | clap v4 |
| 异步 | tokio + reqwest + tokio-tungstenite |
| 存储 | SQLite(sqlx 0.8) |
| 脚本 | mlua 0.11(Lua 5.4, vendored) |
| 指标 | ta 0.5 |
| 数值 | rust_decimal(金额/价格) |
| 密钥 | 单一配置文件 `ricow.toml`(明文; Unix 0600 / Windows 仅当前用户 ACL; 取舍理由见 §七 与 README) |
| AI 助手 | rig 0.42(`rig-core`/`rig-agent`) + reqwest; **rustls crypto provider 进程启动时显式安装**(019: 依赖图内 aws-lc-rs 与 ring 并存时, 用户数据流 WS 会在建 TLS 时 panic) |

## 十一、CLI 残留与文档-实现缺口

### 已清理(2026-09-12, 随 008 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| 1 | `backtest --market` 帮助文本含不可用的 `us` | ✅ 帮助文本改为 `spot\|futures` |
| 2 | `backtest --dump-dir` 无消费方 | ✅ 删除该参数 |
| 3 | `scan` 帮助文本写 `横截面选币 (P2 骨架)` | ✅ 改为现状描述 |
| 4 | `ctrl.rs` 全部转发 systemctl(非 Linux 不可用) | ✅ 重写为 daemon 控制通道客户端 |
| 5 | `logs` 为空壳 | ✅ 实装(`logs/<name>.log` + `--follow`) |
| 6 | `on_stop` 已承诺但 run 循环未调用 | ✅ 接线并区分"策略是否实现清理" |
| 7 | `insert_fill` 无生产调用点(成交不落库) | ✅ run 循环接线, `strategy_id` = 策略名 |

### 已清理(2026-09-13, 随 011 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| B | `live_enabled=true` 的配置仍按 Dry Run 运行 | ✅ 实盘运行器落地; 启动门禁双条件(配置 **且** `--live`), `list/info` 不再提示"仅 Dry Run 可用", 改为提示"启动需 --live" |
| C | 实盘撤单兜底未实现 | ✅ 停机清理实现: `get_open_orders` → 按 `<策略名>-` 前缀过滤 → 逐个撤(单个失败不中断) → 复查残留如实上报; `stop --close-all` 另可市价平仓 |
| D | `ricow info` 无实盘持仓/挂单/余额视图 | ✅ `info` 对声明实盘的策略查交易所实时快照(查询失败如实标注为"未知", 不显示陈旧值) |
| E | 现货用户流 legacy listenKey 已失效 | ✅ 改走现货 WebSocket API `userDataStream.subscribe.signature`(币安 2026-02-20 下线 legacy listenKey, 实测 410 Gone); 顺带修 demo WS host 映射与现货 `executionReport` 事件解析 |
| F | 合约下单丢弃调用方 `client_order_id`(F1) | ✅ 合约客户端改用传入值(缺省才生成), 与现货一致 |

### 仍存(待后续变更)

| # | 项目 | 说明 |
|:--|:--|:--|
| A | ~~`dry_run_started_at` 字段无消费方~~ | ✅ 随 002 落地: 首次 Dry Run 启动写入(ISO8601), 实盘启动按 `params.min_dry_run_hours`(默认 24h)核验 |

### 已清理(2026-09-13, 随 012 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| G | 合约实盘(USDT-M)未实现 | ✅ `BnFuturesExchange` 落地(`Exchange` 适配 + `fstream` WS 盘口/增量合并 + listenKey 用户流); hedge 定向持仓与 one-way/hedge 平仓参数实测通过 |
| H | 平仓单号超 36 字符被交易所拒 | ✅ `close_client_order_id()` 生成端截断并保留归属前缀(实测 `Client order id length should be less than 36 chars`) |
| I | `ricow info` 无成交策略时 panic + 手续费 f64 噪音 | ✅ `fill_stats` 改 Rust 侧 `Decimal` 精确聚合(单查询), 单测覆盖无成交/零手续费/小数三类 |

### 已清理(2026-09-13, 随 013 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| J | 合约记账为"按对共享钱包", 与真实"按侧独立逐仓"不符(真实清算实测推翻) | ✅ 钱包按侧寻址(`pair\|long`/`pair\|short`, one-way 退化 symbol 级)+ hedge 按侧独立强平 + 两侧资金费独立结算 |
| K | 资金费结算时点与 interval 相关(L1)/ 尾 bar 不结算(L2) | ✅ `funding_settlements_in_bar`(与 interval 无关, 结算点不重不漏)+ `finalize()` 收尾补结算 |

### 已清理(2026-09-13, 随 014 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| L | 实盘资金费不入账(长期持仓成本漏计) | ✅ 以交易所账单为准(`GET /fapi/v1/income`)+ `funding_fees` 表(`tran_id` 幂等)+ 启动/30min/停机前增量拉取(水位 = `MAX(funding_time)`); `info` 显示累计资金费 |
| M | 杠杆持仓的强平风险不可见 | ✅ `liquidation_distance` 纯函数 + 快照/成交后刷新时低于阈值(默认 15%, params `liq_warn_pct`)告警; 缺强平价标"未知"; **只提示不动作** |
| N | 合约 `Exchange` 无资金费接口 | ✅ trait 新增 `funding_income`(默认实现空, 现货零改动) |

### 已清理(2026-09-13, 随 003 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| O | 熔断/成交/强平/残留等高风险事件只落本地日志(product.md §三.4 的承诺未兑现) | ✅ 出站通知(`ricow_engine::notify`): 用户自选 webhook + 事件白名单/限速/去重 + 失败只 warn 不反压循环; 配置走 params, 默认关闭。**熔断事件已于 2026-09-15 随 020 删除**(现行 3 类: 成交 / 接近强平 / 停机残留) |

### 已清理(2026-09-13, 随 017 实施一并处理 — dogfood 实测发现)

| # | 残留 | 处置 |
|:--|:--|:--|
| R | 现货停机 `stop --close-all` **静默不平仓**(报告"无持仓", 账户实际持有 2.016 ETH) | ✅ 012 起清理用 `get_positions_directional`, 而现货实现 `get_position` 恒返回 None → 清理与残留复查的持仓源**按市场分支**(现货走引擎侧 `spot_position_of`, 与成交后刷新同源) |
| S | 现货成交后**只刷持仓不刷现金** → 策略按 equity 决策时以为"只有币没有钱", 每 tick 再卖一半, 几何级数清仓 | ✅ 现货分支补刷 base+quote 余额(失败只 warn); 实测修复前 `提交订单=216/成交=8/持仓清空` → 修复后 `提交订单=1/成交=2/残留 0.000058` |
| T | 成交回写(异步用户流)窗口内**重复下单**(同价同量 1.4s 内 3 次) | ✅ 实盘下单批次出现成交即立刻对齐快照(`refresh_positions`), 闭合窗口 |

### 已清理(2026-09-13, 随 002 实施一并处理)

| # | 残留 | 处置 |
|:--|:--|:--|
| P | 引擎侧建策略链路完整但**无 CLI 入口**(AI 客户端无法提交, 用户只能手写 TOML) | ✅ `ricow create` + `ricow deploy`(批准复用既有 `ricow approve`); 部署物 = `<name>.toml` + `<name>.lua`, 同名拒绝覆盖, 路径穿越拒绝 |
| Q | `dry_run_started_at` 无生产方也无消费方 | ✅ 见 §十一 A |

> 另记: `ricow_engine` 仍 `pub use` 组合回测三函数(`run_portfolio_backtest` / `build_daily_ticks` / `build_interval_ticks`),
> 当前仅被单测(`backtest_runner.rs` 内)使用 —— 属 2026-09-11 / 2026-09-12 两次明确保留的通用能力(见 roadmap "已终止的探索"),
> **不按死代码删除**; 暂无内置策略消费者(`bs_rs_rotation` 已于 2026-09-12 未实施即终止)。
