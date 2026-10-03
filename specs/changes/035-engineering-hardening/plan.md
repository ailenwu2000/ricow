# 实施计划: 035 工程底座加固

**功能目录**: `specs/changes/035-engineering-hardening`

**创建日期**: 2026-10-03

**依据**: `spec.md` §二需求与验收; 推进顺序按分析文档第七节(先三条咬合的高 ROI → 再补工程底座 → 决策项单列)

## 一、推进顺序与依赖

分析文档第七节的顺序是刻意设计的: 2.1/2.2/2.3 互相咬合(重试让取数稳 → 缓存让取数可复现 →
run card 记录"这次用的是缓存还是网络"), 先做; 2.4/2.5/3.3 是底座(迁移、门禁、观测), 后做;
3.1–3.5 顺手补健壮性; 第四节 walk-forward 属决策项, 用户未回应 → 不做。

```
批次 A (咬合三件): 2.1 重试 ─┬→ 2.2 缓存 ──→ 2.3 run card
                              └ (重试让分页取数不再一抖全废)
批次 B (工程底座): 2.4 迁移 ──→ 2.5 CI 门禁 ──→ 3.3 事件流
批次 C (健壮性):   3.1 锁毒化 + 3.2 错误归一 ──→ 3.4 AI 重试/历史 ──→ 3.5 配置版本
批次 D (收尾):     全量验证 + docs 同步
```

## 二、批次 A: 三条咬合的高 ROI

### A1 (2.1) 币安 REST 统一重试 (FR-1)

1. 新建 `crates/ricow_binance/src/retry.rs`, 导出 `send_with_retry` / `status_error` / `RequestKind` / `RetryPolicy`。
2. `lib.rs` 注册 `mod retry;`。
3. 三处客户端把 `http.get(...).send().await` 换成 `send_with_retry(|| http.get(...), kind, RetryPolicy::default())`:
   - `client.rs`: `fetch_klines_paged` 与四处签名端点;
   - `futures_client.rs`: 五处;
   - `futures_data.rs`: `get_klines*` 两处。
4. 非 2xx 的归一改走 `retry::status_error`(429/418 → `RateLimit`)。

**风险**: 低(不改协议语义)。**验证**: `retry.rs` 4 单测 + 全量编译。

### A2 (2.2) 回测优先读本地缓存 (FR-2)

1. `db.rs`: `klines` 主键加 `market`; `insert_klines(market, ...)` / `get_klines_range(market, pair, interval, start, end, limit)`。
2. `commands/mod.rs`: 新增 `open_cache_db(root)`(只读缓存入口, 打不开就 `None`)。
3. `commands/backtest.rs`: 取数分支改为
   `cache_eligible = window_end <= now - step` → 命中(`>= fetch_limit` 根)则 `local-cache`;
   否则走原分页逻辑并**回填**(warn-only)。

**风险**: 中(改取数主路径, 但只在"窗口已收盘"时启用, 交易实时路径不碰)。**验证**: `get_klines_range` 单测 + 真实回测日志。

### A3 (2.3) 回测 run card (FR-3)

1. `backtest.rs`: 定义 `RunCard{,Strategy,Window,Metrics}` + `RUN_CARD_SCHEMA_VERSION` + `write_run_card(root, card)`。
2. 在 `config` move 进引擎**前**采集策略快照(源码 sha256)与参数快照; 引擎返回后组装指标并落盘(warn-only)。

**风险**: 低。**验证**: 2 单测(哈希/净化、参数快照不含源码)。

## 三、批次 B: 工程底座

### B1 (2.4) 数据库编号迁移 (FR-4)

1. `db.rs`: 加 `SCHEMA_VERSION`; `migrate()` 拆成 `create_baseline_tables()` + `run_version_migrations(v)`; 版本已最新即早返回。
2. `run_version_migrations`: `v < 1 && klines_lacks_market_column()` → `rebuild_klines_with_market()`(单事务, 幂等清临时表)。
3. `Database::open`: 加 10 次线性退避重试, **仅** `is_locked_error`(SQLITE_BUSY=5 / SQLITE_LOCKED=6); 用 `tokio::time::sleep` 而非 `std::thread::sleep`。

> **为什么用 async sleep**: 阻塞式 sleep 会卡死 current_thread 运行时, 把上一个池的
> `close()`(WAL checkpoint + 删文件)收尾一起阻塞 → 重试永远等不到锁释放。

**风险**: 中(动数据库打开路径)。**验证**: v0→v1 迁移单测 + 连续 3 次 `cargo test -p ricow --bin ricow` 全绿(抖动根治取证)。

### B2 (2.5) CI 门禁 (FR-5)

1. `deny.toml`: `[graph].targets` 对齐 dist 发布矩阵; `[advisories]`(v2 语义: `unsound`/`unmaintained = "workspace"`, `yanked = "warn"`);
   `[licenses].allow` = 实测 16 条; `[bans]`(`multiple-versions = "warn"`, `wildcards = "deny"`); `[sources]` 禁未知 registry/git。
2. 内部依赖收敛到 `[workspace.dependencies]`(path + version), 成员写 `xxx.workspace = true` —— 解 `wildcards = "deny"` 对 path 依赖的误报。
3. `cargo update -p rustls --precise 0.23.45`(修 RUSTSEC-2026-0285)。
4. `scripts/ci_grep_gates.sh`: 四条红线 + `#[cfg(test)]` 配平跳过。
5. `.github/workflows/ci.yml`: 加 `gates` / `deny` job, `test` 矩阵加 `macos-latest`。
6. `dist-workspace.toml` 加 `pr-run-mode = "skip"`; `release.yml` 去掉 `pull_request` 触发(并注释持久开关位置)。

**风险**: 低。**验证**: 本地 `bash scripts/ci_grep_gates.sh` 全绿 + 反面样例(临时目录)能抓到违规 + `cargo deny` 四绿。

### B3 (3.3) 结构化运行事件流 (FR-7)

1. 新建 `ricow_strategy/src/events.rs`: `EVENTS_SCHEMA_VERSION` / KIND 常量 / `RunEvent` / `EventWriter`(`create(root, name)` → `run/<name>/events.jsonl`) / `read_events`。
2. `context.rs`: `LiveContext` / `DryRunContext` 加 `events` + `run_mode` 字段, `set_events` + `emit_event`;
   埋点 `place_order`(成功/失败)、`cancel_order`、`cancel_owned_orders`、Dry Run 挂单/撤单/`reduce_only` 拒单。
3. `command.rs`: `RunTelemetry` 打包 `mode + events`; `run_dry_run`/`run_live` 收 `events` 并在启动/收尾各记一条实例级事件。
4. `commands/run.rs`: `EventWriter::create(&project_root(), &config.name)`, 失败只 warn → `None`; 三处调用点注入。

**风险**: 低(旁路, 不反压主循环)。**验证**: 5 + 4 单测。

## 四、批次 C: 健壮性

### C1 (3.1+3.2) 锁毒化 + 错误归一 (FR-6)

1. `multiframe.rs` / `backtest_jobs.rs`: `lock_or_recover()` 替 `unwrap()` / `expect(...)`。
2. `ricow_core/src/error.rs`: 加 `CoreError::Db(String)`(注释说明为何与 `Exchange` 分开)。
3. `db.rs` 加 `SqlxResultExt::core()`; ~16 处 `map_err(|e| CoreError::Exchange(e.to_string()))` → `.core()?`; 清掉不再需要的 import。

### C2 (3.4) AI 重试 + 历史预算 (FR-8)

1. `provider.rs`: `LLM_MAX_ATTEMPTS` / `llm_backoff` / `has_status_token` / `is_retryable_llm_error`;
   `ask` / `ask_stream` 包重试循环, 拆出 `ask_once` / `ask_stream_once(..., emitted)`(已 emitted 不重试)。
2. `provider.rs`: `HISTORY_BUDGET_CHARS` / `message_chars` / `message_text` / `history_digest` / `compact_history`。
3. `session.rs::reply()` 内静默折叠 + `tracing::debug!`。

### C3 (3.5) 配置 schema 版本 (FR-9)

1. `config_file.rs`: `SCHEMA_VERSION` / `SCHEMA_VERSION_KEY` / `pub mod defaults`; `File` 加 `schema_version` + 手写 `Default`;
   `template_text()` 与解析共用 `defaults`; `parse_schema_version` + `migrate_to_current`(内存, 不落盘)。
2. `ai/config.rs`: `default_preset()` + 一致性单测。

## 五、批次 D: 收尾

1. `cargo fmt --all` → `--check` 0 差异。
2. `cargo clippy --workspace --all-targets --locked -- -D warnings` → 0。
3. `cargo test --workspace --locked --no-fail-fast` → 记录基线(含 2 例已知环境性失败)。
4. `cargo deny --locked check` → 四绿。
5. 同步 `specs/roadmap.md`(测试基线 + 变更档案表 + 变更摘要段)、`specs/architecture.md`(数据布局/测试策略)、`specs/backtest.md`(缓存 + run card)。
6. 落档本目录四件套。

**不做**: git commit(用户明确说"提交"才可)。
