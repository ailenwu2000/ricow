# 009 收敛核对 (converge)

日期: 2026-09-12

| 验收标准 | 实测 | 结论 |
|:--|:--|:--|
| `cargo build --workspace` 无 error/warning | Finished dev profile, 无 warning | ✅ |
| `cargo test --workspace` = 204 / 0 / 6 | 删除 `locus_hl`/`keys.rs` 前后均为 204 passed / 0 failed / 6 ignored | ✅ |
| 残留 grep 零命中 | 代码/权威文档零命中; 仅剩 `specs/research/*`(调研档案, 按宪法不回改) 与 009 档案自身的决策记录 | ✅ |
| 破坏性删除 | 用户授权后执行 `rm -rf crates/locus_hl crates/locus_engine/src/keys.rs`（git 历史可回溯） | ✅ |
| CLI 冒烟 | `locus --version` / `locus keyring list`(仅剩 binance) 正常 | ✅ |

## 结论

**已完全收敛**。后续不需要再动本变更。

## 留档

- 唯一遗留的"HL"字样都在决策记录里（product.md D14、constitution 修订记录、architecture.md/lua-api.md 的"已移除"标注、
  roadmap 基线说明），属有意保留的历史，不是实现残留。
- 若将来再接 Hyperliquid 或第二家交易所，按新变更重新立项，不复活本次删除的代码。
