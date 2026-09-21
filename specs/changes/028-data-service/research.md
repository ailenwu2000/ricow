# 028 调研: 同行做法与数据源事实

**日期**: 2026-09-20 | **状态**: Phase 0 产物(证据留档, 结论已并入 spec/plan)

## 一、同行怎么做"策略自取数据"(三家已查证)

### NautilusTrader(数据 + 实盘一体化框架)

- 策略在 `on_start` 里**先请求历史、再订阅增量**: `request_bars(bar_type, start=...)` → 历史经 `on_historical_bars()` 批量到达; `subscribe_bars(bar_type)` → 增量经 `on_bar()` 逐根到达。官方明确"两操作配合的标准用法: 请求历史初始化指标, 订阅续上实时流"。
- 数据统一走 `DataEngine` → `Cache`(缓存) + `MessageBus`(发布给订阅者); "backtest / sandbox / live 三种环境下数据走同一条通路" —— 回测由引擎直接喂, 实盘由 venue adapter 归一化后送进同一条通路。
- 关键结论: **同一份策略源码同时跑回测与实盘**(官方 tip 原话); 数据以 `BarType`(标的+周期+价格类型+聚合方式)为键; 回测数据由 `BacktestDataConfig` 从本地 catalog(Parquet)装载, 而非联网现拉。
- 出处: https://nautilustrader.io/docs/latest/concepts/data/ , https://nautilustrader.io/docs/latest/concepts/strategies/ , https://nautilustrader.io/docs/latest/concepts/live/

### freqtrade(声明式取数 + 统一 DataProvider)

- 策略用 `informative_pairs()` **声明**需要的 `(pair, timeframe)` 列表(可含通配 `*` 覆盖白名单全部标的, 也可指定固定参照标的如 BTC); 数据由框架在回测前/实盘每根 candle 时刷新并缓存。
- 取数统一经 `DataProvider.get_pair_dataframe(pair, timeframe)`: 回测返回历史, Dry-Run/Live 返回缓存 —— **同一个调用, 三种模式**。
- 明确警告: 跨周期合并必须用 `merge_informative_pair`(按 bar 时间对齐), **不要**用普通 merge, 否则日期字段(open 时间)会引入**前视偏差**; 并建议"能重采样就不要多拉一个周期, 避免把交易所请求打爆"。
- 回测/超参优化必须**本地先有数据**(预先下载), 实盘才在线取 —— 与我们 D3 拍板一致。
- 出处: https://docs.freqtrade.io/en/stable/strategy-customization/

### Jesse(策略主动取任意标的/周期)

- `self.get_candles(exchange, symbol, timeframe)` 可跨"路由"取任意标的与周期(不限于当前交易路由); 额外的数据源用 `data_routes` 声明 `(exchange, symbol, timeframe)`, 与交易路由解耦。
- K 线统一入库: 取数与存库(`store_candles`)分开, 研究(回测/ML)与实盘共用同一份库表。
- 出处: https://docs.jesse.trade/docs/strategies/api , https://docs.jesse.trade/docs/research/candles

### 三家共识(= 本计划采纳的形态)

1. 策略**声明 / 请求**要什么数据, 框架负责取、缓存、对齐、供给; 框架**不替策略决定**标的与周期。
2. 数据以 `(来源/交易所, 标的, 周期)` 为键, 多标的 / 多周期是一等公民。
3. 回测与实盘**同一 API、同一份策略代码**, 差异只在数据装配。
4. 回测数据来自**本地库**, 下载与回测分离(可复现性)。

## 二、数据源事实(2026-09-20 查证)

### Yahoo Finance(v8 chart, 免 key)

- 端点: `https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?range=1y&interval=1d`, 也可用 `period1` / `period2`(unix 秒)界定区间(两者不同时使用)。返回 OHLCV + `adjclose`(复权收盘, 回测比对时更准确)。
- 覆盖: 美股个股 / ETF / 指数, 日线与周线**回溯到上市**(`range=max` 对大盘股可到 1980 年代); 分钟级 1m ≈ 最近 7 天、5m ≈ 60 天、15m/1h 更长 —— **分钟级历史不能依赖它**, 分钟级仍以交易所数据为主。
- 注意: 需要浏览器 UA(默认库 UA 会被拒); 非官方接口、按 IP 限速(实测 ≲2 req/s 稳定), `/v7/finance/quote` 需 cookie/crumb(v8 chart 不需要)。
- 出处: https://dev.to/avabuildsdata/how-to-get-historical-stock-data-from-yahoo-finance-without-paying-for-an-api-key-5ein

### Stooq(免费但已需 key)

- `https://stooq.com/q/d/l/?s=aapl.us&i=d` 返回 CSV; 2026 年初起**需要 apikey**(站内 CAPTCHA 获取), 有每日请求配额, 超限时以 HTTP 200 + 错误文案返回。
- 结论: 本轮**不内置**(D4 拍板只做 Yahoo + Nasdaq); 若日后要批量整包下载再评估。
- 出处: https://github.com/api-evangelist/stooq

### Nasdaq 官方(已有)

- 仓库现有 `ricow_engine::nasdaq` 客户端(免 key, 日线), 落 `us_klines` 缓存; 本轮适配到统一数据源接口, 不重写解析。

## 三、对设计的直接影响

| 事实 | 设计落点 |
|:--|:--|
| 三家都是"声明/请求 + 框架供给" | `data:series` / `data:subscribe` / `data:history` 三动词(plan d1) |
| 跨周期合并是前视高风险点 | 可见性单点按 `close_time`(plan d7) + 专门单测 |
| 回测数据来自本地库 | 回测只读本地库 + `ricow data pull`(plan d4) |
| 分钟级第三方历史窗口很短 | 分钟级以交易所源为主; 长历史用日线/小时线 |
| Yahoo 限速且非官方 | 源级限速 + 串行化 + 失败如实报错(不回落近似数据) |
| 新增源不该动主流程 | trait + 注册名(plan d3; 验收 SC-002) |

## 四、本轮取舍的落点(与上表的差异说明)

1. **"能重采样就不要多拉一个周期"(freqtrade) vs 本计划 d6"优先原生周期"**: 方向相反而刻意为之 —— freqtrade 是"全标的按固定周期批量刷新", 多拉一个周期会成倍放大请求量; 我们是**策略按需声明单条序列**, 请求量可控, 且原生周期能避免重采样误差与缺口守卫的近似。两者约束条件不同, 故取舍不同。
2. **`adjclose` 口径**: Yahoo 返回复权收盘, 与币安价不同源; 计划把 `adjclose` 作为 Bar 的可选字段并由策略声明用哪个价(FR-015), 报告标注所用口径 —— 避免混用改变信号。
3. **回测数据来自本地库**: 采纳 freqtrade 的"回测必须先有本地数据", 落为回测只读本地库 + `ricow data pull`(D3 / FR-031)。
4. **统一入库**: 采纳"K 线统一入库"的思路, 落为单表 `data_klines`; 既有 `us_klines` 专桶退役(D8), Nasdaq 成为普通 source。
5. **两处能力退役(D7)**: 装配层信号预装(`universe`/`signal_klines`/`SIGNAL_TAIL`)与新数据服务功能重叠且零内置消费者, 由策略经 `data:series` 表达; 组合回测的多标的同一账本撮合能力保留。
