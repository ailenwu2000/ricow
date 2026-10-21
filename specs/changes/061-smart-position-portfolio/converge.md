# 061 收敛记录 — smart_position 多标的组合化

> 日期: 2026-10-21。前序: 060(单币版 + Simple Earn)。本档案为唯一收敛出口, 计划蓝本见 `.trae/documents/smart_position_portfolio_plan.md`。

## 一、交付内容

1. **策略组合化**(`strategies/spot/smart_position.lua` + `.toml`, 唯一模式):
   - `universe` 逗号串多 pair(缺省回落单 `pair`, 与 060 等价); `weights` 一一对应, 缺省等权; 校验(个数/范围/合计≈1)违反即 fatal, 不自动归一。
   - `target_pct` = 币腿合计占比; 币 i 目标 = target_pct×w_i; 阈值闸逐币独立(只调越界的币); 时间闸/min_notional 不变。
   - 资金池共享: 本周期先卖后买、卖出所得滚动进买腿预算; 某币现金不足 → 赎回申请 + 仅跳过该币, 下周期再试。
   - 理财腿只存 USDT; `ctx:earn_balance` 返回 nil → 静默关闭理财(不报错、不刷日志)。
2. **引擎回测理财支持面**(`ricow_strategy/backtest.rs`): 配置键 `earn_supported_assets`(默认 `USDT`), 不在列表 → earn_balance=None、申赎=false(bStock 无赚息可真实演练)。
3. **组合内核补齐**(`ricow_engine/backtest_runner.rs`): `run_portfolio_backtest` 收尾补 `on_stop` + `strategy_stats` 透传(对齐 `run_backtest`)。
4. **CLI 接线**(`ricow/src/commands/backtest.rs`): universe≥2 → 逐 pair 取数(缓存/并发分页/缺口/预热硬错同单标口径) → `build_interval_ticks` 统一时间轴 → 组合报告(概览+分标的明细+策略统计) + `--export-dir` 四件套(fills 带 pair 列); 单标的路径与 Web 路径零改动; 组合敏感性扫描硬拒。
5. **架构守卫**: `universe`/`earn_supported_assets` 进引擎通道键白名单; `weights` 进 TOML `[[params]]`。

## 二、实施中发现并修复的缺陷

- **卖单同周期申购理财被拒 → 账本漂移**(本次 Lua 重写引入): 理财划转 drain 先于卖单成交执行, 按"卖后滚动现金"申购会被引擎拒绝, 而策略入队即乐观记账、不回滚 → cash/earn 各漂移一笔申购额(实测 244.76)。修复 = 卖出腿发出后本周期不申购(与 buy_emitted 同"下周期自愈"语义); 回归断言收紧回三项 <0.01(`test_smart_position_over_threshold_sell`)。

## 三、测试

- 组合六件套(`builtin_tests.rs`): 等权建仓 / weights 落位 / weights 校验 fatal / 逐币阈值独立触发 / earn 不支持静默跳过 / 赎回延后补买。
- `test_backtest_earn_supported_assets_whitelist`(缺省/大小写空白/空串)。
- e2e(`backtest_runner.rs`): `test_run_portfolio_smart_position_e2e` — 真实 Lua `include_str!` 走 `run_portfolio_backtest` 全链路, 断言双 pair 成交、strategy_stats 含 stat_universe、final_equity 含理财腿。
- 060 报告回归锚点 `test_smart_position_report_final_equity_includes_earn_leg` 保持通过。
- 全量: `cargo test --workspace` 0 失败(跳过 1 个存量网络 flaky) + clippy `-D warnings` 零告警 + fmt。

## 四、132 天 4 资产真实回测(§〇 v1 全量)

- 命令: `ricow backtest --strategy smart_position --param universe=BTCUSDT,ETHUSDT,TSLABUSDT,NVDABUSDT --interval 1h --start 2026-06-11 --end 2026-10-21 --cash 10000 --export-dir backtests/061-smart-position-portfolio-132d`
- 窗口硬约束: bStock 现货 1h 历史自 2026-06-11 起(用户指定只回测 132 天)。
- 结果: 期末权益 **11083.99(+10.84%)**; 已实现净盈亏 181.86(已实现 187.84 − 手续费 5.98) + 未实现 902.13 = 1083.99 ✓; 成交 8 笔(买 5/卖 3, 拒单 0), 胜率 3/3=100%; 总成交额 5979.51, 费率 0.1000%; 最大回撤 6.55%, 夏普 1.68; 期末现金 54.33 + 理财 5421.98 + 持仓市值 5607.68; 利息估算 36.69(不入账); **账本核对三项误差 0**。
- 分标的: BTC 买1卖1(卖 @78338 vs 买 @61511, 盈 47.7)/ ETH 买1卖2(@1934/@2516 vs @1622, 盈 33.86+106.03)/ TSLAB 买2卖0(@399/@324)/ NVDAB 买1卖0(@206)。
- 基准: 等权满仓持有(各币首成交价→期末收盘) +22.23%; 半仓口径 ≈ +11.12% vs 策略 +10.84%(差 = 手续费 + 调仓时点 + 利息未入账) ✓。
- 异常核验: 100% 胜率属机制属性(卖腿仅在币价大涨越上带时触发); 成交仅 8 笔属 2pp 带宽对 12.5% 目标需 ±16% 相对波动的必然结果; 期末价两口径差 0.09% = 策略见 bar open(防前视) vs 引擎按 bar close 估值, 属既定语义。
- 产物: `backtests/061-smart-position-portfolio-132d/` 四件套。

## 五、文档同步

- `specs/backtest.md` §三 4c(earn_supported_assets); `specs/roadmap.md` 061 行; 060 档案补指向本变更。

## 五a、代码审核修复 (2026-10-21, TRAE-code-review 双校验)

审核确认 4 项 minor 并全部修复:
1. **死兜底分支清理**: `target_pct/earn_enabled/earn_apr` 的 `== nil` 判断在 `config_f64` 缺键返回 0(非 nil)的绑定语义下永不生效 → 删除死分支, 注释明示缺省值由清单 `[[params]]` default 注入保证(三参数可合法取 0, 故不走 `num()`)。
2. **universe 报价一致性校验**: `on_init` 逐项 `detect_quote` 校验全 universe 同报价币, 混报价(如 BTCUSDT,ETHUSDC)即 FATAL 停机(此前混报价会按首币报价静默撮合)。回归测试 `test_smart_position_portfolio_mixed_quote_halts`。
3. **组合报告 market 文案**: `format_portfolio_report` 不再硬编码"现货组合", 按生效 `config.market` 输出"现货组合/合约组合"。
4. **状态持久化扩展**: `save_state`/`on_init` 增补 buy_count/sell_count/dev_max_pp/earn_time_sum/equity0/m_fee 六项, 实盘重启后观测统计不再跨会话归零(账本 m_cash/m_earn/m_pos 仍按引擎重基线, 维持原设计)。

剔除误报 3 项(校验共识): ① `ctx:earn_redeem` 返回值=入队受理而非执行结果, "已提交赎回申请"日志语义准确; ② `num()` 视 0 为未配置 = 全仓内置策略统一惯例; ③ 卖量 1e-9 安全折扣 = 现货防 f64 往返超卖拒单的文档化既有修法(与合约 cap_close 禁项语境不同)。

## 六、遗留

- 实盘 runner 多 pair 装配(组合路径目前仅回测; LiveContext 单 pair)另立变更。
- demo 测试网验证仍受阻于本机 DNS 污染(060 遗留), 待网络恢复复跑。
