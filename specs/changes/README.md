# 变更档案索引（specs/changes/）

> 每个变更一个目录：`NNN-<slug>/`，内含 `spec.md`（需求）/ `plan.md`（方案）/ `tasks.md`（任务）/
> `converge.md`（收尾证据）。**编号唯一、不复用** —— 新增变更取「**目录 ∪ 全仓引用**」的最大编号 + 1
> （只数目录会踩雷，见下「045/046/048/050」那节）。

## 编号占用（截至 2026-10-11）

| 编号 | 目录 |
|:--|:--|
| 001 | `001-vwap` |
| 002 | `002-ai-quant-researcher` |
| 003 | `003-notifications` |
| 004 | `004-risk-guards` |
| 005 | `005-market-filter` |
| 006 | `006-bs-momentum-lua` |
| 007 | `007-bs-momentum-v2` |
| 008 | `008-platform-process-model` |
| 009 | `009-remove-hyperliquid` |
| 010 | `010-bs-rs-rotation` |
| 011 | `011-live-runner` |
| 012 | `012-futures-live-adapter` |
| 013 | `013-futures-margin-model` |
| 014 | `014-live-funding-and-liq` |
| 015 | `015-backtest-noop-fill` |
| 016 | `016-dryrun-initial-cash` |
| 017 | `017-spot-live-snapshot-fix` |
| 018 | `018-first-use-risk-ack` |
| 019 | `019-ai-assistant` |
| 020 | `020-platform-scope-trim` |
| 021 | `021-clippy-cleanup` |
| 022 | `022-rustfmt-alignment` |
| **023** | `023-ai-chat-ux` **＋** `023-shannon-etf-accum`（重号） |
| 024 | `024-dryrun-position-side` |
| 025 | `025-web-ui` |
| 026 | `026-trade-visibility` |
| 027 | `027-stop-unknown-instance` |
| 030 | `030-shannon-spot-grid` |
| 031 | `031-strategy-catalog-manifest` |
| **032** | `032-futures-paired-grid` **＋** `032-web-ui-console`（重号） |
| **033** | `033-key-manager` **＋** `033-uniswap-v2-spot-grid`（重号） |
| **034** | `034-event-driven-onfill` **＋** `034-ui-themes-fold`（重号） |
| **035** | `035-engineering-hardening` **＋** `035-live-heartbeat-tick`（重号） |
| **036** | `036-backtest-sensitivity-web-hardening` **＋** `036-paired-futures-event-model`（重号） |
| **037** | `037-live-safety-hardening` **＋** `037-uniswap-v2-event-model`（重号） |
| **038** | `038-engine-intrabar-fill-fidelity` **＋** `038-p1-robustness-hardening`（重号） |
| **039** | `039-paired-grid-event-model` **＋** `039-web-chart-upgrade`（重号） |
| **040** | `040-realtime-market` **＋** `040-shannon-grid-futures`（重号） |
| **041** | `041-shannon-futures-delev` **＋** `041-web-dev-loop`（重号） |
| **042** | `042-notify-im-alerts` **＋** `042-strategy-js-split`（重号） |
| **043** | `043-ops-log-enhance` **＋** `043-web-token-sse-fix`（重号） |
| **044** | `044-optimize` **＋** `044-webui-ux-polish`（重号） |
| 045 | **合约策略线占用**（全仓 cross）· **无档案目录** |
| 046 | **合约策略线占用** · **无档案目录** |
| 047 | `047-shannon-grid-futures-auto-delev` |
| 048 | **合约策略线占用**（去降杠杆+强平清市）· **无档案目录** |
| 049 | `049-shannon-short-grid-futures` |
| 050 | **合约策略线占用**（回测 K 线并发分页）· **无档案目录** |
| 051 | `051-audit-remediation`（本索引建立时的最新） |

### 045 / 046 / 048 / 050：被占用但**没有目录**（重要）

这四号与 047/049 同属合约策略线，但**档案目录没建**（047/049 建了）。证据（不是推测）：

- commit `6e94a48 feat(045-046): 合约全仓(cross)模式 + 纯现金记账修复 + 网格漂移死锁修复 + 策略更名…`；
- 代码里留有 `045-R3` / `045-R4` 的成片注释（`crates/ricow_strategy/src/backtest.rs`）；
- `specs/backtest.md:127,128,132` 写着「全仓(cross) margin 模式(045, 2026-09-30)」；
- `specs/changes/047-shannon-grid-futures-auto-delev/plan.md:39` 写着「策略 id 复用空闲旧名（045/046 改名让位）」；
- `specs/research/webui-ux-help-plan-2026-10.md:21` 的「覆盖变更」一行列到 050。

**推论（写进规则）**：`取目录最大编号 + 1` 这条判据**不够** —— 目录最大是 049，但 050 也被占了。
正确判据 = **目录 ∪ 全仓引用**取最大 + 1。若只看目录，下一个变更会踩到 050。

## 关于重号（历史遗留，不追溯重命名）

032–044 各存在两个同号目录：这是**两条并行工作流**（合约策略线 032–044 与 Web UI 线 032–044）
各自递增造成的。**不物理重命名**，理由：

1. 仓库多处（`architecture.md` / `roadmap.md` / 代码注释 / skill 文档）以 `specs/changes/NNN-slug/`
   路径引用，重命名会大面积断链；
2. 宪法「定稿不回改」；
3. 收益（编号好看）远小于断链与 git 历史错乱的风险。

**前向约定**：此后**一个编号一个变更、不复用、不跳号倒退**；新变更前先 `ls specs/changes/` **并**
`grep` 全仓引用，取「目录 ∪ 引用」的最大编号 + 1。051 起遵守。

**自体追认（051）**：本索引与整改档案最初落成时取了 **045**，而 045 已被合约线占（见上），
**属复用**。已改号为 **051**，并同步改正 `specs/`、`crates/`、`scripts/` 中全部「051 复核…」注释。
留此记录，说明这条约定是被自己踩过之后才写准的。

## 档案齐备性说明

部分目录缺 `tasks.md`/`spec.md`/`plan.md`：其中一部分是**调研/收尾归档**（如 `033-uniswap-v2-spot-grid`、
`037-uniswap-v2-event-model`）而非完整 SDD 变更；另一部分是历史变更的欠账。宪法要求**新变更**齐
`spec.md` + `plan.md` + `tasks.md`，历史欠账不追溯补齐（补写会与当时事实不符）。051 起新变更一律齐备。
