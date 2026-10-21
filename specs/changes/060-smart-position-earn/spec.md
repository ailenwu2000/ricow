# 060 现货智能持仓策略 + Simple Earn 理财集成 — 规格

> 依据: 用户 2026-10-20 需求(`/plan` 计划已批准, 蓝本 `.trae/documents/spot_smart_position_earn_plan.md`)。
> 参考: 币安现货智能持仓机器人(目标占比 + 时间/阈值双触发, 全市价单)、Binance Simple Earn Flexible API。

## 一、目标

1. 新增内置现货策略 `smart_position`(现货智能持仓): 维持 pair 内 base 币市值 ÷ 总权益 = 目标占比; 每 `cycle_days` 天检查一次, 偏离 ≥ `rebalance_threshold_pct` 个百分点才市价调仓。
2. 平台新增 Simple Earn 活期理财能力(引擎级通用 API, 非策略参数): 查询在理财资产 / 申购 / 赎回。
3. 策略把闲置现金腿自动存入理财吃利息; 调仓时现货不够先提赎回申请, 未到账等下一周期重试。
4. 防循环调仓: 时间闸 + 阈值带宽 + 最小名义下限。

## 二、功能需求

- FR-01 `Exchange` trait 新增 `earn_balance/earn_subscribe/earn_redeem` 默认方法(默认不支持), `BnSpotExchange` 经 `/sapi/v1/simple-earn/flexible/*` 实现。
- FR-02 `Context` trait 同名三方法(默认 None/false), LiveContext 桥接交易所(理财余额 60s TTL 缓存, 申赎后刷新现货缓存), BacktestContext/DryRunContext 虚拟理财钱包(即时划转, 划转非成交不计盈亏)。
- FR-03 Lua 绑定: `ctx:earn_balance(asset)`(快照只读)、`ctx:earn_subscribe/earn_redeem(asset, amount)`(写 `_RICOW_EARN_OPS` 全局表, 回调返回后由 LuaStrategy 经 `&mut dyn Context` 执行 —— 与 `need_klines` 的 `_RICOW_DECLARATIONS` 同机制)。
- FR-04 现货权益口径: `ctx:equity()` = 现货现金 + 现货持仓市值 + 理财市值(两腿); 不调用理财的策略恒空 → 零影响。
- FR-05 策略 `smart_position`: 参数 target_pct/cycle_days/rebalance_threshold_pct/earn_enabled/earn_buffer_pct/earn_apr/min_notional/fee_side/interval; on_tick 时间闸 → 估值(现货+理财) → 阈值闸 → 资金就绪检查(不足则提赎回, 下周期重试) → 市价调仓 → 理财管理(现金腿超缓冲部分申购, base 币留现货)。
- FR-06 统计导出: `stat_rebalance_count`/`stat_earn_days`/`stat_earn_interest_est`(估算)/`stat_pending_redeem_count`/`stat_dev_max`。
- FR-07 demo 测试网理财端点不可用 → 三方法返回不支持 → 策略 WARN 一次并降级纯调仓; 交易流程(下单/成交)仍 testnet 真实验证(宪法原则三)。

## 三、边界与非目标

- 单交易对(base+quote 两资产篮子), 不做多币篮子(实盘 runner 单 pair, 另立变更)。
- 只接活期(flexible), 不接锁定; 不做 auto-subscribe 开关。
- 回测/DryRun 理财划转即时, 不建模赎回延迟与利息(利息为策略侧估算)。
- 不改引擎撮合/风控/通知路径; 引擎零策略参数名(架构守卫必须通过)。

## 四、验收

- 单元测试: sapi 响应解析、Backtest/DryRun 划转与权益、Lua ops 队列执行、策略行为(阈值内不动/超阈买卖/理财缓冲/赎回重试)。
- `cargo test` 全绿 + 架构守卫 + clippy + fmt。
- SOLUSDT 1 年真实 K 线回测, 报告按 specs/backtest.md §〇 v1 全量交付。
- demo 测试网 Dry Run + 实盘下单路径验证(理财端点降级路径)。
