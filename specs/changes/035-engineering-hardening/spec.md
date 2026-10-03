# 功能规格: 工程底座加固(重试 / 缓存 / run card / 迁移 / CI 门禁 / 事件流 / 配置与 AI 健壮性)

**功能目录**: `specs/changes/035-engineering-hardening`

**创建日期**: 2026-10-03

**状态**: 已实施(2026-10-03 当日收敛)

**输入**: 用户指令: "参考 `tmp/analysis/ricow-optimization-vs-vibe-trading-2026-10-03.md`, 按照建议的推进方式(七), 依次帮我实现中高优先级功能。"

**依据文档**: `tmp/analysis/ricow-optimization-vs-vibe-trading-2026-10-03.md` 第二节(高优先 2.1–2.5)与第三节(中优先 3.1–3.5)。
该文档第四节(walk-forward / 滑点敏感性)**用户未回应, 视为不做**; 第五节(Web 前端工程债)低优先, 本轮不做; 第六节(不建议照搬)明示不做。

**需求澄清**(AskUserQuestion, 2026-10-03): ① K 线缓存隔离 → **给 `klines` 加 `market` 列**;
② 写操作重试 → **仅连接失败时重试**; ③ run card 落盘位置 → **默认写 `run/backtest/`**; ④ 实施节奏 → **一次性全做完**。

**前置依赖**: 无(纯工程加固, 不改策略语义、不改产品面)。

## 一、问题与目标

对标 Vibe-Trading 的工程做法, 本轮只补**底座**、不扩产品面(YAGNI):

1. **2.1 币安 REST 无重试/退避/限流**: 长窗口回测分页(1000 根/页)任意一页抖动即整轮失败;
   `CoreError::RateLimit` 是**死变体**(全仓无一处构造); 无 429/418 识别、无 `Retry-After`。
2. **2.2 回测不读本地缓存**: 每次现拉交易所, `klines` 表只由 `db sync` 写、与回测脱节; 同窗口重跑不可复现、弱网不可用。
3. **2.3 回测产物不可归档**: 报告只打印、不落盘; 无策略源码哈希、无数据窗口指纹 → 无法机器化比对"是否逐位一致"。
4. **2.4 数据库无版本迁移**: `migrate()` 全是 `CREATE TABLE IF NOT EXISTS`, 无 `PRAGMA user_version`;
   `Database::open` 每次跑全量建表写事务, 在多池同库下偶发 `SQLITE_BUSY`。
5. **2.5 CI 门禁缺项**: 无 `cargo-deny`、无 macOS 测试矩阵(却在发 macOS 产物)、无安全红线 grep 门禁;
   `release.yml` 在 `pull_request` 也触发(dist plan 白跑)。
6. **3.1 锁毒化面**: `multiframe.rs` / `backtest_jobs.rs` 的 `lock().unwrap()` / `.expect(...)` —— 长跑服务单点 panic 级联失败。
7. **3.2 `sqlx::Error` 未归一**: 上层手工 `map_err(|e| CoreError::Exchange(e.to_string()))`, 边界语义丢失。
8. **3.3 无可观测事件流**: 实例运行只有明文日志, 无结构化、可机读、可归档的运行事件。
9. **3.4 AI: LLM 无重试 + 对话历史无界**: 瞬时抖动直接报错给用户; `history` 只增不减, 长会话顶穿上下文窗口。
10. **3.5 配置无 schema 版本 + 默认值散落**: 无 `schema_version` 锚点, 默认值散在 `unwrap_or` 与各 `Default` impl。

## 二、需求与验收

### FR-1 币安 REST 统一重试 / 退避 / 限流 (2.1, P1)

新增 `ricow_binance/src/retry.rs`: `send_with_retry(build, kind, policy)` 单点定义, 现货 / 合约 / fapi 公共数据三处客户端共用。

- 按**请求语义**分档: `Read`(K 线/行情/查询)可重试传输错误 + 429/418/408/5xx; `Write`(下单/撤单/改杠杆)**仅**连接层失败(`is_connect`)可重试。
- 退避: 指数 + ±20% 抖动, 默认 300ms 起、上限 8s; 429/418 优先尊重 `Retry-After`(仅秒形式, 上限 30s)。
- `CoreError::RateLimit` 由死变体**接上**: 429/418 经 `status_error` 归一。

**验收**: 单元测试覆盖"写操作任何状态码都不重试"、"读操作状态码矩阵"、"429→RateLimit"、"退避增长且封顶"。

### FR-2 回测优先读本地 K 线缓存 (2.2, P1)

- `klines` 表加 `market` 维度, 主键 `(market, pair, interval, open_time)` —— 现货/合约同 pair 不互相污染。
- 回测取数改为"**先读本地 → 未命中/根数不足直连交易所 → 回填**"; 仅当窗口**已全部收盘**(`end` 早于 now 至少一根 bar)才允许命中缓存, 防"实时尾 bar 未收盘被固化"。
- 回填是 best-effort, 失败只 warn、不影响回测结果。
- 生效数据源写进 run card(`local-cache` / `binance-rest`)。

**验收**: `test_get_klines_range_recent_ascending_and_span` 覆盖区间读取与升序语义; 真实回测日志可见命中/回退分支。

### FR-3 回测 run card 落盘 (2.3, P1)

回测结束写 `run/backtest/<毫秒时间戳>-<策略名>-<pair>.json`(标识符经 `sanitize_ident` 净化), 内容:
`schema_version` / `generated_at` / `engine_version` / 策略(`name`/`kind`/源码 `sha256` / 字节数) / 参数快照(排除源码本体) / 数据窗口(`pair`/`interval`/`market`/`position_mode`/请求根数/预热带/实得根数/首末 `open_time`/`end`/`data_source`) / 全部报告指标。

**验收**: `test_run_card_sha256_and_sanitize`(哈希与文件名净化)、`test_run_card_params_json_excludes_script`(参数快照不含源码)。

### FR-4 数据库编号迁移 (2.4, P1)

- 引入 `PRAGMA user_version` 驱动的编号迁移: `SCHEMA_VERSION` 常量 + `run_version_migrations(v)`。
- `migrate()` 分两步: ① 幂等基线建表(全新库直接是最新结构); ② `v < SCHEMA_VERSION` 时才跑迁移并推进版本号。
- **版本已最新时零写事务直接返回** —— 这消除旧实现"每次 `open` 全量建表"在多池同库下的偶发 `SQLITE_BUSY`。
- v0 → v1: `klines` 重建加 `market` 列(旧数据只可能是现货 → 标记 `spot`), 单事务「建新表 → 搬数据 → 删旧表 → 改名」, 幂等可重入。

**验收**: `test_migration_v0_to_v1_adds_market_column`(造 v0 结构库 → 迁移 → 数据保留 + 版本推进 + 可重入)。

### FR-5 CI 门禁补齐 (2.5, P1)

- 新增 `deny.toml` + CI `deny` job: 许可证白名单 / 已知漏洞 / 重复版本(warn)/ 来源(禁未知 registry 与 git)。
- 新增 `scripts/ci_grep_gates.sh` + CI `gates` job, 四条安全红线:
  ① AI 工具层零落盘; ② 明文密钥不进日志; ③ 无 `dbg!/todo!/unimplemented!` 残留; ④ `execute_strategy` 调用点白名单。
  门禁**按 `#[cfg(test)]` 花括号配平跳过测试代码**。
- CI `test` job 矩阵加 `macos-latest`(发布 macOS 产物却未测)。
- `dist-workspace.toml` 加 `pr-run-mode = "skip"` 并去掉 `release.yml` 的 `pull_request` 触发。
- 顺带修真实漏洞 RUSTSEC-2026-0285(rustls → 0.23.45)。

**验收**: `bash scripts/ci_grep_gates.sh` 本地全绿; 反面样例能抓到违规且正确跳过测试代码; `cargo deny --locked check` = `advisories ok, bans ok, licenses ok, sources ok`。

### FR-6 锁毒化容忍 + `sqlx::Error` 归一 (3.1+3.2, P2)

- `multiframe.rs` / `backtest_jobs.rs` 的 `lock().unwrap()` → `lock_or_recover()`(`unwrap_or_else(|e| e.into_inner())`)。
- 新增 `CoreError::Db(String)` 变体 + `SqlxResultExt::core()` 边界归一; 命令层与引擎层的 `map_err(|e| CoreError::Exchange(...))` 统一换成 `.core()?`(约 16 处)。

**验收**: `test_sqlx_error_normalizes_to_core_db_variant`; 全仓不再有 `sqlx::Error → Exchange` 的手工映射。

### FR-7 结构化运行事件流 (3.3, P2)

- 新增 `ricow_strategy/src/events.rs`: `RunEvent`(一行一 JSON)+ `EventWriter`, 落在 `RICOW_ROOT/run/<name>/events.jsonl`。
- 写入**永不返回错误**(锁毒化用 `into_inner`, 失败只提醒一次)—— 观测能力缺失不阻断交易。
- 埋点: 实例 `started`/`stopped`(引擎侧); 下单 `order_placed`/`order_rejected`、撤单 `order_canceled`(上下文侧, Live 与 Dry Run 都有)。
- `read_events` 容错读取(坏行跳过), 供后续归档/回放。

**验收**: `events.rs` 5 例(round-trip / 坏行跳过 / Decimal 精确 / 追加语义 / 无 null 字段)+ `context.rs` 4 例(实盘下单入流 / 失败入流 / 无 writer no-op / Dry Run 下单+撤单)。

### FR-8 AI: LLM 重试 + 对话历史预算 (3.4, P2)

- LLM 调用(`ask` / `ask_stream`)加 3 次退避重试(400×3ⁿ ms), **仅瞬时错误**:
  先一票否决 400/401/402/403/404/422/501, 再认 408/425/429/500/502/503/504 + 少量软文案;
  状态码按**数字边界**匹配(`1500 tokens` 里的 500 不算 HTTP 500)。
- 流式重试硬边界: **已往 sink 吐过字就不再重试**(重来一遍会把同一段话显示第二次)。
- 对话历史字符预算(`HISTORY_BUDGET_CHARS = 24_000`): 超限时从最前**按整轮**折叠(user+assistant 成对丢弃),
  在最前面插一行确定性摘记(回列最多 3 段更早提问, 每段 60 字符); 只有一轮时不动。
- 刻意**不调 LLM 做摘要**(Vibe-Trading 的 L3 做法): 为纯运维目的多付一次调用与等待不值。

**验收**: `provider.rs` 6 例(可重试判定 / 数字边界 / 预算内 no-op / 整轮折叠 / 单轮不折 / 摘记回列)。

### FR-9 配置 schema 版本 + 默认值收敛 (3.5, P2)

- `ricow.toml` 加顶层 `schema_version` 键: 缺省 = 当前版本(老文件**内存迁移, 不落盘改写**);
  非法值(非整数/负数)硬失败; 大于当前版本**拒绝并给出解法**(不静默丢弃未知配置)。
- 新增 `pub mod defaults`, 模板与解析共用同一份默认值(单一来源)。
- `ai/config.rs` 加 `default_preset()`(= `PRESETS.first()`), 消除"默认供应商"的第二处定义。

**验收**: `config_file.rs` 5 例 + `ai/config.rs` 1 例。

## 三、边界与决策

- **D1 写操作不重试(安全红线)**: 响应超时 / 5xx 可能"其实已成交", 重放 = 重复下单。写操作只重试连接层失败(`is_connect` = 请求确定未送达)。
- **D2 缓存只在窗口已收盘时启用**: 交易的实时尾 bar 未收盘, 固化它等于把"过程中快照"当成历史事实。
- **D3 run card 是旁路产物**: 落盘失败只 warn —— 回测结果本身有效, 不因辅助文件回滚一次成功的回测。
- **D4 迁移不落盘改写老文件 (配置侧)**: `ricow.toml` 的 v0 无结构变化, 迁移只在内存标版本; **DB 侧**相反, 结构确实要变(v0→v1 重建 `klines`), 故落盘迁移。
- **D5 事件流不反压主循环**: `emit` 不返回 `Result`、写完即 flush、失败只 warn。
- **D6 摘记是确定性文本**: 不调 LLM; 折叠粒度是**整轮**, 留下半个来回比丢一整轮更糟(模型会答非所问)。
- **D7 摘记是一条 user 消息**: 故折叠后 user 比 assistant 恰好多 1(仅此一条), 其余必须成对。
- **D8 内部依赖收敛到 `[workspace.dependencies]`**: `cargo-deny` 的 `wildcards = "deny"` 对 path 依赖也报(`publish = false` 不豁免), 故成员改为 `ricow_core.workspace = true` 形式。
- **D9 `RunTelemetry` 打包**: `run_live` 独立传参时 8 个(超 clippy 阈值), 把 `mode` + `events` 打成一个结构体。

**范围外**(明确不做): walk-forward / 滑点敏感性(第四节, 用户未回应); 多交易所 fallback 链(违 YAGNI);
Prometheus/OTLP 监控(重型); Web 前端工程债; `cargo-audit`/覆盖率/`cargo-udeps`/bench(Vibe-Trading 有但 ricow 当前不需要)。

## 四、实施落点

| 层 | 文件 | 内容 |
|---|---|---|
| 币安 | `ricow_binance/src/retry.rs`(新)、`client.rs`、`futures_client.rs`、`futures_data.rs`、`lib.rs` | 统一重试/退避/限流 + 4 单测; 三处客户端接入 |
| 存储 | `ricow_strategy/src/db.rs` | `SCHEMA_VERSION` + 编号迁移 + `klines.market` + 打开重试 + `SqlxResultExt` |
| 错误 | `ricow_core/src/error.rs` | `CoreError::Db(String)` |
| 事件流 | `ricow_strategy/src/events.rs`(新)、`context.rs`、`lib.rs` | `RunEvent` / `EventWriter` / `read_events`; 上下文埋点 |
| 引擎 | `ricow_engine/src/command.rs`、`lib.rs` | `RunTelemetry` + `emit_run_event` + `run_dry_run`/`run_live` 埋点 |
| CLI | `commands/run.rs`、`commands/{approve,create,db,deploy,instances,web}.rs`、`web/{store,backtest_jobs}.rs` | 事件流注入; `.core()?` 归一; 锁毒化容忍 |
| 回测 | `commands/backtest.rs` | 本地缓存优先 + 回填; run card 落盘 |
| AI | `ai/provider.rs`、`ai/session.rs`、`ai/config.rs` | LLM 重试 + 历史预算; `default_preset()` |
| 配置 | `commands/config_file.rs` | `schema_version` + `defaults` 单一来源 |
| CI | `deny.toml`(新)、`scripts/ci_grep_gates.sh`(新)、`.github/workflows/{ci,release}.yml`、`dist-workspace.toml`、`Cargo.toml`、`Cargo.lock` | deny + grep 门禁 + macOS 矩阵 + pr-run-mode skip + workspace deps + rustls 升级 |

## 五、测试基线

`cargo test --workspace --no-fail-fast` = **642 passed / 2 failed(环境性, 见下) / 22 ignored**;
较 034 的 611: **+31 通过 / ignored 不变(22)**。增量构成:

| 条目 | 文件 | +用例 |
|---|---|---|
| 2.1 重试 | `ricow_binance/src/retry.rs` | 4 |
| 2.2 缓存 | `ricow_strategy/src/db.rs` | 1 |
| 2.3 run card | `commands/backtest.rs` | 2 |
| 2.4 迁移 / 锁抖动 | `ricow_strategy/src/db.rs` | 3 |
| 3.3 事件流 | `ricow_strategy/src/{events,context}.rs` | 9 |
| 3.4 LLM | `ai/provider.rs` | 6 |
| 3.5 配置 | `commands/config_file.rs` + `ai/config.rs` | 6 |

三门禁: `cargo fmt --all -- --check` 0 差异 / `cargo clippy --workspace --all-targets --locked -- -D warnings` 0 /
`cargo deny --locked check` 全绿。**如实**: 2 例失败是 `ai_live_smoke` 的 `approve_requires_interactive_tty` /
`piped_confirm_phrases_never_reach_the_host`, 本机(agent 沙箱)报 `ERROR_PIPE_BUSY(231)` —— 环境对管道创建的拦截,
非代码缺陷(与 032/033/034 基线同因)。
