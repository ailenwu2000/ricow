# 竞品架构机制对比：数据供给 + 订单事件（2026-09-24）

> 目的：对照 ricow 五目标（①运行环境+API ②逻辑在策略 ③新增策略不改代码 ④架构清晰简洁 ⑤架构是主要约束），
> 比较主流框架的「数据供给机制」与「订单事件可见性」，找出可改进点。
> 方法：Freqtrade + Nautilus Trader 源码下载到 /tmp 逐文件比对；QuantConnect / Backtrader 参考官方文档。
> 与 competitor-gap-2026-09.md 的区别：那篇是「功能面 × 竞品」差距，本文是「架构机制」同构性结论。

---

## 一、数据供给机制（策略如何声明/获取数据）

| 框架 | 声明方式 | 预热长度 | 读数据 API |
|:--|:--|:--|:--|
| QuantConnect LEAN | `AddEquity(symbol, resolution)`（Initialize 里订阅） | — | `OnData(slice)` / `slice[symbol]` |
| Freqtrade | `timeframe` 类属性 + `informative_pairs()` 返回 `[(pair, tf)]` | `startup_candle_count`（单一数字） | `dp.get_pair_dataframe(pair, tf)` |
| Nautilus Trader | `request_bars()`（历史）/ `subscribe_bars()`（实时） | — | `on_bar()` / `on_historical_data()` |
| Backtrader | 外部 `cerebro.adddata()` 注入 | — | `self.datas` / `self.data0` |
| **ricow** | `need_klines(role, tf, min_bars)`（on_init 里声明） | `min_bars`（每 tf 各自根数） | `ctx:atr_tf(pair, tf, period)` 等 |

**结论**：五家全部是「策略声明数据需求 → 引擎照单供给 → 策略读」，无一让策略直拉历史 K 线。
LEAN 文档原文 "you can't access price data beyond the Time Frontier"（流式分析系统，策略无法越过时间前沿）。
ricow 的 `need_klines` 与 `AddEquity` / `informative_pairs` 同构，方向正确；差异仅在预热长度的表达（单一数字 vs 每 tf 根数，ricow 更精确）。

## 二、订单事件（拒单/撤单对策略可见性）

| 框架 | 订单回调 | 拒单/撤单可见 |
|:--|:--|:--|
| Freqtrade | 无策略订单回调（订单 expired/rejected 只记日志 + trade 状态查询，freqtradebot.py:1150） | ❌ 不可见 |
| Nautilus Trader | 16 个事件：on_order_submitted / rejected / canceled / filled / accepted / expired / ... | ✅ 完整 |
| **ricow**（2026-09-24 接线） | `on_fill`（成交）+ `on_order_update`（拒单/撤单/过期） | ✅ 核心覆盖 |

**结论**：Freqtrade 的订单终态（expired/rejected）只 `logger.warning` + `return False`，策略无回调——这正是 ricow 审计 #3 的同款坑（挂单被拒 → 策略不知 → 停摆）。ricow 接线 `on_order_update` 后已超过 Freqtrade。Nautilus 的 16 个细粒度事件是机构级 HFT 粒度，ricow 的 YAGNI 原则下不需要。

## 三、架构分层

| 框架 | 技术栈 | 分层 |
|:--|:--|:--|
| Freqtrade | Python 单层 | 策略(IStrategy) + 引擎(freqtradebot) + 数据(dataprovider) |
| Nautilus Trader | Rust 内核(crates) + Python 策略层(python) | 双层（重） |
| **ricow** | Rust 引擎 + Lua 沙箱 | 引擎 + 策略（比 Freqtrade 硬，比 Nautilus 轻） |

## 四、对照五目标的结论

1. 运行环境+API：✅ 五家一致——引擎供环境 + 数据 API，策略纯逻辑。
2. 逻辑在策略：✅ 五家一致。
3. 新增策略不改代码：✅ 五家一致——策略是独立文件/类。
4. 架构清晰简洁：ricow 比 Freqtrade 硬（Rust+Lua vs Python 单层），比 Nautilus 轻（无 Rust+Python 双层）。
5. 架构是主要约束：ricow 独有 `architecture_guard.rs` 机械锁（引擎/CLI 零策略参数名，注入即红），竞品无此机制。

## 五、可改进点（状态）

1. **订单事件可见性**（已落实，2026-09-24）：ricow 原缺拒单/撤单回传（审计 #3），已接线 `on_order_update`，超过 Freqtrade。
2. **预热语义文档**（已落实）：`need_klines` 的 `min_bars` 是 tf 自身根数，已补"防 24 根 vs 24 小时陷阱"说明（lua-api.md）。
3. **低成本小功能**（记录，暂不做）：Freqtrade 的 `version()` / `plot_annotations()`（策略自报版本号、可视化标注），符合"少而精"前提下再考虑。
4. **事件驱动决策**（已落实，2026-09-26 / 034）：`on_fill` 可返回订单 + 引擎"成交→决策"有界递归闭环（深度 8），成交后零节流重挂，对齐 NautilusTrader 事件驱动语义；现有策略零改动（回测逐分不变）。竞品架构优势全景清单与后续优先级（启动对账 > 回测滑点/部分成交 > 实盘心跳兜底）见 `specs/changes/034-event-driven-onfill/plan.md` §五。
