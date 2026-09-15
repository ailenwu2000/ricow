# 021 收敛报告(clippy 存量告警清零 + CI 硬门禁)

日期: 2026-09-15 | 结论: **已收敛**

## 一、验收项逐条核对

| 项 | 判据 | 实测结果 |
|---|---|---|
| 告警清零 | `cargo clippy --workspace --all-targets --locked -- -D warnings` 零输出 | ✅ 退出码 0, 无 error/warning 行 |
| 测试不回归 | 与变更前同基线 | ✅ `cargo test --workspace` = **339 passed / 0 failed / 12 ignored**(变更前同为 339/0/12) |
| CI 门禁生效 | lint 作业含 `-D warnings` 且能挡住新告警 | ✅ 见第二节"反向验证" |
| 行为零变化 | 只做机械修复 + 局部 allow/类型别名 | ✅ 逐文件复核 diff: 全部语义等价(`into_values`、`if let`、`inspect` 替代 `map` 后返回值不变、测试夹具 `&mut→&`、`from_ref` 替代 `clone`) |
| 文档同步 | 测试基线数字 | ✅ `specs/architecture.md` 与 `specs/roadmap.md` 同步为 339/0/12 |

## 二、反向验证(门禁真的会红)

临时分支 `tmp/ci-gate-red-test`(已删除)注入 `unused_mut` 探针后推送:

| 作业 | 结果 | 说明 |
|---|---|---|
| lint (clippy + fmt) | ❌ failure(`clippy` 步) | 正是预期: `-D warnings` 把告警变成硬失败 |
| test (ubuntu-latest) | ✅ success | 说明失败**只**来自 lint, 门禁定位准确 |
| test (windows-latest) | ✅ success | 同上 |

## 三、实施中发现并一并处理的问题

**工具链版本漂移**(根因, 详见 spec.md §五): `rust-toolchain.toml` 原为浮动 `stable`, 本地解析 1.96.1 而 CI 解析 1.98.1 →
同一份代码在 CI 上出现本地看不到的告警, 门禁打开即红。处置: 钉死 `1.96.1`, CI 去掉 `dtolnay/rust-toolchain@stable` 改用 `rustup show`
(版本唯一来源 = `rust-toolchain.toml`)。**升级到 1.98.1 单独立项**(需本地安装新工具链并清掉新告警), 不在本变更内顺手做。

## 四、遗留(显性技术债, 非本变更范围)

- 4 处 `too_many_arguments`(`engine/command.rs` ×2、`engine/live.rs`、`cli/supervisor/procs.rs`)、2 处测试 `await_holding_lock`:
  以 `#[allow]` + 注释保留, 参数聚合重构另行立项(本变更刻意不做行为不变的大重构, 以免污染"清零"的 review 面)。
- `cargo fmt --all --check` 仍有 38 个文件差异 → 独立变更 022(门禁仍未开 fmt, 已在 CONTRIBUTING 说明)。
