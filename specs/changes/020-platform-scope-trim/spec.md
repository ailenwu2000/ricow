# 020 功能规格: 平台瘦身 — 删 scan、删平台级亏损熔断、给 Lua 暴露盈亏状态

**状态**: 已定稿(2026-09-15 用户拍板) | **日期**: 2026-09-15
**起因**: 019 T047 的第三方 agent 真机验证暴露"平台越界做投资判断"(二级熔断把只亏 0.06 USDT 的策略判成"回撤 415%"并停掉全部新交易); 以及用户对平台定位的明确收敛。
**定位基准(本次确立)**: Locus = **策略运行平台**。平台提供执行、数据、门禁与状态; **赚赔政策属于策略**。平台护栏只做"防程序/接口失控、交易所规则、执行安全"。

## 一、为什么

1. **平台越界**: `004-risk-guards` 引入的两级亏损熔断(一级: 连续 N 结算周期净亏损停开仓; 二级: 已实现盈亏峰值回撤 ≥ X% 停全部新交易),是**投资判断**,不是工程保护。真实反例(T047): 峰值已实现盈亏 0.19 USDT、亏损 0.60 USDT 被判"回撤 415.77%",熔断全部新交易并刷 80 条 WARN —— 平台用一套固定口径替任意策略做盈亏判断,必然出现这类自相矛盾。
2. **策略想自管也管不了**: 查 `specs/lua-api.md` 与 `locus_strategy/src/lua.rs`,策略只能读 `ctx:position_side/size/entry` 与 `ctx:balance(asset)`,**看不到已实现盈亏与权益** —— "风控交给策略"当前缺原语。
3. **`locus scan` 与平台定位正交**: 它是选币/研究入口,不是执行链的一环;不在 AI 工具面,无集成测试,唯一消费者是它自己。用户的取舍: 删掉,不留废弃代码(2026-09-15)。
4. **顺带确认的死代码**: `market_class`(`is_bstock_base` / `bstock_spot_pool`)只被 `locus_engine/src/lib.rs` 重导出,全仓**零调用**(`scan.rs` 实际用的是 `us_tickers::us_ticker_of`)。它本是 005 为 scan 池参数引入 → 随 scan 一并删除。

## 二、范围

**做**:
1. **删除 `locus scan`**: CLI 子命令 + `locus_engine::scan` + 文档引用; engine 侧仅被 scan 使用的 `indicators::{adx, ema}`(及其测试)一并删除; **`market_class` 保留**(见 §一.4)。
2. **删除平台级亏损熔断**: `LossCircuitBreaker`(两级)/`LossPeriod`/`RiskError::{ConsecutiveLossHalt, PeakDrawdownHalt}`/`RiskSettings` 与 `RiskConfig` 的 5 个参数(`risk_consecutive_loss_periods` / `risk_loss_period` / `risk_peak_drawdown_pct` / `risk_level1_enabled` / `risk_level2_enabled`)/仅服务熔断的 `is_reducing` 与 `circuit_reject` 留痕 → 连带的 003 通知事件"熔断触发"一并删除(**不留废弃代码**)。
3. **给 Lua 暴露只读盈亏状态**: 新增 `ctx:net_pnl()`(已实现净盈亏, quote 计)与 `ctx:equity()`(总权益: 现金 + 持仓市值),使策略能自己实现回撤/止损规则;`specs/lua-api.md` 补条目,内置 `shannon_grid` 示范一条可选的"策略自管回撤"写法(默认关闭,不改变既有行为)。
4. **文档同步**: `product.md`(§三.4 通知事件四类 → 三类)/`architecture.md`(护栏描述、命令集、crate 角色)/`backtest.md`(风控参数表)/`roadmap.md`(004/005/scan 相关行加修订指引)/`004`、`005` 档案追加修订指向本档案。

**不做**:
- 不保留任何"开关式弃用"(用户: 留废弃代码没有意义)。
- 不改既有**工程护栏**: 下单频率上限(默认 100/s)、最小订单、最大滑点、资金不足/无仓可平拒单、实盘时钟预检。
- 不动**用户显式声明的**静态限额: `max_position_notional` / `max_daily_loss_usd`(配置才启用)。边界判定: 平台不替用户定政策,但执行用户自己写在 TOML 里的政策 —— 故保留。
- 不引入新机制/新表/新通道;不动下单路径、门禁顺序、凭据口径。

## 三、验收

- **SC-001** `cargo test --workspace` 全绿(基线 354 passed / 0 failed / 12 ignored,最终数字以实跑为准)。
- **SC-002** 残留引用归零: 全仓 `grep -rn` 不再出现 `ScanConfig|ScanEntry|LossCircuitBreaker|LossPeriod|PeakDrawdownHalt|ConsecutiveLossHalt|risk_peak_drawdown_pct|risk_consecutive_loss_periods|risk_loss_period|risk_level1_enabled|risk_level2_enabled|market_class|bstock_spot_pool|circuit_reject|take_circuit_reject`(文档中"已删除/修订说明"的历史记录除外)。
- **SC-003** 真机: `locus --help` 无 `scan`;`locus backtest`/`locus run`(Dry Run)真跑一次,报告口径不变、无熔断类日志。
- **SC-004** 真机: 用一段 Lua 策略读到 `ctx:net_pnl()` / `ctx:equity()` 的真实数值(回测中打印),并据此实现一条回撤规则(触发时机与数值可对照报告核验)。
- **SC-005** 配置侧: 旧策略 TOML 里若仍写着已删除的 `risk_*` 参数,装载行为必须**明确**(不得静默生效或静默丢失)—— 采用与既有非法值同一直径: 未知参数不报错(TOML 表已有 `#[serde(default)]` 语义),但在文档中如实说明"这几个参数已删除,不再生效"。

## 四、方案(即实现顺序)

1. 删 scan:`crates/locus_cli/src/commands/scan.rs`、`main.rs`、`commands/mod.rs`、`crates/locus_engine/src/{scan.rs,indicators.rs(adx/ema),market_class.rs}`、`lib.rs` 导出;`us_tickers` 保留(nasdaq/backtest 仍在用)。
2. 删熔断: `locus_strategy/src/{risk.rs,config.rs,context.rs,lib.rs}` + `locus_engine/src/{notify.rs,command.rs}`。
3. 加 Lua 状态: `locus_strategy/src/lua.rs`(同步 `net_pnl`/`equity` + 注册方法)+ `specs/lua-api.md` + 内置脚本示例。
4. 文档同步 + 004/005 修订指引。

> 删除型收敛不另立 `plan.md`(方案即上表)。
