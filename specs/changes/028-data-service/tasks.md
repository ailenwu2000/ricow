---
description: "028 策略自取数据 —— 数据服务 + 策略运行时: 实施任务清单"
---

# 任务: 028 策略自取数据 —— 数据服务 + 策略运行时

**输入**: [spec.md](./spec.md) | [plan.md](./plan.md) | [research.md](./research.md)

> **执行状态以 [checklists/execution.md](./checklists/execution.md) 为准**(逐条证据/命令/实测记录)。
> 本文件的勾选与之同步: T027(Yahoo 长历史, 本机 403 阻塞) / T031(测试网 demo, 需凭据) / T044
> (内置 6 脚本迁移, 见清单里的阻塞点) 三条未完成, 其余已完成 —— 收敛对照见 [converge.md](./converge.md)。

**前置条件**: plan.md(必填)、spec.md(用户故事必填)、research.md

**测试**: 依据宪法原则三 —— 纯逻辑(周期表 / 重采样 / 可见性 / 句柄指标 / 缓存幂等 / DataHub 三态 / HTTP 错误)用单元测试; 交易链路(下单 / 撤单 / 成交 / 停机清理)走币安 demo 真实调用; 第三方数据源真实端点拉取, **不用假响应替代**; 需真实外部环境的用例放 `crates/<crate>/tests/*_live.rs` 并标 `#[ignore]`。

**组织方式**: 阶段 2/3 是阻塞性地基, 阶段 4~6 按用户故事分组(各自可独立验证), 阶段 7 退役与回归面同步必须同批做完。

## 格式: `[ID] [P?] [故事] 描述 —— FR/SC 溯源`

- **[P]**: 可并行(不同文件, 无依赖)
- **[故事]**: US1(P1 策略自取 K 线) / US2(P2 策略自选驱动方式) / US3(P3 用户策略自取 URL)
- 描述含精确文件路径; 行尾标注该任务覆盖的 FR / SC(供 converge 逐条对账)

## 路径约定

- 生产代码: `crates/{ricow_core,ricow_binance,ricow_strategy,ricow_engine,ricow}`; 测试与被测代码同文件(`#[cfg(test)]`), 真实外部用例 `crates/<crate>/tests/*_live.rs`
- 内置脚本: `strategies/builtin/shannon_grid.lua` + `strategies/builtin/executors/{dca,twap,vwap,pullback,ladder}.lua`(共 6 个, **编译期 `include_str!` 嵌入**, 改动必须重编译; 登记表见 `crates/ricow/src/commands/templates.rs`)
- 数据目录: `RICOW_ROOT`(默认当前项目根); 缓存 `ricow.db`

---

## 阶段 1: 搭建 (共享基础设施)

- [x] T001 仓库根执行 `cargo test --workspace`, 记录 passed / failed / ignored 基线 —— SC-009
- [x] T002 记录 6 个内置脚本的回测基准: `strategies/builtin/shannon_grid.lua` 与 `strategies/builtin/executors/{dca,twap,vwap,pullback,ladder}.lua`(smoke 脚本 `ricow_strategy/tests/*_live.rs`) —— SC-009
- [x] T003 记录实盘门禁与停机回执的现有文案与退出码(逐条列表), 作为 SC-006 比对基准 —— SC-006

## 阶段 2: 地基 —— 数据面

- [x] T004 `crates/ricow_core/src/interval.rs`: 新增 `Interval`(标签 ↔ 毫秒唯一口径); 收敛 `crates/ricow_binance/src/client.rs:323 interval_ms` 与 `crates/ricow_binance/src/futures_data.rs:60` 的散落表; 单测覆盖全部支持标签 —— FR-020
- [x] T005 `crates/ricow_core/src/types.rs`: 新增 `SeriesKey(source,symbol,interval)` 与 `Bar`(毫秒 UTC 的 `open_time`/`close_time` + 可选 `adjclose`) —— FR-008、FR-015
- [x] T006 `crates/ricow_core/src/exchange.rs`: 新增 `KlineSource` trait(`name` / `supported_intervals` / `fetch_klines(symbol, interval, from, to)`)与来源注册表(未知源名报错 + 列可用源); `Exchange` 自身签名不动 —— FR-017、FR-019
- [x] T007 `crates/ricow_core/src/resample.rs`: 自分支 `feat/shannon-atr-grid:crates/ricow_strategy/src/multiframe.rs` 迁入 `resample_complete`(完整桶 + 缺口守卫 + 粒度众数探测); 单测: 首尾半桶丢弃 / 缺口桶丢弃 / 1m·5m·1h 三种输入粒度 —— FR-014
- [x] T008 `crates/ricow_strategy/src/db.rs`: 新增 `data_klines` 表(键 `source|symbol|interval|open_time`)+ 幂等写入 + 水位查询 + 按区间读回; 单测: 重复写不产生重复行 / 增量补不覆盖已有行 / 读回顺序正确 —— FR-029
- [x] T009 `crates/ricow_engine/src/data/mod.rs`: DataHub(键解析 / 缓存命中 / 按需回补 / 同键并发去重 / 源级限速 / 未知源报错); 单测覆盖命中 / 回补 / 去重三态 —— FR-016、FR-022
- [x] T010 `crates/ricow_engine/src/data/source_binance.rs`: 既有 `Exchange::get_klines` 适配为 `KlineSource`(`from/to` → 1000 根分段循环 + 去重), 注册名 `binance_spot` / `binance_futures`; 单测跨段拼接 —— FR-018
- [x] T011 `crates/ricow_engine/src/data/source_nasdaq.rs`: 复用 `crates/ricow_engine/src/nasdaq.rs::parse_historical`, 窗口改 `from/to`(去 `FROM_DATE=2016-01-01`), 落 `data_klines`; 同步改 `crates/ricow_engine/tests/nasdaq_live_smoke.rs` —— FR-018、FR-032
- [x] T012 `crates/ricow_engine/src/data/source_yahoo.rs` + `crates/ricow_engine/tests/yahoo_live_smoke.rs`: v8 chart(period1/period2/interval、浏览器 UA、≈2 req/s 限速、解析 OHLCV + `adjclose`); 真实拉取用例(QQQ 日线 10 年 + 近期 1h, 标 `#[ignore]`, 记录根数与首末时间)—— FR-018、FR-015
- [x] T013 `crates/ricow/src/commands/data.rs` + `crates/ricow/src/main.rs`(mod data + `Command::Data`) + `crates/ricow/src/commands/mod.rs`: `ricow data pull --source/--symbol/--interval/--start/--end`, 增量补齐, 输出根数与区间, 未知源报错列可用源 —— FR-030

## 阶段 3: 地基 —— 运行时面

- [x] T014 `crates/ricow_strategy/src/strategy.rs`: Strategy trait 回调扩为 7 个(`on_init`/`on_bar`/`on_quote`/`on_timer`/`on_tick`/`on_fill`/`on_stop`, 均可选); 明确 `on_tick` = 盘口驱动回调(与 D5 一致), `on_order_update` 保持 no-op —— FR-001
- [x] T015 订单出口协议: 回调返回值 = 订单数组(沿用现状) + `ctx:place_order` + `ctx:cancel_order{pair, order_id|client_order_id}` 与无参形式(撤本实例归属挂单, 非归属只上报); 单测覆盖三种粒度 —— FR-002、FR-003
- [x] T016 `crates/ricow_strategy/src/host.rs`: 宿主接缝 trait `HostServices`(取数 / HTTP / 行情订阅); `crates/ricow_engine/src/host_impl.rs` 实现它并在装配时注入 `LuaStrategy`(新注入点); 依赖方向核验(engine→strategy, 无循环)—— FR-008、FR-027、plan d11
- [x] T017 `crates/ricow_strategy/src/series.rs`: 序列句柄(尾窗 = 声明窗口, 默认 300 / 下限 `max(最大指标周期+1, 2)` / 上限 5000; 12 个指标; `stale` 标记)—— FR-009、FR-011、FR-016
- [x] T018 [P] 单测: 句柄指标与 `ctx:*` **同输入长度**逐位一致(附 EMA 种子差异说明; 窗口更长时的差异属预期)—— FR-010、SC-008
- [x] T019 [P] 单测: 无前视(单源单周期 / 跨源日线+1h / 跨周期 / 缺口日 / 预热段边界)—— FR-013、SC-003
- [x] T020 `crates/ricow_strategy/src/lua.rs`: 注册 `data:series/history/subscribe`、`market:subscribe/best_bid/best_ask`、`http:get`(仅 GET); 回调派发 7 个(全部经 `LuaStrategy::call` → 各自 1M 预算)—— FR-008、FR-023、FR-024、FR-027
- [x] T021 [P] 单测: 单轮多回调的预算语义(轮内上限 = 回调数 × 1M, 如实断言)+ 句柄存在时快照体积不随序列数线性增长 —— FR-007、FR-012
- [x] T022 `crates/ricow_core/src/exchange.rs` + `crates/ricow_binance/src/{ws.rs,spot.rs,futures.rs}`: `subscribe_orderbook` 支持多 symbol(复用既有断线指数退避守护; 增量 diff 合并语义不变)—— FR-023
- [x] T023 `crates/ricow_engine/src/command.rs` + `crates/ricow_engine/src/market.rs`: 主循环改为按策略声明装配(序列集合 / 盘口集合 / 定时器); 替换单 pair 主循环; `on_timer` 用真实时钟; 回补失败标 `stale` —— FR-004、FR-006、FR-016、FR-023
- [x] T024 `crates/ricow_engine/src/backtest_runner.rs`: 按声明从本地库装序列(含预热段, 预热根数由策略声明或按最大指标周期推导)+ 虚拟时钟推进 `on_bar`/`on_timer`; 缺数据以非零退出码失败并打印 `ricow data pull` 命令 —— FR-031、FR-004

## 阶段 4: 用户故事 1 - 策略自己决定用哪条 K 线 (优先级: P1) 🎯 MVP

- [x] T025 [US1] 单测: 缺数据三类文案(缺序列 / 缺区间 / 序列过短)—— FR-031
- [x] T026 [US1] Dry Run 接线: 启动按声明预装 + 周期增量回补; 回补失败 → 序列标 `stale` 且策略可见 —— FR-016、FR-031
- [ ] T027 [US1] 真实验证: `ricow data pull --source yahoo --symbol QQQ --interval 1d` 拉 10 年 → 回测取该序列出信号; 断网重跑同一回测结果逐位一致 —— SC-001、SC-004
- [x] T028 [US1] 真实验证: 同一份脚本零改动跑 Dry Run; 序列口径(`native|resampled`、来源、窗口)进日志 —— SC-001

## 阶段 5: 用户故事 2 - 策略自己决定用 ws 还是某周期 K 线 (优先级: P2)

- [x] T029 [US2] [P] 单测: 只订阅 1h 时盘口 tick 不驱动策略; 只订阅盘口时 K 线不驱动; `on_timer` 跨日与缺口日行为(含声明时区)—— FR-006、FR-023
- [x] T030 [US2] 真实验证: 多标的策略(`yahoo/QQQ/1d` + `binance_spot/ETHUSDT/1h` + 盘口)在 Dry Run 连续运行 ≥10 分钟无异常 —— SC-005
- [ ] T031 [US2] demo 真实验证: 多标的下单 + 撤单(币安 demo 现货与合约各一次)→ 成交落库 → 停机清理零残留; 记录进 `specs/testnet.md` —— SC-005、SC-006
- [x] T032 [US2] 单测: 100 单/秒护栏回归(多标的共享计数)—— FR-026、SC-006

## 阶段 6: 用户故事 3 - 用户策略自取任意 URL (优先级: P3)

- [x] T033 [US3] `http:get` 宿主侧实现(`crates/ricow_engine/src/host_impl.rs`): 独立线程 + 独立 runtime + 墙钟超时(默认 10s)+ 响应体上限(默认 5MB); 不消耗指令预算; 请求与结果摘要入日志 —— FR-027、plan d13
- [x] T034 [US3] [P] 单测(不依赖网络): 超时 / 超体积 / 非 2xx / 非法 URL 四类错误可判别且不中断策略; tokio 上下文内不 panic —— FR-027、SC-010
- [x] T035 [US3] 真实验证: 用户策略用 `http:get` 取公开端点并参与决策; 断网时拿到可判别错误并可自降级 —— FR-027、SC-010
- [x] T036 [US3] 内置脚本扫描断言: `strategies/builtin/**/*.lua` 零出现 `http.`(与 `crates/ricow/src/commands/templates.rs` 的 `include_str!` 清单对齐)—— FR-028
- [x] T037 [US3] 网络条款全树同口径修订(点名): `specs/constitution.md`(网络条款)、`specs/product.md:158/:159/:165`、`specs/architecture.md` 网络行、`README.md` §7、`README_zh.md` §7 —— FR-038

## 阶段 7: 退役与回归面同步 (必须同批完成)

- [x] T038 退役装配层信号预装: `crates/ricow_strategy/src/lua.rs`(`universe` 读取 / `full_klines`)、`crates/ricow_strategy/src/backtest.rs`(`SIGNAL_TAIL=400` / `set_signal_klines` 装配)、`crates/ricow_engine/src/backtest_runner.rs`(信号预装); **保留**多标的同一账本撮合路径 —— FR-035
- [x] T039 [P] 退役 `us_klines` 专桶: `crates/ricow_strategy/src/db.rs` 的 `insert_us_klines`/`get_us_klines` + `:1358` 用例 + 交易日历读取点改走 `data_klines` —— FR-029
- [x] T040 [P] 退役 `ctx:klines` 100 根硬编码 cap(`crates/ricow_strategy/src/lua.rs:202` 附近), 统一为声明窗口; 改写 `:1049 test_single_klines_still_capped_at_100` 与 `:978 test_portfolio_universe_full_klines_in_lua` —— FR-011、SC-007
- [x] T041 [P] 断言同步(点名): `crates/ricow/src/commands/templates.rs:206/:237`(`contains("on_tick")` 与分类器)、`crates/ricow/src/commands/mod.rs:659/:698/:723/:773`、`crates/ricow/src/commands/run.rs:494`、`crates/ricow/src/ai/prompt.rs:116/:138`、`crates/ricow/src/ai/tools.rs:806/:2221`、`crates/ricow_strategy/src/builtin_tests.rs` 全文 —— FR-037
- [x] T042 `crates/ricow_engine/src/strategy.rs:42 extract_code`: 回调识别集扩为 `on_init`/`on_tick`/`on_bar`/`on_quote`/`on_timer`(否则只写新回调的策略被判"未提取到 Lua 代码"并被编译门禁挡掉); 同步 `:285` 断言 —— FR-037
- [x] T043 残留核对: `universe|signal_klines|SIGNAL_TAIL|us_klines|insert_us_klines|get_us_klines` 在 `crates/` 零命中(历史档案除外)—— SC-007

## 阶段 8: 样板 · 文档 · 门禁 · 收敛

- [ ] T044 内置策略迁移到新 API(`strategies/builtin/shannon_grid.lua` + `executors/{dca,twap,vwap,pullback,ladder}.lua`), 同步 `crates/ricow/src/commands/templates.rs` 与 `crates/ricow_strategy/src/builtin_tests.rs`; 迁移后回测与 T002 基线逐位比对, 差异逐条解释 —— SC-009
- [x] T045 [P] 多源多周期样板: 直接升格 T027/T028 的验证脚本(不新造第二条), 放进 `strategies/builtin/`(日线长历史判据 + 高周期 ATR + 定时节奏)—— SC-005
- [x] T046 [P] `specs/lua-api.md`: 新增"运行时回调 / 数据服务 / 行情服务 / HTTP"章节, 旧 `ctx:*` 定位说明(只读视图), `ricow data pull` 用法 —— FR-036
- [x] T047 [P] `specs/architecture.md` + `specs/backtest.md`(§一时间与可见性、§二 bar 顺序改为按声明驱动)+ `specs/product.md:135` 保留清单改"美股数据能力 / 组合撮合" —— FR-036
- [x] T048 [P] agent-kit 手册(`crates/ricow/src/commands/agentkit.rs`)与 AI 提示词(`crates/ricow/src/ai/prompt.rs`)同步新 API 与 `ricow data pull` —— FR-037
- [x] T049 README 两份(§7 网络与数据源、FAQ"回测要下载 K 线"改为"回测只读本地库 + `ricow data pull`"、"唯一策略样板"表述)+ `website/index.html` + `website/zh.html` —— FR-038
- [x] T050 门禁回归: 实盘三判据 / 二次确认 / 停机清理的文案与退出码逐条比对 T003 基线, 零变化 —— SC-006
- [x] T051 收尾: `cargo test --workspace` 全绿 + fmt + clippy(**禁 `cargo fmt --all`**); `specs/roadmap.md` 补 027/028 档案状态登记并改写保留清单 —— SC-009、FR-038
- [x] T052 异常路径验收: 源不可达 / 回补失败(stale)/ 停机时 in-flight HTTP 三类各一条实测或构造用例 —— SC-010
- [x] T053 `specs/changes/028-data-service/converge.md`: 逐条对照 FR-001~038 与 SC-001~011 给结论(不收敛则继续)—— 收敛产物

## 依赖与执行顺序

### 阶段依赖

- 阶段 2(数据面)与阶段 3(运行时面)是阻塞性地基, 阶段 4~6 全部依赖
- 阶段 7(退役)必须在阶段 4~6 之后(先有替代路径再退役), 且 T038~T043 同批完成
- 阶段 8 依赖全部前序

### 用户故事依赖

- US1 独立可交付(MVP); US2 依赖 T016/T020/T022/T023; US3 依赖 T016/T020/T033, 与 US1/US2 相互独立

### 每个用户故事内部

单测(可见性 / 指标一致 / 错误路径)→ 接路径 → 真实验证(第三方真实拉取 / 币安 demo)

### 并行机会

- T004 / T007 / T008 可并行(不同文件)
- T018 / T019 / T021 可并行(单测不同文件)
- T039 / T040 / T041 可并行(退役不同文件)
- T046 / T047 / T048 可并行(文档不同文件)

## 实施策略

### MVP 优先

T001~T028: 策略能用 `data:series` 取 Yahoo 长历史日线并在回测与 Dry Run 里出信号 —— 直接解决最痛的一条(第三方长历史)。

### 增量交付

MVP → 多标的与三种驱动(US2)→ 用户策略任意 URL(US3)→ 退役与断言同步(阶段 7)→ 迁移与文档(阶段 8)。

## 备注

- 分支 `feat/shannon-atr-grid` **不做整体合并**(D6): 只搬 `resample_complete`(T007)与策略侧撤单语义(T015), 其余(含 `--start/--end`、023 文档、`shannon_etf_accum.lua`)不并入。
- 旧策略不处理(D2): 不写兼容层; 已部署的旧策略需按新 API 重写(不在本计划范围)。
- 内置脚本改动必须重编译(`include_str!` 嵌入); `strategies/builtin/` 之外的用户策略默认私有(git 忽略)。
- 每次改动只碰相关文件, 不顺手重构; 提交须用户明确说"提交"。
- 退役类任务(T038~T043)执行顺序: **先改测试与调用点, 再删实现**(删除前 `grep` 全仓引用), 避免中间态编译失败。
