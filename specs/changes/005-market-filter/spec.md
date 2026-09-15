# 功能规格: 市场分类层 + scan 池参数化(005-market-filter)

**功能目录**: `specs/changes/005-market-filter`

**创建日期**: 2026-09-06

**状态**: 已实施完成(M1 市场识别 + Nasdaq 适配 2026-09-07 落地; 消费方 bs_momentum 策略经
2026-09-09 Lua 化整改后落地, 组合入口见 specs/changes/006-bs-momentum-lua/spec.md;

> **⚠ 已下线 (2026-09-11)**: 本变更交付的策略 `strategies/builtin/bs_momentum.lua` 已删除。
> 依据: 真实 bStock 成交轨期望 ≈0 (spot 91 天 每 bar −0.0198%, 期末 −5.38%; futures 220 天
> 每 bar +0.0367%, 期末 −1.08%), 十年 R1 的 +15,088% 含幸存者偏误不作证据 → 用户判定
> "期望值为负, 不合格"。通用能力 (组合信号模式 / 组合回测入口 / `us_klines` 缓存) 保留,
> 暂无内置消费者。证据与判定细节: `specs/research/bs-momentum-attribution-2026-09.md` §七。

历史计划 `.hermes/plans/2026-09-06_150751-bstock-rotation-strategy.md` 的 M2 Rust 版已撤销)

**编号说明**: 原 P4 发布基线批次 C 为 `004-market-filter`,编号 004 已被 risk-guards(2026-09-06 立项)占用,故本档案顺延为 005-market-filter。

**输入**: bStock 动量轮动策略计划 §三 G1/D8 —— 市场分类/筛选能力缺失,先做最小能力(市场分类层 + scan 池参数化),数据侧零新增依赖。

## 背景与范围

bStocks(美股代币现货)与股票类永续(EQUITY perps)此前在 Locus 中无法从普通 crypto 市场中区分:scan 全市场混扫。实测(2026-09-06 免 key)确认:
- bStocks 现货 = 标准 spot exchangeInfo 内, symbol = `股票代码+B+USDT`, quote=USDT;
- fapi `underlyingType=EQUITY` + `contractType=TRADIFI_PERPETUAL` = 免 key 股票池锚点(155 TRADING);
- 识别规则 = spot base 去 B 后缀 ∈ fapi EQUITY base 白名单 ∩ spot TRADING;单靠"base 以 B 结尾"有 ARBUSDT/STXBUSDT/QNTBUSDT 假阳性。

**范围(最小能力,对应计划 D8)**:
1. bStock 现货池识别规则(纯函数 + 单测)+ 股票类永续(EQUITY)池枚举;
2. bStock base → 美股代码映射表固化(67 只,含 BRK.B 点号特例);
3. Nasdaq 官方 API 美股日线适配器(信号数据源,免 key)+ 本地 SQLite 缓存(us_klines 独立表);
4. `locus scan --pool bstock-spot`(候选池参数化,缺省行为不变)。

**范围外(不在此档案)**: 组合回测引擎与 bs_momentum 策略(原计划 M2, 已由
006-bs-momentum-lua 以 Lua 形态落地)、实盘执行(M3, 另行立项)、下单通道实测(Q1)、GUI。

## 用户场景与测试 *(必填)*

### 用户故事 1 - scan 只扫 bStock 现货池 (优先级: P1)

用户执行 `locus scan --pool bstock-spot`,扫描范围收敛到 bStocks 现货(≈63-69 只,规则复算),不混入 ARBUSDT(Arbitrum)/STXBUSDT(Stacks)/QNTBUSDT(Quant)等 crypto 假阳性;缺省 `locus scan` 行为与旧版一致(全市场)。

**优先级理由**: 轮动策略标的池依赖此识别;004 阻塞前置调研已完成(见 hyper-local-development refs/bstocks-market-data-2026-09.md)。

**独立测试**: 识别规则纯函数单测(TSLABUSDT→TSLA 命中;ARBUSDT/STXBUSDT/QNTBUSDT 排除);CLI 冒烟 `locus scan --pool bstock-spot` 输出全池无假阳性。

**验收场景**:

1. **给定** spot exchangeInfo 全量, **当** 按识别规则过滤, **则** 命中 bStock 现货池且不含 ARBUSDT/STXBUSDT/QNTBUSDT
2. **给定** fapi exchangeInfo, **当** 按 underlyingType=EQUITY 过滤, **则** 枚举出股票类永续池(TRADING, ~155)
3. **给定** 缺省参数, **当** 执行 `locus scan`, **则** 候选池行为与旧版一致(回归)

### 用户故事 2 - 美股日线可取可缓存 (优先级: P1)

信号轨需要真实美股日线(Nasdaq 免 key,10 年,拆股复权)计算长窗动量,数据按标的落本地 SQLite,重复拉取命中缓存;拉取失败(网络/HTTP/解析)重试 3 次后退避,仍失败按"该标的当日信号缺失 → 跳过当日调仓、维持现持仓"语义处理(适配器只读,无写风险)。

**优先级理由**: 轮动策略信号数据源(计划 D10);美股交易日历天然对齐信号节奏。

**独立测试**: 单标的真拉取冒烟(TSLA ≥2500 根、2016-09 close≈13.52 拆股复权);db 缓存单测(open_in_memory)。

**验收场景**:

1. **给定** 有效美股代码(如 TSLA), **当** 调用 Nasdaq 适配器拉取日线, **则** 返回 ≈10 年日线且拆股复权价连续
2. **给定** 已缓存标的, **当** 再次请求, **则** 命中本地缓存不重复联网
3. **给定** 不可达/解析失败, **当** 拉取, **则** 重试 3 次后返回错误, 由调用方按"跳过当日信号"处理

### 用户故事 3 - 映射表单一权威 (优先级: P2)

bStock base → 美股代码 + assetclass(stocks/etf)映射固化(代码常量/数据文件 + 单测),全项目引用同一份,防 TSLA 类映射漂移。

**优先级理由**: 信号-执行双轨的桥;单一权威防双轨(用户强偏好)。

**独立测试**: 映射单测抽查 TSLA/AAPL/SPY/BRK.B。

**验收场景**:

1. **给定** bStock base(TSLAB/SPYB/BRKB…), **当** 查映射表, **则** 返回正确美股代码与 assetclass
2. **给定** 映射表, **当** 全池遍历, **则** 每只均有映射且无重复冲突

## 关键实体 *(涉及数据时填写)*

- **bStock 现货池规则**: fapi EQUITY base 白名单(155)∩ spot TRADING 且 base 以 B 结尾 → 现货池(离线固化映射表)
- **us_klines 表**: pair=美股代码, interval='1d', 与 klines(币安)表隔离
- **ScanConfig.pool**: `all`(缺省)/ `bstock-spot`;scan 层不做 ≥21 根/流动性过滤(属策略层 M2)

## 成功标准 *(必填)*

### 可度量结果

- **SC-001**: `cargo test --workspace` 全过;新增识别规则/映射/缓存单测
- **SC-002**: `locus scan --pool bstock-spot` 真数据输出全池(规则复算 ≈63-69 只),无 ARBUSDT/STXBUSDT/QNTBUSDT
- **SC-003**: Nasdaq 全池拉取实测可行(并发 4, ~2min/全池);与 bStock K 线抽查对齐(美股交易时段对应收盘价价差 <1%)
- **SC-004**: specs/backtest.md D6 注记修正(bStocks quote USDC→USDT,2026-09-06 实测)

### 实测数据与默认值定稿 (2026-09-07)

**流动性分布 (70 只 bStock 现货, Binance 24hr quoteVolume, 周末+盘前低流动时段)**:
min $4.5K (GSB) / p25 $37.8K / **median $89K** / **p75 $306K** / max $8.0M (CRCLB)。
<$50K: 21/70; <$100K: 38/70; <$250K: 47/70 (67% 被 $250K 滤掉)。

**min_volume 默认值定稿 = $100K (24h quote volume)**, 依据:
- 实测时点为周末/盘前低流动段 (7 日均 ≈ 3-4× 周末单日: 5 个美股高流动日 + 2 个周末日),
  $100K 周末口径 ≈ 7 日均 $300-400K, 与计划建议的 "$0.25M 量级" 同档;
- $250K 直接按周末单日套用会滤掉 67% 池 (含 AAPL/TSLA/NVDA 之外大量中盘), 过严 — 该口径只在盘中瞬时成立;
- $100K 周末口径保留 32/70 (46%), 蓝筹+活跃中盘全覆盖 (CRCL/TSLA/NVDA/QQQ/MSTR/SNDK/HOOD 等), 冷门微盘 (GS $4.5K / CRWD $6.5K / SQQQ $16K) 出局, 符合宁缺毋滥 + 可成交性;
- 参数可配 (CLI --min-volume / TOML), 实盘启用后按真实 7 日均再校。
- scan --pool bstock-spot 通用命令默认仍 $1M (与全市场一致, 防误伤); bstock 策略层 (bs_momentum) 默认用 $100K。

**Nasdaq 拉取实测**: 全池 70/70 成功 (并发 4, 81s), 10 年 2514 根 (服务端上限); TSLA 2016-09 close=$13.52 拆股复权确认;
ETF 14 只须 assetclass=etf (SPY/QQQ/TQQQ/SOXL/EWY/KORU/SMH/DRAM/INTW/MUU/MVLL/SNXX/SOXS/SQQQ);
NBIS 历史含前身 YNDX symbol 延续 (2022-2024 停牌段 volume=N/A, 2024-10-21 重新上市) — R1 报告注明。
响应坑: fromdate 参数必填 (缺省报 400 code=1011); 大响应 gzip (Binance exchangeInfo 17MB 强制 gzip,
reqwest 需 gzip feature, 2026-09-07 根因修复); ETF close 无 $ 前缀、停牌日 volume="N/A" → 解析层已兼容。

## 假设

- 假设 A1: 池随市场绑定隔离(spot→bStock 现货池 / futures→fapi EQUITY 池),策略层校验交叉引用报错(计划 §2.6-3,D12)
- 假设 A2: 识别规则离线固化映射表,不运行时依赖 fapi 请求(公共端点可用,但映射表稳定性优先)
- 假设 A3: Nasdaq 数据无 SLA;适配器失败语义 = 跳过当日信号不动单(幂等安全,计划 §四 M1)
- 假设 A4: 流动性门槛(min_volume)默认值按 24hr ticker 实测分布定(计划 M1-10),不留无依据余量(宪法原则五)

---

> **修订(2026-09-15)**: 本档案交付的 `locus scan`(含 `--pool bstock-spot`)已**删除**(见 `specs/changes/020-platform-scope-trim/`);
> `market_class` / `us_tickers` / `nasdaq` / `us_klines` 按用户明示"通用能力保留"继续留在代码库。本档案作为历史决策记录保留。
