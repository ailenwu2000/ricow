# 022 技术方案

1. **一次性执行**: `cargo fmt --all`(工具链由 `rust-toolchain.toml` 钉死为 1.96.1, 保证可复现);
2. **单独提交**: 该提交只含格式差异, 便于 `git log --stat` 核对与 `git revert`;
3. **验证顺序**(全部通过才算收敛):
   - `cargo fmt --all -- --check` → 零差异;
   - `cargo clippy --workspace --all-targets --locked -- -D warnings` → 零输出;
   - `cargo test --workspace --locked` → 339 passed / 0 failed / 12 ignored;
   - `cargo build --release --locked` → 通过;
4. **开门禁**: `ci.yml` 增加 `cargo fmt --all -- --check` 步骤(与 clippy 同作业);
5. **文档同步**: CONTRIBUTING 去掉"尚未对齐"的说明, 改为"格式由 CI 强制"; `specs/roadmap.md` 的待办项关闭。

## 风险

- 大 diff(约 6k 行量级)但如果与其它改动混提, review 会失效 → 因此单独提交是硬要求;
- rustfmt 只做格式化, 理论上语义不变;仍以 clippy + 全量测试 + release 构建双确认。
