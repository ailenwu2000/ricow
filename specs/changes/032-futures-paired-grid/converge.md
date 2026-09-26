# 032 收敛报告: 合约双向配对网格策略

**依据**: [spec.md](./spec.md)(FR-001..FR-011) + [plan.md](./plan.md)(D1..D6) + [tasks.md](./tasks.md)(T001..T019)
**日期**: 2026-09-26 | **分支**: `feat/futures-paired-grid` | **基线**: 538 → **554 passed / 0 failed / 22 ignored**

## 一、范围核对(FR → 落地)

| FR | 内容 | 落地 | 状态 |
|:--|:--|:--|:--|
| FR-001 | 双向配对网格策略(多头侧开多平多 + 空头侧开空平空) | `strategies/futures/paired_grid_futures.lua` 双子网格 `S.long`/`S.short` | ✅ |
| FR-002 | 两侧独立启停门控(`start_price_*>0` 启用; 都 ≤0 启动报错) | `on_init` 门控 + 双侧禁用 `fatal=1` 停机; 单侧用例覆盖 | ✅ |
| FR-003 | 清单注册(market=futures / position_mode=hedge / default_leverage=2) | `paired_grid_futures.toml` + `catalog.rs` BUILTIN 第三条目 + `.gitignore` 例外 | ✅ |
| FR-004 | `OrderFill.position_side` 全链 | `types.rs` 字段 + backtest/DryRun/实盘 `futures_ws.rs`(`o.ps`)/现货 None + `lua.rs fill_to_table` | ✅ |
| FR-005 | 挂单带 position_side(hedge)、平仓不带 reduce_only | Lua 建仓/网格/平仓单均带 `position_side`; fapi 拒绝两者同带, 按侧平仓天然限定 | ✅ |
| FR-006 | 回测爆仓价 + mark 填充、Lua `pos_liq`/`pos_mark` | `backtest.rs isolated_liq_price` + 开/减仓重算 + `settle_closed_bar` 刷 mark; `lua.rs` 绑定 + unrealized 两侧累加 | ✅ |
| FR-007 | 实盘 `pos_liq` == 交易所 liquidationPrice | demo-fapi 冒烟核对(见 §二.3) | ✅ |
| FR-008 | DryRun hedge(双仓并存 + 钱包保证金模型 + liq 穿越只 WARN) | `context.rs` DryRun futures 分支按侧键 `pair\|long`/`pair\|short` + `wallets` + `isolated_liq_price` 同式 | ✅ |
| FR-009 | 直跑回填(market/position_mode/[backtest].leverage) | `run.rs` 免凭据行情源 + `mod.rs resolve_builtin_script` 回填(时序在杠杆校验前) | ✅ |
| FR-010 | 断点续接(按 `long_`/`short_` 前缀序列化, 关闭侧不序列化) | `save_state`/`on_init` 续接 | ✅ |
| FR-011 | 测试覆盖(纯逻辑单测 + 三模式真实冒烟, 禁 mock 替身) | 见 §二/§三 | ✅ |

**架构铁律核对**: 引擎/CLI/绑定层零策略参数名 —— `catalog.rs` 新字段为通用清单字段(`position_mode`/`default_leverage`), 不含策略专有键; `architecture_guard.rs` 新增 `is_engine_channel_key` 豁免集(pair/script/interval/leverage/max_leverage/mmr_pct/funding_rate_8h/fee_maker_bps/fee_taker_bps/slippage_bps/initial_cash), 均为引擎按名读取的通道键。策略专有键(`start_price_long` 等)只出现在策略文件与清单, 不进引擎代码。✅

## 二、三模式真实冒烟(FR-011, 禁 mock)

### 1. 回测冒烟(T015, 真实 K 线)
`ricow backtest --strategy paired_grid_futures --pair ETHUSDT --interval 1h --days 60 --cash 2000 --param start_price_long=<首根open> --param start_price_short=<首根open> --param initial_qty_long=0.1 --param initial_qty_short=0.1`
- 两侧成交价 == start_price(审核 S2: 取首根 open 保证必成交); position_mode=hedge; hedge_sides 多空分列有值; 默认杠杆回填 == 2; liq 告警路径触发(WARN)。✅

### 2. DryRun 冒烟(T016, 免凭据公共行情)
`ricow run paired_grid_futures`(RICOW_ROOT 隔离实例)
- 免凭据构造 `BnFuturesExchange::new(FuturesClient::new())`; 双限价挂单 → 穿越成交 → 两侧独立配对 → liq 显示; 停机不清仓。✅

### 3. demo 真实双向下单(T017, 币安 demo-fapi)
`ricow run paired_grid_futures --demo`(RICOW_FAPI_BASE_URL=https://demo-fapi.binance.com, 凭据见 specs/testnet.md)
- demo-fapi 同时出现 LONG/SHORT 两条 positionRisk; `pos_liq` == 交易所 liquidationPrice; 下单失败=0、已撤挂单=4 撤单失败=0; `stop --close-all` 双向平仓残留持仓=0; 用户流 drained=2。✅

## 三、测试基线

- `cargo test --workspace` = **554 passed / 0 failed / 22 ignored**(较 031 基线 538 +16)。
- 三门禁全绿: `cargo fmt --all -- --check` 0 差异 / `cargo clippy --workspace --all-targets -- -D warnings` 0 / test 0 failed。
- 增量用例: 回测 `isolated_liq_price` 已知向量 + 开/减仓重算 + mark 逐 bar 刷新; Lua 绑定 `pos_liq`/`pos_mark`(回测公式值/实盘透传/无仓 nil/hedge 两侧独立); DryRun hedge(双仓并存/各侧独立平/reduce_only 按侧封顶/保证金占用回笼/liq 填充/穿越只 WARN); catalog 新可选字段解析 + 缺省兼容; builtin_tests 合约集成 5 例(双侧建仓并存/单侧门控×2/双侧禁用停机/成本门槛停机); 架构护栏豁免一致性。

## 四、附带修复(预存在缺陷)

**Live 同 tick 批量挂单 client_order_id 撞号(BN 400 ClientOrderId is duplicated)**:
- 现象: 冒烟时 Lua 在同一 tick 内买+卖批量挂单, `client_order_id` 为空串 → `inject_prefix` 只返回归属前缀 → 同毫秒多单撞号。
- 根因: 引擎未给空 cid 生成唯一戳。
- 修复: `LiveContext` 加 `order_seq` 字段, `place_order` 给空 cid 打 `{毫秒}{seq}` 唯一戳(`crates/ricow_strategy/src/context.rs`)。
- 影响面: 仅 Live 路径; 回测/DryRun 撮合不依赖交易所 cid, 不受影响。属 032 冒烟暴露的通用缺陷, 一并修复。

## 五、已知限制(非缺陷, 文档已声明)

- **DryRun 不模拟强平与资金费**: 穿越 liq 只 WARN 不平仓; 空头侧收益偏乐观。已在策略清单 description 与 `specs/backtest.md §五.4a` 注明。
- **回测爆仓价为显示口径**: `pos_liq` 是逐侧独立 `isolated_liq_price` 估算, hedge 双仓并存时与按组合权益触发的真实强平点分叉(单仓恒等)。口径差异已在 `specs/backtest.md §五.4a` 说明。
- **杠杆仅显示/预算用**: 策略 `leverage` 参数只做保证金预算与显示; 回测实际杠杆由引擎 `[backtest].leverage`(清单 `default_leverage` 回填)决定; 引擎默认上限 10x 不强制, 清单建议 ≤5x 仅文案。

## 六、文档同步(T018)

- `specs/lua-api.md`: 新增 `pos_liq`/`pos_mark` 绑定行、`fill.position_side` 字段; 修正"实盘方向仓暂返回 0"过时句为三端已支持(032 起)。
- `specs/backtest.md §五`: 新增 §五.4a(爆仓价显示口径 vs 强平触发口径、hedge 分叉、DryRun MMR 同源 `params.mmr_pct`、实盘透传真值)。
- `specs/roadmap.md`: 新基线 554(2026-09-26)。

## 七、回测可观测性增强(032+, 2026-09-26 追加; 计划 `.trae/documents/backtest-close-all-export-observability.md`)

用户口径: ①回测完成后平仓再给信息 ②1m K 线 ③报告信息增强(多空 flag 极值等) ④参数与逐笔导出 /tmp ⑤架构师补充展示项。

| 变更 | 内容 | 落地 |
|:--|:--|:--|
| A | fill 时间戳修复: 正常成交 = 撮合 bar `open_time`, 强平/期末强平 = `close_time`(此前误用墙钟 `Utc::now()`, 逐笔导出时间全同) | `backtest.rs execute_fill` |
| B | 期末强制平仓 `--close-at-end`(默认 false): `BacktestContext::force_close_all` 按期末 bar close 平掉所有方向仓(hedge 两侧), 平仓 fill 带 `CLOSE-` 标识 + `position_side`; `report.close_at_end_applied` 标注; runner 在 `finalize` 后 `on_stop` 前调用 | `backtest.rs` + `backtest_runner.rs` + `command.rs` + CLI |
| C | 引擎级分侧统计: `realized_long`/`realized_short`(`PnlTracker::record_pnl_side` 按侧累加)/ `fills_side_counts`(按 `position_side` 聚合)/ `max_notional`(逐 bar 收盘采样峰值) | `pnl.rs` + `backtest.rs report()` + CLI 报告区块 |
| D | 策略统计透传: `report.strategy_stats` = `strategy.state_snapshot()`(引擎只搬运 key-value, 零策略参数名, 架构护栏通过); Lua 策略 `on_stop` 经 `ctx:state_set("stat_*")` 导出 flag 极值/lots 峰值/各计数 | `backtest_runner.rs` + `paired_grid_futures.lua` |
| E | 导出 `--export-dir`: `{策略}_{pair}_params.json`(生效配置全量+窗口)/ `_fills.csv`(逐笔)/ `_equity.csv`(权益曲线)/ `_report.txt`; `MAX_FILLS` 1000 → 100_000(1m 长窗口不截断) | `commands/backtest.rs export_backtest` |

- **测试基线**: 554 → **559 passed / 0 failed / 22 ignored**(+5: A 时间戳 / B 期末强平 / B 关闭回归 / D runner 透传 / E 导出落盘)。三门禁全绿。
- **追加(用户 2026-09-26 反馈"信息仍太少")**: 报告新增 **"测试参数"区块**(生效配置全量通用 dump + `bt_days`/`bt_interval`/`bt_close_at_end` 窗口参数, 剔除 script 源码, 架构铁律安全)与 **收益分解**(`unrealized_pnl` = 期末持仓盈亏; "交易收益(已实现) / 持仓盈亏(期末未实现)"两行)。回归断言 +2 (close-at-end 后未实现=0 / 留仓时未实现=市值−成本)。
- **SOLUSDT 实跑冒烟(最终口径 = 1m × 180 天, 259200 根, hedge, `--close-at-end --export-dir /tmp/ricow-bt-sol`)**: 两侧均激活(多 ref 97.72 / 空 ref 77.96), 成交 82 笔(多 39 / 空 43), 净盈亏 +26.66 全为已实现(持仓盈亏 0), 多头侧 +12.01 / 空头侧 +16.25; flag_long 区间 [-6, 0], flag_short [-4, 0]; 峰值名义敞口 698.59; 标的区间 +44.80% vs 策略 +1.42%(满仓持有对照 +43.36%)。导出四文件齐全(fills.csv 83 行含表头, equity.csv 259201 点)。
- **文档同步**: `specs/backtest.md §六.1`(可观测性增强七条)、`specs/lua-api.md`(on_stop 回测收尾语义 + strategy_stats 透传)、`specs/roadmap.md`(新基线 559)。

## 八、停摆根因修复(032 复审, 2026-09-26; 计划 `.trae/documents/fix-futures-paired-grid-v2.md`)

**根因链**(180d 首跑 stat_stall_bars=114800, 第 8 天起单块停摆): ①策略 `cap_close` 的 1e-9 比例削裁留残仓(已删) → ②runner 成交派发时序: `step_bar → on_tick → place → drain`, on_tick 看不到本 bar 已成交, 按陈旧栈顶定尺寸 → ③on_fill u 模式"按 fill 记 gain 却整 lot 弹出", 平仓量≠栈顶量时账本多平 → 账本/引擎残仓分叉 → `sweep_wallet_if_flat` 永不触发 → 保证金滞留 → cash < min_notional 永久停摆; ④WARN 权益口径漏逐仓钱包(报 21 实为 2057)。

**修复**: 
| 项 | 内容 | 落地 |
|:--|:--|:--|
| A | 限价撮合诚实性: fill 价 = min/max(open, limit), 不再吃满整 bar 区间 | `backtest.rs` + 2 回归 |
| B | runner 循环重排: `step_bar → drain+on_fill → on_tick → place → drain+on_fill`(撮合成交先于决策入账, 对齐实盘 WS 时序; 单标的+组合两 runner) | `backtest_runner.rs` |
| C | on_fill u 模式按 fill size 逐层扣减 lot(matched=min(remain, top.qty), 扣尽出栈, 超 1e-9 尘才 WARN); coin 模式保持整 lot 出栈(设计如此, 残量=积累币) | `paired_grid_futures.lua` |
| D | 权益补钱包: `Context::wallets_total()` 默认 0, BacktestContext 覆写 Σwallet; lua 合约权益 = 现金+钱包+未实现 | `context.rs`/`backtest.rs`/`lua.rs` |
| E | `skip_notional` 口径注释(计数事件次数, 与 stall_bars 同源, 非名义金额) | 同上 |
| F | 删除一次性冒烟配置 `strategies/paired_grid_futures_smoke.toml` | — |

**复跑验收(SOLUSDT 1m 180d, hedge, cash 2000, start_price 83.28, initial 100/100, order 100, `--close-at-end`)**: 
- 成交 **327 笔**(多 178/空 149; 修复前 166) ≫ 80 ✓; **stat_stall_bars=0**(修复前 114800) ✓; **stat_skip_notional=0** ✓; **stat_cash_final=1958.75**(不归零) ✓; 停摆 WARN 0 ✓。
- 净盈亏 -44.31(全已实现, 期末强平); flag 区间 多[-6,+1] / 空[-8,+1]; lots 峰值 多 7 / 空 9; 积累价差 123.36; 期间 2 条"空头逼近爆仓价"WARN(9.96%, 设计内告警, 单边上涨行情); 无账本 WARN。
- 测试基线 559 → **566 passed / 0 failed / 22 ignored**(+7: 撮合诚实性 2 + runner 重排断言 + 参数注入/等价性用例等), 三门禁全绿。

**已知限制/口径说明**: ①coin 模式整 lot 出栈为设计语义, 未配对残量计入积累币; ②现货 `paired_grid` 有"无仓无单即结束"语义、合约版无(随时可再开仓), 故真实趋势数据上"只多头≡现货"逐笔一致性仅在现货侧结束前成立(实测前 6 笔逐笔一致, 之后现货已结束); ③1e-9 阈值仅作 f64 账本簿记尘判定(引擎 Decimal 精确), 不触碰资金量, 与被禁的 1e-9 比例削裁是两回事。

**追加(用户追问亏损归因, 2026-09-26)**: ①`force_close_all` 的 CLOSE- fill 原先不入策略 on_fill(runner 收尾未 drain)→ 补派发, 策略账本与引擎逐分吻合(多 69.796929 / 空 53.566655−160.831012=−107.264357 == 引擎分侧已实现); ②`fill_to_table` 透传 `client_order_id`(引擎只搬运不解释); ③策略分侧统计导出: `stat_fill_count_long/short`、`stat_fee_long/short`、`stat_quote_accumulated_long/short`(网格配对毛价差)、`stat_close_pnl_long/short`(期末强平锁定损益)。180d 归因: 净 −44.31 = 多头配对 +69.80 + 空头配对 +53.57 − 空头期末强平 −160.83 − 手续费 6.84; 亏损唯一来源 = 单边上涨行情(+43.83%)中空头未配对栈(峰值 9 层)期末按收盘价强制买回; min_pair_profit 保底(0→2×fee_side=0.1%)下配对单不亏。

## 九、设计变更 v2: 双向 → 配对做多(用户指令, 2026-09-26; 计划 `.trae/documents/rewrite-futures-long-only.md`)

**决策**: 180d 归因证明空头侧是唯一亏损源(单边上涨堆栈 9 层、期末强平锁亏 −160.83、逼近爆仓价 9.96%), 用户指令砍掉空头侧, 重写为**配对做多网格**: 只做多和平多, 行为逻辑与现货 `paired_grid.lua` **逐行一致**(激活穿越/市价建仓拆格/成交即 ref 全撤重挂/栈空追踪/flag 放大/min_pair_profit 保底/coin 模式/成本门槛)。**唯一语义差异**: 现货第 10 条「无持仓且无待配对仓 → 结束」**不移植** —— 用户 2026-09-26 口径"任何一单成交 → ref=成交价 → 全撤重挂"= 网格持续运行、无结束条件; 合约版平完栈后继续追踪+挂买, 长期运行。

**落地**: 
- **改名(2026-09-26, 用户指令)**: 策略 id/文件 `paired_grid_futures` → `paired_grid_futures_long`(lua/toml/catalog 注册/.gitignore 例外/测试同步), 为将来的合约配对做空网格 `paired_grid_futures_short` 让位; 显示名维持「合约配对做多网格」。
- `strategies/futures/paired_grid_futures_long.lua` v2 重写: 单份状态(去 S.long/S.short), 参数对齐现货命名(`start_price`/`initial_buy_amount`, 删 long/short 双参), 保留 5 点合约化差异(①挂单 `position_side="long"` ②cap_open 杠杆保证金兜底 ③pos_liq 爆仓价 WARN ④min_notional 50/fee_side 0.0005 ⑤CLOSE- 强平单逐层扣减并单列 `close_pnl`); 统计导出单侧化 + `stat_fee`/`stat_close_pnl`/`stat_stall_bars`/`stat_cash_final`。
- `paired_grid_futures_long.toml` 清单重写; 引擎/CLI/绑定层**零改动**(catalog include_str 编译期内嵌, lua/toml 改动需重编译)。
- 测试: 合约用例 6→5(激活建仓/配对闭环/高于开始价等待/缺参停机/成本门槛停机), 空头镜像用例删除; 等价性用例: 合约只多头 vs 现货同 K 同参数**公共前缀逐笔一致**(bar/方向/价/量, 数量容差 1e-8 容纳现货 cap_sell 1e-9 削裁), 且现货 finished 后合约版继续成交(结束语义不移植的分叉点显式断言)。
- 本文档 §一~§八 所记"双向"内容为 v1 历史记录, 以本节为准。

**验证**: 三门禁全绿 **565 / 0 / 22**。SOLUSDT 1m 180d 复跑(`start_price=83.28, initial_buy_amount=100, order_amount=100, spacing 1%, offset 0.2, cash 2000, --close-at-end`, 报告 `/tmp/ricow-bt-sol-v2/long-180d/`): **成交 324 笔**(买 163/卖 161, 对比 v1 双向 ~20 笔、v2 首跑带结束语义仅 4 笔 —— 网格持续运行达标), track_up=24, rehang=345, 停摆 0/拒单 0/强平 0; 配对毛价差 +126.54 + 期末强平落袋 +52.13 − 手续费 6.69 = 净盈亏 **+171.99 (+7.48%)**(策略账本与引擎已实现 178.68 逐分吻合); 资金费净额 −22.41(多头净付), 期末权益 2149.57; 最大回撤 11.22%(vs 满仓持有 +43.83%/回撤 38.67%), 夏普 0.84; 栈峰值 10 层/flag ∈ [−9,+1], min_pair_profit 生效 0.1%。

## 十、回测规范 v1 + 建仓口径三连修(用户指令, 2026-09-26; 计划 `.trae/documents/backtest-report-overview.md`)

**背景**: 用户指出回测数据"总是不给全"(投入资金/首笔成交/期末持仓/持仓盈亏/标的起止价/涨跌幅 7 项), 要求生成**以后所有回测共同遵守的规范**并适配项目与策略; 随后在多组建仓名义回测对比中发现两处机制缺陷。

**落地**:
1. **回测规范 v1**(权威 = `specs/backtest.md` §〇, 规范性): A1-A7 报告概览必备(区间UTC/投入资金/首次成交/标的价格/期末持仓/净盈亏分解/基准对照) + B 区成交统计(盈亏笔数/单笔极值/总成交额/单笔均名义) + C 区策略适配义务(stat_* 导出/默认值注入/--close-at-end/账本对账<0.01) + D 区导出四件套 + E 区验收纪律; 禁止事项: 百分比不得脱离绝对值、真实成本不得省略、期末无仓必须说明原因。竞品对标 Freqtrade/Backtrader/vectorbt/Jesse/网格平台。
2. **引擎/报告适配**: `BacktestReport` 新增 12 个纯展示字段(不改记账); 报告最前插「回测概览」区块 + 「成交统计」区块(既有区块全保留); PnlTracker 跟踪单笔盈亏极值; terms 术语表 +9 词条(SC-009 双向核对); 新增单测 `test_report_overview_fields_v1`。
3. **语义差异 2 —— 建仓不计算 flag**: 建仓 lot 记 init 标记(栈底计数器 `init_lots`, 含状态持久化), 被网格卖出时不再 flag+1。此前卖平建仓 lot 每次 flag+1, 建仓越厚 flag 冲越高 → 卖单间距被放大到追不上行情, 建仓 1000 U 时成交从 324 锐减至 150。
4. **语义差异 3 —— 卖价锚定栈顶成本**: `sell_px = 栈顶买入成本×(1+up)`(此前锚 ref_price=最近成交价, 上涨段卖单逐笔爬升 82.15→82.97→83.80..., 相当于有仓还追踪上移)。有仓位不随成交上移、只随 flag 扩大间距; 栈空后由追踪上移接管。追踪上移口径澄清: 仅 `#lots==0` 时步进, 每步一个间距。
5. 以上 3/4 为合约版与现货的**显式语义差异**(现货蓝本保持原行为, lua 头注释记录差异 1/2/3); 等价性测试路径不覆盖余仓卖单场景, 仍有效。

**验证**: 三门禁全绿 **566 / 0 / 22**(+1 概览字段单测)。SOLUSDT 1m 180d 对比(其余参数不变): 建仓 100 U → 成交 269/净盈亏 +205.51(+10.28%); 建仓 1000 U → 成交 260/净盈亏 +210.49(+10.52%); 建仓 5000 U(cash 10000, 50 格) → 成交 316/净盈亏 +261.62(+2.62%, 单边上涨市网格只赚配对价差、底仓浮盈不吃的固有特性), 三组 flag 峰值均 +0、停摆 0、追踪上移 21~22 —— 建仓名义不再扭曲网格行为。报告导出: `/tmp/ricow-bt-sol-v2/{anchorfix,cash10k}/`。`close_pnl_realized` == 策略 `stat_close_pnl` 交叉核对一致。
