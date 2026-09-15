# 004 实施计划: RiskEngine 风控补项(下单频率上限 + 两级亏损熔断)

**分支**: `004-risk-guards` | **日期**: 2026-09-12 | **规格**: [spec.md](spec.md)(2026-09-06 起草, 本次按代码复核后实施)

## 摘要

给引擎加两道"用户不会莫名亏损"的护栏: ①**下单频率上限**(滑动窗口, 参照 NautilusTrader 100/s, 防策略 bug 导致的下单风暴);
②**两级亏损熔断** —— 一级: 连续 N 个结算周期已实现净亏损 → 停新开仓(平仓/减仓放行, 转盈自动恢复);
二级: 已实现净盈亏相对运行期峰值回撤 ≥ X% → 停该策略全部新交易(不强制平仓, 手动重启恢复)。

**复核发现的关键缺口(本次一并补上)**: `RiskEngine` 目前**没有接入任何真实下单路径** —— 唯一调用点 `StrategyScheduler` 无消费者
(等价死路径); `BacktestContext` / `DryRunContext` / `LiveContext` 三条 `place_order` 全部**跳过风控**。
即: 现有 4 条规则(最大持仓/单日亏损/最小订单/最大滑点)在回测与 Dry Run 里实际上**从未生效**。
本变更把风控接进三条路径(同一 `RiskEngine` + 同一构造函数), 这是"三态一致"与 SC-003/SC-004 的前提。

## 技术上下文

**语言/版本**: Rust(workspace 声明 1.83), 本变更**零新增依赖**

**主要依赖**: `rust_decimal`(金额/比率) + `chrono`(窗口与结算周期时间) + `tracing`(拒单与告警日志) + `serde`(TOML 风控字段), 均已在 workspace

**时间语义**: 规则取时间一律走 `Context::now_utc()` —— 回测 = 本 tick 已开盘 bar 的 `open_time`(虚拟时间, 与策略同源, 可复现);
Dry Run / 实盘 = 真实 UTC(为此给 `DryRunContext` / `LiveContext` 补 `now_utc()` 实现, 现为 trait 默认 `None`)。同一代码路径、同一语义, 时间源按运行态取值。

**存储**: 无新增。熔断状态仅驻内存(重启 = 清零, 见 spec 假设 A3); 不落库避免"状态陈旧导致误熔断"。

**测试**: 规则级单元测试(已知向量/边界) + 既有回测冒烟(默认参数零误拒) + demo 测试网真实调用(`#[ignore]`, 极小频率上限复现拒单)

**规模**: `risk.rs` 约 +260 行(两规则 + 引擎统计 + 配置构造) + 三条 context 各 +3~8 行接线 + 配置校验 3 处

## 宪法检查

- [x] 原则一(完全本地化): 纯本地规则, 无网络/无遥测
- [x] 原则二(策略层 Lua): 不改策略层; 护栏在引擎侧(Rust 规则), Lua 只是被约束方
- [x] 原则三(测试纪律): 护栏不产生交易流程 → 规则用单元测试; 真实链路验证走 demo 真实 Dry Run(`#[ignore]`, 禁 mock)
- [x] 原则四(产物一律中文)
- [x] 原则五(少而精): 零新增依赖、无新表/新通道、无预计算; 默认值按实测节奏定(不留无依据大余量)

## 项目结构

```text
specs/changes/004-risk-guards/
├── spec.md      # 功能规格(2026-09-06 起草)
├── plan.md      # 本文件
└── tasks.md     # 任务分解
```

### 源码改动

```text
修改
  crates/locus_strategy/src/risk.rs        # 新规则 OrderRateLimit / LossCircuitBreaker; RiskRule::check 改 &mut self;
                                           # RiskEngine 增拒单统计 + from_config 构造; 新增 RiskError 变体
  crates/locus_strategy/src/config.rs      # RiskConfig 增 4 个可选字段; StrategyConfig::validate_risk() 参数校验
  crates/locus_strategy/src/context.rs     # DryRunContext / LiveContext: 加 risk 字段 + place_order 前置检查 + now_utc()
  crates/locus_strategy/src/backtest.rs    # BacktestContext: 加 risk 字段 + place_order 前置检查
  crates/locus_strategy/src/lib.rs         # 导出新类型
  crates/locus_strategy/src/scheduler.rs   # 适配 check(&mut self) 签名(仅签名, 行为不变)
  crates/locus_cli/src/commands/backtest.rs# resolve_config 后调用 validate_risk()
  crates/locus_cli/src/commands/run.rs     # Dry Run 配置装配处调用 validate_risk()
  crates/locus_engine/src/command.rs       # (若需) 风控拒单计入运行结果 —— 见 P4
  specs/backtest.md / architecture.md      # 风控章节同步(默认值/拒单语义/三态一致)
```

## 决策

| # | 决策 | 结论 |
|:--|:--|:--|
| D1 | 护栏归属 | 引擎侧 `RiskRule`(Rust), 不进策略 Lua —— 护栏的意义就是"不依赖策略自律"(spec US1 优先级理由) |
| D2 | 接入点 | 三条 `place_order` 顶部统一 `risk.check()`; 拒单**返回 `OrderAck{status: Rejected}`** 而非 `Err` —— 与既有"资金不足/无仓可平拒单"同形, 策略循环不中断, Lua 侧看得见"未成交"; 同时 `tracing::warn!(target: "risk", ...)` 输出规则名与关键数值 |
| D3 | 熔断粒度 | 按策略实例(每个 context 一个 `RiskEngine` 实例), 不跨策略联动; 状态不持久化, 重启清零(spec 假设 A2/A3) |
| D4 | 平仓豁免 | `req.reduce_only == true` **或** 该 pair 现有持仓方向与请求方向相反(即该单减少净持仓) → 视为退出路径, 不受两级熔断拦截(spec FR-006); 频率上限对所有请求一视同仁(防风暴不区分方向) |
| D5 | 已实现盈亏口径 | `PnlTracker::net_pnl()`(已实现盈亏 − 累计手续费), 与回测报告口径一致; 不含浮盈(spec FR-008)。资金费: 引擎合约路径已计入 pnl, 无需额外处理 |

### 实现决策

| # | 议题 | 结论与依据 |
|:--|:--|:--|
| P1 | `RiskRule::check` 可变性 | 两规则都带状态(滑动窗口时间戳 / 周期与峰值基线) → trait 签名改 `fn check(&mut self, ...)`; `RiskEngine::check` 同步 `&mut self`。context 在 `place_order(&mut self)` 内调用, 无需内部锁 |
| P2 | 参数来源 | 单一优先级: `--param` 覆盖(`risk_max_orders_per_sec` 等 4 个键) **>** `[strategy.risk]` TOML 新字段 **>** 内置默认。用 `--param` 是为了不改 TOML 就能在回测里扫参数/复现拒单; 读取优先级只有一处实现(避免两套机制漂移) |
| P3 | 默认值(按实测节奏定) | 频率上限 **100/s**(参照 NautilusTrader 默认, 且远高于内置策略单 tick 最大下单数 —— 实测值见 tasks T009); 一级 **连续 3 个自然日**; 二级 **峰值回撤 30%**; 结算周期默认 `day`(与既有"单日最大亏损"粒度为日一致), 可配 `hour` |
| P4 | 拒单可见性 | 回测: 拒单计入既有 `rejected_count`(报告"拒单"列), 日志含规则名与数值 → SC-003"零误拒"可由"报告拒单数与基线一致 + 无 risk warn"判定; Dry Run: 同形 Rejected ack + warn; 不新增报告字段(少而精, 无需新数据通道) |
| P5 | 周期结算时机 | **惰性结算**: 周期键变化在**下一次检查订单时**被发现并结算上一周期 —— 熔断只影响"是否放行订单", 而无人下单时也无需判定; 规则内保证"周期内无平仓 → 计数不变"(spec 边界情况第 1 条) |
| P6 | 二级基准 | 峰值 = 运行期 `net_pnl` 最高点; **仅当峰值 > 0 时**计算回撤(分母为峰值), 峰值 ≤ 0 视为未触发(避免除零与"未盈利就熔断") |
| P7 | 参数校验位置 | 配置装载边界(`resolve_config` / Dry Run 装配)统一 `validate_risk()`: 频率 ≤0 / N ≤0 / `loss_period` 非 `day\|hour` / 回撤 ∉ (0,100] → 明确报错拒绝(spec FR-009)。构造期不再重复校验, 仅对绕过校验的构造做防御性收敛(上限取 max(1)、回撤收敛到 (0,100]) |
| P8 | `StrategyScheduler` | 唯一旧调用点, 只改签名适配; 它当前无消费者, **是否删除随 011 实盘运行器一并决定**(不在本变更扩大范围) |

## 改动清单(按依赖顺序)

1. `RiskError` 新变体 + `RiskRule::check(&mut self)` + `RiskEngine` 统计与 `from_config`
2. 规则实现: `OrderRateLimit`(滑动窗口) → `LossCircuitBreaker`(两级 + 惰性周期结算 + 平仓豁免)
3. 规则级单元测试(边界/恢复/不误伤/重启清零/参数非法)
4. 接线: `BacktestContext` / `DryRunContext` / `LiveContext` 的 `place_order` + 后两者补 `now_utc()`
5. 配置: `RiskConfig` 4 新字段 + `StrategyConfig::validate_risk()` + 两处装载点调用
6. 文档同步: `specs/backtest.md`(风控语义/默认值/拒单口径)、`specs/architecture.md`(风控接入三条路径)、`specs/roadmap.md`(004 状态)

## 验证

- `cargo test --workspace` 全绿, 新增用例覆盖: 频率边界(第 N+1 次拒)/窗口过期恢复/一级计数与转盈清零/无平仓周期不变/二级触发与平仓放行/峰值 ≤0 不触发/重启清零/默认参数零误拒/参数校验拒绝非法值
- 真实回测冒烟(免 key 公共数据): `locus backtest --strategy shannon_grid --pair ETHUSDT --days 20` 与基线同一报告(成交笔数/拒单数一致) → 默认参数零误拒
- 复现拒单真实冒烟: 临时 TOML(或 `--param risk_max_orders_per_sec=1`) 跑 `1m` 回测 → 报告拒单数 > 0 且日志含 `risk` warn
- demo 测试网真实调用(`#[ignore]`, 需 env key, 见 `specs/testnet.md`): Dry Run 起策略 + 极小频率上限 → 真实行情下观察到超限拒绝, 策略不中断
  (注: Dry Run 只用公共行情 + 虚拟撮合, 不需签名 → 不受 WSL 时钟问题影响; 现货/合约分开跑仍遵守)
