# 018 实施计划

## 一、决策

| # | 决策 | 理由 |
|:--|:--|:--|
| D1 | 一次性确认(记在数据目录), 而非每次实盘都要带开关 | 承诺是"**首次**使用风险确认"; 每次都要求会变成噪音, 用户会直接把它加成别名绕过 |
| D2 | 确认入口 = `--accept-risk` 开关, 不新增子命令 | 少一个命令面; 拒绝消息里已给出确切命令; 与 `--live` 同一条命令行, 语义连贯 |
| D3 | 记录文件放 `$LOCUS_ROOT/risk_ack.json` | 与 `run/daemon.json` 同性质(本机自有状态), 不新增 DB 表、不新增配置面 |
| D4 | 带 schema 版本 | 披露内容实质变更(如新增风险类目)时可递增版本触发重新确认, 不必改代码逻辑 |
| D5 | 判定放在**最前**(早于时长门禁与时钟预检) | 未确认时不该产生任何交易所往返; 也避免"时钟不对"掩盖真正的合规提示 |
| D6 | 判定为纯函数(`risk_gate`), 文件读写留在 CLI | 可单测; 与 002 的 `dry_run_gate`、011 的 `check_clock_skew` 同一风格 |

## 二、改动清单

| 文件 | 改动 |
|:--|:--|
| `crates/locus_engine/src/live.rs` | `RISK_DISCLOSURE` 常量 + `RiskGate` 枚举 + `risk_gate()` 纯函数 + 2 单测 |
| `crates/locus_engine/src/lib.rs` | 导出 |
| `crates/locus_cli/src/commands/mod.rs` | `risk_ack_path()` / `risk_acked()` / `write_risk_ack()` |
| `crates/locus_cli/src/commands/run.rs` | `--accept-risk` + 实盘分支最前置判定 |
| `crates/locus_cli/src/commands/ctrl.rs` | `--accept-risk` + `start --live` 前置判定(子进程据此放行) |
| 文档 | `architecture.md` / `product.md` / `roadmap.md` / P4 runbook |

## 三、风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 用户嫌烦直接抄 `--accept-risk` 不看披露 | 披露要点直接印在拒绝消息里(不点 README 就看不到); 只要求一次 |
| R2 | `start --live` 由 daemon 派生 `run --live` 子进程, 子进程是否也要求确认 | 父进程在发起请求前落地确认记录 → 子进程读到即放行; 未确认则父进程直接拒绝(不起子进程) |
| R3 | 记录文件损坏/版本不符被当成"已确认" | 解析失败或版本不符一律视为未确认(宁可再问一次) |

## 四、验收动作

1. 全量测试(306 → 308)+ `cargo build`。
2. 全新数据目录三步真实验证(拒绝 / 确认 / 不再要求)。
3. demo 账户核对(不留下持仓与挂单)。
4. 文档同步。
