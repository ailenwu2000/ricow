# 香农 ETF 指数增加策略(shannon_etf_accum)实施计划

> 分支: `feat/shannon-atr-grid`(已创建, 工作树干净)
> 本文档为规划产物; 按宪法 §文档体系, 实施前需登记为变更档案 `specs/changes/023-shannon-etf-accum/{spec,plan,tasks}.md`。
>
> **修订记录(2026-09-17)**:
> ① 删除原「EMA 金叉死叉触发买卖」通道 —— 平衡价上下 `atr_mult×ATR` 已挂双边单, 价格到了自然成交。**EMA10/20 金叉只保留一个用途: 建仓**; 建仓后不再使用 EMA。
> ② 挂单量 = 该限价成交后**精确回到 `target_ratio`(默认 50:50)** 的量, 非固定份数。
> ③ **命名**: 新策略 = 「**香农 ETF 指数增加策略**」(`shannon_etf_accum`); 旧中轴再平衡样板**改名** `shannon_rebalance`。
> ④ **阶段 C 并入本分支**: K 线通道接线 / 重启续接 / Dry Run / demo 联调都在本分支做完。
> ⑤ **目标定位**: 长期收益优于满仓持有、短期回撤更小、波动中不断增加 ETF 份额 —— 含精确判据与诚实边界(§一)。
> ⑥ **回测方案**: 窗口 3 个月(数据不足按实际); 基准 = **以建仓成交价满仓持有**(+ 敞口对齐基准); 标的 = **QQQBUSDT + SPYBUSDT + PAXGUSDT**; ATR 乘数 = **2 / 3 / 4.5**(用户 2026-09-17 拍板; 原 1.5 档因单笔名义过低/被跳过率过高弃用)。
> ⑦ **本金 2000 → 10000 USDT**(实测 2000 下单笔名义 < `minNotional`, 网格无法成交)。
> ⑧ **计划审核修订(3 个独立审核员 + 自测)**: ①ATR 口径澄清(7×24 vs 美股时段差 ~1.8×); ②份额增长公式系数修正(1/2 → **1/4**); ③手续费表述修正(费用不是策略变量); ④三目标不可独立性/靠运气概率量化(μ < 0.75σ² 判据); ⑤任务标题规范化为 `### Task N:`; ⑥补 Task 5 撤单的三处既有拦截点; ⑦补改名/文档遗漏面(website / CONTRIBUTING / agentkit / backtest.md / roadmap); ⑧补 Task 16 在 DryRun 的前提缺陷。审核发现逐条落地位置见各任务。

**目标**: 用 1 小时 ATR 定间距、1 分钟 EMA10/20 金叉建仓、平衡价上下双边限价单抓插针的香农再平衡策略, 在 bStock ETF 上跑通「真实数据回测 → Dry Run → 币安 demo 真实调用」全链路(本金 10000 USDT), 并与满仓持有基准同口径对照。

**架构**: 策略本体仍是 Lua(`strategies/builtin/shannon_etf_accum.lua`, 宪法二); 引擎补**通用能力**: ①高周期序列与 ATR(1h) 通道(含 DryRun/实盘的 K 线刷新与重采样); ②策略侧撤单指令; ③单标的 K 线克隆性能修复; ④启动注入上次成交价(重启续接); ⑤回测报告增「以建仓价满仓持有」基准与「敞口对齐基准」。

**技术栈**: Rust(引擎) + Lua 5.4 沙箱 + Binance 现货 bStock(主网行情 / demo 模拟盘交易)。

---

## 一、策略定位与目标

**名称**: 香农 ETF 指数增加策略(`shannon_etf_accum`; 显示名「香农 ETF 指数增加」)。

**三个目标**(用户原话)与它们的**机制 + 可验证指标**:

| 目标 | 机制 | 验证指标 |
|:--|:--|:--|
| ② 回撤小于满仓 | **纯机械**: 常态只有 `target_ratio` 敞口, 下跌被现金垫着。实测 9/9 组「策略回撤 ÷ 满仓回撤」= 0.5002~0.5028 ≈ 恒等于 `target_ratio` | 策略最大回撤 vs 满仓最大回撤(注意: 该比值 ≈ `target_ratio` 是**敞口的算术结果, 不是策略有效性证据**) |
| ③ 波动中增加份额 | 每次完整往返净买回份额 > 卖出份额: `ΔQ/Q = s²/(4·P(P−s))`(一次干净往返的精确解, 已独立推导+数值校验) | 份额变化%(`coin_change_pct`), 但必须**扣除机械成分** —— 50:50 不变式下 `Q ≈ E/(2P)`, 价格下跌本身就会增持 |
| ① 长期收益 | 低买高卖收割波动: 再平衡溢价 ≈ `σ²/8` —— **这是相对"同敞口组合"的溢价, 不是相对满仓** | 策略总价值变化% vs 满仓收益% vs **敞口对齐基准(半仓买入持有)**; 只有 (策略 − 敞口对齐基准) 才是策略行为的净贡献 |

**诚实边界(有精确判据, 不许含糊)**:

- **默认 50:50 的长期期望是跑输满仓的**(数学, 不是猜测): 连续再平衡组合 `G(f) = f·μ − f²σ²/2`, 满仓 `G(1) = μ − σ²/2`; **`G(0.5) > G(1) ⟺ μ < 0.75σ²`**。QQQ 量级(μ≈10%/年、σ≈18%/年)下 `0.75σ² ≈ 2.4%` 远小于 μ → 目标① 的期望为负; 3 个月窗口(σ_3m≈9%)靠运气跑赢的概率约 **4 成**。要真跑赢满仓, `target_ratio` 需提到接近 Kelly 锚 `μ/σ²`(QQQ 量级 ≈3, 即需杠杆), 回撤同步放大 —— 这是目标①与目标②的**结构性冲突**, 只能靠实测数字说话, 不做承诺。
- **③ 与 ① 在 50:50 不变式下同源**(`Q ≈ E/(2P)`): "份额净增" ⟺ "策略收益 > 满仓收益"。所以不能用③单独宣称策略有效, 必须与敞口对齐基准一起看。
- **收割量级很小**: 独立复刻(10.4 天, 含每小时重挂)显示"策略 − 敞口对齐基准" 9/9 为负(−0.036~−0.051pp), 同期手续费 0.061~0.170% —— 即**收割只补回约一半手续费**; 3 个月窗口能否转正**只能靠实测, 不能靠机制宣称**。

---

## 二、回测方案(用户指定)与实测口径

**窗口**: 三只标的**统一 3 个月(90 天)**; 数据不足 90 天的按实际可用跑。下表"数据可用长度"只是历史长短, **不是回测窗口**。报告必须记录实际首末 bar 时间与根数。

| 腿 | 交易对 | 性质 | 1m 数据可用长度 | 参数/现价 |
|:--|:--|:--|:--|:--|
| 纳指 100 | `QQQBUSDT` | bStock 现货 ETF | 79 天(2026-06-30 13:30 起)→ **按 79 天跑** | step 0.001 / minNotional 5; 现价 ≈715.6 |
| 标普 500 | `SPYBUSDT` | bStock 现货 ETF | 72 天(2026-07-07 起)→ **按 72 天跑** | step 0.001 / minNotional 5; 现价 ≈761.2 |
| 黄金 | `PAXGUSDT` | 代币化黄金(1 盎司), **非 bStock ETF**, 24/7 | 6 年(2020-08-28 起)→ **仍只取最近 90 天** | step 0.0001 / minNotional 5 / tick 0.01; 现价 ≈4356 |

> **GLD 不可交易(实测)**: 主网现货无 `GLDBUSDT`; fapi 156 只 EQUITY 合约里也没有 GLD(仅有金矿 ETF `GDX` 的合约, 非现货 bStock)。用户拍板黄金腿用 `PAXGUSDT`(Paxos 代币化黄金, 非 SPDR GLD), 报告与文档必须注明性质, 不与 GLD 混谈。
> 备选腿(如需): `SOXLBUSDT`(72 天) / `TQQQBUSDT`(57 天, step 0.01) / `SMHBUSDT`(50 天) / `EWYBUSDT`(86 天, **实测无 1m 数据**, 只能 1h/1d)。

**ATR 口径必须先定(7×24 vs 美股时段, 实测差 ~1.8×)**: 表内 ATR14 = **标的自身 1h K 线 + 引擎同款 Wilder 平滑**(`indicators_api::atr` ← `ta::AverageTrueRange`):

| 标的 | 7×24 口径(占价) | 仅美股时段(13:30–20:00 UTC)口径 | 倍差 |
|:--|:--|:--|:--|
| QQQBUSDT | 1.71(**0.239%**) | 3.01(0.421%) | 1.76× |
| SPYBUSDT | 1.22(**0.161%**) | 2.28(0.300%) | 1.86× |
| PAXGUSDT | 20.94(**0.481%**) | 30.71(0.705%) | 1.47× |

时段口径含**隔夜跳空**(两个时段 bar 之间的缺口算进 TR), 这正是"真 QQQ 1h ATR ≈ 0.4%"的来源; QQQBUSDT 实际 7×24 都在成交 → **本计划默认 7×24 口径**(自洽, 引擎可直接算)。用户口算的"2×ATR ≈ 0.75%"属真美股口径, 在 7×24 口径下对应 `atr_mult ≈ 3`(实测 3 档 = 0.716%); 若要与真美股口径对齐间距, 把 `atr_mult ×1.75`。报告必须标注所用口径。

**ATR 乘数**: `2 / 3 / 4.5`(用户拍板) → **3 标的 × 3 乘数 = 9 组**, 其余默认(`atr_period=14`、`atr_interval=1h`、`target_ratio=0.5`、`interval=1m`、**本金 10000 USDT**)。实测三档间距(7×24 口径): QQQB 0.48%/0.72%/1.07%、SPYB 0.32%/0.48%/0.72%、PAXG 0.96%/1.44%/2.16%。
> 取值理由: 用户口算的"2×ATR ≈ 0.75%"在 7×24 口径下 ≈ `atr_mult=3`; 2/3/4.5 三档 = 0.32~2.16% 间距, 既覆盖用户预期, 又避开 1.5 档的低名义/高跳过区间(单笔名义 1.2~3.6 USDT < `minNotional`)。

**本金与交易所最小名义**: 单笔再平衡名义 = `target_ratio × (1−target_ratio) × 权益 × spacing / 价格`(默认比例 = `权益 × s / (4 × 价格)`):

| 标的 | spacing(2/3/4.5) | 单笔名义 @2000 | 单笔名义 @10000 |
|:--|:--|:--|:--|
| QQQB | 3.42 / 5.13 / 7.69 | 2.39 / 3.58 / 5.37 | 11.94 / 17.91 / 26.87 ✅ |
| SPYB | 2.45 / 3.67 / 5.50 | 1.61 / 2.41 / 3.62 | 8.03 / 12.05 / 18.08 ✅ |
| PAXG | 41.88 / 62.82 / 94.23 | 4.81 / 7.21 / 10.82 | 24.04 / 36.05 / 54.08 ✅ |

`minNotional` = 5 USDT: 2000 USDT 下 QQQB/SPYB 九档全部不足(实盘每单被拒; 回测按守卫跳过则网格一次不成交 → 退化成「建仓后持有半仓」)。按**中位 ATR** 反推 `权益 ≥ 20 × 价格 / spacing`, 覆盖九档需 ≥ 7800 → **本金 = 10000 USDT**。
**但 ATR 逐小时变化, 低 ATR 小时会掉到门槛以下**: 独立复刻(10.4 天)实测 —— 旧 1.5 档跳过占比 SPYB **377 跳 / 33 成交(92%)**、QQQB 65%; 1.5 档 `s/P` 的 p10 = QQQB 0.140% / SPYB 0.084% / PAXG 0.131%, 均低于 0.2% 门槛(**这就是弃用 1.5 档的量化依据**)。**新矩阵 2/3/4.5 档的跳过占比必须逐组实测输出**(预期显著下降, 但不预先宣称); 跳过 >80% 的组不作结论依据, 只作"本金/乘数不匹配"的记录。

**保本约束与费用真实量级**: 每往返毛收割 ≈ `s²/(4P²) × 权益`; 手续费 = 0.2% × 单笔名义 ≈ `0.05% × s/P × 权益` → **保本条件(必要条件) `spacing > 0.2% × 价格`**。实测(7×24 口径, 按近 79 天价格总行程估往返):

| 标的 | mult | 每往返 毛收割 / 手续费(% 权益) | 估算往返 | 季度净收割(% 权益) |
|:--|:--|:--|:--|:--|
| QQQB | 2 / 3 / 4.5 | 0.00057/0.00024 · 0.00128/0.00036 · 0.00289/0.00054 | 103 / 69 / 46 | 0.034 / 0.064 / 0.108 |
| SPYB | 2 / 3 / 4.5 | 0.00026/0.00016 · 0.00058/0.00024 · 0.00131/0.00036 | 95 / 64 / 42 | 0.009 / 0.022 / 0.040 |
| PAXG | 2 / 3 / 4.5 | 0.00231/0.00048 · 0.00520/0.00072 · 0.01170/0.00108 | 81 / 54 / 36 | 0.148 / 0.242 / 0.382 |

读法: 手续费 ≈ `0.0002% 权益/往返`, 三个月合计 **0.003~0.03% 权益 —— 绝对量级可忽略, 手续费不是策略成败的变量**; 决定 P&L 的是持仓价格变动(敞口)。(注: 曾把"费用 ÷ 二档收割项"写成"费用吃掉 52~78% 收益", 那只描述收割项内部比例, 属误导表述, 已修正。)

**基准(用户指定口径)**: **以建仓成交价满仓持有** —— 同一时点、同一价格(策略建仓成交价)、同一本金(10000 USDT)全额买入持有到期末, 从**建仓时刻**起算(建仓前策略空仓, 不计)。**不用期初 open 作基准**。另加**敞口对齐基准**(半仓买入持有, 零再平衡, 由满仓 ×`target_ratio` 派生 —— 收益与回撤都 ×0.5), 用于把"半仓敞口的算术效应"从"策略行为"里剥出来。
口径声明: ①期末价 = 与策略同源的末根 close; ②基准不计手续费/滑点而策略计费 → 对策略不利, 属已知偏差, 报告注明; ③若某组**从未建仓/0 成交**(ATR 未就绪或全被跳过), 该组标"不可比"、不计入目标结论, 单列原因统计。

**运行命令**(本机 `api.binance.com` 直连超时, 用公开镜像):

```
RICOW_BN_BASE_URL=https://data-api.binance.vision \
ricow backtest --strategy shannon_etf_accum --pair <SYMBOL> --interval 1m --days 90 --cash 10000 --param atr_mult=<1.5|2|3>
```

**选做**: ①QQQB 上扫 `target_ratio ∈ {0.5, 0.6, 0.7, 0.8}`(回答"能否跑赢满仓"及回撤代价, 窗口同为 3 个月); ②保留一次 2000 USDT(QQQB×mult2)对照, 用于如实记录"本金过小 → 单笔名义不足 → 无法成交"。

---

## 三、策略语义

**一句话**: 用 1m EMA10/20 **金叉**建仓(买入 `现金 × target_ratio`, 10000 → 5000 USDT); 之后以「最近一次成交价」为**平衡价**, 上下各挂一张 `atr_mult × ATR` 的**香农再平衡限价单**; 每次成交后平衡价 = 该成交价, 撤余单并按新平衡价重挂; 插针触及挂单价即成交, 不再看 EMA。

**参数**(默认值由 CLI 注入, Lua 侧只做覆盖):

| 参数 | 默认 | 说明 |
|:--|:--|:--|
| `pair` | `QQQBUSDT` | 交易对(bStock 现货; 也可 PAXGUSDT 等) |
| `atr_interval` | `1h` | ATR 用的 K 线周期 |
| `atr_period` | `14` | ATR 周期(需 ≥15 根完整高周期 bar 才有值) |
| `atr_mult` | `2.0` | 间距 = `atr_mult × ATR`(回测扫 **2/3/4.5**, 默认档 = 扫的中间档) |
| `ema_fast` / `ema_slow` | `10` / `20` | **仅建仓**用(1m 金叉) |
| `target_ratio` | `0.5` | 中轴: 建仓买 `现金 × target_ratio`; 回平衡目标。**敞口旋钮**(见 §一 判据) |
| `rehang_secs` | `3600` | 定期重挂(ATR 刷新周期; 0 = 关闭)。**唯一的自愈通道** |
| `min_notional` | `5` | 名义额下限守卫(与交易所 `minNotional` 对齐; 0 = 关闭) |
| `initial_cash` | 10000(测试) | 引擎级虚拟本金: 回测 `--cash 10000`, DryRun `params.initial_cash`; 须满足 `权益 ≥ 20×价格/spacing` |
| `resume_balance_price` | 引擎注入(**仅实盘/demo**) | 重启续接: 上次成交价(方向不入参, DB `fills` 已有且策略自记 `last_side`) |

**主序列周期** = 引擎 `--interval`(用 `1m`); **ATR 序列周期** = `atr_interval`(1h)。**决策时间点** = 每根 1m 收线(策略比对 `ctx:klines` 末根 `ts` 变化)。

**流程**:

1. **建仓前**(无平衡价且无持仓): 每根 1m 收线算 EMA 快慢线, 与上一根比较判定**金叉** → 市价买入 `现金 × target_ratio`。ATR 未就绪 → 不下单, 只记日志。
2. **建仓成交后**(`on_fill`): 记 `balance_price = 成交价`(**该价同时是回测基准的满仓买入价**)、`last_side = buy`; **下一个 tick** 挂 `[买 @ 平衡价 − spacing, 卖 @ 平衡价 + spacing]`(限价)。
3. **之后每次成交**(含部分成交, 每笔都触发): `balance_price = fill_price`, `last_side = fill.side`; **下一个 tick** 返回 `[撤单指令, 新买单, 新卖单]`(撤单在前, 保证任意时刻挂单 ≤2 张)。
   - **时序澄清**: on_fill 触发的重挂**不受"新 1m 收线"门限制**(成交感知即重挂), 否则实盘最久要等 1m + 15s(K 线刷新)才撤陈旧对侧单; 回测里成交在 `step_bar` 的 `match_pending` 发生, 早于 `on_tick`, 因此 `on_tick` 已看到成交后的持仓/现金(但策略自身的 `balance_price` 状态要等本 tick 末的 `on_fill` 才更新 → 重挂实际落在下一根 bar, 属设计)。
4. **定期重挂**(每 `rehang_secs`): 按最新 ATR 重算 spacing, 撤旧挂新 —— ①间距跟随 ATR; ②自愈(挂单被拒/丢失后恢复)。
5. **回平衡量**(纯函数): 以目标价 `P` 计 `delta = |equity(P)·target_ratio − pos_size·P| / P`, `equity(P) = 现金 + pos_size·P`; `delta>0` 买, `delta<0` 卖。守卫: 卖 ≤ 持仓; 买 ≤ `现金/(P×(1+fee))`; 名义 < `min_notional` 或 `spacing% ≤ 0.2%` → 跳过并记日志; `delta ≤ 0` / `size ≤ 0` → 该侧不挂。
6. **建仓后不使用 EMA**(只算 ATR)。
7. **重启**: `on_init` 试读 `resume_balance_price` → 有则按它挂双边单; 无且 `position_size > 0` → 日志"未取得平衡价, 网格待机", 不猜价格。

**机制特点**: 单一通道 = 挂单。**代价**: 挂单被拒/丢失(低于 `minNotional`/保本线、交易所侧撤销)会让网格停摆 —— 靠定期重挂自愈, 日志必须记录拒单、跳过与重挂次数。

---

## 四、现状核查结论: 为什么必须动架构

| # | 缺口 | 证据(已逐条实测) | 影响 |
|:--|:--|:--|:--|
| G1 | **拿不到高周期 ATR**: ctx 指标只基于当前 interval 的已收盘 K 线 | `lua.rs:243-246`(`atr` 注册, 指标段 219-266)、`indicators_api.rs:298` | 1m 主序列下拿不到 1h ATR |
| G2 | **1m 回测不可用(O(n²) 克隆)**: 快照每 tick 克隆**全部**已收盘 bar, 100 根上限在克隆**之后**才截断; 指标每次调用再各克隆一遍 | `lua.rs:367`(快照)、`lua.rs:205`(截断位置)、`backtest.rs:1805`→`:392`(`closed_klines.clone()`) | 1m/90 天 ≈ 13 万根 → 克隆量 10⁹ 级 |
| G3 | **策略发不出撤单**: `on_tick` 只回下单请求, `OrderRequest` 无 action; 实盘挂单只由停机兜底撤 | `strategy.rs:13`、`types.rs:153`、`command.rs:785-813`(停机清理) | 「成交后撤余单重挂」做不到, 残单累积 |
| G4 | **模拟盘/实盘无 K 线通道**: `update_klines` 无调用方 | `context.rs:202`(Live)/`:517`(DryRun) 仅定义 | DryRun/实盘 `ctx:klines/ema/atr` 恒 nil |
| G5 | **重启无状态续接**: 策略全局变量随进程消失, 无 `ctx:state` | 30 个注册方法无 `state`(`lua.rs:157-266`); DB 有 `fills`(`db.rs:316/348`) | 重启后平衡价丢失 → 有持仓却不挂单 |
| G6 | **报告缺基准**: 无"以建仓价满仓持有"的基准收益/回撤(且标的最大回撤也缺) | `BacktestReport`(`backtest.rs:35-102`)、`pnl.rs:96`(`max_drawdown`)、`metrics.rs:68`(`annual_volatility`) | 目标①②无法验证 |

**数据事实**(2026-09-17 实测): 见 §二(QQQB 79.06 天 / SPYB 72.06 天 / PAXG 2020-08-28 起; GLD 不存在; 三标的 `minNotional` 均 5, `stepSize` 0.001/0.001/0.0001); 三标的 1m 覆盖率 = 1.000(零成交量 bar 存在但无缺分钟); demo 模拟盘 3705 个 symbol 含 QQQB/SPYB/PAXG。测试基线(实跑): `cargo test --workspace --locked` = **339 passed / 0 failed / 12 ignored**。

---

## 五、阶段 A: 引擎能力(回测闭环必需)

### Task 1: 高周期重采样纯函数

- **目标**: 由 1m 序列合成完整的高周期 bar(1h), 剔除不完整桶/缺口桶。
- **文件**: 新增 `crates/ricow_strategy/src/multiframe.rs`; `lib.rs` 注册(`mod multiframe;` + **`pub use multiframe::resample_complete;`** —— 装配层在 `crates/ricow` 是另一 crate, 私有 mod 不可见)。
- **步骤**:
  1. 先写失败单测: 6 根 1m bar + `tf=1h` → 0 根完整桶; 120 根跨 2 小时 + 首尾各半个桶 → 恰好 1 根完整桶且聚合值 = 手算。
  2. 实现 `resample_complete(bars: &[Kline], tf_ms: i64) -> Vec<Kline>`: 桶键 = `open_time.ms.div_euclid(tf_ms)`; **完整桶判据** = 桶起点 ≥ 序列首根 open_time 且 桶终点 ≤ 序列末根 close_time(首尾半桶丢弃); 聚合 open=首根 open / close=末根 close / high=max / low=min / volume=Σ, close_time = 桶起点 + tf_ms − 1ms; 桶内 bar 数 < `tf_ms/60000 × 0.9` 视为缺口 → 丢弃并计数。
  3. `cargo test -p ricow_strategy multiframe` 通过。
- **完成判据**: 单测含"首尾半桶丢弃 / 缺口桶丢弃 / 聚合值手算一致"三类向量。(三标的 1m 覆盖率 1.000 已实测, 缺口规则不会误删正常小时。)

### Task 2: 高周期 ATR 通道(`Context::atr_tf`)

- **目标**: 任意上下文按高周期取 ATR, **每小时只算一次**, 无前视。
- **文件**: `crates/ricow_strategy/src/context.rs`(trait + 实现)、`multiframe.rs`(缓存)、`crates/ricow_engine/src/backtest_runner.rs`、`crates/ricow/src/commands/backtest.rs`(装配)。
- **步骤**:
  1. trait 增 `fn atr_tf(&self, pair: &str, period: usize, tf: &str) -> Option<f64> { None }` + `fn set_tf_klines(&mut self, pair: &str, tf: &str, bars: Vec<Kline>)`(默认 no-op)。实现者共 **4** 个: `BacktestContext`/`DryRunContext`/`LiveContext` + 测试替身 `TestCtx`(`risk.rs:446`, 靠默认实现即可)。**前三个必须各自 override** —— 否则 DryRun/实盘 ATR 恒 nil, Task 15 白做。
  2. `multiframe::TfAtrCache`: 持 tf bars(升序)+ 记忆 `(tf, period) → (最后可见桶 ts, atr)`; 可见性 = 桶 `open_time + tf_ms ≤ 当前 tick 时间`(`partition_point` 定位); 只对可见前缀切片算(`indicators_api::atr`), **不克隆**; 同一"最后可见桶 ts"直接返缓存(实现"每小时算一次")。
  3. 三个实现分别接: `BacktestContext` 用 `current_bar.open_time`; `DryRunContext`/`LiveContext` 用 `now_utc()`。
  4. 装配(`crates/ricow/src/commands/backtest.rs`): ①拉 `[窗口起 − 24h, 末根]` **全段 1m**(24h = 36 个整桶; ATR 需 15 桶且 Task 1 会丢不完整/缺口桶, 16h 恰好 15 桶属临界, 不留这种余量); ②对**全段**重采样 1h → `ctx.set_tf_klines`(每小时的 tick 都能看到新桶, 不是一次性静态快照); ③**主循环只传 `&klines[warmup..]`** —— 预热段不入 `total_bars`/`first_open`/权益曲线/报告。
  5. 单测: ① 前 14 根 1h 未凑齐 → None; ② **第 15 根可见 → 有值**(`indicators_api.rs:299` 的 `len < period+1` 是硬边界, 这才是边界向量); ③ 与 `indicators_api::atr(可见前缀, 14)` 一致; ④ **无前视**: 当前 tick 落在未收盘的 1h 桶内时, 返回值不受本桶数据影响。
- **完成判据**: 四条单测通过; 缓存命中可断言(同一小时内多次调用只算一次)。

### Task 3: Lua 暴露高周期 ATR `ctx:atr_tf(pair)`

- **文件**: `crates/ricow_strategy/src/lua.rs`、`specs/lua-api.md`(指标契约 + `warmup_bars` 运行时参数)。
- **设计(2026-09-17 实施时定稿, 取代原"`ctx:atr` 第三参"方案)**: 周期由**配置**决定, 不做调用参数 —— 用户原始要求是"可以设置使用哪个 K 线、多少周期的 ATR", 属策略参数(`atr_interval` / `atr_period` / `atr_mult`)。收益: ①不必给 mlua 尾参做兼容分支; ②快照只填一个标量(每 tick O(1))—— 若走"第三参"方案, 每次调用都要把高周期序列克隆进 Lua 层, 1m×11 万 tick 会多出上亿次 bar 拷贝; ③与"ATR 每小时算一次"天然一致(引擎按桶缓存)。
- **步骤**: ①`LuaCtxData` 增 `tf_atr: HashMap<pair, f64>`; ②`fill_snapshot` 在策略声明 `atr_interval` 时按 `atr_period`(缺省 14)向引擎取值(引擎侧桶内记忆, 不重算); ③注册 `ctx:atr_tf(pair)` → 查 `tf_atr`(通道未就绪 → nil); ④文档: 指标表加 `ctx:atr_tf`、写明装配责任(回测自动预热 24h / 模拟盘实盘由刷新任务装入)、明确 `ctx:atr` 与 `ctx:atr_tf` 口径差异。
- **完成判据**: ①新测试 `test_atr_tf_exposed_to_lua` 通过(未就绪 → Lua 侧 nil 而非 0; 就绪 → 与独立重采样直算一致); ②旧测试零改动通过。

### Task 4: 单标的 K 线尾窗克隆修复(性能)

- **文件**: `crates/ricow_strategy/src/backtest.rs`(+ DryRun/Live 同款)、`lua.rs`。
- **步骤**:
  1. 先量化(每 tick 不止一次整段克隆: 快照 1 次 + 每个指标各 1 次, 新策略一 tick ≈ 4~5 次): **before 基线用 3~5k 根 1m**(3 万根会跑数十分钟), **after 用 3 万根**; 判据 = "改后 ≥100× **或** 3 万根总耗时 < N 秒"(N 实测填入收敛记录)。
  2. `BacktestContext` 增 `closed_tail: VecDeque<Kline>`(封顶 100, `step_bar` 入/出队; 需 `use std::collections::VecDeque;`); `klines_for` 单标的返回尾窗; 组合/信号模式保持"全段 + `SIGNAL_TAIL`(400)封顶"不变(既有长度断言在组合模式测试 `backtest.rs:3101/3168`, 不受影响)。
  3. **同步声明语义契约**: 单标的路径下 `ctx:klines` **与 12 个指标 API 的输入**上限统一 = 100 根(指标读同一份 `data.klines`; 现在拿到的是全段) —— 需 >100 根窗口的指标改走 `atr_tf`/信号模式。**这是契约变更, 必须写进 `specs/lua-api.md` §指标 API**, 否则属静默语义变更。
  4. `lua.rs:205` 的 100 根截断保留(双保险); 复跑性能用例并记录前后秒数。
- **完成判据**: 全量测试无回归; 性能判据达标(实测数字入收敛记录); `specs/lua-api.md` 的长度契约已补。

### Task 5: 撤单指令(挂单可撤, 供重挂)

- **文件**: `crates/ricow_core/src/types.rs`、`lua.rs`、`backtest.rs`/`context.rs`、`specs/lua-api.md` §三(订单格式在 `:159`)。
- **步骤**:
  1. `OrderRequest` 增 `action: OrderAction { Place, CancelPending }`(serde `default` = Place), 更新 **7 处字面量构造**(实测: `types.rs:174,187` / `lua.rs:500` / `live.rs:350` / `backtest.rs:1985,1995` / `risk.rs:482`; 5 文件)。
  2. Lua 订单表增可选 `action = "cancel_pending"`(缺省 `"place"`; 未知值告警按 place)。**action 判定必须放在 size 解析与丢弃守卫之前** —— 现有 `parse_orders` 对 `size <= 0` 先 warn 再 `continue`(`lua.rs:466-473`/`:497-499`), 照字面在解析后加分支, 撤单指令根本到不了 `place_order`; 撤单只读 `pair`, 跳过 side/size/price。
  3. 三处实现: 回测/DryRun 从 `pending_orders` 删该 pair 全部挂单并返 `Cancelled` ack; 实盘 `get_open_orders(pair)` 按本实例 `client_order_id` 前缀逐个撤(单个失败不中断)。**撤单分支必须在 `risk_reject` / `prepare_live_order` 之前短路**: 回测 `backtest.rs:1632`、DryRun `context.rs:759` 首行即 `risk_reject`; 实盘 `context.rs:308-341` 先对齐(`align.rs:80-84` 对 size=0 判"对齐后数量为 0"→ `Rejected`) —— 不短路则撤单静默失效(正是 G3 的病)。撤单**不计入**限频与 `rejected_count`。
  4. 单测: ① 挂单未成交 → `cancel_pending` → `pending_order_count` 归零、后续 bar 不再成交; ② **撤单不被 `risk_reject` 拦**(构造会触发限频/静态限额的配置, 断言撤单仍生效); ③ Place 行为零改动。
- **完成判据**: 三条单测通过; 实盘撤单路径由 Task 18 demo 验证。

### Task 6: 旧策略改名 `shannon_grid` → `shannon_rebalance`

- **文件**: `strategies/builtin/shannon_grid.lua` → `shannon_rebalance.lua`(`git mv`); `crates/ricow/src/commands/{mod.rs, backtest.rs, run.rs}`; `crates/ricow_strategy/src/builtin_tests.rs`; `crates/ricow/src/ai/tools.rs`; 各测试字面量。
- **步骤**:
  1. `git mv` + 头注释/日志前缀同步。
  2. 注册表与文案: `BUILTIN_SCRIPTS`(`commands/mod.rs:296-297`)、CLI 默认参数分支(`backtest.rs:131` / `run.rs:409`)、帮助文案(`backtest.rs:15` / `run.rs:21`)、AI 工具清单(`ai/tools.rs:304`)。
  3. `builtin_tests.rs` 常量与 4 个测试名(`:22/88/337/357/389`)。
  4. TOML/断言字面量: `config.rs:376,416`、`config.rs:391`(断言)、`commands/backtest.rs:382,435`、`mod.rs:485`、`run.rs:485`(测试 TOML)。
  5. 全仓 `grep -rn "shannon_grid"` 复查(实测 `crates/` 共 **28 处**)。清单外还需处置 7 处: `lib.rs:3`(模块 doc)、`agentkit.rs:98`(**生成给外部 agent 的手册文案**)、`mod.rs:328`(doc 注释), 及 `name.rs:92`/`ai.rs:237`(属"任意样例名/命令解析", **可豁免但必须在计划里写明理由**)。
- **完成判据**: `grep -rn "shannon_grid" crates/ strategies/` 归零(或仅剩已注明的豁免项)。
- **破坏性提示**: 已部署 TOML 的 `type = "shannon_grid"` 语义消失, 需手工改 `shannon_rebalance`。本机 `strategies/` 实测无用户 TOML, 无本地迁移负担。(版本/发布不在本次范围, 用户 2026-09-17 明确"专注策略")。

### Task 7: 新策略注册与 CLI 装配

- **文件**: `crates/ricow/src/commands/mod.rs`、`{backtest.rs, run.rs}`、`crates/ricow/src/ai/tools.rs`。
- **步骤**: 注册 `shannon_etf_accum` → 新脚本; 注入默认 `pair/atr_interval/atr_period/atr_mult/ema_fast/ema_slow/target_ratio/rehang_secs/min_notional`(**默认值必须由注入层保证** —— Lua 侧 `config_f64` 缺省返回 0, 沿用 v4 教训)。**默认参数只在一处定义**, Task 17 的 TOML 引用同一组值(否则 9 组回测口径不可比 —— `resolve_config` 命中 `strategies/<name>.toml` 后不再走 CLI 默认分支)。
- **完成判据**: `ricow backtest --strategy shannon_etf_accum --pair QQQBUSDT --interval 1m --days 5 --cash 10000` 跑完并打印报告。

### Task 8: 回测报告增基准(满仓 + 敞口对齐)

- **文件**: `crates/ricow_strategy/src/backtest.rs`(记录首笔成交 + 报告字段)、**`pnl.rs`**(`max_drawdown` 在这里)、`metrics.rs`(`annual_volatility` 可喂价格序列)、`crates/ricow/src/commands/backtest.rs`(排版)。
- **步骤**:
  1. `BacktestContext` 记 `first_fill_price` / `first_fill_time`: 落点在 `execute_fill`(成交路径, 约 `:1494+`), 参考 `first_pos_size`/`first_cash_after_build`(`:1557-1567`)的"资金变动之后"位置, 但**必须无条件记录** —— 照抄 `if sz > 0` 守卫会漏"首笔即卖出"路径, 基准字段恒 None。
  2. `BacktestReport` 增 `benchmark_entry_price` / `benchmark_return_pct` / `benchmark_max_drawdown`(从**建仓时刻**起算); 缺建仓成交 → `None`; 组合路径 → `None`(不编造)。
  3. CLI 排版三行: `满仓持有(以建仓价 <时间> @ <价>): 收益 A% / 最大回撤 B%` / `敞口对齐基准(半仓买入持有, 同价同时点, 由满仓 ×target_ratio 派生): 收益 0.5A% / 回撤 0.5B%` / `策略: 总价值变化 C% / 最大回撤 D% / 份额变化 E%`。只有 (策略 − 敞口对齐基准) 才是策略行为的净贡献。
  4. 单测(手算向量): 已知序列(如 100→80→120)断言基准收益与回撤; 无建仓成交 → None 不 panic; 首笔即卖出 → 仍有基准值(守住步骤 1 的无条件记录)。
- **完成判据**: 单测通过; 报告三行并列、口径标注清楚(起算点 = 建仓时刻、期末价 = 末根 close、基准不计费)。

---

## 六、阶段 B: 策略脚本(阶段 A 完成后)

### Task 9: 骨架 + 纯函数 + 参数读取

- **文件**: `strategies/builtin/shannon_etf_accum.lua`; 测试并入 `crates/ricow_strategy/src/builtin_tests.rs`。
- **步骤**: 头注释写清语义(金叉建仓 / 双边挂单 / 回平衡公式 / 成交后重挂 / 建仓后不看 EMA / 目标与判据); 纯函数 `rebalance_size(cash, pos_size, price, ratio, fee) -> (side, size)`、`cross_state(fast, slow, prev_fast, prev_slow)`、`order_pair(balance, spacing, cash, pos_size, ratio, fee)`; `on_init` 读参数(禁用类开关选"语义上 0 天然无害"的参数)。**需求 5「记录买入还是卖出」要真接线**: `last_side` 不只赋值 —— on_fill 日志打印方向、Task 11 断言"新挂单方向与上次成交相反"、`ricow info`/报告可见(DB `fills` 已持久化方向, 但策略侧必须可观测, 否则该需求不可验证)。
- **完成判据**: 装载成功单测(编译门禁 + 首 tick 无数据不 panic); `rebalance_size` 的 `delta ≤ 0` / `size ≤ 0` 边界单测。

### Task 10: 建仓通道(唯一的 EMA 用途)

- **步骤**: 无平衡价且无持仓 + 金叉 → 市价买入 `现金 × target_ratio`; 日志打印 ATR/spacing/买入量; 有持仓重启 → 跳过建仓; **建仓后不再调用 EMA**。
- **完成判据**: 构造 1m 序列(先跌后涨造金叉, 含预热 1h 数据) → 建仓 1 笔、名义 ≈ 现金×target_ratio; 建仓后 tick 无市价单。

### Task 11: 挂单成型 + 成交后重挂(策略主体)

- **步骤**: 建仓成交后置 `balance_price`/`last_side`, 下一 tick 挂 `[买 平衡价−spacing, 卖 平衡价+spacing]`; 挂单成交 → 更新平衡价(部分成交每笔都更新) → 下一 tick `[cancel_pending, 新买单, 新卖单]`; `rehang_secs` 到期按新 ATR 重挂。
- **完成判据**: 构造"跌到买单价→成交→涨回卖单价→成交"序列(**用例显式 `min_notional=0`**, 否则默认守卫会跳过小额单、断言恒不成立), 断言: 平衡价随成交价更新、新挂单价 = 新平衡价 ± 最新 spacing、**撤单后 ≤2 张**(成交被感知前允许 1 个 tick 的陈旧对侧单在场 —— 设计使然, 不是缺陷)、重挂前必有撤单、**该往返后份额净增**(目标③)。

### Task 12: 守卫、计数与残余失衡

- **步骤**: ①买量 ≤ 现金/价, 卖量 ≤ 持仓; ②名义 < `min_notional` → 跳过 + 日志 + 计数; ③**`spacing% ≤ 0.2%×价格`(双边费保本线) → 跳过 + 日志 + 计数**; ④买量守卫带费: `size ≤ 现金/(P×(1+fee))`(现有写法不含费, 引擎要求 `cash ≥ size·price+fee`); ⑤**残余失衡守卫**: on_fill 记 `fill_size` 与期望量之差、`|ratio − target_ratio|` 超阈值(如 0.2%)→ 日志 + 计数(部分成交/同 bar 双边成交会留下失衡; 回测 `execute_fill` 恒用 `req.size` **永不部分成交**, 该风险只在实盘出现); ⑥ATR 未就绪 → 不下单; ⑦`rehang_secs` 到期无条件重挂。每次挂单日志输出 `spacing%`、距保本线余量、名义额。
- **完成判据**: 小本金回测不产生 <5 USDT 委托; **构造 `spacing% < 0.2%` 的序列(如 atr_mult 设极小)→ 0 挂单 + 跳过计数 > 0**(证明守卫拦得住, 不是永真断言); 计数与日志一致。"挂单被拒/丢失 → 重挂恢复"**在回测里不可构造**(回测挂单只在成交/撤单时消失), 移到 Task 18 demo 验证。

### Task 13: 集成测试

- **文件**: `crates/ricow_strategy/src/builtin_tests.rs`。
- **步骤**: 手工构造 1m 序列(含预热 1h 数据; **用例显式 `min_notional=0`**)跑四场景: 建仓 → 买单成交重挂 → 卖单成交重挂 → ATR 未就绪静止; 断言成交笔数/平衡价序列/挂单数/份额变化方向。
- **完成判据**: 新用例全绿, 既有 339 例无回归。

### Task 14: 文档同步(含命名与契约)

- **文件**: `specs/lua-api.md`(`atr` 第三参 + 订单 `action` + **指标/klines 长度契约** + §九 内置资产清单)、`specs/product.md`(**§十二 标题现为「香农动态网格(shannon_grid)策略规格…」须改名**, 并并列两个策略)、`specs/architecture.md`(§四 内置资产 + 高周期/K 线刷新现状)、**`specs/backtest.md` §六 报告内容**(新增基准行与字段 —— 宪法点名 converge 要同步 backtest)、`specs/roadmap.md`(**测试基线数字会变**, 且 `:139` 有旧策略名)、`README.md` / `README_zh.md`、**`website/index.html:59,74` + `website/zh.html:59,74`**、**`CONTRIBUTING.md:105`**、`crates/ricow/src/commands/agentkit.rs:98`(生成手册)、`crates/ricow_strategy/src/lib.rs:3`、`specs/changes/023-shannon-etf-accum/*`。
- **步骤**: 逐份更新; **豁免(宪法依据)**: `specs/changes/**`(变更档案)、`specs/research/**`(已定稿不回改)、`specs/testnet.md:172/178`(历史实测记录) 不回改, 在 converge 里注明影响面。
- **完成判据(修正: 原判据与"历史不回改"自相矛盾, 必然失败)**: `grep -rn "shannon_grid\|shannon_atr_grid\|香农动态网格" --include='*.md' --include='*.html' --include='*.rs' --include='*.lua' . | grep -v 'specs/changes/\|specs/research/\|specs/testnet.md'` **无命中**; 文档与代码一致。

---

## 七、阶段 C: 模拟盘/实盘通道(本分支内做)

### Task 15: K 线通道接线

- **目标**: DryRun/实盘下 `ctx:klines`(1m)、`ctx:ema`、`ctx:atr(pair, n, "1h")` 可用。
- **文件**: `crates/ricow_engine/src/command.rs`、`crates/ricow_strategy/src/context.rs`。
- **步骤**: ①两循环各加 15s 刷新任务: `run_live` 照搬既有范式(`:608-636`, `FUNDING_POLL_SECS` + `MissedTickBehavior::Delay`); **`run_dry_run`(`:340-400`)目前 select 只有 stop/stream 两臂且仅在有 stop 时 select, 需重构加 interval 臂**。②拉 1m `limit=2000`(分页) → `ctx.update_klines` → Task 1 重采样 → `ctx.set_tf_klines`; 失败只 warn + 计数, 不中断主循环; 间隔与 limit 从 params 读(默认 15s/2000)。③策略侧收线判定 = `ctx:klines` 末根 `ts` 变化(回测/实盘语义统一); **on_fill 触发的重挂不受收线门限制**。
- **完成判据**: Dry Run ≥30 分钟: 日志有"K 线刷新 N 次 / 末根时间推进", ATR 就绪后按计划挂单; 人为断网 30s → 主循环不中断, 恢复后自愈。

### Task 16: 重启续接

- **文件**: `crates/ricow_engine/src/command.rs`、`crates/ricow_strategy/src/db.rs`(复用 `recent_fills`)、策略 `on_init`。
- **步骤**: ①**只在实盘/demo 启动路径**查本策略最后一次成交(`recent_fills(strategy_name, 1)`)并注入 `resume_balance_price`; 查询失败只 warn。**DryRun 不注入** —— DryRun 每次启动都是全新空仓(`context.rs:440` 空 `virtual_positions`, 无持仓恢复), 注入会让策略"挂出卖单却无币可卖"(现货拒单), 判据不自洽。②`on_init`: 有值 → 设平衡价, 首 tick 直接挂双边单(不等金叉); 无值且 `position_size > 0` → 日志"未取得平衡价, 网格待机", 不猜价格; 无值且无持仓 → 走建仓。③单测覆盖三种情形。
- **完成判据**: 续接行为**在 Task 18 demo 验证**(停→启, 有持仓时日志显示恢复平衡价并重挂); DryRun 侧只验"不注入 `resume_*` 时行为不变"。

### Task 17: Dry Run 观察与部署

- **步骤**: ①写 `strategies/shannon_etf_accum.toml`(`type = "shannon_etf_accum"`、`pair = "QQQBUSDT"`、params 10000 本金 + 与 Task 7 同一组默认值、`market = "spot"`、`enabled = true`、`live_enabled = false`); ②`ricow run shannon_etf_accum` 观察, 留 `logs/`、`ricow info/fills` 证据; ③门禁口径: `min_dry_run_hours`(默认 24)仅在上实盘时需累计满。
- **完成判据**: 观察到"金叉建仓 → 挂 2 张 → 插针成交 → 撤余单重挂"至少一个完整循环; 日志无报错; `ricow.db` 有成交。

### Task 18: demo 模拟盘真实调用验证

- **前提**: `ricow.toml` 的 `[exchange] demo_key/demo_secret` 已配置(缺失即拒绝启动, 不回落)。
- **步骤**: ①`ricow run shannon_etf_accum --demo`; 联调先用 `ema_fast=2/ema_slow=3`、`min_notional=1`(让金叉/挂单/成交/撤单重挂在几分钟内出现; 配置调整, 非改代码); ②逐项核对: 建仓成交落库 → 交易所侧 2 张限价单可见 → 触发成交 → 撤余单重挂(始终 ≤2 张) → **人为撤掉交易所侧一张挂单 → 下一重挂周期自愈恢复 2 张** → 停机清理(本实例挂单全撤 + 残留复查如实输出); ③换回默认参数再跑一段; ④记录写入 `specs/testnet.md`。
- **完成判据**: 交易所侧挂单/持仓与日志一致、零残留; 结论文档化。**demo 验证的是链路, 不是收益**。

---

## 八、验收(全部阶段完成时)

1. `cargo test --workspace --locked` 全绿(**基线 339 passed / 12 ignored 已实跑确认**; 新增用例后数字会上升, 以实跑为准并同步 `specs/roadmap.md`)。
2. **9 组真实数据回测**(`QQQBUSDT`/`SPYBUSDT`/`PAXGUSDT` × `atr_mult` **2/3/4.5**, `--days 90 --cash 10000`, 数据不足按实际窗口), 每组给出: 实际窗口(首末 bar 时间 + 根数)、成交笔数、**跳过次数(与成交笔数的比)**、`rejected_count`、策略总价值变化% / 最大回撤% / 份额变化% / 期末现金 **vs** 满仓(以建仓价买入) / **敞口对齐基准(半仓)**。
   通过判据: 成交笔数 > 0; `rejected_count = 0`; 单笔再平衡名义 ≥ `minNotional`(5 USDT) 且 spacing > 0.2%×价格(两项余量入表); 期末持仓市值 ≤ 本金可负担; 除建仓外无市价单; 日志有重挂次数、跳过计数与「收割/费用」分解。**跳过 >80% 的组不作结论依据**(单列记录)。
3. **目标逐条对照(用上表数字, 不与机制宣称混用)**: ①(策略 − 敞口对齐基准) 的符号与量级 + 与 `G(0.5) vs G(1)` 期望的对照; ②回撤比 ≈ `target_ratio`(机械结果, 如实说明); ③份额变化% 并**扣除机械成分**(`Q ≈ E/(2P)` 由价格变动带来的增持)。不许用"通常/一般"含糊带过, 也不许把 ① 说成"跑赢满仓"(默认参数下期望为负, 见 §一)。
4. **选做**: ①QQQB 上 `target_ratio ∈ {0.5,0.6,0.7,0.8}` 对照(回答"能否跑赢满仓"及回撤代价); ②一次 2000 USDT(QQQB×mult2)对照, 记录"本金过小 → 无法成交"; ③`atr_mult=1.5` 一档留档对照, 用实测跳过率复现"为何弃用"。
5. **常识自检**: 成交笔数应与"价格穿越 ±spacing 次数"同量级; 期末持仓不得"远超本金可负担"; 若出现, 先查挂单撮合与资金校验再谈策略。
6. **Dry Run 闭环**(Task 17)与 **demo 链路零残留 + 挂单自愈**(Task 18)各一份实测记录。
7. **窗口诚实性**: 3 个月样本(QQQB 79 天 / SPYB 72 天不同窗口)只够冒烟与方向对照, 不足以论证长期有效性; 横向比较必须注明窗口差异, 不下"长期"结论。

---

## 九、涉及文件

**新增**: `strategies/builtin/shannon_etf_accum.lua`、`crates/ricow_strategy/src/multiframe.rs`(含单测)、`specs/changes/023-shannon-etf-accum/*`、`strategies/shannon_etf_accum.toml`(运行时私有, git 忽略)。
**改名**: `strategies/builtin/shannon_grid.lua` → `shannon_rebalance.lua`。
**修改**: `crates/ricow_core/src/types.rs`; `crates/ricow_strategy/src/{lib.rs, context.rs, backtest.rs, lua.rs, db.rs, pnl.rs, metrics.rs, risk.rs, builtin_tests.rs}`; `crates/ricow_engine/src/{command.rs, backtest_runner.rs}`; `crates/ricow/src/commands/{mod.rs, backtest.rs, run.rs, agentkit.rs}`; `crates/ricow/src/ai/tools.rs`; `specs/{lua-api.md, product.md, architecture.md, backtest.md, roadmap.md, testnet.md}`; `README.md` / `README_zh.md`; `website/{index.html, zh.html}`; `CONTRIBUTING.md`。

**提交纪律**: 每个任务只跑测试, **git commit 待用户明确说"提交"**。

---

## 十、风险与注意事项

1. **回测窗口短**: 最大 79 天(各标的不同窗口), 只作冒烟与方向对照; 横向比较注明窗口差异。
2. **目标① 默认参数下期望为负**(数学判据 `μ < 0.75σ²`); 目标③ 与 ① 同源且被价格方向污染(机械增持) —— 结论只认 (策略 − 敞口对齐基准) 的实测数字。
3. **收割项量级小**(季度 0.003~0.25% 权益), **手续费可忽略**(0.003~0.03% 权益/季度); 决定 P&L 的是敞口与行情。别把"看起来在收割"当成在赚钱。
4. **保本与最小名义是两道硬门**: `spacing > 0.2%×价格`(新矩阵最薄是 SPYB×2 = 0.32%, 1.6× 保本线); 单笔名义 ≥ `minNotional` 5 USDT → 本金须满足 `权益 ≥ 20×价格/spacing`(**中位 ATR 口径, 低 ATR 小时仍会跳过**) — 验收必须逐组输出跳过占比。
5. **ATR 口径**: 7×24(本项目默认)与美股时段(含隔夜跳空)差 ~1.8×; 报告必须标注; 与真美股口径对齐需 `atr_mult ×1.75`。
6. **1m × 13 万 bar 的 Lua 开销**: Task 4 修复后每次 ≤100 根克隆; 若过慢先查是否还有别处全量克隆, 不要用"加大周期"掩盖。
7. **单一通道的脆弱面**: 挂单被拒/丢失 → 网格停摆(无信号兜底), 靠定期重挂自愈; 日志必须记录拒单、跳过与重挂。
8. **残余失衡**: 部分成交 / 同 bar 双边成交会留下小量失衡(实盘), 只由 Task 12 的日志+计数观测, 不主动补单。
9. **命名/破坏性**: `type = "shannon_grid"` 外部含义消失(需改 `shannon_rebalance`); README / website / CONTRIBUTING 同步; 0.x 升 minor。
10. **Dry Run 与实盘的 K 线来源不同**: DryRun/实盘靠 15s 拉取(建仓 EMA 最多晚 ~15-20s 感知收线); 挂单轨不受影响(订单真实挂在交易所)。
11. **DryRun 无持仓恢复**: 续接只能在实盘/demo 验证(见 Task 16)。
12. **`on_fill` 无返回通道**: 重挂落在下一个 tick(回测 = 下一根 1m bar, 实盘 = 下一个盘口 tick); 本计划不改这条设计。
13. **`OrderRequest.action` 改动面**: 7 处字面量构造 + 3 处 `place_order` 实现; 组合回测路径不下挂单, 不受影响。撤单须绕过 `risk_reject`/`prepare_live_order`(Task 5)。
14. **限频提示**: 本策略每 tick 最多 3 单(撤单 + 买 + 卖), 用户若配 `risk.max_orders_per_sec` 不要低于 3(默认 100 无碍)。

---

## 十一、已定案 / 未定项

**已定案**: 单通道(EMA 仅建仓); 挂单量 = 精确回 `target_ratio`; 命名(新 `shannon_etf_accum` / 旧改 `shannon_rebalance`); 阶段 A/B/C 全在本分支; 回测窗口 3 个月(不足按实际); 基准 = 以建仓成交价满仓持有(+ 敞口对齐基准); 标的 = `QQQBUSDT` + `SPYBUSDT` + `PAXGUSDT`; 本金 = 10000 USDT; ATR 口径 = 7×24 标的自身 1h; **ATR 乘数 = 2 / 3 / 4.5**。

**未定项**: 无。版本/发布由用户决定, **不在本次范围**(用户 2026-09-17: "不用管版本问题, 专注于编写策略")。
