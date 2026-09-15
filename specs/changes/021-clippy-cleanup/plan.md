# 021 技术方案

## 一、告警分类与处置(2026-09-15 实测 28 条)

| 类别 | 数量 | 处置 |
|---|---|---|
| `clippy::unnecessary_mut_passed`(测试里 `rule.check(&req, &mut ctx)`) | 11 | 机械修复:去掉多余的 `mut`(`risk.rs` 测试块) |
| `clippy::too_many_arguments`(`engine/command.rs` 9/8、`engine/live.rs` 8、`cli/supervisor/procs.rs` 8) | 4 | **局部 `#[allow]` + 注释**,参数聚合另行立项(避免混入行为不变的大重构) |
| `clippy::await_holding_lock`(测试 `ENV_LOCK` 跨 await) | 2 | **局部 `#[allow]` + 注释**:测试串行化环境变量的既有设计,跨 await 持有是有意的 |
| `clippy::doc_lazy_continuation`/文档列表缩进(`name.rs`×2、`us_tickers.rs`) | 3 | 机械修复:补缩进 |
| `clippy::redundant_closure` / `useless_format` / `manual_inspect` | 4 | 机械修复(自动建议) |
| `clippy::single_match` / `needless_return` / 可省略生命周期 / `op_ref` / `needless_collect` | 5 | 机械修复 |
| `clippy::redundant_clone` → `std::slice::from_ref`(`engine/live.rs`×2) | 2 | 机械修复(`clippy --fix` 建议) |
| `clippy::type_complexity`(`ricow_strategy/src/backtest.rs`) | 1 | 引入局部 `type` 别名 |

## 二、执行顺序

1. `cargo clippy --workspace --all-targets --locked --fix`(只应用机器可判定建议)→ `git diff` 逐文件复核;
2. 手工处理 4 处 `#[allow(too_many_arguments)]`、2 处 `#[allow(await_holding_lock)]`、1 处 `type` 别名;
3. `cargo test --workspace` 复核不回归;
4. `.github/workflows/ci.yml` 打开 `-- -D warnings`,推送后看 CI;并做一次"故意引入告警 → 确认变红 → 撤销"的验证。

## 三、风险

- `clippy --fix` 的机器建议理论上语义等价,但仍逐文件看 diff(尤其 `engine/live.rs` 的 `from_ref` 与测试夹具改动);
- 局部 `#[allow]` 是"技术债显性化",不是修复:已在 tasks 里留待办,后续聚合参数时一并清掉。
