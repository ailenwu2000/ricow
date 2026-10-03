# 收敛记录: 035 工程底座加固

**功能目录**: `specs/changes/035-engineering-hardening`

**收敛日期**: 2026-10-03

**结论**: 分析文档第二/三节的 9 项(2.1–2.5、3.1–3.5)全部落地并验证; 第四节(决策项)**未做**(用户未回应); 第五节(低优先)**未做**。

## 一、三门禁实跑结果

| 门禁 | 命令 | 结果 |
|---|---|---|
| 格式 | `cargo fmt --all -- --check` | **0 差异**(先 `cargo fmt --all` 落平) |
| 静态 | `cargo clippy --workspace --all-targets --locked -- -D warnings` | **0 告警**(见 §四.1 修掉 `too_many_arguments`) |
| 依赖 | `cargo deny --locked check` | `advisories ok, bans ok, licenses ok, sources ok` |
| 测试 | `cargo test --workspace --locked --no-fail-fast` | **642 passed / 2 failed(环境性)/ 22 ignored** |

测试增量按 crate 精确核对(`git diff` 统计新增 `#[test]`/`#[tokio::test]`):

| 条目 | 文件 | +用例 |
|---|---|---|
| 2.1 币安 REST 重试 | `ricow_binance/src/retry.rs`(新文件) | 4 |
| 2.2 缓存区间读取 | `ricow_strategy/src/db.rs` | 1 |
| 2.3 run card | `commands/backtest.rs` | 2 |
| 2.4 迁移 v0→v1 / 重开不被锁 / sqlx 归一 | `ricow_strategy/src/db.rs` | 3 |
| 3.3 事件流 | `ricow_strategy/src/events.rs`(新文件 5)+ `context.rs`(4) | 9 |
| 3.4 LLM 重试 + 历史预算 | `ai/provider.rs` | 6 |
| 3.5 配置 schema 版本 | `commands/config_file.rs`(5)+ `ai/config.rs`(1) | 6 |
| **合计** | | **31** |

611(034 基线) + 31 = **642** ✓ 与实跑一致。

## 二、2.4 顺带根治: `SQLITE_BUSY` 抖动

**现象**: 高负载轮次偶见 `ai::tools` 部署类用例失败 —— `打开本地库失败: ... (code: 5) database is locked`。

**根因**(定性到行):
- `Database::open` 每次跑 `migrate()` —— 旧实现全是 `CREATE TABLE IF NOT EXISTS`, 即**每次 open 都开一个写事务**;
- 测试脚手架会在同一进程内"开池 → `pool.close().await`(WAL checkpoint + 删文件)→ 立刻再开";
- 新池建池时执行 `PRAGMA journal_mode=WAL` **需要独占锁**, 而 SQLite 对"另一连接正在使用"**不调用 busy handler**, 直接返回 `SQLITE_BUSY(5)`。竞争者只活几十毫秒。

**修法双管**:
1. 迁移改为 `PRAGMA user_version` 驱动, **版本已最新时零写事务直接返回**;
2. `Database::open` 加 10 次线性退避重试, **仅** `is_locked_error`(code 5/6 或 message 含 "database is locked")时重试。
   用 `tokio::time::sleep` 而非 `std::thread::sleep` —— 后者会阻塞 current_thread 运行时, 把上一个池的 `close()` 收尾一起卡死, 重试永远等不到锁释放。

**取证**: 连续 3 次 `cargo test -p ricow --bin ricow`:

```
=== 第 1 次 === test result: ok. 317 passed; 0 failed; 1 ignored
=== 第 2 次 === test result: ok. 317 passed; 0 failed; 1 ignored
=== 第 3 次 === test result: ok. 317 passed; 0 failed; 1 ignored
```

三层线索互证: ① `specs/roadmap.md` 034 基线已把该抖动记为**已知环境噪声**; ② 033 曾顺手修掉两处同类; ③ 本轮从**产品代码语义**处根治, 而非再打补丁。

## 三、2.5 门禁本地验收

### 3.1 `deny.toml` 调优过程(如实)

- 首版把 `unmaintained`/`unsound` 写成字符串 → cargo-deny v0.20.2 报反序列化失败
  (`expected '["all", "workspace", "transitive", "none"]'`); 改 `"workspace"` 并删掉不存在的 `vulnerability` 键(v2 语义下默认即 deny)。
- `[bans].wildcards = "deny"` 对 9 条内部 path 依赖报警(`publish = false` **不豁免**)→ 把内部依赖收敛到 `[workspace.dependencies]`(path + version), 成员写 `ricow_core.workspace = true`。
- `cargo deny check` 报**真实漏洞 RUSTSEC-2026-0285**(rustls 0.23.43)→ `cargo update -p rustls --precise 0.23.45`; 复跑转绿。
- 终态仍有 `winnow 0.7.15 / 1.0.4` 重复版本条目, 但按 `multiple-versions = "warn"` 只出警告(上游 `toml` 与 `rig` 各自拖的), 不阻断。

### 3.2 `scripts/ci_grep_gates.sh` 四条红线

① AI 工具层零落盘; ② 明文密钥不进日志; ③ 无 `dbg!/todo!/unimplemented!` 残留; ④ `execute_strategy` 调用点白名单。
测试代码由 `non_test_lines`(按 `#[cfg(test)]` 花括号配平整块跳过)排除。

- 本地全绿。
- 反面样例验证: 在 `/tmp/gateprobe` 造违规(含"违规写在测试模块内"这一对照组)→ 能抓到违规 **且**正确跳过测试代码。

> **踩过的坑**: 红线④最初用 `grep -vE '...\t'` 按 TAB 过滤白名单 —— ERE 里 `\t` **不是转义**, 恒不匹配导致误报;
> 改用 `awk -F'\t' '$1 == "..."'` 按第一列过滤才正确。

## 四、收敛期修掉的三处问题

### 4.1 `clippy: too_many_arguments`(8/7)

`run_live` 增传 `events` 后变成 8 个参数(clippy 阈值 7)。**修法**: 引入 `RunTelemetry { mode, events }`
打包两个"运行观测元信息"(都由 CLI 装配、引擎只消费), `run_live` 回到 6 个参数;
函数体内 `let RunTelemetry { mode, events } = telemetry;` 解构 —— **函数体一行未改**。

### 4.2 `test_compact_history_folds_whole_rounds_and_leaves_digest` 断言写错

原断言 `users == assistants`, 实跑得 4 vs 3。**根因**: 摘记本身**就是一条 user 消息**, 故折叠后 user 恰好多 1。
**修法**: 断言改为"**去掉摘记后**必须整轮成对"(`&h[1..]`), 把不变式表达准 —— 这是测试的错, 不是实现的错。

### 4.3 `test_sqlx_error_normalizes_to_core_db_variant` 编译不过

`sqlx::Database`(trait 关联类型)**不实现 `Debug`**, 无法用 `unwrap_err()`。改用显式 `match` 分派。

> 另有两处纯编译摩擦: 事件入流处 `Ok((base, cancelled, failed))` 类型推断失败 → 写全 `Ok::<_, CoreError>(...)`;
> `.core()?` 替换后 `deploy.rs` / `web.rs` 的 `CoreError` import 变为未用 → 删除。

## 五、环境噪声(非代码缺陷, 如实记录)

- `warning: error deleting lock file for incremental compilation session directory ... (os error 5)`: Windows 上增量编译锁文件的清理告警, 与代码无关, 不影响 `Finished`。
- `ai_live_smoke` 2 例失败(`approve_requires_interactive_tty` / `piped_confirm_phrases_never_reach_the_host`): 本机(agent 沙箱)报 `ERROR_PIPE_BUSY(231)` —— 环境对"spawn 子进程 + 喂 stdin 管道"的拦截; 与 032/033/034 基线同因, 真机终端为全绿。
- `cargo-deny` 本地安装耗时约 11 分钟(`cargo install cargo-deny --locked`, 装成 v0.20.2)。

## 六、不变量复核(未破坏既有行为)

- **架构铁律**: 引擎/CLI/绑定层**零策略参数名** —— 035 未新增任何策略参数; `executors` 与 Lua 侧零改动。
- **确认渠道**: 新事件流是**只写旁路**, 未参与任何写操作确认; live 逐字短语门禁一字未动(`crates/ricow/src/commands/run.rs` 的 `RunTelemetry` 只搬参数、不改判据顺序)。
- **落库结构**: `fills` / `pnl_snapshots` / `orders` / `positions` / `previews` / `funding_fees` / `web_*` / `us_klines` **表结构零改动**; 唯一结构变更是 `klines`(加 `market` 维度), 且带 v0→v1 迁移与旧数据保留。
- **真实调用纪律**: 本轮无新增需真实交易所环境的用例; 2.1 的重试与 2.2 的缓存**未在真机 testnet 重跑**(改动面是"网络失败时的行为", 需构造抖动才可复现, 故以单测 + 半开判定覆盖)。
  > ⚠️ **遗留**: 2.2 缓存"命中 vs 回退"两条分支的**真实网络取数**未做端到端取证 —— 若后续要发布, 建议在真机跑一次同窗口二次回测, 观察 `data_source` 从 `binance-rest` 变 `local-cache`。
