# ricow 架构文档 v4.1(现状, 纯 CLI 客户端)

> 状态: ✅ 已实施(P1-P3 完成; P4 dogfood 实盘待开始, 进度见 specs/roadmap.md)
> 职责: 本文描述**当前实际架构**(与代码逐份复核, 2026-09-11)。产品定位见 specs/product.md, 里程碑见 specs/roadmap.md。
> 策略 API 唯一权威见 specs/lua-api.md; 回测口径唯一权威见 specs/backtest.md。

---

## 一、目标架构

CLI 单入口直连同一引擎与同一下单路径, 安全模型一致(confirm + RiskEngine + Dry Run 默认):

```
ricow CLI (clap)
      │
      ▼
ricow_engine (headless 核心: 命令分发 / 回测 / 确认通道)
      │
      ▼
ricow_strategy (Lua 沙箱 + ctx.* API + exec.* 组件 + 指标 / 风控 / 回测 / SQLite)
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
| ricow_strategy | 策略引擎: Lua 沙箱 / ctx 与 exec 注册 / 指标(ta)/ 风控 / 回测 / PnL / SQLite |
| ricow_engine | headless 核心: Engine 命令分发 / backtest_runner(单标的 + 组合) / confirm(preview+approve)/ loader / market / **美股层 `nasdaq`(Nasdaq 日线客户端) / `us_tickers`(bStock↔美股映射, 70 只快照) / `market_class`(bStock 现货池识别, 通用能力保留: 当前无内置消费者)** |
| ricow | 二进制 `ricow`: clap 子命令分发 + **AI 助手 `ai/`(019: 提示词、工具白名单 L0 只读 + L1 虚拟、审批门 `ToolGuard`)** + 单一配置文件读写 `commands/config_file.rs` |

> **内置 AI 助手已落地**(019-ai-assistant): `ricow ai` 调用用户自配的 LLM(`ricow.toml [ai]` provider+api_key; 亦可零成本指向本机 Ollama),
> 写实动作**不作工具注册**(L2), 工具面只有只读(L0)与虚拟(L1)。
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
- **exec.\*** 执行组件(引擎内置 Rust 实现, 加载时注册全局表, 脚本内可覆盖): `levels` / `pullback_triggered` / `detect_quote` / `ticks_per` / `slice_due` / `side_order`
- 内置资产(**编译期 include_str! 嵌入二进制**, 登记表 = `crates/ricow/src/commands/mod.rs:BUILTIN_SCRIPTS`):
  - `strategies/builtin/shannon_grid.lua` — 策略样板(香农 50:50 中轴再平衡, 单标的, `target_ratio` 中轴可调 + ATR 自适应 band)
  - `strategies/builtin/executors/{dca,twap,vwap,pullback,ladder}.lua` — 执行模式示例(最简信号 + exec.* 执行, **非策略**; 复制改信号即自定义)
  - ~~`strategies/builtin/bs_momentum.lua`~~ — **已于 2026-09-11 删除** (真实 bStock 成交轨期望 ≈0:
    spot 91 天 每 bar −0.0198% / futures 220 天 +0.0367%; 七年 R1 数字含幸存者偏误不作证据);
    同批删除的还有 `bs_intraday_top5.lua`(日内 Top5, 成本算术否决)。证据见 specs/research/。
    **其引擎机制保留为通用能力**: 组合回测路径 `run_portfolio_backtest` + 组合信号模式
    (`universe` 键 / `signal_klines` / `SIGNAL_TAIL` 尾窗) + 任意 interval tick 对齐 `build_interval_ticks`,
    暂无内置消费者; 组合回测 CLI 入口随策略一并删除。
- 用户策略: `<项目根>/strategies/<name>.toml` + `strategies/scripts/<name>.lua`(git 忽略默认私有; builtin 例外)
- **单一配置文件**(019 D31): `$RICOW_ROOT/ricow.toml`(权限 0600, 进 `.gitignore`)—— `[ai]`(provider / model / base_url / max_turns / api_key)与
  `[exchange]`(demo_key / demo_secret / binance_key / binance_secret)同文件; 该文件**即界面**(无 `ricow keyring` / `ricow setup` / 写凭据命令), 未知键硬失败。
  目录顺序: `RICOW_ROOT` env > 当前目录(含 `ricow.db`/`strategies/`)> 平台数据目录。

## 五、CLI

命令集(**以 `ricow --help` 实测为准**, 2026-09-14):
`start [--demo]` / `stop [--close-all]` / `restart` / `list` / `status [name]` / `info` / `fills` / `logs` / `run [--live|--demo]` /
`backtest` / `ticker` / `orderbook` / `create` / `approve` / `deploy` / `db` / `daemon {start|stop|status|run}` /
**`ai`**(019: 内置 AI 助手, 交互 / 单次 / `--plain`)/ **`agent-kit`**(019: `ricow agent-kit [--install [目录]]` 生成给外部 agent 的手册 —— AGENTS.md / SKILL.md / CLAUDE.md / lua-api.md, 与内置 AI 同源); `mcp` **不做**(2026-09-15 定案)。

~~`keyring`~~ / ~~`setup`~~ / ~~`credentials`~~ / ~~`config`~~ —— 2026-09-14 随单一配置文件方案**全部删除**(019 D31: 文件即界面)。

- 建策略(002, 写操作不得一步落盘): `ricow create --name <n> --pair <p> [--script <file|->] [--param k=v] [--days N] [--interval] [--market spot|futures]`
  —— 编译门禁 → 真实 K 线沙箱回测 → 打印报告 + `preview_id`(**不写任何策略文件**);
  `ricow approve <preview_id>`(人工批准: 打印**确认块**(动作/目标/关键参数/后果), 要求**逐字输入** `确认部署 <策略名>` —— 裸 `y` 不接受, **且必须来自交互终端**(stdin 非终端即拒绝: 管道/脚本/agent 工具调用喂入的短语一律无效, 019 spec §七 R2 已实现); 通过后发一次性 token, 15 分钟有效)→ `ricow deploy <preview_id> --token <t>`
  —— 落盘 `strategies/<name>.toml`(`params.script_path` 指向)+ `strategies/<name>.lua`; 同名策略存在即拒绝(不覆盖)
- 首次使用风险确认(018, product.md §十): 实盘启动前一次性确认 —— 未确认时**拒绝启动**并打印披露要点(仅供学习/无止损与选品责任/先 Dry Run + 子账号小额/不代管资金密钥)与确认方式; `--accept-risk` 确认一次后记入 `$RICOW_ROOT/risk_ack.json`(带 schema 版本, 披露实质变更可递增触发重新确认)。判定顺序: **风险确认 → Dry Run 时长门禁 → 时钟预检**(未确认时零交易所往返); Dry Run/回测不受影响
- Dry Run 虚拟本金可配(016): `params.initial_cash`(缺省 100000) —— 用小资金同口径预演才能让 `[risk]` 限额同时适配 Dry Run 与实盘; 非法值报错不静默回落; 启动打印本金额
- Dry Run 起点与实盘时长门禁(002): 首次 Dry Run 启动时把 `dry_run_started_at`(ISO8601)写入策略 TOML;
  实盘启动要求 Dry Run 累计 ≥ `params.min_dry_run_hours`(默认 24 小时, 设 0 关闭), 不足则**拒绝启动**(不降级)

- 进程管理: `daemon start`(自后台化) / `daemon stop`(优雅停全部策略) / `daemon status`;
  `start|stop|restart <name>` 经 daemon 控制通道; `list|status` 为总览(daemon ∪ 台账 ∪ 部署清单), `status <name>` = 详情;
  `info <name>` 运行信息 + 成交统计; `fills [name] [--limit N]` 按 `strategy_id` 过滤; `logs <name> [-f] [--lines N]`
- 前台调试: `ricow run <name>`(Dry Run; 进程内监听 stdin `stop` / 管道 EOF / Ctrl-C 优雅停机, 不被 daemon 管理)
- 回测: `ricow backtest --strategy <名|类型> [--pair] [--days] [--interval] [--script] [--param k=v] [--market spot|futures] [--position-mode one-way|hedge] [--fee/--fee-maker/--fee-taker/--slippage-bps/--cash/--leverage/--max-leverage/--mmr-pct/--funding-rate]`(杠杆默认上限 10x,超限须 --max-leverage 显式放宽;MMR 默认按 symbol 内置首档表, 表外 1.0%)
  - 数据源按市场分支: 现货走交易所 REST; 合约 (futures) 走 fapi 公共数据源 (K 线; MMR 按 symbol 内置首档表,表外回落 1.0%)
  - 直跑模式: `--strategy {shannon_grid|dca|twap|vwap|pullback|ladder|lua}` — 内置脚本经 BUILTIN_SCRIPTS 常量表注入(后五项为执行模式示例, 需 --pair)
  - 部署模式: `--strategy <name>` 命中 `strategies/<name>.toml` 加载(`script_path` 引用文件或内嵌 `script`)
- 启动: `ricow start <name>`(经 daemon 后台运行) / `ricow run <name>`(前台调试; 默认 Dry Run, TOML `enabled=false` 拒绝启动)
- ~~选币: `ricow scan …`~~ —— **已于 2026-09-15 删除**(020-platform-scope-trim: 选币/研究入口与"运行策略的平台"定位正交; 用户拍板删除, 不留废弃代码)

## 六、数据布局

- `RICOW_ROOT`(数据目录, 决策 D4): 显式覆盖 > 当前目录已有 `ricow.db`/`strategies/` 时沿用现状 > 平台标准目录
  (Windows `%APPDATA%\ricow` / macOS `~/Library/Application Support/ricow` / Linux `$XDG_DATA_HOME|~/.local/share`+`/ricow`)
- `RICOW_DB`(默认 `RICOW_ROOT/ricow.db`): SQLite — `klines`(交易所 K 线缓存)/ `fills`(成交, 含 `strategy_id`)/ `pnl_snapshots`(盈亏快照)/ `previews`(写操作预览)/ `us_klines`(美股 Nasdaq 日线, 信号轨与交易日历; **缓存不回源, 需手工增量补最后若干天**)
- `RICOW_ROOT/run/`: `daemon.json`(daemon pid/端口/token, 权限 0600)/ `<name>.json`(实例台账: pid/启动时间/模式/上次退出码与原因)
  > ⚠️ 多进程共享同一 `ricow.db` 的并发写依赖 WAL + `busy_timeout`(sqlx 默认 5s): 实测 3 进程 × 200 事务在 busy_timeout=5s 下全部成功, =0 时失败 83%(`.hermes`→已归档 `specs/research/process-model-probe-2026-09.md` §四)。**不得把 `busy_timeout` 设为 0, 也不得把数据目录放在网络盘/云同步盘**(SQLite WAL 明确不支持网络文件系统)
- `RICOW_ROOT/logs/`: `<name>.log`(策略进程 stdout/stderr 追加日志; 启动时 >10MB 轮转 `.log.1`)与 `daemon.log`
- 密钥: OS Keyring + headless 加密文件 fallback(无 Secret Service 时)

## 七、安全模型

- 写操作强制确认: `create_strategy` 提交 → Lua 编译门禁 → 沙箱回测 → preview → `ricow approve`(一次性 token 重放)→ 部署运行
  > ✅ 现状(2026-09-13, 002 落地): `ricow create`(门禁 → 真实 K 线沙箱回测 → preview)→ `ricow approve`(人工批准, 一次性 token)
  > → `ricow deploy`(落盘 `<name>.toml` + `<name>.lua`, 同名拒绝覆盖)。引擎侧 `Engine::create_strategy_preview` /
  > `backtest_and_preview` / `ricow_engine::execute_strategy` 与 CLI 同源; AI 客户端只读 `specs/lua-api.md` 写代码, **无法自行批准**。
- Dry Run 时长门禁(002): 实盘启动额外要求该策略 Dry Run 累计 ≥ `params.min_dry_run_hours`(默认 24 小时 = 覆盖三个交易时段的一个完整日周期;
  设 0 关闭)。起点由首次 Dry Run 启动写入 TOML `dry_run_started_at`(ISO8601); 不足即拒绝启动, 并给出已运行时长与解除方式。
  Dry Run / 回测 / 沙箱不受此门禁影响。
- RiskEngine 下单前硬检查 — 回测 / Dry Run / 实盘**同一引擎同一装配**(`RiskEngine::from_config`), 三条 `place_order` 顶部统一拦截:
  - 静态限额(用户显式配置才启用): 最大持仓 / 单日最大亏损 / 最小订单 / 最大滑点 —— 平台不替用户定政策, 只执行用户写下的政策;
  - 工程护栏(默认启用): **下单频率上限**(滑动窗口 1s, 默认 100/s) —— 防风暴下单被交易所限流封禁;
  - ~~两级亏损熔断~~ **已于 2026-09-15 删除**(020): 盈亏政策属于策略, 平台不再代做投资判断; 策略用 `ctx:net_pnl()` / `ctx:equity()` 自管 (内置 `shannon_grid` 的 `dd_stop_pct` 为参考写法);
  - 被拒请求返回 `Rejected` ack(与资金不足同形)并计入回测报告"拒单次数", 同时 `tracing::warn!(target: "risk")` 输出规则名与关键数值; 平仓/减仓不受熔断限制。详见 `specs/backtest.md` §二.6 与 `specs/changes/004-risk-guards/`。
  - > 修正记录(2026-09-12): 004 之前 RiskEngine 的唯一调用点是无消费者的 `StrategyScheduler`, 三条真实下单路径**均未过风控** —— 即上述四条规则当时实际从未生效; 已随 004 接入。
- Dry Run 默认, 确认后切实盘(011 落地): 门禁**双条件** = TOML `live_enabled=true` **且** 命令行 `--live`(缺一即按 Dry Run 运行并打印原因; `ricow start` 同口径, 台账 `mode` 与实际运行器一致)
- **实盘二次分离**(019 D4/T029-T030): 双条件之外, 每次实盘启动必须在**交互终端逐字输入** `确认实盘 <策略名>`(裸 `y`/空/EOF 一律拒绝, 零副作用);
  确认只发生在父进程 CLI, daemon 协议 `Request::Start.confirmed` 缺失时**明确拒绝**(不静默降级), 子进程由 daemon 注入内部 `--live-confirmed` 不再索要 stdin。
  实盘三判据(018 风险披露确认 → 002 Dry Run 时长门禁 → 008 时钟预检)顺序与判据**未改动**。
- **demo 运行模式**(019 T062): `ricow run|start <name> --demo` —— 真实调用币安**模拟交易(demo)**平台下单接口(无真实资金),
  因此**不适用**实盘三判据, 但仍需 demo 凭据、并按 **demo 服务器**做时钟预检; `--demo --live` 同时给直接拒绝; 台账与 CLI 文案均标注 `测试网模拟盘(demo)`。
- 实盘启动前**时钟预检**: 本机超前交易所服务器 >1000ms 或滞后 >4000ms → 拒绝启动并打印对齐步骤(币安对签名请求超前 >1s 直接拒绝, 见 `specs/testnet.md`); 取数走公共 `/api/v3/time`
- 实盘/demo 凭据(019 D31): 来自单一配置文件 `$RICOW_ROOT/ricow.toml` 的 `[exchange]` 段 —— `binance_key/binance_secret`(实盘)与
  `demo_key/demo_secret`(币安模拟交易 demo)**两套独立槽位, 永不跨套回落**; 缺失即拒绝启动并**点名键名与文件路径**, 不静默用空凭据发请求。
  旧方案(env `RICOW_BN_*` / OS Keyring / `ricow keyring` / `credentials.toml`)已于 2026-09-14 全部移除 —— 不保留回退(keyring 在本机不可用)。
- 实盘下单参数由引擎按交易所过滤器**自动对齐**(数量按 `step_size` 向下取整 / 限价取"不劣于意图"的一侧 / 不足 `min_qty`·`min_notional` 拒单并如实报错); 订单号统一带 `<策略名>-` 前缀, 停机撤单**只撤本实例归属**的单, 非归属单只上报不撤
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
- **基线(2026-09-13, 018 实施后)**: `cargo test --workspace` = **308 passed / 0 failed / 11 ignored**(ignored = 需真实外部环境的联调用例, 不 mock 替代; 构成: BN demo 现货 4 + 合约 5 + Nasdaq 冒烟 2)
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
| 密钥 | 单一 0600 配置文件 `ricow.toml`(明文; 取舍理由见 §七 与 README) |
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
| O | 熔断/成交/强平/残留等高风险事件只落本地日志(product.md §三.4 的承诺未兑现) | ✅ 出站通知(`ricow_engine::notify`): 用户自选 webhook + 四类事件 + 白名单/限速/去重 + 失败只 warn 不反压循环; 配置走 params, 默认关闭 |

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
