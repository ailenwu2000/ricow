# 009 任务分解

- [x] T1 `Cargo.toml` 删 workspace member；cli/engine 两个 Cargo.toml 删 `locus_hl` 依赖
- [x] T2 删 `hl_exchange()`（cli/commands/mod.rs）
- [x] T3 删 `mod keys` 与 `pub use keys::load_hl_signer`（engine/lib.rs）
- [x] T4 删 keyring 的 `hl` 别名与提示行
- [x] T5 删 `FeeModel::hl_default()`
- [x] T6 注释/测试样例去 HL：`types.rs`、`keyring_store.rs`（含失效的 engine/keys.rs 指向）、`types_tests.rs`、
      `context.rs`、`backtest.rs`
- [x] T7 `cargo build --workspace` 无 error/warning
- [x] T8 `cargo test --workspace` = 204 / 0 / 6（确认差额 = locus_hl 16 测试）
- [x] T9 文档：constitution(1.0.1) + .specify 镜像、AGENTS.md、product.md(+D14)、architecture.md、lua-api.md、
      README/README_zh、backtest.md 基线、roadmap.md(基线 + 档案表)
- [x] T10 残留 grep 复核（仅 research 与 009 档案命中）
- [x] T11 删除 `crates/locus_hl` 与 `crates/locus_engine/src/keys.rs`（用户授权后由 agent 执行, 2026-09-12）
- [x] T12 删除后复跑 `cargo test --workspace` = 204 / 0 / 6 ✅；`locus --version` / `keyring list` 冒烟通过
