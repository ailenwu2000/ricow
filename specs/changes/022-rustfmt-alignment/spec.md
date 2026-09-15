# 022 全仓 rustfmt 对齐

## 背景

CI 至今**没有格式门禁**:`cargo fmt --all -- --check` 实测有差异(`specs/roadmap.md` 曾记 34 个文件, 2026-09-15 改名后实测 38 个)。
格式门禁缺席的代价是贡献者的格式噪声会持续进 PR;而 `roadmap.md` 早就把"全仓对齐"列为待办, 只是当时**缺一个确定的 rustfmt 基准版本**。

021 已把工具链钉死为 `1.96.1`(`rust-toolchain.toml`), 基准问题解决 → 现在可以一次性对齐并开门禁。

## 目标

1. `cargo fmt --all` 一次性对齐全仓, `cargo fmt --all -- --check` 变为**零差异**;
2. `ci.yml` 的 lint 作业打开 `cargo fmt --all -- --check`;
3. 行为零变化(格式变更不碰语义), `cargo test --workspace` 保持 339 passed / 0 failed / 12 ignored;
4. clippy `-D warnings` 仍为零(格式化不得引入新告警)。

## 不做

- 不调 `rustfmt.toml` 参数(保持 `max_width = 100` / `use_small_heuristics = "Max"`);
- 不趁机改代码语义、不留 TODO、不重命名;
- 不把本变更与其它改动混在一个提交里(格式噪声必须可单独 review / 可单独 revert)。
