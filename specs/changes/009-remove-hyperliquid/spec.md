# 009 移除 Hyperliquid

> 状态: ✅ 已实施 (2026-09-12)
> 依据: 用户 2026-09-12 指令 —— "Hyperliquid 当前并没有实现, 现在看对文档和开发都有影响。去掉 Hyperliquid 功能和代码。暂时不考虑。"
> 上游文档: [`specs/product.md`](../../product.md) D14 / [`specs/architecture.md`](../../architecture.md) §二 §九 / [`specs/constitution.md`](../../constitution.md) 原则三(1.0.1 修订)

## 一、问题

`locus_hl`(Hyperliquid 适配 crate, 1696 行: REST `/info` + 签名下单 + EIP-712 + WS)自始未落地：

- 运行路径全走币安 —— 唯一入口 `hl_exchange()` 带 `#[allow(dead_code)]` 且无调用者；
- `locus_engine::keys::load_hl_signer` 无调用者（HL 私钥加载器）；
- `FeeModel::hl_default()`、keyring 的 `hl` 别名同样无消费方。

但文档把它当成"保留待启用"的能力写进产品承诺（`product.md` 差异点/返佣/密钥/测试策略四处、
`constitution.md` 原则三"HL testnet 必测"、`architecture.md` crate 表与架构图）。后果：

1. **产品承诺 ≠ 实际能力** —— 测试纪律要求"HL testnet 真实调用"，而代码里根本没有可跑通的 HL 交易路径；
2. **误导开发** —— 后续按文档做"平台无关"设计，但第二平台实为半成品，返工风险持续累积。

## 二、范围

移除（代码）：
- crate `locus_hl` 整个删除；`Cargo.toml` workspace member、`locus_cli`/`locus_engine` 依赖行删除
- `locus_cli/src/commands/mod.rs` 的 `hl_exchange()`
- `locus_engine/src/keys.rs`(load_hl_signer) + `lib.rs` 的 `mod keys` / `pub use keys::load_hl_signer`
  —— `Signer`/`KeyringSigner` 抽象唯一消费者就是它，随 crate 一并移除，不留无消费者抽象
- `FeeModel::hl_default()`；keyring 命令的 `hl` 别名与提示文案
- 注释/测试样例中的 HL 提法：`parse_pair("hl:…")` 测试样例改为 `binance:`/`bn:`；`"hl:ETH"` 注释示例改 `"binance:ETH"`；
  keyring 单测 key 名去 `hl_` 前缀；`Market.step_size` 注释去 HL；`keyring_store.rs` 文档注释去 `hl/binance` 与失效的 `engine/keys.rs` 指向

移除（文档）：
- `constitution.md`(+`.specify/memory/constitution.md` 镜像) 原则三：测试纪律改为"币安 demo 测试网"，版本 1.0.1 + 修订记录
- `AGENTS.md` 测试纪律行；`product.md`(差异点 §四.3 / 返佣 §七 / 密钥 §十 / 测试策略 §十一 / D4) + 新增 D14
- `architecture.md`(crate 表 6→5、架构图、§九 测试策略)；`lua-api.md` §八末行；`README.md` / `README_zh.md` crate 列表

明确保留：
- `specs/research/*` 中的 HL 竞品事实（调研档案定稿不回改）
- 币安现货/USDT-M 合约/bStock 既有能力不做调整；不新增任何平台适配

## 三、验收标准

1. `cargo build --workspace` 无 error / warning
2. `cargo test --workspace` = **204 passed / 0 failed / 6 ignored**（基线 220 → 204，差额 = locus_hl 的 16 个内联测试）
3. 代码与权威文档 grep `hyperliquid|locus_hl` 零命中（仅 `specs/research/` 与 009 档案自身除外）
4. 破坏性删除由用户执行：`rm -rf crates/locus_hl crates/locus_engine/src/keys.rs`
   （两者已不在编译图内，删除前不影响构建）
