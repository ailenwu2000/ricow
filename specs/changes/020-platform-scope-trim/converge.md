# 020 收敛报告: 平台瘦身(删 scan / 删平台级亏损熔断 / 暴露盈亏状态)

> 日期: 2026-09-15 | 基线: 354 passed / 0 failed / 12 ignored → 本变更后 **337 passed / 0 failed / 12 ignored**(删除的功能连同其测试一并移除)

## 一、逐项对照(spec §二 范围)

| 项 | 状态 | 证据 |
|:--|:--|:--|
| 删 `locus scan`(CLI + engine) | ✅ | `main.rs` / `commands/mod.rs` / `locus_engine/src/lib.rs` 引用已移除;`locus --help` 无 `scan` |
| 删 engine `indicators`(`ema`/`adx` + 测试) | ✅ | 该模块仅 scan 使用;文件删除后 `cargo build` 无未用告警 |
| `market_class` **保留**(改判) | ✅ | 用户 2026-09-11 明示保留"美股数据层"通用能力 → 未删;见 spec §一.4 |
| 删两级亏损熔断(代码/参数/告警/通知/测试) | ✅ | `risk.rs` 删除 `LossCircuitBreaker`/`LossPeriod`/2 个 `RiskError` 变体/`is_reducing`;`config.rs` 删 5 个 `risk_*` 参数;`notify.rs` 删"熔断"事件(4→3);`context.rs` 删 `circuit_reject` 留痕;`command.rs` 删两处通知块 |
| 保留工程护栏 + 用户显式静态限额 | ✅ | `RiskEngine::from_config` 默认装配 1 条(频率上限);单测 `test_from_config_defaults_only_rate_limit` 断言;配静态项后为 2 条 |
| 新增 `ctx:net_pnl()` / `ctx:equity()` | ✅ | `lua.rs` 同步 + 注册;`lua-api.md` 补表;真机实测见 §二 |
| 文档同步 | ✅ | product(D3/D5/D10/§三.1/§三.4/新增 D15/D16) / architecture(命令集/§二 crate/§八 删节/护栏) / backtest(风控参数表) / roadmap(P2/004/005/保留清单) / 004+005 档案追加修订 / README(网络与数据源) / agent-kit 手册(网络 + 平台边界两条) |

## 二、真实验证(demo/真实数据, 非 mock)

1. **回归**: `locus backtest --strategy shannon_grid --pair ETHUSDT --days 30 --interval 1h` 跑通, 成交 12 笔、手续费 63.339975539270422074983046803(与删除熔断前同一量级/同一路径)。
2. **策略自管回撤生效**: 同命令加 `--param dd_stop_pct=0.001` → 日志 `自管回撤触发, 停止买入 (权益=99857.34, 峰值=100000.0, 阈值=0.001)`, 成交笔数 12 → **6**(买入被抑制)。默认 `dd_stop_pct=0` 时无该日志、成交仍 12 → **默认关闭、零行为变化**。
3. **新增状态真实可用**: 探针策略(买入 0.5 ETH 后逐 tick 打印)输出
   `PROBE 买入前: equity=100000.00 net_pnl=0.000000` → `PROBE tick: equity=100017.3523 net_pnl=-1.237700 pos=0.500000 px=2512.58`,
   与同次报告 `手续费: 1.237700000`(net_pnl = 已实现盈亏 0 − 手续费)一致; equity = 现金 + 0.5×2512.58 自洽。
4. **无残留引用**: `grep` 检查 `ScanConfig|ScanEntry|ScanReport|run_scan|LossCircuitBreaker|LossPeriod|PeakDrawdownHalt|ConsecutiveLossHalt|risk_peak_drawdown_pct|risk_consecutive_loss_periods|risk_loss_period|risk_level1_enabled|risk_level2_enabled|circuit_reject|take_circuit_reject|CircuitBreaker` → crates/ 下 **0 命中**(待删文件除外)。

## 三、遗留与如实说明

- **文件删除**: `crates/locus_cli/src/commands/scan.rs`、`crates/locus_engine/src/scan.rs`、`crates/locus_engine/src/indicators.rs` 三个已解引用文件
  已删除(**用户于 2026-09-15 显式授权 agent 执行删除**); 删除后复跑 `cargo build` 零错误零告警、`cargo test --workspace` 337 passed / 0 failed / 12 ignored(exit 0)。
- **网络**: 本机 WSL 直连 `api.binance.com` 不通(宿主代理未暴露给 WSL), 上述真机验证通过 `LOCUS_BN_BASE_URL=https://data-api.binance.vision`(官方公开数据域, 真实 K 线)完成;**未做签名链路(demo 下单)验证** —— 该路径不受本次改动影响(未动下单/门禁/凭据), 但如需复验需先让 WSL 走代理。
- 旧策略 TOML 里若残留已删的 `risk_*` 参数: TOML 未知键被忽略(**不报错也不生效**), 已在 `specs/backtest.md` 写明。
