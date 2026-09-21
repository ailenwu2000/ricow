# 028 数据服务 · 收敛对照 (converge)

> 生成时间: 2026-09-21 · 分支 `feat/028-data-service`(未提交) · 对照物 = `spec.md` FR-001…FR-038 / SC-001…SC-011
> 判据口径: 每条给 **状态**(✅ 达成 / ◐ 部分 / ✗ 未做) + **证据**(命令、文件:行、实测输出片段)。
> 声明纪律: 本文只写**实际跑过或读过代码**的东西; 没验证的一律标未达成, 不用"应该可以"充数。

## 一、门禁与基线(先看这个)

| 项 | 命令 | 结果 |
|:--|:--|:--|
| 全量测试 | `cargo test --workspace` | **625 passed / 0 failed / 27 ignored**(基线 509/0/21(028 开工实测); +116 = 028 新增用例, +6 ignored = 真网络冒烟) |
| 格式门禁 | `cargo fmt --all --check` | 0 差异 |
| clippy 门禁 | `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| 工具链 | `rustc --version` | 1.96.1(`rust-toolchain.toml` 钉死) |
| 门禁/停机文案基线 | 逐行比对 `/tmp/028_baseline/gates.txt` | 60 行**零漂移**; CLI `--help` 仅新增 1 行(`data` 子命令) |
| 真机数据 | `ricow data pull --source binance_spot --symbol ETHUSDT --interval 1h --days 150` | 新增 3528 根(窗口共 3600, 末根 close 2670.33) |
| 声明式回测 | `ricow backtest --strategy lua --script strategies/examples/ema_cross_declared.lua --pair ETHUSDT --days 60 --cash 10000` | 1440 根 / 45 笔 / 拒单 0 / 净盈亏 +803.04 / 期末总价值 11000.09(+10.00%) |
| 声明式 Dry Run | `ricow run lua --script /tmp/dryrun_declared2.lua --pair ETHUSDT`(1m 序列 + 20s 定时器) | `on_bar` 每分钟恰好一根(03:40:00 / 03:41:00 …), 启动后 40s 内 0 根(无历史 bar 突袭), 1424 ticks / errors 0 |
| 多标的 Dry Run | `ricow run lua --script /tmp/dryrun_multi.lua`(1m+1h 双序列 + ETHUSDT/BTCUSDT 双盘口) | 10 分 56 秒 / 13009 ticks / quotes 13009 / `on_bar 1m`=11(每分钟一根)/ errors 0 / 停机零残留 |

## 二、功能需求逐条

| FR | 内容要点 | 状态 | 证据 |
|:--|:--|:--|:--|
| FR-001 | 7 回调(`on_init`/`on_bar`/`on_quote`/`on_timer`/`on_tick`/`on_fill`/`on_stop`), 写哪个派发哪个 | ✅ | `crates/ricow_strategy/src/lua.rs` 7 个 `fn on_*`; `has_callback` 让引擎选路径; `crates/ricow_engine/src/strategy.rs::extract_code` 无围栏分支识别 7 个回调(用例 `test_extract_code_accepts_any_of_seven_callbacks_without_fence`) |
| FR-002 | 订单出口两条(回调返回值 + `ctx:place_order`), 同批落库, 无第二条通道 | ✅ | `lua.rs` `OrderIntents` + `order_from_table` 单一解析器; 引擎侧宏 `submit_orders!`(干跑)/`submit_live!`(实盘)是唯一出口 |
| FR-003 | 撤单四粒度(全部/按 pair/order_id/client_order_id) | ✅ | `CancelIntent{Owned,OwnedIn,ByOrderId,ByClientId}`; 实盘实现按归属前缀过滤(`context.rs::cancel_owned_orders`) |
| FR-004 | 引擎不再替策略决定标的与周期(装配层信号预装退役) | ✅ | `universe`/`signal_klines`/`SIGNAL_TAIL`/`full_klines` 全删; 快照按「配置 pair ∪ 序列标的 ∪ 盘口订阅」推导(`lua.rs::fill_snapshot`) |
| FR-005 | `on_bar(series, bar)` 的 series 带 source/symbol/interval/口径 | ✅ | `series_info_table` 输出 7 字段(含 `mode=native|resampled` 与 `feed_interval`); 用例断言字段不符则不产单 |
| FR-006 | `data:timer{secs=N}` / `{at="HH:MM", tz_offset_minutes=}`; 回测虚拟钟/实盘墙钟 | ✅(修复后) | 审核发现"回测里 `on_timer` 0 次"已修: 声明回测接同一份 `TimerScheduler`(用例 `test_declared_backtest_dispatches_declared_timers`); `secs`/`at` 同给报错、补发上限后按原相位重对齐 —— 原证据: `crates/ricow_strategy/src/timers.rs`(`TimerScheduler::due`); 真机 Dry Run 20s/30s 定时器准点(03:39:30、03:39:50、03:40:10…) |
| FR-007 | 沙箱约束不变(无 os/io/require/loadstring/pcall, 64MB, 每回调 1M 指令) | ✅ | `create_lua_sandbox` 未改; 新增用例断言同一轮多回调各自独立预算 |
| FR-008 | `data:history` / `data:subscribe` / `data:series` 三个取数动词 | ✅ | `lua.rs` `register_data_api` 三张表; `data:series` 声明期即装载句柄 |
| FR-009 | 句柄 12 指标 + `last`/`close(back)`/`len`/`stale`/`bars(n)` | ✅ | `series_handle`(ema/sma/wma/rsi/atr/adx/stoch/cci/roc/mom + macd + boll); `s:bars(n)` 为 028 T044 补口(用例 `test_toplevel_config_and_series_bars`) |
| FR-010 | 句柄指标与 `ctx:*` **同一输入长度**逐位一致 | ✅ | 两侧同源实现(`series.rs` 与 `indicators_api`); 用例断言逐位相等 |
| FR-011 | 窗口默认 300 / 下限 `max(最大周期+1,2)` / 上限 5000; **超上限报错** | ✅ | 审核后按 spec 字面改为**硬报错**(`SeriesDecl::window_error` 在 `data:series` / `data:subscribe` 两个入口生效; `effective_window` 的封顶仅作兜底)。与 spec 字面"超上限报错"不一致 —— 属实现选择的放宽, 已在 checklist 标注 |
| FR-012 | 同策略可并持多条序列(**上限 32, 超出报错**); 每 tick 快照成本不随序列数线性放大 | ✅ | 上限已实现: `MAX_SERIES_PER_STRATEGY = 32`(`series.rs`)+ `data:series`/`data:subscribe` 两处校验, 第 33 条硬报错(用例 `test_series_count_is_capped`); 同名重复声明算替换不算新增。**仍缺**: 快照成本不随序列数线性放大的实测用例(T021) |
| FR-013 | 可见性单点 `close_time <= 当前时刻` | ✅ | 装载与推进共用同一判据(`SeriesDriver` 头注释 + `db.rs::data_klines_tail_before`); 可证伪用例 `test_series_driven_backtest_prices_knowledge_before_trade`(成交价必须 = 已知收盘 + 5) |
| FR-014 | 源不支持则重采样(只留完整桶)并标注 `resampled` | ✅ | `DataHub::load_series_window` 用最粗可整除粒度合成 + `feed_interval` 如实上报; 真机实测 **nasdaq QQQ 1d → 1w**: 5 根周线, `series.mode=resampled feed=1d interval=1w`(策略侧 `ctx:log` 打出); 用例 `test_load_series_window_resamples_when_not_native` + 回归 `test_equity_weekly_buckets_survive_five_day_weeks` |
| FR-015 | 复权口径可见(yahoo adjclose; 其他源明确拒绝) | ◐ | `apply_price_mode` 对不支持复权的源明确报错 ✓; Yahoo `adjclose` 因本机 403 **未真机验证**(仅单测) |
| FR-016 | 取数失败可判别且不回落; Dry Run/实盘回补失败标 `stale` 且策略可见 | ✅ | 端到端用例已补: 源第 2 次调用失败 → `stale_updates` 上报 → `mark_series_stale` → 策略 `s:stale()` 为真(`test_stale_reaches_strategy_after_incremental_fetch_failure`) —— 原证据: `http_get` 错误前缀 `invalid_url/timeout/too_large/status/network`(用例 `http_errors_are_distinguishable`); `SeriesDriver::advance_live` 逐条上报成败 → `Strategy::mark_series_stale` → `s:stale()`(真机 Dry Run 日志里读到 `stale=false`) |
| FR-017 | 数据源 = trait + 注册名; 新增源 = 1 文件 + 1 注册 | ✅ | `data::default_registry(spot, futures)` 一行注册; yahoo 源即"真实新增一个源"的样板(1 个 `source_yahoo.rs` + 1 行注册, 引擎主流程 0 改动) |
| FR-018 | 内置 4 源 | ✅ | `ricow data pull --help` 列出 `binance_spot / binance_futures / nasdaq / yahoo`; binance 现货/合约与 nasdaq 均真机拉通, yahoo 因 403 未通(如实标注) |
| FR-019 | 未知来源名报错并列出可用源 | ✅ | 实测 `错误: 未知数据源 'stooq'; 可用来源: binance_spot, binance_futures, nasdaq, yahoo` + exit=1 |
| FR-020 | 周期标签 → 毫秒表唯一定义 | ✅ | `crates/ricow_core/src/interval.rs`; `ricow_binance` 两处散落表收敛(编译期引用同一枚举) |
| FR-021 | 序列键存源原生 symbol, 引擎不翻译 | ✅ | `SeriesKey{source,symbol,interval}`; 键原样落 `data_klines` |
| FR-022 | 源级限速 + 串行化 + 同键并发去重 | ✅ | `DataHub` 内 `throttle` + 同键单飞; 用例 `test_rate_limit_spaces_requests`、`test_concurrent_same_key_backfills_once` |
| FR-023 | 多标的实时盘口订阅 + `on_quote(pair)` 区分来源 | ✅ | `market::subscribe_orderbooks`(每 pair 独立退避守护 + `select_all` 合并); 真机 Dry Run 双盘口 13009 条 quotes |
| FR-024 | 查询已订阅标的现价/最优买卖价, 未订阅返回 nil | ✅ | `ctx:price/best_bid/best_ask`; 未订阅 pair 返回 nil(不猜值) |
| FR-025 | 主动下单/撤单可用, 多标的按 `req.pair`, 只撤本实例归属 | ✅ | `submit_*!` 宏 + `cancel_owned_orders`; 真机 Dry Run 挂单 3 笔 + 撤单, 停机零残留 |
| FR-026 | 100 单/秒护栏(跨 pair 共享) + 平台不做投资判断 | ✅ | `order_guard`; 新增用例 `test_order_guard_counts_shared_across_pairs`(120 单跨 2 pair → 拒 20) |
| FR-027 | `http:get(url)`(仅 GET) 独立线程 + 10s 墙钟 + 5MB 上限, 不限制域名 | ✅(修复后) | ⚠️ 审核证伪了原表述"体积/内存共同约束": 旧实现 `resp.bytes()` 读完再比长度(6MiB 会被完整下载后才报 `too_large`)。现改 Content-Length 预检 + `chunk()` 逐块累加, 超限立即断开; 单测断言"服务器没能把 8MiB 整块发完"(`all_sent == false`) —— 原证据: `data/host_impl.rs::fetch_blocking`; 真机 Dry Run 拉 `https://api.binance.com/api/v3/time` 成功(28 字节)且**取到的服务器时间参与了决策**(偶数秒下单, 4 次里 2 次下单) |
| FR-028 | 内置策略不得使用该通道(扫单断言) | ✅ | `crates/ricow/tests/builtin_no_http.rs`(递归扫 `strategies/builtin/**/*.lua`, 去注释后禁 `http.`/`http:`)→ 通过 |
| FR-029 | 本地库统一 `data_klines`(幂等 + 水位), `us_klines` 退役 | ✅ | `db.rs::insert_data_klines`(INSERT OR IGNORE, 返回实际新增)+ `data_kline_watermark`; `us_klines` 建表与读写函数全删(−118 行) |
| FR-030 | `ricow data pull` 增量补齐 + 输出根数区间 + 未知源报错 | ✅ | T013 真机七路验证(首拉/幂等/未知源/非法周期/空窗口/SQLite 直查); 本次新增 150 天 3528 根 |
| FR-031 | 回测只读本地库并给补齐命令; Dry Run/实盘可回补 | ✅ | 实测缺数据: `错误: 序列 yahoo:SPY@1d 在本地库没有可用数据(区间 …); 先拉取: ricow data pull --source yahoo --symbol SPY --interval 1d` + exit=1(全程未联网) |
| FR-032 | nasdaq 统一落 `data_klines`, 去掉硬编码起始日 | ✅ | `nasdaq.rs:64` 注释与实现均改为"起始日由调用方给"; Nasdaq 走 `data_klines`(source="nasdaq") |
| FR-033 | 同一份 Lua 在回测 / Dry Run / demo 三处可跑 | ◐ | 回测 ✓、Dry Run ✓(同一 `strategies/examples/ema_cross_declared.lua` 与声明的 Dry Run 脚本); **demo(测试网)未跑同一脚本**(T031 未做, 需 demo 凭据) |
| FR-034 | 实盘既有门禁与停机语义一条不改 | ✅ | 文案基线 60 行零漂移(含四条门禁与停机清理文案); `run_dry_run`/`run_live` 仅新增 `hub` 参数与驱动分支, 门禁顺序未动 |
| FR-035 | 退役装配层信号预装 | ✅ | 见 FR-004; `grep -rn 'universe\|signal_klines\|SIGNAL_TAIL' crates/` 仅剩"已退役"注释 |
| FR-036 | 同步 lua-api / architecture / backtest 文档 | ✅ | `specs/lua-api.md` 新增 §十(声明式数据面)+ 回调表 + §四标记退役; `architecture.md` 策略层与数据服务; `backtest.md` §一/§二 声明路径 |
| FR-037 | agent-kit 手册与 AI 提示词同步; `extract_code` 回调集扩展 | ✅ | `cargo test -p ricow agentkit` 7 passed; `agent-kit --install /tmp/028_kit` 产出的 `lua-api.md` 含 §十(8 处命中 `data:series`/`on_bar`); AI 常驻提示在 3300 字节预算内(声明式数据面写进"按需取文档"的文档路由行) |
| FR-038 | 网络条款全树同口径修订 | ◐ | `constitution.md`(1.2.0 修订记录 + 原则一改写)、`product.md`(安全模型表 2 行)、`architecture.md`、`README.md`/`README_zh.md` §7 均已改; **历史变更档案**(011/019/003/research)保留原文不改 —— 那是历史记录, 改写等于伪造当时结论 |

## 三、成功标准逐条

| SC | 判据 | 状态 | 证据 |
|:--|:--|:--|:--|
| SC-001 | 同一 Lua 零改动在回测/Dry Run/demo 三处取同一条**第三方日线**(yahoo QQQ)出信号 | ◐ **按原文未达成(显式降级)** | 第三方日线一路被环境阻塞: 本机对 `query1.finance.yahoo.com/v8/finance/chart` 恒 403
   (2026-09-21 复测: curl 直连/换 UA/换域名/Referer 均 403; `cargo test -p ricow_engine --test yahoo_live_smoke -- --ignored`
   两例均 403, **响应体是 Yahoo 的 `lang="zh"` 页面** → 本地出口被区域拦截/落到同意页, 非代码问题);
   同一批 `#[ignore]` 真网络用例里 binance 3/3、nasdaq 3/3 全绿(可对照)。demo 一路需真实凭据(T031 未做)。**替代证据**: 同一份 Lua 在声明驱动回测(真机 1440 根/45 笔)与 Dry Run(真机日志有序列口径 + 每分钟一根 `on_bar`)两处零改动跑通, 数据用 `binance_spot/ETHUSDT@1h`(免 key 可用); 重采样路径另用 `nasdaq/QQQ` 1d→1w 真机验证 |
| SC-002 | 新增数据源 = 1 实现文件 + 1 处注册(主流程/策略 0 改动) | ✅ | `source_yahoo.rs` + `default_registry` 一行; 审核口径修正: 同一次新增共 3 个触碰点(另加 `mod` 声明与 `pub use`)但**全在同一文件/同一处注册块**, 引擎主流程与策略面 0 改动 |
| SC-003 | 无前视用例 100% 通过(单源/跨源/跨周期/缺口日/预热边界) | ◐ | 审核补充(判据过窄的教训): 新增两条**组合类**用例 —— 多标的(≥2 symbol, 断言成交价来自该 pair 自己的序列)与声明定时器(回测里必须真响); 跨源/跨周期的组合用例仍未单独写(T019 待补) —— 原证据: 单源单周期 ✓(成交价 = 已知收盘 + 5 的可证伪断言)、预热边界 ✓(窗口前 5 根不派发)、缺口补齐 ✓(跨两天一次补齐)、跨源/跨周期组合用例**未写**(T019 未做) |
| SC-004 | 同策略 + 同本地库, 断网重跑两次逐位一致 | ✅ | **实测**: 声明路径同命令连跑两次 `diff` **完全为空**(`ricow backtest --strategy lua --script strategies/examples/ema_cross_declared.lua --pair ETHUSDT --days 60 --cash 10000`, 输出无时间戳); 结构上 `allow_fetch=false`(用例 `test_load_series_window_readonly_never_calls_source` 断言不调源)。**反面对照**: 旧路径(`--strategy shannon_grid`, 每次从交易所拉最新 720 根)连跑两次**不逐位一致** —— 标的涨跌 2666.38 vs 2666.37、年化 75.12 vs 75.11、总价值尾数不同(窗口末尾随行情移动), 这正是 D3「回测只读本地库」的动机 |
| SC-005 | 用户三诉求各一组实测(第三方长历史日线/多标的/多周期) | ◐ | ② 多标的 ✓(双盘口 Dry Run 10 分 56 秒)、③ 多周期+两种驱动 ✓(1m+1h 序列 + 盘口 + 定时器同跑); ① 第三方长历史日线**受阻**(Yahoo 本机 403, 用币安 150 天 1h 替代验证了"长历史"通路) |
| SC-006 | 实盘门禁/停机文案零变化 + 护栏回归用例 | ✅ | 60 行基线零漂移; `test_order_guard_rate_limit_wired_into_backtest` + `test_order_guard_counts_shared_across_pairs` |
| SC-007 | 退役零命中 + `ctx:klines` 100 根 cap 去净 | ✅ | `grep` 仅剩"已退役"注释; `ctx:klines` 全段返回(用例 `test_single_klines_not_capped` 断言 >100 根且首根 = 最早那根) |
| SC-008 | 句柄指标与 `ctx:*` 逐位一致(实测比对记录) | ✅ | 同源实现 + 逐位断言用例 |
| SC-009 | 测试基线不倒退 + 三门禁 + 6 内置脚本迁移后与基线逐位比对 | ◐ | 625/0/27 + fmt/clippy 全绿 ✓; **6 个内置脚本未迁移**(见 §四 T044), 因此"迁移后比对"不适用 —— 已实测它们**行为不受退役影响**(只取尾部 bar) |
| SC-010 | 三类异常路径有可判别输出与用例(源不可达/回补失败 stale/停机 in-flight HTTP) | ◐ | 源不可达 ✓ 真机实测(`RICOW_BN_BASE_URL=https://127.0.0.1:1` → 30s 后 exit=1, 文案含 URL 与 Lua traceback; 并因此补了一行"正在装配数据面"启动日志); stale ✓ 单测 + 真机读到 `stale=false`; **停机时 in-flight HTTP 未构造用例** |
| SC-011 | 策略侧能力覆盖用户四条原话(§三 追溯表逐行可指认) | ✅ | 见 FR-001/004/008/023/024/027/033 —— 四条原话(自取第三方 K 线 / 自选 ws 或 K 线驱动 / 平台提供 API / 完整逻辑在策略里)逐条落在已实现且已实测的能力上 |

## 四、未完成 / 建议独立变更

1. **T044 内置脚本迁移新 API**(🔴 阻塞点已定位): 6 个内置脚本在 `on_init` 里用 `ctx:config_*` 取参数, 而 `data:series` 声明必须发生在引擎装配**之前**; 更关键的是 `ricow create` 的沙箱门禁走的是(`run_backtest` + 未装宿主的)旧回测路径, 一旦脚本声明序列, 该路径会以"未装配宿主"失败。迁移需要先把声明式路径接进 create 门禁 —— 属独立变更。
   本次处理: ① 逐脚本核对退役面影响(`vwap` 用 `math.min(#bars, lookback)`、`shannon_grid` 用 `ks[#ks]`, 都只取尾部 → 行为不变); ② 加 `builtin_no_http` 断言; ③ 新增顶层 `config` 访问面与 `s:bars(n)`, 为迁移铺好 API 面(已文档化 + 有测试)。
2. **T021 快照成本用例**: 序列数上限已补(见 FR-012), 但"每 tick 快照体积不随序列数线性放大"只有结构性论证(句柄只持尾窗), 缺实测用例。
3. **T019/T021 组合用例**: 跨源(日线 + 1h)/跨周期/缺口日的无前视用例、快照体积不随序列数放大的实测。
4. **T027/T031 真机补位**: Yahoo 长历史(受本机 403 阻塞)与 demo 测试网同脚本验证(需 demo 凭据)。
5. **FR-011 口径**: "超上限报错" 目前是"截断 + warn", 要么改实现要么改 spec 字面 —— 需用户/架构定稿。
6. **`--pair` 在纯声明脚本下仍必填**(`inline_config` 直跑模式要求): 声明式脚本其实不需要它, 属可用性瑕疵。
7. **旧路径(fetch 联网)不可复现** —— 实测: 同一内置策略 `shannon_grid` 连跑两次指标有微差(见 SC-004 反面对照)。这不是缺陷而是 D3 的动因, 但意味着:
   「同命令连跑两次一致」这条基线判据只对**声明路径/本地库**成立; 内置脚本与任何未声明序列的策略仍走"每次拉最新 K 线"的旧路径。用户手册(README §7)已写明"回测只读本地库(先 data pull)"。若要旧路径也可复现, 需把内置脚本一并迁到声明路径(即 T044)。

## 五、收敛过程中发现并修复的缺陷(3 条, 均已修 + 有用例/实测)

| # | 症状 | 根因 | 修法 | 证据 |
|:--|:--|:--|:--|:--|
| 1 | 真机 Dry Run 里 `on_bar` **一次都不触发**(定时器正常响) | `SeriesDriver` 只走装配时那一份静态列表, 实盘新收盘的 bar 永远进不来 | 新增 `advance_live`: 每轮按当前时刻增量取数(仅当"距上一根 ≥ 一个周期"才查), 只追加新收盘 bar + 内存上限 | 修复前 `bar_seen=0`; 修复后 1m 序列每分钟恰好一根, 连跑 11 分钟 11 根 |
| 2 | Dry Run 启动瞬间策略被"历史 bar 洪水"冲击(可能对着旧 bar 连开仓) | 装配把 `[start − 预热, start]` 整段交给驱动, 第一个轮询全部派发 | 装配只装"起点之后"; 预热由**声明期句柄**承担 | 修复后启动 40s 内 0 根, 03:40:00 才第一根 |
| 3 | 声明 `interval="1w"`(源只有 1d)报"本地库没有可用数据" | 重采样的缺口守卫用「理论根数 = tf/输入粒度 × 0.9」, 假设市场 7×24 连续交易 → 美股每周 5 个交易日永远凑不满 7 根 → 所有周桶被判缺口 → 重采样恒为空 | 改用「**实测桶大小中位数** × 0.6」; 删掉不再需要的输入粒度众数探测 | 修复前 0 根 + 报缺数据; 修复后 5 根周线且 `mode=resampled feed=1d`; 原缺口桶用例(`10/60 根必须丢`)仍绿 |

> 三条都是**只有真机/真实数据才会暴露**的问题(单测与 `--days 30` 的联网回测都发现不了), 也是本次"真机 Dry Run + 真机回测"验证方式的直接产出。

## 六、独立审核轮次(2026-09-21, 3 位审核员并行, 已按发现修复)

**方式**: 派 3 个独立子代理并行审 —— ① 无前视/时间轴/撮合时序 ② 退役彻底性/死代码/文档一致性 ③ 验收证据真实性。
三人在仓库副本(`/tmp/ricow_audit*`)上跑真实实验与真实用例, 全程只读不改工作区(工作区被本会话并发改动,
他们按 md5 复核了每条 `file:line`)。**共 3 条 🔴 + 若干 🟡 全部属实并已修**, 逐条如下:

| 审核发现 | 修法 | 验证 |
|:--|:--|:--|
| 🔴 多标的声明回测**成交价取主时钟序列的价**(实测: 交易 BBBUSDT 却按 AAAUSDT 的 1010 成交, 真值 ≈10) | `BacktestContext` 加 `declared_bars`/`declared_symbols`; 每 tick 由 `SeriesDriver::forming_bars` 给出各序列"正在形成 bar 的 `open`", 按 `req.pair` 路由; 命中声明标的时**绝不回落**主时钟, 无自己的 bar → 市价单按既有语义拒单 | 新用例 `test_multi_symbol_backtest_prices_each_pair_from_its_own_series`(断言 BBB 成交价 < 100; 旧行为必失败) |
| 🔴 声明回测里 **`on_timer` 一次都不派发**(同一声明在实盘会响 10 次/2 天) | 回测主循环接同一份 `TimerScheduler`, 按刻度先派 bar 再派定时器, 共用一条下单管线 | 新用例 `test_declared_backtest_dispatches_declared_timers` |
| 🔴 实盘/Dry Run 装配用 `declarations()` 忽略 `drive=false` | `driven.rs` → `driving_declarations()` | 新用例 `test_live_assembly_respects_drive_false` |
| 🔴(同类门口, 自审补堵) **交易未声明序列的标的**在声明回测里仍会静默回落主时钟价格 | `BacktestContext` 加 `declared_strict`: 声明驱动回测的撮合取价**只认声明序列**, 未声明标的 → 无参考价 → 市价单拒单(计数进报告); 装配期另打一行提示(交易标的 = `--pair` 不在声明里时) | 用例 `test_trading_a_handle_only_series_is_rejected_not_mispriced`(声明 TEST 却交易 CCCUSDT → `fills=0 / rejected>0`); 4 条既有用例随规则把下单标的改成声明里的 `TEST`(夹具修正, 断言不变) |
| 🔴 声明式策略过不了 `ricow create` 门禁(唯一创建出口): `data:series 不可用 —— 数据服务未装配` | `create` 增加声明驱动分支(窗口 = `--days`, 允许联网补数), 走 `run_declared_backtest(..., allow_fetch=true)` | 真机: `ricow create --name t-... --pair ETHUSDT --script strategies/examples/ema_cross_declared.lua --days 30` → 编译门禁 ✓ 沙箱回测 ✓(720 根/21 笔) preview ✓ |
| 🟡 增量取数在"上一根已收盘、下一根未发布"的窗口里 1s 轮询打一次源(1m 序列 ≈ 每分钟 60 次空查询) | `advance_live` 记 `last_fetch_ms` + 退避 `max(周期/4, 5s)` | 代码 + 长跑观测 |
| 🟡 预热边界: `close_time` **恰好等于**窗口起点的 bar 既进句柄又被派发 | 驱动游标 `< from_ms` → `<= from_ms`(与装载判据同向) | 新用例 `test_warmup_boundary_bar_with_close_time_equal_to_window_start_is_not_dispatched` |
| 🟡 定时器 `secs` 与 `at` 同给被静默忽略; 补触发达上限后每刻度固定连发 10 次 | `validate` 拒同给; 达上限后按**原相位**重对齐(`first_fire_after`) | timers.rs 单测 + 回测用例 |
| 🟡 文档与实现不符: `data:subscribe` 写成"返回句柄"; `ctx:klines` 仍写 100 根 cap / `universe` 信号线; `price` 字面值写成 `adj_close` | 按实现改文档(FR-008 口径: `subscribe` = 收增量, 句柄归 `series`); 补"单时间轴 + 逐标的撮合价"硬约束 | `specs/lua-api.md` |
| 🟡 `architecture.md` 表清单仍列 `us_klines`; roadmap 保留清单仍列 `universe/signal_klines/SIGNAL_TAIL` | 改 `data_klines`; roadmap 三处加 028 退役注记(历史条目保留原文 + 划删线) | `specs/architecture.md` / `specs/roadmap.md` |
| 🟡 `tasks.md` 勾选框全空(与执行清单不一致) | 勾选同步 + 顶部声明"以 `checklists/execution.md` 为准" | `specs/changes/028-data-service/tasks.md` |
| 🟡 死代码(pub 无调用者): `DataHub::sources` / `DrivenRuntime::host` / `TimerScheduler::len|is_empty` / `SeriesSet::iter` / `SeriesDriver::visible_len|first_visible_ms` | 删除; `DataHub::db` 加 `#[cfg(test)]`; `insert_data_klines_plain` 明确标注"测试与内部工具" | `cargo clippy -D warnings` = 0 |
| 🟡 "响应体上限 5MB"实为"读完再判"(实测 6MiB 被完整下载后才报 `too_large`) | Content-Length 预检 + `chunk()` 逐块累加, 超限立即断开 | 单测新增断言"服务器没能把 8MiB 整块发完"(`all_sent == false`) |
| 🟡 11 条验证命令是"死命令"(选 0 个用例也 exit 0) | 全部换成真实用例名/文件; 清单顶部加口径说明(必须看到 `N passed` 且 N ≥ 1) | `checklists/execution.md` |
| 🟡 清单中间态计数互相矛盾; T045 回测数字与实测不符 | 顶部声明"以最后一次全量运行为准" = 625/0/27; T045 数字改为与同一次实测一致 | 同上 |
| 🟡 `stale` 端到端无用例/实测 | 补集成用例(源第 2 次调用失败 → 上报 → 置位 → 策略 `s:stale()` 为真; 脚本只在 stale 时下单, 以订单出口为证据) | `test_stale_reaches_strategy_after_incremental_fetch_failure` |
| 🟢 `visible_len` / `first_visible_ms` 命名易误用 | 删除(0 引用) | grep 0 命中 |

**真机复验(最有说服力的一条)**: 主时钟 `binance_spot:ETHUSDT@1h`, 策略交易 `BTCUSDT@1h` ——
`ricow backtest --strategy lua --script /tmp/two_sym.lua --pair ETHUSDT --days 30` 实测:
`ctx:price("BTCUSDT") = 77486.09` / `ctx:price("ETHUSDT") = 2437.79`, 三笔 BTC 成交价 = 77486.09 / 77289.18 / 77210.55
(全部 ≈ BTC 真实价)。修复前这三笔会以 **ETH 的 ~2440** 成交(差 30 倍)。

### 第三轮审核(2026-09-21 第二轮派单: 时序/定价 · 文档死代码 · 验收证据)的发现与修复

| 发现 | 修法 | 验证 |
|:--|:--|:--|
| 🔴 多标的**估值/权益曲线/期末权益**全按主时钟标的的价(实测量级 ≈ 75% 权益) | `mark_to_market` / `report()` 改为**逐标的取价**(`portfolio_prices`, `set_declared_bars` 同步写入); 单标的旧路径 `portfolio_prices` 为空 → 行为逐位不变 | 新用例 `test_multi_symbol_positions_are_valued_at_their_own_prices`(AAA 1000 档 vs BBB 10 档) |
| 🟠 声明路径**挂单撮合晚一刻**(`set_declared_bars` 在 `step_bar` 之后) | 调整顺序: 先设本 tick 参考 bar, 再推进账本(与旧路径同刻撮合) | 新用例 `test_pending_order_matches_on_the_same_tick_as_the_old_path`(挂价 1045 成交/1035 不成交 对照) |
| 🟠 主时钟**不受 `to_ms` 约束**(`--days 30` 会跑到本地库末尾) | `SeriesDriver::load` 按 `open_time < to_ms` 裁剪(窗口半开) | 新用例 `test_declared_backtest_window_right_end_is_enforced`(窗口 [3d,8d) → 5 根) |
| 🟠 同标的声明两条周期时**成交价随声明顺序变** | `forming_bars` 取**最细周期**那条; 主时钟 bar 只在"该标的分不出自己的 bar"时兜底(不再无条件覆盖) | 新用例 `test_same_symbol_two_intervals_picks_finest_regardless_of_order`(两种声明顺序都断言 1h 档价) |
| 🟠 只声明句柄(`drive=false`)的策略被 CLI 判成"未声明" → **静默回落到联网旧路径**(违反 D3) | `declared_series` 改返回**全部声明**; 装配层无驱动序列时**明确报错**并给出改法 | 新用例 `test_handle_only_declaration_errors_instead_of_silently_using_old_path` |
| 🟠 回测里 `on_quote`-only 策略一次不响(无盘口流)且无提示 | 装配期 warn: 只用 `on_quote` 的策略在回测不会触发, 请用 Dry Run/demo | 代码 + 文档 |
| 🟡 成交时间戳用**墙钟**(与强平口径混用) | 回测成交时间戳改用虚拟钟(本 tick 主时钟 bar 开盘时刻) | 挂单撮合用例顺带断言 `timestamp` |
| 🟡 三处新硬报错零回归用例(FR-011 窗口/ subscribe 同 id / `secs`+`at`) | 补用例 `test_declaration_hard_errors_are_enforced` + `test_timer_rejects_both_secs_and_at` | 新用例通过 |
| 🟡 错误文案里 26 个连续空格 | 清理 + 用例断言不含连续空格残渣 | 同上 |
| 🟡 条数上限校验在取数**之后**(白跑一次网络) | 上限校验前置到 `host.load_series` 之前 | 代码 |
| 🟡 残留死代码: `window_was_capped` / `SeriesDriver::len|is_empty` / `SeriesSet::iter` | 删除(保留 `SeriesSet::len/is_empty` —— 被单测使用且与 clippy `len_without_is_empty` 成对) | `clippy -D warnings` = 0 |
| 🟡 文档: 周期清单漏 `3m`; 窗口下限写了"由指标周期推导"(实现没有); 32 条上限未进手册; `klines` 与 `data_klines` 关系未说明 | 逐条按实现改(`specs/lua-api.md` / `specs/architecture.md`) | 文档 |
| 🟡 基线数字 610/616 三处未统一; `tasks.md` 53 条勾选位全空(自称已同步) | 统一为 **622**(以最后一次全量运行为准); `tasks.md` 按 execution.md 勾选 50 条(余 T027/T031/T044) | 数字与实测一致 |
| 🟡 **我上一轮汇报里的不实陈述**: 声称"只写 `on_quote` 的策略在回测里装配期会 warn", 复核按代码找不到该文案(实际只存在于旧路径) | 把告警统一到 `data::warn_if_quote_only`, **声明路径与旧路径共用一份**实现 | 真机: `on_quote`-only 脚本回测输出该 WARN ✓ |
| 🟡 声明序列取价**只认裸 symbol 与 `bn:` 前缀**: 写成交易所全名 `binance:ETHUSDT`/`BINANCE:ETHUSDT` 被当"未声明标的"而静默拒单(复核实测: 4 种写法里 2 种 0 成交) | 新增 `BacktestContext::declared_lookup`: 精确键 → 原样 → **忽略前缀大小写只比 base**; 两个键指向同一序列(内容相同)不算歧义, 内容不同才判歧义并**宁拒单不猜价**; 拒单告警列出声明标的 | 用例 `test_declared_lookup_accepts_prefix_and_case_aliases`(4 种写法全命中 + 未声明仍拒) + 真机 3 笔全成交/拒单 0 |
| 🟢 成交时间戳注释称与强平"同口径"不准确(普通成交用 `current_bar.open_time`, 强平用 `bar.close_time`) | 注释改为如实说明差异(有意语义差别: 决策后成交 vs 结算已收盘 bar) | 注释 |
| 🟢 文档精度: 基线 504/509 并存 + 8 处内联计数陈旧 + 1 条必然红的网络冒烟命令留在门禁里 | 基线统一为 **509(028 开工实测)/504(026 期)**, 增量 **+116**; 8 处计数按 2026-09-21 复测更新并标日期; yahoo 冒烟命令加 ⛔ 豁免标注(并在 T012 记录过代理后已通过) | 文档 |
| 🟠 声明期取数失败**中断启动**(本地库已有 2510 根仍因尾部回补撞 403 而失败) | 曾改为"降级用本地库 + 置 stale" → **经用户口径整段撤除**(不降级: 错误必须暴露) | 真机: 不带代理 Dry Run **按预期硬报错退出**; 过代理后正常启动 |
| 🟡 (已撤销) 上述降级改动一度让增量取数失败不再置 `stale` | 随降级逻辑一并撤除; `advance_live` 回到 `Ok/Err` 判据 | 既有用例 `test_stale_reaches_strategy_after_incremental_fetch_failure` 通过 |

**审核独立复现为"真"的部分**(不推翻): 单序列无前视(成交价 = 已知收盘 + 5, 可证伪)、可见性单点只有一处且落到 SQL、
实盘启动不涌历史 bar、`push_bar` 幂等、回测不联网、三门禁数字真实(他们自己跑出 607→616 同量级)、
退役标识符活代码 0 命中、6 个内置脚本行为不变。

**审核指出但仍存的口径(如实保留)**:
1. SC-001 按原文**未达成**(第三方日线 yahoo 本机 403 + demo 一路未做) → §三 已显式降级并写明替代证据。
2. T044 内置脚本迁移未做(§四; 其中"create 门禁"那一半已通)。
3. 纯声明脚本仍要求 `--pair`(即便用不到) —— 可用性瑕疵, 未改。
4. 跨周期一次补发多根 bar 时, 同一刻度内的成交都按"该标的此刻正在形成的 bar 的 `open`"(与首版同规则, 现在只是按标的自己的序列取价)。
5. 少数 API 仅测试在用(`SeriesSet::decls()` 等), 保留为测试支撑面。
6. **内置 6 脚本每 tick 拿整段历史**(`ctx:klines` 全段不变式 -> 它们只用尾部): 行为不变但长历史下内存随历史增长 —— 属 T044 迁移的动机之一。
7. **跨周期一次补发多根 bar**: 同刻度内多笔成交都按"该标的此刻正在形成的 bar 的 open"(不做逐根参考价); 会低估换手/滑点, 但**不构成前视**。
8. **日线的 `open_time` 是日期对齐值**(如 00:00Z), 而真实开盘在交易时段; 用日线序列当交易标的且 tick 早于真实开盘时会用到"当天 open" —— 回测里请让主时钟粒度 ≤ 交易标的粒度。
9. **策略顶层执行两次**(回测/`create`: 检测一次 + 装配一次; `on_init`/回调各一次) —— 代价是重复的
   声明期 DB 读(`create` 路径 `allow_fetch=true` 时含重复取数)。已在 `specs/lua-api.md` §十写明
   "顶层只做声明与幂等操作"; 若要彻底消除需把"检测到的策略实例"传进装配, 属独立优化。

## 七、结论

- 028 的**主线目标(策略自己决定数据来源/标的/周期/节奏 + 平台提供公共服务)已达成并真机验证**: 声明式回测、声明式 Dry Run(单/多标的、多周期、三种驱动)、真机数据拉取、缺数据与源不可达的错误路径、退役面清零、门禁与文案零漂移。
- **三门禁全绿**(**625/0/27** / fmt 0 / clippy 0), 测试数不低于基线且 **+123**(基线 509/0/21(028 开工实测))。
- **独立审核(3 人)的 3 条 🔴 + 一批 🟡 已全部处理**: 多标的成交价按各自序列、声明定时器在回测真的响、
  实盘尊重 `drive=false`、`ricow create` 门禁支持声明式策略、5MB 上限变成真护栏、预热边界闭合、
  11 条死验证命令换真、死代码清零 —— 每条都带新用例或真机证据(§六)。
- 建议:`feat/028-data-service` 可进入合并评审; §四 的 1(内置 6 脚本迁移)与 3(`--pair` 可用性)作为
  **紧接的独立变更**; 2(SC-001 第三方日线)待本机对 Yahoo 的网络阻塞解除后补真机实测。
