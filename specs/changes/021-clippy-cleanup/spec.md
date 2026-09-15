# 021 clippy 存量告警清零

## 一、背景与问题

CI 已接入(`.github/workflows/ci.yml`),但 lint 作业只跑 `cargo clippy`(不加 `-D warnings`),原因:**存量告警非零**。
2026-09-15 实测(改名后重跑):`cargo clippy --workspace --all-targets --locked` = **0 error / 28 条唯一告警**(12 个文件)。
一个永不失败的门禁等于没有门禁,新告警会持续堆积 —— 因此先把存量清零,再让 `-D warnings` 真正生效。

## 二、目标(可验证)

1. `cargo clippy --workspace --all-targets --locked -- -D warnings` **零输出**;
2. `.github/workflows/ci.yml` 的 lint 作业启用 `-D warnings`,并在 PR 上实测"门禁真的会红"(故意引入问题后撤销);
3. `cargo test --workspace` 与变更前同基线(339 passed / 0 failed / 12 ignored);
4. 行为零变化:本变更只做机械修复 + 少量类型别名/局部注解,不改交易语义。

## 三、不做(YAGNI)

- 不做 fmt 对齐(独立变更 022;当前 38 个文件与 rustfmt 有差异);
- 不顺手重构:4 处 `too_many_arguments` 以局部 `#[allow]` + 注释保留,参数聚合单独立项(避免把行为不变的重构混进"清零"变更);
- 不改动任何交易路径的判断逻辑、阈值、时序。

## 四、验收

| 项 | 判据 |
|---|---|
| 告警清零 | `cargo clippy --workspace --all-targets --locked -- -D warnings` 无输出 |
| 测试不回归 | `cargo test --workspace` = 339 passed / 0 failed / 12 ignored |
| CI 生效 | lint 作业含 `-D warnings`;PR/推送后 CI 绿;并验证过"引入告警会红" |

## 五、实施中发现并一并处理的问题(2026-09-15)

打开 `-D warnings` 后 CI 立即红, 而本地同命令零告警 —— 根因不是新 lint, 而是**工具链版本漂移**:
本地 `stable` = 1.96.1, 而 CI 的 `stable` 当时已是 **1.98.1**(`rust-toolchain.toml` 写的是浮动的 `stable`, CI 又用 `dtolnay/rust-toolchain@stable`)。

处置(并入本变更):
- `rust-toolchain.toml` 由浮动 `stable` 改为**钉死 `1.96.1`**(附升级流程注释);
- CI 两个作业去掉 `dtolnay/rust-toolchain@stable`, 改为 `rustup show` —— 工具链版本**唯一来源 = `rust-toolchain.toml`**;
- 升级到 1.98.1 单独立项(需本地装 1.98.1 + 清掉新告警), 不在本变更内顺手做。
