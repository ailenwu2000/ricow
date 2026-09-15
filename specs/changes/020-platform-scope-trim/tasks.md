# 020 任务分解: 平台瘦身

> 输入: [spec.md](spec.md) | 基线: **354 passed / 0 failed / 12 ignored**(以实跑为准)
> 约定: 每条含**文件位置**与**验证(期望)**。

## 阶段一: 删 `locus scan`

- [x] **T001** 删除 CLI 入口: `crates/locus_cli/src/commands/scan.rs`(整文件)、`main.rs`(import / `Command::Scan` 变体 / dispatch)、`commands/mod.rs`(`pub mod scan;`)。
      —— 验证: `locus --help` 不再有 `scan`;`cargo build` 通过。
- [x] **T002** 删除引擎实现: `crates/locus_engine/src/scan.rs`(整文件)、`lib.rs`(`mod scan;` 与 `pub use scan::…`);`backtest_runner.rs` 里作为**Lua 示例字符串**出现的 `pool = {...}` 不动(与 scan 无关)。
      —— 验证: `cargo build` 通过;`grep -rn "ScanConfig\|ScanEntry\|ScanReport\|Trend::" crates/` 为空。
- [x] **T003** engine `indicators.rs`: 删除仅服务 scan 的 `adx`(及测试)与 `ema`(及测试);`lib.rs` 的 `pub use indicators::{adx, ema}` 一并删除。**注意**: `locus_strategy/src/indicators_api.rs` 的 `adx`/`ema` 是 Lua 侧 API(`ctx:adx`),**保留**。
      —— 验证: `grep -rn "crate::indicators\|indicators::adx\|indicators::ema" crates/locus_engine/` 为空;`cargo test -p locus_engine` 绿。
- [x] **T004** ~~删除 `market_class`~~ **改判: 保留**(用户 2026-09-11 明示"美股数据层(nasdaq/us_tickers/market_class/us_klines)通用能力保留, 不算死代码, 不删", 见 `specs/roadmap.md` 已终止的探索)。
      本次只从 `lib.rs` 保留其 `pub use`(`is_bstock_base` / `bstock_spot_pool`)不动; `us_tickers` 同样保留(nasdaq / backtest 在用)。
      —— 验证: `cargo build` 通过; `market_class.rs` 与 `us_tickers.rs` 仍在且可编译。

## 阶段二: 删平台级亏损熔断

- [x] **T005** `crates/locus_strategy/src/risk.rs`: 删除 `LossPeriod` / `BreakerState` / `LossCircuitBreaker`(含 `impl RiskRule`)/ `RiskError::{ConsecutiveLossHalt, PeakDrawdownHalt}`(含 Display 分支)/ `DEFAULT_CONSECUTIVE_LOSS_PERIODS` / `DEFAULT_PEAK_DRAWDOWN_PCT` / 仅被熔断使用的 `is_reducing`。
      —— 验证: `cargo build -p locus_strategy` 通过;相关单测(test_level1_* / test_level2_* / test_consecutive_* / 周期结算类)一并删除后测试绿。
- [x] **T006** `risk.rs` 的 `RiskSettings` 与 `RiskEngine::from_config`: 去掉 5 个熔断字段与 breaker 装配,保留静态限额(最大持仓/单日亏损/最小订单/最大滑点)与**频率上限**。
      —— 验证: `RiskEngine::from_config` 在"只配默认"时 rule_count == 1(仅频率上限);配了静态项后相应增加。
- [x] **T007** `crates/locus_strategy/src/config.rs`: `RiskConfig` 删除 5 个字段并修正文档注释(不再提"默认启用熔断")。
      —— 验证: `grep -rn "risk_consecutive_loss_periods\|risk_loss_period\|risk_peak_drawdown_pct\|risk_level1_enabled\|risk_level2_enabled" crates/` 为空;`cargo build` 通过。
- [x] **T008** `crates/locus_strategy/src/context.rs` + `crates/locus_engine/src/{notify.rs,command.rs}`: 删除 `circuit_reject` 留痕字段与 `take_circuit_reject()`;删除 003 通知事件"熔断触发"(`NotifyEvent` 变体 + 文案 + 测试);`command.rs` 两处 `take_circuit_reject()` 调用块与 notify 启用日志文案(四类 → 三类事件)。
      —— 验证: `grep -rn "circuit_reject\|熔断" crates/` 无残留(注释里的历史说明也清掉);`cargo test -p locus_engine` 绿。
- [x] **T009** `crates/locus_strategy/src/lib.rs` 导出表: 去掉 `LossCircuitBreaker` / `LossPeriod`。
      —— 验证: `cargo build --workspace` 通过。

## 阶段三: Lua 只读盈亏状态

- [x] **T010** `crates/locus_strategy/src/lua.rs`: Lua ctx 同步 `net_pnl`(已实现净盈亏)与 `equity`(现金 + 持仓市值),注册 `ctx:net_pnl()` / `ctx:equity()` 两个只读方法;删除 `is_reducing` 后确认 Lua 侧无相关依赖。
      —— 验证: 单测/真机回测中策略能读到非零数值,且与报告"已实现盈亏/总价值"一致(同一 tick 口径)。
- [x] **T011** `specs/lua-api.md`: ctx API 清单补 `net_pnl` / `equity` 两行(含口径说明: net_pnl = 已实现净盈亏(扣手续费), equity = 现金 + 持仓市值;回测/Dry Run/实盘同一套)。
      —— 验证: 与代码实现对得上(字段来自同一 `PnlTracker`/余额快照)。
- [x] **T012** `strategies/builtin/shannon_grid.lua`: 加一段**默认关闭**的自管回撤示例(读 `ctx:net_pnl()` 与 `ctx:equity()`,达到用户设定比例就只平不开),注释说明"平台不再代做熔断,策略可自管"。
      —— 验证: 真机 `locus backtest --strategy shannon_grid` 行为与改动前逐位一致(默认关闭 ⇒ 零行为变化)。

## 阶段四: 文档与修订指引

- [x] **T013** `specs/product.md`: §三.4 通知事件四类 → 三类(去掉熔断);§六/§八 若有 scan 表述一并删除;决策清单追加"平台护栏边界"条目(工程护栏保留 / 赚赔政策归策略)。
- [x] **T014** `specs/architecture.md`: 命令集删 `scan`;§八"选币 scan"整节删除;§二 crate 角色去掉 scan 与 market_class;护栏描述改为只剩频率上限 + 静态限额。
- [x] **T015** `specs/backtest.md`: 风控小节的默认护栏描述与参数表按新口径改写(去掉 5 个熔断参数),补一句"盈亏政策属于策略,平台不代做"。
- [x] **T016** `specs/roadmap.md`: 004/005/scan 相关行追加修订指引(指向本档案);`specs/changes/004-risk-guards/spec.md` 与 `005-market-filter/` 追加一行修订说明(不改历史正文)。
- [x] **T017** 收敛: `cargo test --workspace` 全绿 + SC-002 的 grep 清单逐项为空 + 真机三例(`locus --help` 无 scan / Dry Run 真跑 / Lua 读到 net_pnl&equity),写 `converge.md`。
