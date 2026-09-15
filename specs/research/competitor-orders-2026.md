# 竞品调研报告：主流交易所与量化平台的"高级订单/策略功能"(2026-08)

> 调研方式：直接抓取官方帮助中心/产品文档原文(curl + 去标签,共抓取 72 个页面)。所有结论标注来源 URL;抓不到的标注"未查证"。
> 调研时间：2026-08-15。来源 URL 见各节末尾。
> 环境限制:Binance 官网 202 反爬;Cryptohopper Cloudflare 403。相关细节标"未查证"。

## 一、对比总表(平台 × 功能 × 参数)

| 平台 | 回调买入/卖出 | 时间分批 TWAP | 阶梯单 | 冰山委托 | 网格 | DCA/加仓 | 追踪止盈/止损 |
|---|---|---|---|---|---|---|---|
| 币安 | 有追踪止损(现货 API trailingDelta 已验证);"回调单"专名未查证 | 算法单(TWAP)存在但反爬 → 参数未查证 | 未查证 | 有(API icebergQty,须 GTC) | 无机器人 | 无官方 DCA bot | 有(trailingDelta) |
| OKX | 有"移动止盈止损"(=回调机制):trail variance(比例或绝对值)+ 激活价 | 有:价格浮动比例、价格上限、间隔、平均数量、总数量 | 无(docs-v5 算法单类型无 ladder) | 有:价格浮动比例、价格上限、平均数量、总数量 | 有:上下界、网格数、等差/等比、投资额、上下移、TP/SL | 有:price steps、TP target、初始/加仓保证金、最大加仓次数、金额倍数、步长倍数 | 有(move_order_stop) |
| Bybit | 有(Trailing Stop:距离或百分比回调 + 可选激活价;现货可买卖、合约仅平仓) | 有:运行 5m-24h、频率 5s-120s(默认 30s)、随机 ±20%、触发/停止价、账户 20 个、每对 10 个 | 有(Scaled Order):2-100 子单、价格区间、Flat/Increasing/Decreasing/Custom、单子占比 0.01%-100% | 有:单笔数量或拆分数量、4 种追价偏好、价格上限、账户 10 个、每标的 1 个、7 天 | 无 | 无官方 DCA bot | 有 |
| 3Commas | 有(Trailing Buy):激活价→跟踪下跌→回落设定值市价买入 | 无独立 TWAP | 有(Step Sell/Split Targets):最多 6 档 | 无 | 有:Long/Neutral/Short/Hedge、区间/无限、Profit per GRID、TP/SL/Trailing、Pump Protection | 有:DCA Mode、Order Size Multiplier、Max DCA Orders、Price Deviation、Price Deviation Multiplier | 有(激活价%+追踪%,TP/SL 均可) |
| Pionex | 有(DCA Martingale - Trailing Mode,App 内):Price deviation + Max Rebound Rate(跌到刻度→等局部低点→反弹 X% 才买入;卖出侧同理);限 App、投资 $2k-$10k | 有:间隔固定 10s/30s/1min/5min、限次(≥2 次)或永久模式 | 无独立阶梯 bot(DCA 等比加仓承担) | 无 | 有:上下限、网格数、等差/等比、投资额、触发价、止盈/止损价、滑点控制、Trailing up | 有(Simple/DIY/Composite/Trailing 四模式):价格偏差、份额、加仓倍数 | 有(追踪止盈:触发卖出价+Max Drawdown%,上限 50%) |
| Gainium | 无独立"回调"策略名;退出侧 Trailing TP/SL(偏差%,建议 TP 目标的 10-25%) | 无独立 TWAP | 买入阶梯 = Scaled DCA(百分比/ATR/ADR 基准+Step Scale/Volume Scale);卖出 = 多 TP 目标 | 无(Smart Orders 近似冰山思想) | 有:网格步长、算术/几何、反向网格、Trailing up/down、Smart Orders | 有(Scaled/Technical Indicators/Custom 三类) | 有(Trailing TP/SL) |
| Cryptohopper | 未查证 | 未查证 | 未查证 | 未查证 | 未查证 | 未查证 | 未查证 |

## 二、交易所要点

**币安**:现货官方 API 文档(GitHub binance-spot-api-docs/rest-api.md 原文)验证 ①冰山委托 icebergQty(LIMIT/STOP_LOSS_LIMIT/TAKE_PROFIT_LIMIT,必须 GTC,exchangeInfo 有 icebergAllowed:true)②追踪止损 trailingDelta(可与 stopPrice 组合)。算法单面板(TWAP/VP)在 developers.binance.com,本环境 HTTP 202 拦截 → 未查证。

**OKX**:①策略委托类型文档(更新 2026-08-11):止损单、移动止盈止损、触发单——移动止盈止损即回调机制,官方示例"价格跌到 20,000 后反弹到 21,000(20,000×1.05)触发市价买入";②TWAP 机器人(更新 2026-08-05):价格浮动比例/价格上限/间隔/平均数量/总数量,子单按 IOC,随机系数 0.5~1;③冰山机器人:每笔为平均数量 50%-100%,偏离超 2×浮动比例撤旧挂新;④docs-v5 确认算法单类型 = TP/SL、Trigger、Trailing、Iceberg、TWAP、Arbitrage、Recurring buy,无 ladder;限额 TP/SL 100/标的、Trigger 500、Trailing 50、Iceberg 100、TWAP 20;⑤Spot Grid(2026-08-11 更新)与 Spot DCA Martingale(2026-08-05 更新)参数如上表。

**Bybit**(帮助中心最完整,5 大算法单全有):Scaled Order(更新 2026-04-24,2-100 子单、4 种分布、0.01%-100% 占比约束);TWAP(2026-04-16,5s-120s、±20% 随机、触发/停止价、账户 20/每对 10);Iceberg(2026-04-16,4 种追价偏好、价格上限、7 天);Chase Limit 追价单(2026-04-19,距离 0.01%-10%);POV(2026-05-19,仅合约,参与率 1%-100%,深度参考 1/3/5/10 档);Trailing Stop 现货(2026-02-05,距离或百分比+激活价)。无官方网格/DCA 机器人,无独立"回调单"名称。

## 三、商业平台要点

**3Commas**(SmartTrade 已并入 Trading Terminal):Trailing Buy(回调买入)、Step Sell/Split Targets(阶梯卖出,官方示例"一半@$10,000、25%@$11,000、剩余@$11,500")、同时 TP+SL 且均可追踪(激活价%+追踪%)、SmartCover、最多 6 个 TP 目标;DCA Bot 6 大参数(DCA Mode/Averaging Method/Max DCA Orders/Price Deviation/Order Size Multiplier/Price Deviation Multiplier)+ 5 种启动条件 + TP/SL/Trailing/Breakeven 退出;Grid Bot 四种方向类型 + Pump Protection。

**Pionex**:TWAP Bot(四档固定间隔+限次/永久模式);Smart Trade Bot(Limit 标准:买入价/数量+固定价或追踪止盈[触发价+Max Drawdown%,上限 50%]+止损价,Market 速度:总投资+追踪%);Grid Bot(区间/网格数/等差等比/触发价/止盈止损价/滑点控制/Trailing up/AI 2.0 策略);DCA Martingale - Trailing Mode = 回调买卖(Price scale + Max Rebound Rate,从局部低点反弹 X% 才买入/冲高回落 X% 才卖出;限 App、投资上限 $2k-$10k);DIY 模式逐笔自定义价格偏差/份额/追踪率。无独立阶梯 bot。

**Gainium**:DCA Mode 三类(Scaled=买入阶梯[百分比/ATR/ADR 基准、Step Scale、Factor、Volume Scale]、Technical Indicators、Custom);Grid(步长建议 0.5-1%、算术/几何、反向、Smart Orders 只挂部分限价单);Trailing TP/SL;Trading Terminal 三种 deal(Simple/Smart/Import);Combo Bot Minigrids(2026 主打:DCA 内嵌小网格,参数 DCA Orders/Orders Step%/Minigrid Levels/Active Minigrids/Step Scale/Volume Scale)。无独立"回调买入"命名。

**Cryptohopper**:全部未查证(DNS 失败 + Cloudflare 403)。

## 四、给 hyper_local 的设计启示

1. 回调单参数共识:激活价(可选)+ 回调幅度(百分比或绝对价差),触发后市价/限价执行——OKX/Bybit 用 trailing 实现,3Commas 叫 Trailing Buy,Pionex 叫 Max Rebound Rate(最直白命名)。
2. 阶梯单参数共识:总数量 + 档数(2-100)+ 价格区间/步进(等差或等比)+ 每档数量分布(均分/递增/递减/自定义)——Bybit Scaled Order 最完整;3Commas Step Sell 限 6 档卖出。
3. TWAP 参数共识:总数量 + 间隔(可调 5s-120s 或固定档)+ 每单数量 + 可选随机化(±20%)+ 可选触发/停止价 + 时长或次数上限。
4. 共性约束:并行实例上限(Bybit TWAP 20 个/账户)、单子最小占比/最小下单量、策略 7 天自动终止、资金预冻结、网格需全额资金覆盖。
5. 2026 新动向:OKX AI Trading Bot + Agent Trade Kit、3Commas QuantPilot(AI 端到端)、Pionex AI 2.0、Gainium Combo Minigrids——AI 出参+自动回测+机器人执行是共同方向。

## 五、主要来源(全部 2026-08-15 实际抓取)

- 币安:github.com/binance/binance-spot-api-docs/blob/master/rest-api.md;算法单面板 → 未查证
- OKX:okx.com/help/xi-strategy-order-types 、/help/xiii-time-weighted-average-price-twap 、/help/xii-iceberg-strategy 、/help/whats-the-spot-grid-bot-and-how-do-i-use-it-sg-trld 、/help/iii-spot-dca-martingale 、okx.com/docs-v5/en/
- Bybit:bybit.com/en/help-center/article/Scaled-order 、/Introduction-to-TWAP-Strategy 、/Iceberg-Order 、/Chase-Order 、/Percentage-of-Volume-POV-Order 、/Trailing-Stop-Order-Spot-and-Spot-Margin-Trading
- 3Commas:3commas.io/smart-trade 、help.3commas.io/en/articles/16281102(DCA)、16281082(Grid)、16281163(Trailing)、16281159(订单类型)
- Pionex:help.pionex.com/en/articles/15258618(TWAP)、14616341(Smart Trade)、14616349(Grid)、14616339(Trailing DCA=回调)、14616338(DIY)
- Gainium:gainium.io/help/dca-mode 、grid-step 、smart-orders 、trailing-take-profit 、trailing-stop-loss 、trading-terminal-deal-types 、minigrids-dca
- Cryptohopper:www.cryptohopper.com/help 、help.cryptohopper.com → 均被拦截,未查证
