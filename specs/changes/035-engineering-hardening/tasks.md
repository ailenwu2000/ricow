# 任务分解: 035 工程底座加固

**功能目录**: `specs/changes/035-engineering-hardening`

**创建日期**: 2026-10-03

**状态**: 全部完成(2026-10-03)

## 批次 A: 咬合三件(高优先)

- [x] **T001** (2.1/FR-1) 新建 `ricow_binance/src/retry.rs`: `RetryPolicy` / `RequestKind` / `send_with_retry` / `status_error` + 退避与 `Retry-After`
- [x] **T002** (2.1/FR-1) `lib.rs` 注册 `mod retry`
- [x] **T003** (2.1/FR-1) `client.rs` 接入(分页取数 + 四处签名端点), 非 2xx 走 `status_error`
- [x] **T004** (2.1/FR-1) `futures_client.rs` 接入(五处)
- [x] **T005** (2.1/FR-1) `futures_data.rs` 接入(K 线两处)
- [x] **T006** (2.1/FR-1) retry.rs 4 单测(写不重试 / 读矩阵 / 429→RateLimit / 退避封顶)
- [x] **T007** (2.2/FR-2) `db.rs`: `klines` 主键加 `market`, `insert_klines` / `get_klines_range` 带 market
- [x] **T008** (2.2/FR-2) `commands/mod.rs`: `open_cache_db(root)`
- [x] **T009** (2.2/FR-2) `backtest.rs`: 缓存优先取数 + `cache_eligible` 判定 + 回填(warn-only)
- [x] **T010** (2.2/FR-2) `data_source` 写入 run card(`local-cache` / `binance-rest`)
- [x] **T011** (2.3/FR-3) `backtest.rs`: `RunCard` 数据结构 + `RUN_CARD_SCHEMA_VERSION`
- [x] **T012** (2.3/FR-3) `backtest.rs`: `write_run_card` 落 `run/backtest/`, 失败只 warn
- [x] **T013** (2.3/FR-3) 2 单测(sha256/净化、参数快照不含源码)

## 批次 B: 工程底座

- [x] **T014** (2.4/FR-4) `db.rs`: `SCHEMA_VERSION` + `migrate()` 拆 `create_baseline_tables` / `run_version_migrations`(版本已最新零写事务)
- [x] **T015** (2.4/FR-4) v0→v1: `klines_lacks_market_column` + `rebuild_klines_with_market`(单事务、幂等)
- [x] **T016** (2.4/FR-4) `Database::open` 锁退避重试(仅 `is_locked_error`, async sleep)
- [x] **T017** (2.4/FR-4) 迁移单测(造 v0 库 → 迁移 → 数据保留 + 版本推进 + 可重入)
- [x] **T018** (2.5/FR-5) `deny.toml`(graph/advisories/licenses/bans/sources)
- [x] **T019** (2.5/FR-5) 内部依赖收敛到 `[workspace.dependencies]`, 解 `wildcards = "deny"`
- [x] **T020** (2.5/FR-5) `cargo update -p rustls --precise 0.23.45`(RUSTSEC-2026-0285)
- [x] **T021** (2.5/FR-5) `scripts/ci_grep_gates.sh` 四条红线 + 测试代码配平跳过
- [x] **T022** (2.5/FR-5) `.github/workflows/ci.yml` 加 `gates` / `deny` job + `macos-latest` 矩阵
- [x] **T023** (2.5/FR-5) `dist-workspace.toml` `pr-run-mode = "skip"` + `release.yml` 去 PR 触发
- [x] **T024** (3.3/FR-7) 新建 `ricow_strategy/src/events.rs`(`RunEvent` / `EventWriter` / `read_events`)
- [x] **T025** (3.3/FR-7) `context.rs`: 两上下文加 `events` + `run_mode`, 埋点下单/撤单/拒单
- [x] **T026** (3.3/FR-7) `command.rs`: `RunTelemetry` + `emit_run_event` + 两运行函数启动/收尾埋点
- [x] **T027** (3.3/FR-7) `commands/run.rs`: `EventWriter::create` + 三处调用点注入
- [x] **T028** (3.3/FR-7) 事件流单测 5 + 上下文单测 4

## 批次 C: 健壮性

- [x] **T029** (3.1/FR-6) `multiframe.rs` / `web/backtest_jobs.rs`: `lock_or_recover`
- [x] **T030** (3.2/FR-6) `CoreError::Db` 变体 + `SqlxResultExt::core()`
- [x] **T031** (3.2/FR-6) ~16 处手工 `Exchange` 映射换 `.core()?`, 清未用 import
- [x] **T032** (3.2/FR-6) 单测: 重开同库不再被锁 / sqlx 错误归一到 `Db`
- [x] **T033** (3.4/FR-8) `provider.rs`: LLM 重试(判定 + 退避 + 流式 emitted 边界)
- [x] **T034** (3.4/FR-8) `provider.rs`: 历史字符预算 + 整轮折叠 + 摘记
- [x] **T035** (3.4/FR-8) `session.rs::reply()` 接折叠
- [x] **T036** (3.4/FR-8) 6 单测(判定 / 边界 / 四例折叠)
- [x] **T037** (3.5/FR-9) `config_file.rs`: `schema_version` 读写 + 迁移(内存) + `defaults` 单一来源
- [x] **T038** (3.5/FR-9) `ai/config.rs`: `default_preset()` + 一致性单测
- [x] **T039** (3.5/FR-9) 5 单测(模板/老文件/未来版本/非法值/默认单一来源)

## 批次 D: 收尾

- [x] **T040** `cargo fmt --all` → `--check` 0 差异
- [x] **T041** `cargo clippy --workspace --all-targets --locked -- -D warnings` → 0(修 `run_live` 参数超限: 引入 `RunTelemetry`)
- [x] **T042** `cargo test --workspace --locked --no-fail-fast` → 642 passed / 2 环境性失败 / 22 ignored
- [x] **T043** `cargo deny --locked check` → advisories/bans/licenses/sources 全 ok
- [x] **T044** 同步 `specs/roadmap.md` / `architecture.md` / `backtest.md`
- [x] **T045** 落档本目录四件套

## 遗留(须显式记录)

- 2 例 `ai_live_smoke` 失败为**本机沙箱环境性**(`ERROR_PIPE_BUSY(231)`), 非代码缺陷。
- 分析文档第四节(walk-forward / 滑点敏感性)属**决策项**, 用户未回应 → 未做。
- 分析文档第五节(Web 前端工程债)低优先 → 本轮未做。
