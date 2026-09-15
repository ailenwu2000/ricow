# 022 收敛报告(全仓 rustfmt 对齐 + 格式门禁)

日期: 2026-09-15 | 结论: **已收敛**

| 验收项 | 判据 | 实测 |
|---|---|---|
| 全仓对齐 | `cargo fmt --all -- --check` 零差异 | ✅ 退出码 0 |
| 规模 | 改动文件/行数记录 | ✅ 41 个文件, +612 −518 |
| 无语义改动 | clippy 零告警 + 测试同基线 + release 构建 | ✅ clippy `-D warnings` 退出码 0;测试 **339 passed / 0 failed / 12 ignored**;`cargo build --release --locked` 通过 |
| 格式门禁 | `ci.yml` 含 `cargo fmt --all -- --check` | ✅ 与 clippy 同作业 |
| 文档同步 | CONTRIBUTING / roadmap | ✅ CONTRIBUTING 改为"格式由 CI 强制";roadmap 待办项关闭 |

## 实施中的发现

- **rustfmt 内部错误**: `crates/ricow/src/commands/instances.rs` 里一处 `line!(out, ` 行尾带空格导致 rustfmt 报
  `error[internal]: left behind trailing whitespace` 并**拒绝格式化**(整体退出非零)。修掉该宏调用的换行后, `cargo fmt --all` 与 `--check` 均干净。
  教训: 全仓对齐前先 `cargo fmt --all -- --check` 看是否有 internal error, 否则会误以为"还对不齐"。
- 规模远小于 2026-09-14 记录(+6069 −1139): 当时的估算基于旧状态/旧 rustfmt 版本;钉死 1.96.1 后实测 41 文件。
