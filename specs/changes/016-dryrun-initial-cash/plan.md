# 016 实施计划

## 一、决策

| # | 决策 | 理由 |
|:--|:--|:--|
| D1 | 参数名 `params.initial_cash`(与回测 `--cash` 同名同义) | 同一件事用同一个词; 复用现有 params 通道, 不新增配置面 |
| D2 | 默认值保持 100,000 | 不改变既有行为/既有用户配置的语义 |
| D3 | 非法值**报错**而非回落默认 | 静默回落 = 预演口径偷偷变回 100k, 正是本次要消除的坑 |
| D4 | 解析放 `locus_engine::live::dry_run_initial_cash`(纯函数) | 可单测; 与 002 的 `dry_run_gate`/011 的 `check_clock_skew` 同处风格 |

## 二、改动清单

| 文件 | 改动 |
|:--|:--|
| `crates/locus_engine/src/live.rs` | `DEFAULT_DRY_RUN_CASH` + `dry_run_initial_cash(Option<f64>) -> Result<Decimal, String>` + 2 单测 |
| `crates/locus_engine/src/lib.rs` | 导出 |
| `crates/locus_cli/src/commands/run.rs` | Dry Run 分支改用该函数 + 打印 `Dry Run 虚拟本金: ...` |
| 文档 | `lua-api.md` / `architecture.md` / `roadmap.md` / P4 runbook |

## 三、风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 默认值变化影响既有 Dry Run 用户 | 默认不变(100k), 且有单测锁定 |
| R2 | 用户设了不合理的小本金导致策略无法建仓(低于 minNotional) | 属用户知情选择; 启动打印本金使其可见; `[risk]` 与交易所 minNotional 仍各自把关 |

## 四、验收动作

1. 全量测试 + 2 个新单测。
2. 真实预演(同一份小资金配置): `Dry Run 虚拟本金: 300 USDT` → 建仓 150 USDT → 拒单 0。
3. 非法值真实复现(0.0 → 报错)。
4. 文档同步检查。
