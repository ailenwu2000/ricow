# 038 技术方案: 引擎 bar 内撮合保真 — on_fill 链上限价单次 bar 生效

## 一、问题（037 实施发现, 用户裁决要修）

回测引擎 `BacktestContext::place_order` 对**同 bar 新挂的限价单按整根 bar 的 [low, high] 重匹配**。
事件模型（034: on_fill 可返回订单）下，网格策略"买 98 成交 → 立即重挂 卖 100 / 买 96"，若本 bar
范围覆盖 100，卖 100 立即成交 → 再重挂 买 98 → 又成交……同一根 bar 内乒乓链，直到
`MAX_FILL_DECISION_DEPTH=8` 封顶**整批静默丢弃**（含 cancel_pending）→ 策略挂单簿与 need_rehang
失同步 → 数周静默停摆（037 两次 180d A/B 复现）。且链内多次成交把一条价格路径假装成多次往返
穿越，属乐观近似。

## 二、竞品调研结论（2026-09-28, 附来源）

| 框架 | 本 bar 新单能否本 bar 成交 | bar 内成交假设 |
|---|---|---|
| backtrader | **否**（明文"当前数据已发生不可撮合", 当 bar 生效叫 cheat-on-open 需显式开） | open 穿越按 open, 否则限价 ∈ [low,high] 按限价 |
| backtesting.py | **否**（先撮合存量单再收新单, 机制性杜绝） | low≤limit≤high 触及成交; 歧义时**悲观序 + 推迟下一 bar 并告警** |
| Freqtrade | **否**（信号收盘评估, 下根 open 成交） | 触及即成交; **low 先于 high（先保本金）** |
| NautilusTrader | **否**（bar 拆 4 个 OHLC 合成点逐点撮合, on_bar 新单在 4 点处理完后才进撮合） | 限价按限价; 默认路径 O→H→L→C（可自适应先走离 open 近的极值） |
| vectorbt | 开源版无限价单（PRO 付费） | stop 用 high/low 触及判定 |
| Hummingbot | 网格用专用 Grid Executor Simulator（high/low 推断 fill） | 未做路径级防乒乓 |

**归纳**:
1. "本 bar 新挂限价单次 bar 生效"是**行业默认**; ricow 的"同 bar 新单整 bar 重匹配"在主流框架中无同类, 属越界乐观。
2. 成交假设普遍"触及即成交", 但用**保守路径序**兜底（low 先于 high / 悲观序 / O→H→L→C）; 没有任何框架把一条路径当多条路径往返穿越。
3. 网格类需专用处理（hummingbot 模拟器 / freqtrade `--timeframe-detail` 细粒度重放）。

来源: backtrader [Orders](https://backtrader.com/docu/order-creation-execution/order-creation-execution/)、backtesting.py [源码 _Broker](https://github.com/kernc/backtesting.py/blob/master/backtesting/backtesting.py)、Freqtrade [Assumptions](https://www.freqtrade.io/en/stable/backtesting/#assumptions-made-by-backtesting)、NautilusTrader [Bar-Based Execution](https://nautilustrader.io/docs/latest/concepts/backtesting/bar-execution/)、Hummingbot [v2.14.0](https://hummingbot.org/release-notes/2.14.0/)

## 三、方案

### R1 核心: on_fill 链内返回的**限价单**改为次 bar 开盘前入簿（deferred fill）

- **规则**: `settle_orders` 中 depth ≥ 1（即 on_fill 回调返回）的订单分两种:
  - **市价单 → 立即撮合**（现状不变; 建仓/强平等依赖此语义）;
  - **限价单 → 不进本 bar 撮合**, 暂存引擎的 `pending_limit_queue`, 在**下一根 bar step_bar 之前**
    逐笔 place_order 入簿（成为"已在簿挂单", 按 bar 正常撮合——含开盘跳空按 open 成交的标准语义）。
- **理由**: ① 对齐行业默认; ② 从机制上消灭乒乓链（同 bar 内 on_fill 不再触发新一轮匹配）;
  ③ 深度上限 8 实际不再可触达（每 bar 至多一层链）, 静默丢批病理随之消失。
- **depth 0（on_tick 返回）的订单语义零改动**: 限价单仍同 bar 即时撮合（现状 = 033/036 基线行为,
  市价建仓也依赖）。即**只有"成交回调里派生的限价单"保守化**, 影响面最小。
- 撤单指令（cancel_pending）不受影响, 立即执行。
- **文档声明**: 该保守化与实盘的差异是"实盘重挂可能立即成交, 回测从下根 bar 起可成交"——
  属保守近似（宁可少算成交, 不假装穿越）, 与 freqtrade/backtrader 同哲学。

### R2 防御: 深度封顶不再静默丢批

- 即使未来出现深度超限, 被丢弃的每笔订单通过 `on_order_update` 回传 `Cancelled`（审计 #3 同款）,
  策略可感知并重挂; WARN 保留。消除"静默失同步"这一类病理。

### R3 uniswap_v2_grid 重新启用事件模型（037 豁免的回补）

- R1 落地后乒乓链不存在, 恢复 037 原 R2 设计: on_fill（建仓/网格分支）记账后 `do_rehang(ctx)`
  并返回订单（限价单将由 R1 次 bar 入簿; cancel_pending 立即生效）;
- 头注释改回事件模型口径, 记录"依赖 038 R1"前提;
- `chain_capped` 测试改回锁 R1 语义: 宽 bar 双侧场景下 fill_count 有界（无乒乓链）、重挂单在
  次 bar 入簿可被断言。

### R4 036 (paired_grid_futures_long) 影响评估与复跑

- 036 的 on_fill 重挂限价单同样会从"本 bar 可成交"变为"次 bar 入簿" → **行为漂移**;
- 复跑其 180d 回测（SOLUSDT 1m）与既有基线对比, 差异作为 R1 的预期效果记录（应表现为同 bar
  链式成交减少、无停摆; 036 间距 ≥4% 链本就罕见, 预期漂移很小）;
- 漂移超出预期 → 按成交明细归因后再定。

## 四、决策点

| # | 决策 | 推荐 | 备选 |
|---|---|---|---|
| D1 | 保守化范围 | 仅 on_fill 链内限价单（R1, 影响面最小） | 全部新限价单次 bar 生效（backtrader 严格式, 所有策略行为大变, 否） |
| D2 | bar 内路径序（存量单同 bar 双触） | 不改——现有 try_match_ohlc 维持, 仅消除"新单回溯重配"; 路径序留 039（freqtrade 式 low-first 可配置） | 本期一起改（范围失控, 否） |
| D3 | 细粒度重放（freqtrade --timeframe-detail 式） | 不做, 记为 roadmap 远期 | — |

## 五、涉及文件与测试

- `crates/ricow_engine/src/backtest_runner.rs`: settle_orders 分流（depth≥1 限价单 → 队列）+ 下一 bar 头部注入; 封顶丢批改回传 Cancelled。
- `crates/ricow_strategy/src/backtest.rs`: `pending_limit_queue`（BacktestContext 新增, 含 step_bar 注入点）。
- `strategies/spot/uniswap_v2_grid.lua`: 恢复 on_fill 返回重挂订单（R3）。
- `crates/ricow_strategy/src/builtin_tests.rs`: univ2 测试按 R1 语义重设（on_fill 返回批次进入次 bar 断言点）; 新增引擎级测试: ①on_fill 链内限价单本 bar 不成交、次 bar 开盘跳空按 open 成交; ②市价单链内仍即时成交; ③深度封顶回传 Cancelled。
- 文档: `specs/lua-api.md`（事件模型小节补 R1 口径）、`specs/backtest.md`（撮合假设章节）、`specs/roadmap.md`（测试基线）。

**测试计划**:
1. 网格宽 bar 场景: 买 98 成交 → 重挂卖 100/买 96 **本 bar 不再成交**, 次 bar 起按正常规则撮合 → fill_count 有界。
2. 建仓链: on_fill 返回市价单仍同 bar 成交（兜底语义不回归）。
3. 跳空: 重挂限价卖 100, 次 bar open=101 → 按 open 成交（优于限价, 标准语义）。
4. univ2 全量既有测试（11 项）按 R3 恢复事件模型后重校。
5. 036 paired 全量既有测试重校（预期: 限价重挂从同 bar 可成交变为次 bar, 索引断言可能 +1 bar）。

**门禁**: cargo fmt / clippy / test --workspace（584 passed 不倒退, univ2/paired 测试按新语义重写后的总数变化如实记录）。

## 六、回测验收

- uniswap_v2_grid: SOLUSDT 1m 180d（投入 10000, start_price 同窗口起点）重新启用事件模型后复跑,
  与 037 终态基线（1915 笔 / +382.85）对比; 成交数差异 = "成交回调立即重挂"新增链式成交的净效果,
  按回测规范 v1 出概览, 无停摆（stat_stall_bars=0）为硬门槛。
- paired_grid_futures_long: 同参数复跑 180d 对比 f2f1115 基线, 漂移归因记录。
- 交付四件套 --export-dir, 按用户偏好写成交付说明待审。

## 七、风险

- 036/033 等既有策略回测结果漂移（仅限"on_fill 返回限价单"路径; 不返回订单的策略零影响）→ R4 复跑量化。
- 次 bar 入簿的开盘跳空成交价可能优于限价（open 越过 limit 按 open）→ 标准语义, 文档声明。
- 实现遗漏: settle_orders 的 step_bar 首轮派发（d=0, Vec::new()）与 on_tick 批次（d=0）不得误伤。

## 八、落地验证记录（2026-09-28 实施完成）

**引擎/策略改动**: `BacktestContext.defer_new_limits` + `set_defer_new_limits()`(固有 impl);
`backtest_runner::settle_orders` 对 depth≥1 派发的限价单置位 defer(place_order 直接入 pending_orders,
次 bar `match_pending` 撮合, 含开盘跳空按 open 成交), 市价单仍同 bar 即时; 深度超限每笔回传
`on_order_update(Cancelled)`(不再静默丢批); uniswap_v2_grid.lua v3 恢复 on_fill 返回 `do_rehang`(R3);
测试辅助 `settle_univ2` / `settle_orders_test` 对齐 defer 语义。

**门禁**: fmt 0 差异 / clippy -D warnings 0 / `cargo test --workspace` = **584 passed / 0 failed / 22 ignored**。

**R1 语义锁定测试**: univ2 `chain_capped` 重写(宽 bar 内 fill_count=2、链内重挂限价单本 bar 不成交、
1:1@98); paired `on_fill_no_order_keeps_rehang` 断言 b1.len()==1(不再同 bar 链式)。

**回测验收**:
- uniswap_v2_grid(SOLUSDT 1m×180d, 投入 10000): **1944 笔 / 净 +379.62 / stat_stall_bars=0**,
  对比 037 豁免基线 1915 笔 / +382.85 —— 漂移极小, 乒乓链根因消除, 硬门槛(零停摆)通过。
  导出 `/tmp/ricow-bt-univ2-038/`。
- paired_grid_futures_long(SOLUSDT 1m×180d, 投入 2000, start_price=85, initial_buy_amount=1000,
  order_amount=100, futures 数据源):
  - 基线 f2f1115: 238 笔 / 净 +389.25(+18.42% 自建仓) / **stat_stall_bars=53115**(≈37 天停摆)
  - 038 引擎: 436 笔 / 净 +210.30(+9.38% 自建仓) / **stat_stall_bars=0**
  - **漂移归因**: 基线的高收益由病理路径贡献 —— on_fill 同 bar 链式重挂允许同一根 bar 内连续成交
    多笔配对卖出(实盘同一根 K 线内价格只触及一次, 不可能逐笔复成交), 收益被高估; 且同 bar 乒乓链
    撞深度上限后整批静默丢弃 → 挂单簿失同步 → 全窗 20% 时间停摆(53115/259200 bars), 037 实测病理
    在 paired 上同样存在。038 后逐 bar 至多一次成交(行业口径), 收益更保守、零停摆。
  - 导出 `/tmp/ricow-bt-paired-038/`(基线对照 `/tmp/ricow-bt-paired-base/`)。

**口径勘误**: 早期一次 A/B 误用基线二进制跑"038"侧(shell cwd 残留), 曾得出"两引擎均 2 笔全窗停摆"
的假象结论; 用绝对路径重跑后修正为上表(教训: 回测 A/B 一律用绝对二进制路径)。

**文档**: `specs/lua-api.md` §on_fill 可返回订单(R1/R2 口径)、`specs/backtest.md` §二.4 追加 038 说明、
`specs/roadmap.md` 测试基线更新(584/0/22)。
