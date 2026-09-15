# 加密货币永续合约回测开源项目调研 (2026-09)

> 调研日期: 2026-09-05。方法: GitHub REST API 查证 star/push 时间 + raw 源码/官方文档直抓 + 小仓库浅克隆本地 grep 核证。
> 触发背景: ricow 回测引擎在币安 demo 实测中发现两个模型失真点 (exchangeInfo maintMarginPercent=2.5% 非首档 /
> hedge 双向实际每侧独立强平), 调研业界成熟项目如何建模, 回答"参考 vs 自研"。
> 报告原文为子代理输出, 本文件为落盘版 (来源 URL 均保留)。

## 对比总表 (star 为 2026-09-05 GitHub API 精确值)

| 项目 | GitHub / Star | 最后 push | 许可证 | USDT-M 合约回测 | 强平建模 | 资金费结算 | MMR 数据源 | demo/测试网 |
|---|---|---|---|---|---|---|---|---|
| freqtrade | [freqtrade/freqtrade](https://github.com/freqtrade/freqtrade) 54,022 | 2026-09-04 | GPL-3.0 | 部分(期货撮合简化) | ✗ 无引擎(靠止损) | ✓ 历史费率结算 | 无分档 | dry-run 纸面 |
| vn.py | [vnpy/vnpy](https://github.com/vnpy/vnpy) 45,117 | 2026-09-01 | MIT | ✗(国内期货 CTA; 无币安网关) | ✗ | ✗ | ✗ | 国内仿真 |
| ccxt | [ccxt/ccxt](https://github.com/ccxt/ccxt) 43,870 | 2026-09-04 | MIT | ccxt 本身无回测 | ✗ | ✗ | **✓ 解析币安分档档位** (唯一) | **✓ demo-fapi 内置** |
| NautilusTrader | [nautechsystems/nautilus_trader](https://github.com/nautechsystems/nautilus_trader) 28,376 | 2026-09-05 | LGPL-3.0 | ✓ (Rust 撮合核心, MARGIN 账户) | 部分(可配置清算引擎, 默认关, 触发比 0.9) | ✓ FundingSettlement | instrument 级平铺, 无档位 | ✓ 环境枚举含 Demo |
| backtrader | [mementum/backtrader](https://github.com/mementum/backtrader) 23,137 | 2024-08 停滞 | GPL-3.0 | 部分(通用期货模式, 非加密原生) | ✗ | ✗ | ✗ | ✗ |
| QuantConnect LEAN | [QuantConnect/Lean](https://github.com/QuantConnect/Lean) 21,483 | 2026-09-05 | Apache-2.0 | ✓ (CryptoFuture/Binance 期货建模) | ✗ 回测无自动强平 | ✓ margin_interest 数据文件 | ✗ 维持=初始=名义/杠杆 | ✗ (0 处 testnet) |
| Hummingbot | [hummingbot/hummingbot](https://github.com/hummingbot/hummingbot) 19,807 | 2026-09-04 | Apache-2.0 | 部分(v2 回测 candle 级仿真) | 实盘侧交易所强平+预警; 回测无 | 实盘 8h; 回测无 | ✗ | ✓ binance_perpetual_testnet |
| backtesting.py | [kernc/backtesting.py](https://github.com/kernc/backtesting.py) 8,931 | 2026-08 | AGPL-3.0 | ✗ (纯 equity 曲线) | ✗ | ✗ | ✗ | ✗ |
| vectorbt | [polakowo/vectorbt](https://github.com/polakowo/vectorbt) 8,991 | 2026-08 | 自定义 (非 OSI) | ✗ (矢量分析, 无账户概念) | ✗ | ✗ | ✗ | ✗ |
| Jesse | [jesse-ai/jesse](https://github.com/jesse-ai/jesse) 8,417 | 2026-09-04 | MIT | ✓ (回测 isolated 模式) | 部分(公式 ✓, 无档位) | **✗ 回测恒为 0** | ✗ 0.4% 硬编码 | testnet 数据驱动 |
| OctoBot | [Drakkar-Software/OctoBot](https://github.com/Drakkar-Software/OctoBot) 6,515 | 2026-09-04 | GPL-3.0 (库 LGPL-3.0) | ✓ (官方仅 Isolated) | 部分(公式同 ricow 同构, 最值得抄) | ✓ 模拟推进资金费 | 平铺 0.01 默认 | 未查证 |
| tensortrade | [tensortrade-org/tensortrade](https://github.com/tensortrade-org/tensortrade) 7,101 | 2026-02 (实质停更) | Apache-2.0 | ✗ (RL 抽象, 无撮合) | ✗ | ✗ | ✗ | ✗ |
| zvt | [zvtvz/zvt](https://github.com/zvtvz/zvt) 4,294 | 2026-07 | MIT | ✗ (A股/期货投研) | ✗ | ✗ | ✗ | ✗ |

## 逐项目要点 (强平/MMR/资金费建模细节)

### freqtrade (2026 已支持期货, 非现货-only)
- 官方 [index.md](https://github.com/freqtrade/freqtrade/blob/develop/docs/index.md) 支持期货的交易所: Binance/Bybit/OKX/Kraken/Gate/Bitget/Hyperliquid; 短仓需 `trading_mode: "futures"`。
- 资金费: 回测用历史费率结算, 缺失有 `futures_funding_rate` 兜底; 文档警告"设非 0 值导致回测失真" ([leverage.md L126-131](https://github.com/freqtrade/freqtrade/blob/develop/docs/leverage.md))。
- 强平: **无自动强平撮合**, 自称"账户唯一使用者"按此算强平价; cross 模式"多仓互相影响在 dry-run/backtest 未完全仿真" ([leverage.md](https://github.com/freqtrade/freqtrade/blob/develop/docs/leverage.md))。无 MMR 分档。
- demo: 仅 dry-run 纸面。

### vn.py
- 官方 [README](https://github.com/vnpy/vnpy/blob/master/README.md) 网关全为国内通道 (hts/xtp/tora/IB…), **无币安/crypto 网关**; ctabacktester 面向国内期货 CTA, 无资金费/分档 MMR/逐仓。USDT-M 走社区第三方网关, 官方回测不覆盖。

### ccxt (对 ricow 两个直接可用能力)
- **币安 demo 内置**: [python/ccxt/binance.py](https://github.com/ccxt/ccxt/blob/master/python/ccxt/binance.py) url 环境含 `test` 与 `demo` (**demo-fapi.binance.com**), `enable_demo_trading()` 切换。
- **币安分档已解析**: `load_leverage_brackets()` 调私有 `/fapi/v1/leverageBracket` 按 symbol 缓存进 `options['leverageBrackets']` ([binance.py](https://github.com/ccxt/ccxt/blob/master/python/ccxt/binance.py)), 统一 `fetch_leverage_tiers()` —— **主流项目里唯一消费币安分档 MMR 数据的代码, 但没有任何项目把它接入强平引擎**。

### NautilusTrader (架构上离 ricow 最近)
- [Accounting](https://nautilustrader.io/docs/latest/concepts/accounting/): `StandardMarginModel` = notional × margin_init/margin_maint (固定比例); `LeveragedMarginModel` = (notional/leverage) × margin_*。**instrument 级平铺, 无 notional 分档**。
- 资金费: `FundingRateUpdate` 数据驱动, `next_funding_ns` 边界触发 `FundingSettlement` ([accounts-and-margin](https://nautilustrader.io/docs/latest/concepts/backtesting/accounts-and-margin))。
- 强平: Rust SimulatedExchange 自动清算引擎 ([liquidation_demo.py](https://github.com/nautechsystems/nautilus_trader/blob/master/examples/backtest/liquidation_demo.py)) — `liquidation_enabled` (默认 False)、`liquidation_trigger_ratio` (默认 0.90, 保证金利用率超阈值清掉该标的全部持仓)。触发比可配置, **非币安档位精确模型**。
- 双向: `OmsType.HEDGING` 每笔成交独立成 Position ([Accounting](https://nautilustrader.io/docs/latest/concepts/accounting/)), 天然按侧。
- Binance 适配器环境枚举 Live/Testnet/**Demo** ([config.rs](https://github.com/nautechsystems/nautilus_trader/blob/master/crates/adapters/binance/src/config.rs))。

### QuantConnect LEAN
- Binance 期货建模为 `CryptoFuture` ([Common/Securities/CryptoFuture/](https://github.com/QuantConnect/Lean/tree/master/Common/Securities/CryptoFuture))。
- 保证金: [CryptoFutureMarginModel.cs](https://github.com/QuantConnect/Lean/blob/master/Common/Securities/CryptoFuture/CryptoFutureMarginModel.cs) initial = |名义|/杠杆, **GetMaintenanceMargin 直接返回 initial (维持=初始)** — 无 0.4% 档位概念。
- 资金费: [BinanceFutureMarginInterestRateModel.cs](https://github.com/QuantConnect/Lean/blob/master/Common/Securities/CryptoFuture/BinanceFutureMarginInterestRateModel.cs) 按 MarginInterestRate 数据结算, 配套 `margin_interest/*.csv`。
- 强平: 自托管回测**无自动强平** (保证金不足直接拒单); 全仓库 0 处 testnet。

### Hummingbot
- 衍生品连接器 22 个; binance_perpetual 资金费 8h 按 income 历史 (`FUNDING_FEE`) + lastFundingRate ([binance_perpetual_derivative.py](https://github.com/hummingbot/hummingbot/blob/master/hummingbot/connector/derivative/binance_perpetual/binance_perpetual_derivative.py)); 强平价实盘侧由交易所返回并告警, 本地无判定; HEDGE key = pair+side。
- 回测: V2 backtesting 对 executor 蜡烛级 triple-barrier 仿真, **grep 零处 funding/margin/liquidation** → 资金费/强平不进回测。
- 测试网: `binance_perpetual_testnet` (testnet.binancefuture.com), 无 demo-fapi。

### Jesse (回测 isolated 公式最直白)
- [Position.py L216-244](https://github.com/jesse-ai/jesse/blob/master/jesse/models/Position.py) (注释注明取自 Bybit 文档): long `entry×(1−1/lev+0.004)`; short `entry×(1+1/lev−0.004)`。**0.004 硬编码维护保证金近似** (≈币安首档 0.4%), 无分档; 另有 bankruptcy_price。
- 回测仅 isolated; cross 回测返回 NaN; live 用交易所每分钟推送的 liquidation_price。
- 资金费: **回测不结算** — [官方 API 文档](https://docs.jesse.trade/docs/strategies/api.html) 明示 funding_rate 回测恒 0。
- 测试网: Binance USDT-M testnet 数据导入驱动; 无 demo-fapi。

### OctoBot (对 ricow 最有直接公式参考)
- 官方文档确认期货仅支持 **Isolated** ([Futures Trading with OctoBot](https://www.octobot.cloud/en/guides/octobot-usage/futures-trading-with-octobot))。
- **逐仓强平价公式** ([linear_position.py L58-83](https://github.com/Drakkar-Software/OctoBot-Trading/blob/master/octobot_trading/personal_data/positions/types/linear_position.py)): `maintenance_margin = size×entry×mmr`; long `liq = entry×(1−1/lev+mmr)`; short `liq = entry×(1+1/lev−mmr)`。**MMR 是变量** (默认常量 0.01, 随交易所数据刷新) — 与 Jesse 同构但 MMR 可配。
- 强平执行: `_check_for_liquidation()` → `LiquidatePositionState`, 损失 = −initial_margin ([position.py](https://github.com/Drakkar-Software/OctoBot-Trading/blob/master/octobot_trading/personal_data/positions/position.py)); 模拟器(回测)走同一状态机。
- 资金费: FundingUpdaterSimulator 在**模拟/回测中同样推进** ([funding_updater_simulator.py](https://github.com/Drakkar-Software/OctoBot-Trading/blob/master/octobot_trading/exchange_data/funding/channel/funding_updater_simulator.py))。
- 双向: position id 含两侧 (hedge 两仓并存)。

### 其余
- backtrader: 23k★ 2024-08 后停滞; 通用期货保证金/乘数可配, 无币安适配、无资金费/强平。
- vectorbt: 8,991★; 矢量回测/组合分析, 无账户-保证金概念; 许可证非 OSI, PRO 闭源。
- backtesting.py: 8,931★; 轻量 equity 曲线, 空头=反向持仓, 无保证金语义; **AGPL 传染注意**。
- tensortrade: 7,101★; RL 抽象, 无交易所撮合; 实质停更未归档。
- zvt: 4,294★; A股/期货投研, 无永续建模。

## 三个痛点问题的业界回答

1. **分档 MMR (leverageBracket)**: 13 个主流项目**无一**在回测强平中按 notional 档位建模。粗到细: LEAN (维持=初始) → Nautilus (instrument 固定比例) → OctoBot (平铺 1%) → Jesse (硬编码 0.004 近似首档) → Hummingbot/其余 (回测不管)。**唯一消费币安 leverageBracket 数据的是 ccxt** (`fetch_leverage_tiers()`)。印证 ricow 实测: exchangeInfo 的 maintMarginPercent 是误导值, 正确数据源是私有 leverageBracket 端点。
2. **isolated 强平判定**: 开源标准形态 = `liq = entry×(1 ∓ 1/lev ± mmr)` (Jesse/OctoBot 同构; LEAN/Nautilus 走"保证金利用率"路线)。"资金费计入后强平价随 margin 漂移"主流回测均未做 (CoinDCX 描述的是真实交易所行为) — **ricow 已实现的 8h 资金费结算领先这些开源项目**。
3. **hedge 每侧独立**: nautilus (HEDGING 每侧独立 Position)、OctoBot-Trading (id 含 side)、Hummingbot (HEDGE key=pair+side) 在**账户/实盘层**都按侧管理; **回测撮合层无成熟实现**。ricow 实测方向与交易所行为一致, 无可抄的成熟回测实现。

## demo-fapi 支持结论
- 直接支持 (接口层): ccxt (demo-fapi 环境内置), NautilusTrader (Binance 适配器环境枚举含 Demo)。
- 测试网 (testnet.binancefuture.com): ccxt / Jesse / Hummingbot。
- 纸面/干跑: freqtrade dry-run、Jesse paper。
- **没有任何回测引擎接 demo-fapi 做回测** — demo 目前只是接口层能力; ricow 的 demo 真实联调 (下单/持仓/强平观察) 在业界属超前实践。

## Top5 参考价值排序 (对 ricow)
1. **OctoBot-Trading** — 唯一"逐仓强平价公式 (MMR 可配) + 维持保证金 + 强平状态机 (损失=初始保证金) + 资金费模拟结算 + hedge 双侧"全链开源且公式显式, 纯 Python 易读。
2. **NautilusTrader** — 与 ricow 最同构的架构标杆 (Rust 核心撮合/MARGIN 账户/资金费事件/可配置清算引擎/HEDGING); 学分层与事件语义, 不学"触发比 0.9"近似。
3. **ccxt binance.py** — 直接补缺口: 币安分档档位表获取与按 symbol 缓存结构 (load_leverage_brackets / fetch_leverage_tiers), 照抄数据形状即可。
4. **QuantConnect LEAN** — 交易所规则可插拔模型抽象 (保证金模型 / IMarginInterestRateModel + 数据文件驱动资金费); "维持=初始"的简化别学。
5. **Jesse** — isolated 公式最简实现 + 回测/实盘差异处理; 同时是"回测漏资金费失真"的反面教材。

## 未查证/局限
OctoBot demo 支持、vnpy_ctabacktester 内部强平细节、Nautilus Binance Demo 实际端点域名、zvt 加密货币数据源 — 未逐一验证; backtrader 期货文档页未抓取。web 搜索后端部分故障, 已用源码级证据替代。

## 附: ricow 差距定位与决策记录 (2026-09-05 架构师拍板, 用户授权)
调研核心结论: **精确的"币安档位 MMR + 逐仓强平 + hedge 独立 + 资金费"全链回测在业界空白**, 各项目在 1-3 个维度简化。
结合 ricow 实测证据与"保持回测简单"原则, 拍板:

- **D1 hedge 按侧独立强平 → 不改引擎**。原因: ①one-way 占绝对多数, 单仓时现组合判定恒等于标准公式; ②hedge 同对双向策略罕见 (币安扣双份 IM, 资金效率低); ③业界回测撮合层无成熟实现 (账户层按侧是实盘管理, 非回测语义); ④per-side 钱包重构成本高、收益仅覆盖小众场景。落点: specs/backtest.md 声明近似边界, hedge 策略回测建议按更严口径 (等效单侧名义) 压力验证。
- **D2 MMR 数据源 → 修 (bug 修复非精度取舍)**。exchangeInfo 2.5% 实为深档值, 导致杠杆 >1/MMR 档仓位"开仓即虚爆"。最小修复: 内置 symbol→首档 MMR 表 (BTC/ETH 0.4%, DASH/XMR 1.5%, 数据源 = 签名 leverageBracket 实测) + 未知回落首档量级 + 保留 --mmr-pct 覆盖; 签名环境用 leverageBracket 校准。
- **D3 强平公式 → 不动**。单仓时现 f(p) 线性穿越判定与标准式 `entry×(1∓1/lev±mmr)` 恒等 (代码注释已声明), 喂对 MMR (D2) 即自动对齐。
- **D4 调研报告 → 本文件落盘 specs/research/backtest-engines-2026.md**。
