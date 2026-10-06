# 实施计划: 工程健壮性加固(038)

**功能目录**: `specs/changes/038-p1-robustness-hardening`

**状态**: 已收敛 (2026-10-06; 实施记录见 [converge.md](converge.md))

**输入**: [spec.md](spec.md)

## 一、关键决策

| # | 决策 | 理由 |
|:--|:--|:--|
| **D1** | **P1-A 内置默认 `limit_fill_penetration_bps = 0`**(= 保持既有"触及即成交") | 用户明确选择"默认不变 + 暴露手段"。改动默认值会让**所有既有回测数字与断言漂移**, 与"不影响已实现功能"冲突。参数 + 报告单列 = 让用户**知情并可选收紧**, 而不是替用户改口径。 |
| **D2** | P1-A 参数进**三层配置**(内置默认 < 策略 TOML `[backtest]` < CLI), 与 `slippage_bps` 同构 | 复用既有三层解析链(`BacktestParams::resolve` / `apply_overrides`), 不新造机制。 |
| **D3** | P1-A 穿透后成交价仍为 `limit`, **不加额外滑点** | 无 tick 数据时伪造滑点是另一种失真; 穿透只表达"排队位置不够就轮不到你"。 |
| **D4** | P1-B 手写 Prometheus 暴露格式, **不引 `prometheus` crate** | 格式极简(`name{label="v"} value\n`), 引依赖违背宪法"少而精"; 只需自己实现标签值转义。 |
| **D5** | P1-B 挂**既有 web 服务**的 `GET /metrics`, 复用既有 token 中间件 | 不新开端口 = 不新增攻击面; 与全部端点同门禁, 不新增放行口。 |
| **D6** | P1-B 只暴露**本地库/台账可推导**的指标; `对账修正次数` **不暴露** | 该计数只在 `RunOutcome` 内存里, **未持久化** → 暴露它会给出假数字。宁可少一项(与 P0-A "不谎报结论"同哲学)。 |
| **D7** | P1-C 配置放 `ricow.toml` 新增 `[supervisor]` 段,**默认 `restart_policy = "none"`** | daemon 级一处即可(per-instance 会再生配置面, YAGNI); 默认 none = 零行为变更。 |
| **D8** | P1-C 重启判定 = **纯函数** `restart_decision(policy, code, retries, max_retries, ran_secs)`; 从 `monitor_loop` 调 `Server::start` | 让"该不该重启"可单测(不涉进程); 复用 `start()` 的**全部既有校验**(TOML / 视野 / 独立启动锁)比自己拼 spawn 安全得多。 |
| **D9** | P1-C 成功运行 ≥ `RESTART_HEALTHY_SECS`(固定 60s)才**重置**重试计数 | 固定常量, 不新增配置; 防止"崩-重启-崩"无限循环被计数重置掩盖。 |
| **D10** | P1-C 主动停机**不触发**重启 | `stop_all` 先 `drain` 掉 `children`, `monitor_loop` 看不到它们 —— 天然满足; 显式测试锁定该性质。 |
| **D11** | P1-D 缺口检测放**绑定层纯函数**(`ricow_strategy`), 由 CLI/Web 回测取数后调用 | 纯函数可单测; 不塞进引擎(引擎不负责数据质量)。 |
| **D12** | P1-D **硬报错**, 不给"容忍缺口"开关 | 与既有"预热不足硬报错"同口径; 喂残缺数据算指标是更坏的结果。 |
| **D13** | P1-E 需要 `RunCard` 可**反序列化** → 给 run card 结构补 `Deserialize` | 现有结构只有 `Serialize`(写卡用), 读卡必须补。字段全部保持原名, 不改 schema 版本。 |
| **D14** | P1-E 实盘侧**只比可直接比的量**(成交笔数/名义/手续费/现金净流入), **不比收益率** | 实盘的收益率需要持仓与资金费口径, 拿不到干净值; 硬比会给出误导数字(FR-5.3)。 |
| **D15** | P1-F 计时点选在**引擎调用 `ctx.place_order` 前后**, 不改 `LiveContext` | 引擎侧计时覆盖 REST 往返 + 本地对齐/护栏(微秒级), 精度足够且**零改动策略层**; 改 `LiveContext` 会牵动绑定层与架构守卫面。 |
| **D16** | P1-F **不动 `RunEvent` 字段与 schema 版本** | 037 已确立事件 schema 冻结; 延迟只进 `RunOutcome` + 收尾日志。 |
| **D17** | P1-F 分位数用**纯函数** `latency::summarize(&mut Vec<u64>)`, 不涉 `Instant` | 直接满足 CI 红线 5(生产代码禁 `Instant` 裸减法), 且测试零时钟依赖。 |

## 二、落点

### P1-A 限价成交模型 (ricow_strategy)

- `crates/ricow_strategy/src/config.rs`: `BacktestToml` 加 `limit_fill_penetration_bps: Option<f64>`; `BacktestParams` 加同名字段(默认 `0.0`); `resolve` / `apply_overrides` 接上。
- `crates/ricow_strategy/src/backtest.rs`: `BacktestContext` 加字段 + `new()` 里从 params 池读取 + 回写 config.params; `try_match_ohlc` 的 `OrderType::Limit` 分支加穿透判定; 新增成交计数(限价/市价分列)并在 `BacktestReport` 暴露。
- `crates/ricow/src/commands/backtest.rs`: CLI 加 `--limit-fill-penetration-bps`; 报告单列限价成交笔数 + 假设说明。

### P1-B /metrics (ricow)

- 新 `crates/ricow/src/web/metrics.rs`: 手写暴露格式 + 纯渲染函数 + 标签转义; `routes()` 注册 `GET /metrics`。
- `crates/ricow/src/web/mod.rs`: `.merge(metrics::routes())`。

### P1-C 崩溃自动重启 (ricow)

- `crates/ricow/src/commands/config_file.rs`: 新 `SupervisorSection` + `SUPERVISOR_KEYS` + `load` 分支(白名单硬校验) + `SECTION_LIST` + 模板。
- `crates/ricow/src/supervisor/server.rs`: `State` 加 `restart` 策略与 `retries` 计数; `Server::new` 读配置; `monitor_loop` 改收 `Server`, 在子进程退出分支走重启判定; 新纯函数 `restart_decision` + 单测。

### P1-D 数据缺口检测 (ricow_strategy + ricow)

- `crates/ricow_strategy/src/backtest.rs`(或新 `gaps.rs`): 纯函数 `find_gaps(klines: &[Kline], step_ms: i64) -> Vec<Gap>` + `gap_error_message(pair, interval, &[Gap])`。
- `crates/ricow/src/commands/backtest.rs`: 取数完成后、喂引擎前调用, 有缺口即 `Err`。
- Web 回测路径 (`web/backtest_jobs.rs` 若自带取数) 同样接上。

### P1-E 实盘/回测对齐 (ricow)

- `crates/ricow/src/commands/backtest.rs`: run card 结构补 `Deserialize`; 新 `pub fn read_run_cards(root) -> Vec<(PathBuf, RunCard)>`(按时间倒序)。
- 新 `crates/ricow/src/commands/align.rs`: `LiveFacts` 汇总(纯函数) + 对照表渲染(纯函数) + 命令入口。
- `crates/ricow/src/main.rs`: 注册 `ricow align`。

### P1-F 下单延迟度量 (ricow_engine + ricow)

- 新 `crates/ricow_engine/src/latency.rs`: `LatencyStats` + `summarize(&mut Vec<u64>)` + 单测。
- `crates/ricow_engine/src/command.rs`: 下单两处(Dry Run 主循环 / 实盘主循环 + 停机清理)计时并累积; 收尾写入 `RunOutcome`.
- `crates/ricow_engine/src/lib.rs`: 导出。
- `crates/ricow/src/commands/run.rs`: 收尾打印延迟统计(无样本不打印)。

## 三、验证方式

- **纯逻辑单测为主**: 每项的核心判定都是纯函数, 各配单测(含边界: 空输入/单样本/全同值/超限)。
- **零行为变更断言**: P1-A 默认参数下 `try_match_ohlc` 与旧实现逐位一致; P1-C 默认 `none` 下不重启。
- **端到端模拟**(用户授权口径): 临时 `RICOW_ROOT` + 手工灌库跑真实二进制验证 `align` / `metrics`; daemon 重启用**假的短命子进程**不可行(需真策略), 故以纯函数单测 + 代码审查为准, 在 converge.md 如实登记"未真机取证"。
- **门禁五件** + `architecture_guard` 三条, 全绿后写入 converge.md。
