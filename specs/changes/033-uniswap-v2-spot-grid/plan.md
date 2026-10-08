# 033 技术方案: 现货 Uniswap V2 网格策略 (uniswap_v2_grid)

**需求(用户 2026-09-26 口径, 逐条)**:
1. 名称: 现货 Uniswap V2 网格; 模拟 uniswap v2 池(资金与仓位价值恒 1:1), 动态 ATR 间距。
2. 指定投入资金 + 开始价格; 价格低于开始价格时, 用**投入资金的一半**市价建仓; 记录资金和仓位。
3. 指定用哪根 K 线算 ATR, 默认 1h; ATR 周期默认 14; ATR 乘数可配, 默认 1。
4. 建仓价 = 平衡价格; 平衡价 + ATR 挂卖单, 平衡价 − ATR 挂买单; **成交后资金与仓位价值恢复 1:1**; 成交价 = 新平衡价格, 全撤重挂两侧。
5. 成交实时(引擎已支持: 限价单逐 bar 盘中按 OHLC 触发, 成交先于决策入账)。
6. 回测按 specs/backtest.md §〇 规范 v1; SOLUSDT, 投入 10000 USDT, 6 个月, 其余参数架构师定。

## 核心机制(与 shannon_spot_grid 的本质差异)

本策略是 shannon(030)的**真实账本简化版**: 砍掉虚拟账本/杠杆/趋势门控, 账本 = 真实现金 C + 真实持仓 Q。

- **激活建仓**: 价格 < start_price → 市价买入 invest_cash/2 名义; 成交价 p0 = 第一平衡价; 建仓后天然 ≈ 1:1(C ≈ Q·p0)。
- **挂单**: buy_px = balance − atr_mult×ATR, sell_px = balance + atr_mult×ATR(ATR 逐次重挂时取当前值 → 动态间距)。无追踪上移(不像 paired_grid), 单子挂着等成交。
- **回平衡量(1:1 恢复, 含手续费修正)**:
  - 买 q = (C − Q·p_b) / (p_b·(2+f))   [花 q·p_b·(1+f) 后 C′ = (Q+q)·p_b]
  - 卖 q = (Q·p_s − C) / (p_s·(2−f))   [收 q·p_s·(1−f) 后 C′ = (Q−q)·p_s]
  - f = fee_side; 若不做手续费修正(理想公式 q=(C−Q·p)/(2p)), 成交后因费差残留 <0.1% 不平衡 → **推荐含费修正**(忠实"恢复 1:1")。
- **成交**: balance := 成交价; 全撤重挂两侧(同 bar 内 on_tick 生效, 新单自下根 bar 起撮合)。
- **无结束语义**: 回平衡卖量 ≤ 持仓一半, 仓位永不清零、资金永不耗尽(公式自平衡), 全程运行 — 与 uniswap 池语义一致。

## 决策点(推荐已给出, 审核时可改)

| # | 决策 | 推荐 | 备选 |
|---|---|---|---|
| D1 | 挂单量口径 | 含手续费修正(精确恢复 1:1) | 理想公式(留微小费差) |
| D2 | 回测数据周期 | **1m** + atr_interval=1h(实时成交, 1h ATR 重采样) | 1h(重挂延迟至多 1h) |
| D3 | 成本门槛 | 同 shannon R7: atr_mult×ATR/价格 ≤ 4×fee_side → [FATAL] 停机 | 仅 WARN |
| D4 | 策略 id | `uniswap_v2_grid`, 显示名「现货 Uniswap V2 网格」 | `univ2_grid` |

## 参数表(清单 [[params]] 全量声明 default → 注入生效配置, 报告可见)

| key | 默认 | 说明 |
|---|---|---|
| pair | 必填 | 现货交易对 |
| start_price | 必填 | 价格低于它才激活(回测设窗口起点价, 勿前视) |
| invest_cash | 0 | 投入资金(0 = 激活时 quote 余额全额, 一半建仓) |
| interval | 1h | 主时钟(回测建议 1m 数据 + 本参数随 CLI 覆盖) |
| atr_interval | 1h | ATR 的 K 线周期(need_klines aux) |
| atr_period | 14 | ATR 周期 |
| atr_mult | 1 | 间距 = atr_mult × ATR |
| min_notional | 5 | 单笔最小名义 |
| fee_side | 0.001 | 单边费率 |

## 守卫与可观测性

- ATR 未就绪(可见桶 < period+1)→ 不挂单, 计数 skip_no_atr。
- 买量受真实现金兜底(预扣费+滑点余量, cap_buy 同款); 卖量受真实持仓兜底(1e-9 折扣, cap_sell 同款)。
- buy_px ≤ 0 → 跳过买单(计数)。
- 停摆可观测性: 连续 1 天无任何挂单 → WARN; 导出 `stat_stall_bars` / `stat_cash_final`。
- 断点续接: state 持久化 balance_price / built / pending_entry / halted / invested0(现金与持仓从引擎读, 不冗余存)。
- on_stop 收益分解: 期末权益 − 投入 = 持仓损益 + 再平衡损益; **账本交叉核对**: 策略自记 C/Q 模型 vs ctx:balance/position_size, 误差 < 0.01(规范 §C.4)。
- stat_* 导出(规范 §C.1): 生效参数(atr_interval/period/mult、末次 ATR、末次间距%)、成交计数(总/买/卖)、期末现金/持仓/平衡价、stall 计数、投入与建仓名义。

## 涉及文件(不改引擎)

- 新增 `strategies/spot/uniswap_v2_grid.lua` + `.toml`(清单: 中文说明 + 参数 schema + suitable/unsuitable)。
- `crates/ricow/src/strategies/catalog.rs`: BUILTIN 数组注册(include_str 两件套)。
- `.gitignore`: 两行例外(`!strategies/spot/uniswap_v2_grid.*`)。
- `crates/ricow_strategy/src/lib.rs`: 头注释策略列表补一句。
- `crates/ricow_strategy/src/builtin_tests.rs`: 集成测试(下节)。
- 文档: `specs/lua-api.md` 策略清单小节、`specs/roadmap.md` 测试基线数字; 变更档案落本目录。

## 测试计划(builtin_tests.rs, 合成 K 线驱动 LuaStrategy)

1. 激活建仓: price < start_price → 市价买单名义 = invest_cash/2; fill 后 balance_price = 成交价。
2. 挂单价: balance ± atr_mult×ATR(用 TR 恒定的合成序列, ATR 精确已知)。
3. 挂单量: 手算期望 q(含费修正公式), 断言恢复 1:1(C′ == Q′×挂单价)。
4. 成交重挂: fill 后新 balance = 成交价, 两侧重挂; 同 bar 内 need_rehang 生效。
5. ATR 未就绪 → 零下单(不猜值)。
6. 成本门槛: atr_mult 极小 → FATAL 停机、零交易。
7. 兜底: 买超现金截断 / 卖超持仓截断。
8. 状态快照含 balance_price(续接最小必需项); 断点续接后重挂。
9. 账本对账: run_backtest 跑合成序列, 策略 C/Q 模型 vs 引擎余额/持仓误差 < 0.01。

门禁: cargo fmt / clippy / test(566 passed 不倒退)。

## 回测验收(specs/backtest.md §〇 v1 全项)

- `SOLUSDT --interval 1m --days 180 --cash 10000`, 参数: invest_cash=10000, start_price=窗口起点价(首根 open 上浮 0.1% 保证当日激活), atr_interval=1h, atr_period=14, atr_mult=1, min_notional=5, fee_side=0.001。
- `--export-dir /tmp/ricow-bt-univ2/` 四件套(report.txt / fills.csv / equity.csv / params.json)。
- 报告自查 A1-A7(概览区块) + B(成交统计)逐项存在、净盈亏分解恒等式成立、策略账本 vs 引擎误差 < 0.01。
- 交付: 回测概览 + 成交统计 + 参数全量 + 文件路径, 按用户偏好写成交付说明待审。

## 风险

- 单边行情: 上涨中池不断卖出跑输持有(uniswap LP 固有特性, 报告基准对照 A7 会如实呈现); 下跌中持续买入摊底, 无套牢爆仓风险(纯现货)。
- 1m×180d 回测耗时与 032 相当(可接受)。
- ATR 动态变化只在重挂时生效(挂着的单不追改) — 与"成交后重新挂单"口径一致, 如实声明。
