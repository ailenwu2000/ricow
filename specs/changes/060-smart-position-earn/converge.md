# 060 收敛记录 (2026-10-20)

## 交付物

1. **新内置策略 `smart_position`** (现货智能持仓, 参考币安智能持仓 + Simple Earn):
   - `strategies/spot/smart_position.lua` / `.toml` (10 参数, 目录注册: catalog.rs / builtin_tests.rs / .gitignore / params.rs 架构守卫)
   - 目标仓位 `target_pct` (默认 0.5); 周期闸 `cycle_days` (默认 1 天, 首 tick 立即执行);
     偏离阈值闸 `rebalance_threshold_pct` (默认 2pp); `min_notional` (默认 5) —— 三重闸防循环调仓
   - 闲置现金腿 (留 `earn_buffer_pct` 权益缓冲) 自动申购活期理财; 买入资金不足 →
     只提交赎回申请 `min(earn, shortfall)`, **本周期不调仓**, 下一周期再试 (异步到账语义)
   - 当周期发生赎回/买单 → 不再申购 (防同周期来回划转磨损)
   - 账本 m_cash/m_pos/m_earn/m_fee 与引擎逐分核对; LIQ-/CLOSE- fill 跳过; halted 即停产单

2. **引擎 earn 集成 (Simple Earn 活期理财)**:
   - `ricow_core::Exchange` trait: `earn_balance/earn_subscribe/earn_redeem` 默认方法 (None/false)
   - `ricow_binance`: `earn.rs` SAPI (`/sapi/v1/simple-earn/flexible/position|subscribe|redeem`)
     + spot.rs 覆写 + client.rs signed_* 开放; 4 单测
   - `ricow_strategy::context`: LiveContext (60s TTL 缓存 + 查询失败吞错收敛 None + WARN 降级,
     负结果也缓存防 429) / DryRunContext (即时虚拟划转) / BacktestContext (virtual_earn 面值划转)
   - `ricow_strategy::lua`: `ctx:earn_*` 绑定; 申赎走 `_RICOW_EARN_OPS` 全局表队列,
     回调返回后 `drain_earn_ops` 执行并清表 (单次执行, 不跨周期重复)
   - 权益口径 D4: 理财面值计入 `mark_to_market` / `mark_to_market_portfolio` /
     `ctx:equity()` / 报告总价值; `balance()` 不混入 (现金腿独立)

3. **Lua API 文档**: `specs/lua-api.md` 新增「Simple Earn 活期理财 (060)」小节

## 回测暴露并修复的报告 bug (回归锚点)

`BacktestContext::report()` 现货分支 `final_equity = cash + pos×price` **绕过 mark_to_market,
静默丢理财腿** → SOL 一年真实结果 -20.65% 被误报 -60.42% (理财面值 3976.79 蒸发)。
修复 = 统一 `final_equity = mark_to_market(&final_price)`; `entry_equity` 同步补理财腿。
锚点测试: `test_smart_position_report_final_equity_includes_earn_leg`
(断言 final_equity > 裸权益、平 bar 权益守恒、equity_change_pct 同源)。

## SOL 一年回测结论 (2025-10-01 ~ 2026-09-30, 1h, 10000 USDT)

- 期末权益 7934.86 (-20.65%) vs 满仓持有 -42.90% (标的同窗口 -42.90%)
- 25 笔调仓 (买 13 / 卖 12), 手续费 10.45 (0.10% 成交额), 利息估算 72.03
- 账本核对误差 0; 理财腿期末 3976.79; 导出四件套 `backtests/20261020-smart-position-1y/`

## 多币验证 (用户要求 ≥3 币, 同参数各 10000 USDT, 同窗口)

| 币 | 标的涨跌 | 策略权益 | 策略盈亏 | 满仓对照 | 超额 | 成交(买/卖) | 最大回撤 | 利息估算 |
|:--|:--|:--|:--|:--|:--|:--|:--|:--|
| BTCUSDT | -26.64% | 8751.24 | -12.49% | -26.64% | +14.15pp | 14 (8/6) | 30.78% | 82.96 |
| ETHUSDT | -35.39% | 8400.50 | -15.99% | -35.39% | +19.40pp | 22 (12/10) | 41.21% | 78.25 |
| SOLUSDT | -42.90% | 7934.86 | -20.65% | -42.90% | +22.25pp | 25 (13/12) | 47.04% | 72.03 |

三币独立实例合计 30000 → 25086.60 (-16.38%), vs 等权满仓 -34.98%。
账本核对误差三币均为 0; 导出 `backtests/20261020-smart-position-1y-{btc,eth}/`。

## demo 测试网验证: 受阻 (环境, 非代码)

本机 `*.binance.com` DNS 污染 (demo-api.binance.com 解析到 108.160.162.98 非币安 IP, 连接失败;
`specs/testnet.md` §注意事项早有同类记录)。可达域名仅 `*.binance.vision`。
- `ricow run smart_position --pair SOLUSDT --demo` 走到 demo 端点即网络失败, 无法完成真实下单取证
- 且即便 demo-api 可达, 其也不提供 `/sapi` 理财端点 (D7 已按"吞错 + WARN 降级"设计)
- **待网络恢复后复跑**: 按 `specs/testnet.md` 时钟对齐流程跑 demo, 验证 ① 测试网真实下单 ② 理财 WARN 降级

## 测试基线

`cargo test --workspace` 全绿 (本机跳过 1 个存量环境 flaky:
`shared_http_reuses_tcp_connection_across_clients`, 干净 HEAD 同样失败, 与本变更无关)。
新增: smart_position 策略测试 6 + earn 引擎测试 3 + 报告权益回归 1 + params 架构守卫 1 + SAPI 单测 4。
clippy 零告警, fmt 通过。

## 后续演进

061-smart-position-portfolio (2026-10-21): smart_position 组合化 (universe/weights 多标的) +
引擎 `earn_supported_assets` 理财白名单 (无赚息资产静默降级), 见 `specs/changes/061-smart-position-portfolio/`。
