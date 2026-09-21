# 028 实施计划: 策略自取数据 —— 数据服务 + 策略运行时

**规格**: [spec.md](./spec.md) | **来源**: 用户 2026-09-20 描述四条 + 拍板决策 D1~D8 | **日期**: 2026-09-20 | **分支**: `feat/028-data-service`(待创建) | **调研**: [research.md](./research.md)

## 摘要

把"引擎喂数据、策略填空"改成"引擎提供运行时 + 公共服务, 策略编排完整流程":

- **运行时**: 回调 = `on_init`/`on_bar`/`on_quote`/`on_timer`/`on_tick`/`on_fill`/`on_stop`(均可选); 订单出口 = 回调返回值 + `ctx:place_order` / `ctx:cancel_order`。
- **数据服务 (DataHub)**: 键 `(source, symbol, interval)`, 三动词 = 历史请求 / 增量订阅 / 序列句柄; 指标从"绑定 ctx 的单一 pair"改为"绑定一条序列"。
- **数据源**: trait + 注册名(4 个内置源); 新增源零主流程改动。
- **两条取数通道**: 内置策略走平台数据服务; 用户策略可 `http:get` 任意 URL(不限制域名)。
- **本地库单表** `data_klines`; 回测只读本地库 + `ricow data pull`; Dry Run / 实盘增量回补。
- **退役**装配层信号预装与 `us_klines` 专桶(D7/D8), 能力本体(组合撮合 / 只读 ctx / 美股数据能力)保留。

## 技术上下文

**Language/Version**: Rust 1.88(工具链钉 1.96.1) + Lua 5.4(mlua 0.11, vendored, 沙箱)

**Primary Dependencies**: 复用现有 workspace 依赖 —— tokio / reqwest(仅 `ricow_engine` 与 `ricow_binance` 已持有)/ serde_json / sqlx+SQLite / ta 0.5 / rust_decimal / tokio-tungstenite。**不新增 crate、不动任何 `Cargo.toml` 依赖项**(`ricow_strategy` 保持无网络依赖, 见 d11/d13)。

**Storage**: SQLite `ricow.db` —— 新增 `data_klines`; 退役 `us_klines` 读写的既有表保留物理存在但不参与流程(删除动作由 D8 的退役任务处理)

**Testing**: `cargo test --workspace`(纯逻辑单测) + 真实调用(数据源真实拉取; 交易链路币安 demo 真实下单, 禁 mock 替身) + 真实外部用例落 `crates/<crate>/tests/*_live.rs`(`#[ignore]`)

**Target Platform**: Linux / macOS / Windows(WSL 为主开发环境)

**Project Type**: CLI 单机应用(workspace 5 crate)

**Performance Goals**: 单策略同时 ≥8 条序列无可见卡顿; 单 tick 开销不随序列数线性放大(句柄只持尾窗, 指标按周期桶缓存一次); 10 年日线一次拉取 ≤ 10 s

**Constraints**: 沙箱不变(无 os/io/require/pcall、内存 64MB、指令预算每回调 1M); 回测无网络且可复现(逐位一致); 第三方源限速(≈2 req/s)且失败如实报错; HTTP 调用受墙钟超时约束

**Scale/Scope**: 策略侧 3 个数据动词 + 2 个 URL/行情动词 + 7 个回调; 内置源 4 个; 新表 1 张; 新 CLI 子命令 1 个; 引擎主循环由单标的改为声明驱动

## 宪法检查

*GATE: Phase 0 前必须通过; Phase 1 设计后复查。*

| 宪法条款 | 判定 | 说明 |
|:--|:--|:--|
| 一、完全本地化 | ⚠️ **需同步修订** | 私钥与数据仍不出本机; 但"网络仅访问交易所 API 与用户自配 LLM API"须扩为**用户策略自取网络数据**(D1), 全树同口径修订见 FR-038(T038) |
| 二、策略层统一 Lua + exec | ✅ 通过 | 策略本体仍全在 Lua; 引擎只提供运行时与数据服务, Rust 不写策略本体 |
| 三、测试纪律 | ✅ 通过 | 纯逻辑(重采样 / 可见性 / 句柄指标 / 缓存幂等 / DataHub 三态)用单测; 交易链路走币安 demo 真实调用; 第三方源真实拉取 |
| 四、产物中文 | ✅ 通过 | 全部档案中文, 头部标签与既有档案(025/026/027)对齐 |
| 五、少而精 (YAGNI) | ✅ 通过(含退役) | 不新增 crate / 不新增依赖; 去 `http:post`; 退役两处双轨(D7/D8 正是"死代码必删") |
| 安全要求 | ✅ 通过 | 写操作确认 / Dry Run 默认 / 实盘门禁 / 100 单·秒⁻¹ 护栏 / 沙箱约束全部不变 |

## 一、决策

| # | 决策 | 理由与落点 |
|:--|:--|:--|
| d1 | 数据服务三动词 `data:history` / `data:subscribe` / `data:series` | 覆盖声明式(订阅)与主动式(句柄/历史); 键统一 `(source, symbol, interval)` |
| d2 | 指标绑定**序列**而非 ctx 的 pair | 多标的 / 多周期无需新增通道; 与既有指标"同输入长度逐位一致"是硬约束(FR-010) |
| d3 | 数据源 = trait + 注册名; 周期表唯一定义在 `ricow_core` | 新增源零主流程改动(SC-002); 消除 `ricow_binance` 两处毫秒表 |
| d4 | 回测只读本地库 + `ricow data pull` | 可复现优先(D3); Dry Run / 实盘增量回补 |
| d5 | 双通道: 平台数据服务(内置策略) + `http:get`(用户策略, GET only) | D1 拍板; 不设域名白名单, 受超时 / 体积 / 内存约束 |
| d6 | 周期优先原生, 源不支持才重采样, 口径显式标注 | 原生周期避免重采样误差与缺口守卫的近似; 与 freqtrade "能重采样就别多拉"的建议**方向相反而刻意为之**(理由: 我们按需拉单条序列, 不是全标的批量刷新, 请求量可控) |
| d7 | 可见性单点按 `close_time` | 无前视; 跨源对齐不使用 open |
| d8 | 旧 `ctx:*` 只读 API 保留 | 它只是"已装配序列的只读视图", 不属"替策略决定"; 内置策略迁移到新 API |
| d9 | 分支只取两处增量(重采样 / 策略侧撤单) | 不取 `--start/--end`(长窗口用现有 `--days` 表达)、不并入 023 文档与脚本; 手工搬运不合并分支 |
| d10 | 不新增 crate | DataHub 落 `ricow_engine::data`; 序列与 Lua 注册落 `ricow_strategy` |
| d11 | **依赖接缝 = 窄 trait 注入**(修审核 C3) | `ricow_core` 定义 `KlineSource`(源侧); `ricow_strategy` 定义宿主接缝 `HostServices`(取数 / HTTP / 行情订阅); `ricow_engine` 实现 `HostServices` 并在装配时注入 `LuaStrategy`(新注入点)。依赖方向始终 engine→strategy, **无循环依赖** |
| d12 | 重采样归 `ricow_core`(修审核 C4) | `resample_complete` 是纯函数, 放 core 让 `ricow_strategy::series` 与 `ricow_engine` 都能用; 分支原位在 strategy, 迁 core 而非 engine |
| d13 | HTTP 由宿主实现(修审核 C1/C2) | `http:get` 的实现在 `ricow_engine`(已持有 reqwest)的**独立线程 + 独立 runtime** 上执行, 带墙钟超时与体积上限 → 避免 `reqwest::blocking` 在 tokio 上下文 panic、避免阻塞期间指令预算失效; 沙箱侧只是同步调用注入的 trait |
| d14 | 退役两处双轨(D7/D8) | 装配层信号预装与 `us_klines` 专桶在 028 内退役, 保留能力本体 |

## 二、改动清单

> 标注: 【新】新建 | 【改】修改既有 | 【分支】自分支 `feat/shannon-atr-grid` 手工搬运(不合并分支)。**无任何 `Cargo.toml` 变更**(d11/d13 用注入而非新增依赖)。

| 位置 | 变更 |
|:--|:--|
| `crates/ricow_core/src/types.rs` | 【改】新增 `SeriesKey` / `Bar`(带毫秒 UTC 时间与可选 `adjclose`) |
| `crates/ricow_core/src/interval.rs` | 【新】`Interval`(标签 ↔ 毫秒唯一口径), 收敛 `ricow_binance/src/client.rs:323` 与 `futures_data.rs:60` 的散落表 |
| `crates/ricow_core/src/exchange.rs` | 【改】新增 `KlineSource` trait; `subscribe_orderbook` 扩展为多 symbol(签名变更, 既有实现同步) |
| `crates/ricow_core/src/resample.rs` | 【新+分支】`resample_complete`(完整桶 + 缺口守卫 + 粒度众数探测)自 `feat/shannon-atr-grid:crates/ricow_strategy/src/multiframe.rs` 迁入 |
| `crates/ricow_core/src/lib.rs` | 【改】导出新模块 |
| `crates/ricow_binance/src/{client.rs,futures_data.rs}` | 【改】毫秒表收敛到 `ricow_core::Interval` |
| `crates/ricow_binance/src/{ws.rs,spot.rs,futures.rs}` | 【改】盘口订阅支持多 symbol(复用既有断线指数退避守护) |
| `crates/ricow_strategy/src/host.rs` | 【新】宿主接缝 trait(`HostServices`: 取数 / HTTP / 行情订阅)—— d11 |
| `crates/ricow_strategy/src/series.rs` | 【新】序列句柄(尾窗 = 声明窗口; 12 指标; `stale` 标记) |
| `crates/ricow_strategy/src/strategy.rs` | 【改】Strategy trait 回调集合扩为 7 个(`on_bar`/`on_quote`/`on_timer` 新增); 订单出口协议 |
| `crates/ricow_strategy/src/lua.rs` | 【改】注册 `data.*` / `market.*` / `http:get`; 回调派发; `ctx:cancel_order`; 退役 `universe`/`full_klines`/100 根 cap; 注入点 `LuaStrategy::with_host(...)` |
| `crates/ricow_strategy/src/context.rs` | 【改】序列查询接缝; 撤单; 多标的持仓 / 权益口径核对 |
| `crates/ricow_strategy/src/backtest.rs` | 【改】退役 `SIGNAL_TAIL` / `signal_klines` / `set_signal_klines` 相关装配 |
| `crates/ricow_strategy/src/db.rs` | 【改】新增 `data_klines`(幂等写 + 水位 + 区间读); 退役 `us_klines` 读写 |
| `crates/ricow_strategy/src/lib.rs` | 【改】导出 `series` / `host` 模块 |
| `crates/ricow_engine/src/data/{mod,source_binance,source_nasdaq,source_yahoo}.rs` | 【新】DataHub(键解析 / 缓存 / 回补 / 去重 / 限速 / 未知源报错) + 三个源适配 |
| `crates/ricow_engine/src/host_impl.rs` | 【新】`HostServices` 实现(HTTP 独立线程 + 独立 runtime + 墙钟超时 + 体积上限)—— d13 |
| `crates/ricow_engine/src/{lib.rs,market.rs}` | 【改】导出 `data`; 多标的盘口封装 |
| `crates/ricow_engine/src/command.rs` | 【改】主循环改为按策略声明装配(序列集合 / 盘口集合 / 定时器); 多标的; 回补失败标 `stale` |
| `crates/ricow_engine/src/backtest_runner.rs` | 【改】按声明从本地库装序列(含预热段) + 虚拟时钟推进 `on_bar`/`on_timer`; 缺数据报错; 退役信号预装 |
| `crates/ricow_engine/src/nasdaq.rs` | 【改】窗口改 `from/to`(去 `FROM_DATE` 常量), 落表改 `data_klines` |
| `crates/ricow_engine/src/strategy.rs` | 【改】`extract_code` 回调识别集扩展(FR-037) |
| `crates/ricow_engine/tests/{yahoo_live_smoke.rs,nasdaq_live_smoke.rs}` | 【新/改】真实拉取用例 |
| `crates/ricow/src/commands/data.rs` | 【新】`ricow data pull` |
| `crates/ricow/src/{main.rs,commands/mod.rs}` | 【改】注册 `mod data` + `Command::Data` |
| `crates/ricow/src/commands/{templates.rs,run.rs}` | 【改】内置模板分类器与 `contains("on_tick")` 断言同步 |
| `crates/ricow/src/ai/{prompt.rs,tools.rs}` | 【改】提示词 / 工具文案与新 API 同步 |
| `crates/ricow/src/commands/agentkit.rs` | 【改】agent-kit 手册与新 API 同步 |
| ~~`crates/ricow_binance/src/ws.rs`~~ | **落点偏离(已记)**: 多标的盘口不改 ws 层, 改在 `market::subscribe_orderbooks` 合成流(单标的时与旧行为逐位一致) |
| ~~`crates/ricow/src/commands/agentkit.rs` / `templates.rs`~~ | **落点偏离(已记)**: 手册文本来自 `specs/lua-api.md`(`STRATEGY_API_DOC = include_str!`), 因此同步文档即同步手册; `templates.rs` 无 `on_tick` 断言(实际断言在 `strategy.rs::extract_code` 的 7 回调列表), 无需改 |
| ~~`crates/ricow/src/ai/tools.rs`~~ | **无需改**: AI 侧工具清单未列回调名, 提示词侧在 `ai/prompt.rs` 已加"按需取文档"路由 |
| `strategies/builtin/*.lua` | 【改】内置策略迁移到新 API(含新样板) |
| `specs/{lua-api,architecture,backtest,product,constitution,roadmap}.md` | 【改】按 FR-036/FR-038 同步 |
| `README.md` / `README_zh.md` / `website/{index,zh}.html` | 【改】网络与数据源表述、FAQ、样板表述 |

## 三、项目结构

### 文档(本次功能)

```text
specs/changes/028-data-service/
├── spec.md          # 功能规格(含原话追溯表与审核记录)
├── plan.md          # 本文件
├── research.md      # 同行做法与数据源事实(Phase 0)
└── tasks.md         # 任务分解(行尾带 FR/SC 溯源)
```

### 源代码(仓库根)

```text
crates/
├── ricow_core/      # SeriesKey / Bar / Interval / KlineSource / resample(纯逻辑, 无网络)
├── ricow_binance/   # 多 symbol 盘口订阅; 周期表收敛
├── ricow_strategy/  # host(接缝 trait) / series(句柄) / lua / db(data_klines)
├── ricow_engine/    # data(DataHub+三源) / host_impl(HTTP) / 声明驱动主循环
└── ricow/           # CLI: data pull / backtest / run
strategies/builtin/  # 内置策略(迁移) + 多源多周期样板
```

**结构决策**: 不新增 crate —— 取数(网络 / 缓存 / 回补)属引擎装配职责, 序列与 Lua 注册属策略沙箱职责, 纯逻辑(键 / 周期 / 重采样)上移 `ricow_core` 供两侧共用; 数据源适配器放 `ricow_engine/src/data/`(不放 `ricow_binance`, 后者语义 = 交易所适配)。

## 四、风险

1. **前视偏差**(最高): 可见性单点按 `close_time`(d7) + 专项单测(SC-003)+ 报告标注序列口径(`native|resampled`、`close|adjclose`)。
2. **交易日历 / 时区**: 美股日线与 24h 币安序列日界不同 → `on_timer` 与日界由策略显式声明时区(FR-006), 引擎不做隐含换算。
3. **第三方源稳定性**: Yahoo 为非官方接口、按 IP 限速 → 限速 + 缓存 + 失败如实报错; 单源失效不影响其他源与平台能力。
4. **阻塞与预算**: HTTP 在宿主线程执行, 受墙钟超时; 指令预算不含 HTTP 时间(d13), 需单测覆盖"阻塞期预算不被消耗"。
5. **性能**: 句柄只持尾窗 + 指标按周期桶缓存一次 + 装配期一次重采样; 单测断言快照体积不随序列数线性增长。
6. **退役风险**: D7/D8 删除既有机制与测试 —— 退役与断言同步必须同批完成(阶段 7), 否则中间态测试红; 每条退役任务都要求 `grep` 零命中验收。
7. **存量内置策略**: 迁移后回测数字可能变化 → 与 T002 基线逐位比对, 差异逐条解释。
8. **并发拉取**: 多序列回补触发限流 → DataHub 串行化 + 同键去重(FR-022)。

## 五、验收动作

1. 单测: 周期表唯一口径 / 重采样(半桶 / 缺口 / 多粒度)/ 可见性无前视(单源 / 跨源 / 跨周期 / 缺口日)/ 句柄指标同输入长度逐位一致 / 缓存幂等与水位 / DataHub 三态与未知源报错 / 预算语义 / 快照体积 / 撤单粒度 / HTTP 四类错误 / 护栏回归。
2. 真实数据: `ricow data pull --source yahoo --symbol QQQ --interval 1d` 拉 10 年 → 回测出信号; 断网重跑逐位一致(SC-004)。
3. 多源多周期: 单策略同时持 `yahoo/QQQ/1d` + `binance_spot/ETHUSDT/1h` + 盘口订阅, Dry Run ≥10 分钟无异常(SC-005)。
4. 交易链路: 币安 demo 现货与合约各一次真实下单 + 撤单 → 成交回写 → 停机清理零残留; 记录进 `specs/testnet.md`。
5. 门禁回归: 实盘三判据 / 二次确认 / 停机清理文案与退出码逐条比对(T003 基线, SC-006)。
6. 退役验收: `grep` 旧机制符号零命中 + 100 根 cap 分支移除(SC-007)。
7. 内置策略迁移后回测与基线逐位比对, 差异逐条解释(SC-009)。
8. 三门禁: `cargo test --workspace` 全绿 + fmt + clippy(**禁 `cargo fmt --all`**)。
9. 文档: lua-api / architecture / backtest / product / constitution / roadmap / README×2 / website×2 / agent-kit / AI 提示词同源更新(FR-036~FR-038)。

## 六、复杂度追踪

| 项 | 为何需要 | 更简做法被否的理由 |
|:--|:--|:--|
| 新增 `HostServices` 接缝 trait(d11) | 策略侧需要取数 / HTTP 能力, 但 `ricow_strategy` 不能依赖网络库(会破坏分层并可能成环) | 直接给 `ricow_strategy` 加 reqwest: 依赖膨胀 + HTTP 会在 tokio 上下文阻塞 / panic; 直连 DataHub: 循环依赖 |
| 序列句柄新模块(d2) | 多标的 / 多周期 / 长窗口无法用"绑定 pair 的 12 个 ctx 指标"表达 | 继续给 ctx 加指标: 每加一种组合都要改引擎, 正是"架构太死" |
| 退役两处双轨(D7/D8) | 与新 API 功能重叠且零消费者; 双轨会让 AI 与策略作者走错路径 | 保留双轨: 与新 FR-004 直接矛盾, 且要维护两套装配语义 |
