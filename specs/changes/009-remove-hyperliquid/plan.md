# 009 实施计划

## 决策

- **D1** 整 crate 删除，不做"只删引用、留 crate 待启用"——留着无用代码与文档承诺是本次问题的根源（用户明确"暂时不考虑"）。
- **D2** `Signer` / `KeyringSigner` 抽象随 crate 移除，不迁到 `locus_core`：其唯一消费者是 `load_hl_signer`，
  迁过去只会得到一个无消费者的抽象（YAGNI，宪法原则五）。将来若接新交易所，按新变更重新设计。
- **D3** 文献处理分层：权威文档(product/architecture/lua-api/constitution/AGENTS/README)就地改正；
  `specs/research/*` 是"已定稿调研档案"，HL 竞品事实保留不回改；变更档案 009 记录决策。
- **D4** `parse_pair` 的"带交易所前缀"语义测试保留，样例前缀由 `hl`(已不存在) 换成 `binance`/`bn`，
  避免删除平台时连带删掉通用能力测试。
- **D5** 破坏性删除（两个路径）由用户执行，agent 只做编辑与验证（宪法：破坏性删除用户自执行）。

## 改动清单

| # | 文件 | 改动 |
|:--|:--|:--|
| 1 | `Cargo.toml` | workspace members 删 `crates/locus_hl` |
| 2 | `crates/locus_cli/Cargo.toml` / `crates/locus_engine/Cargo.toml` | 删 `locus_hl` 依赖 |
| 3 | `crates/locus_cli/src/commands/mod.rs` | 删 `hl_exchange()` |
| 4 | `crates/locus_engine/src/lib.rs` | 删 `mod keys` / `pub use keys::load_hl_signer` |
| 5 | `crates/locus_engine/src/keys.rs` | 整文件待用户删除（已不在编译图） |
| 6 | `crates/locus_cli/src/commands/keyring.rs` | 删 `hl` 别名与提示行 |
| 7 | `crates/locus_strategy/src/fee.rs` | 删 `hl_default()` |
| 8 | `crates/locus_core/src/{types.rs,keyring_store.rs,types_tests.rs}` | 注释/测试样例去 HL |
| 9 | `crates/locus_strategy/src/{context.rs,backtest.rs}` | 注释与测试样例 `hl:ETH` → `binance:ETH` |
| 10 | `specs/constitution.md` + `.specify/memory/constitution.md` | 原则三去 HL；版本 1.0.1 + 修订记录 |
| 11 | `AGENTS.md` | 测试纪律行去 HL |
| 12 | `specs/product.md` | §四.3 卖点口径 / §七 返佣 / §十 密钥 / §十一 测试源 / D4 + 新增 D14 |
| 13 | `specs/architecture.md` | crate 表 6→5 / 架构图 / §九 测试源 / 基线 204 |
| 14 | `specs/lua-api.md` | §八 末行改"已移除" |
| 15 | `README.md` / `README_zh.md` | crate 列表 6→5 |
| 16 | `specs/backtest.md` | 基线 220 → 204（两处） |
| 17 | `specs/roadmap.md` | 测试基线 204；变更档案表新增 009 |

## 验证

- `cargo build --workspace`（无 error/warning）
- `cargo test --workspace`（204 / 0 / 6）
- 残留 grep：`hyperliquid|locus_hl` 仅命中 `specs/research/` 与 009 档案
- 用户执行 `rm -rf crates/locus_hl crates/locus_engine/src/keys.rs` 后再次 `cargo test --workspace`
