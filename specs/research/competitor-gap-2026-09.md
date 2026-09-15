# ricow 开源竞品功能差距调研(2026-09)

> 调研日期: 2026-09-06 | 方法: 父代理 GitHub API 实测 + 3 组并行子代理官方文档/源码逐维度取证(来源 URL 见各节)
> 前置资产: specs/research/competitor-features-2026.md(2026-08 主整合稿, 含商业 12 家)、competitors.md、competitor-orders-2026.md、ai-mcp-2026.md、telegram-bot-2026.md、backtest-engines-2026.md(2026-09-05)
> 本文只交付竞品对比 + 差距分析 + 决策清单, 产品方案/roadmap 更新等用户拍板后另写。
> 产物边界: "桌面原生 GUI 复核"结论已修正旧稿表述(§4.3); 所有"未核实"= 官方一手来源不可得, 非不存在。

---

## 一、现状功能基线(ricow, 2026-09-06 代码/specs 实测)

| 功能面 | 现状 | 出处 |
|:--|:--|:--|
| 策略编写 | Lua 5.4 沙箱(mlua), 4 回调 on_init/on_tick/on_fill/on_stop; ctx 只读快照; exec 组件库(levels/pullback_triggered/detect_quote/ticks_per/slice_due/side_order); 编译门禁 → 沙箱回测 → 两步确认 | specs/lua-api.md, architecture.md |
| 指标 | ta 库 12 种(EMA/SMA/WMA/RSI/MACD/BOLL/ATR/ADX/Stoch/CCI/ROC/MOM), 基于已收盘 K 线无前视 | lua-api.md |
| 内置策略 | shannon_grid(唯一样板) + 5 执行示例 dca/twap/vwap/pullback/ladder | architecture.md |
| 回测 | v0.2 现货+合约统一引擎; bar 级撮合(无前视/保守假设); 费用/滑点/杠杆/MMR 首档表/8h 资金费; 报告指标集完整(年化收益/波动/夏普 rf 3.8%/索提诺/Calmar/盈亏比/胜率/资产变化 4 指标等) | specs/backtest.md §六 |
| 复杂度不集成 | 全仓保证金/MMR 分层/保险基金/tick 重放/部分成交+冰山/标记价等 9 项(默认不集成, 需拍板) | backtest.md §七 |
| 实盘 | BN 现货+fapi 合约(FuturesClient, demo 联调 2026-09-04) + HL(REST/WS/EIP-712, 当前主用 BN); RiskEngine + confirm + Dry Run 默认 | roadmap.md, architecture.md |
| 行情驱动 | **WS 事件驱动**: BN @depth@100ms 盘口流 → on_tick; 成交回报 WS 已实现未接入 run; 实盘 P4 接线 | crates/ricow_engine/src/command.rs:37-52 |
| 数据/复盘 | 本地 SQLite(K 线缓存/成交/PnL); ricow db sync/stats/**export(K 线导出)**; scan 本地缓存增量 | architecture.md, db.rs |
| 选币 | ricow scan(横截面动量 7d×0.4+30d×0.6, EMA20+ADX14, 白黑名单, 前10/后10) | product.md §六 |
| 通知/提醒 | **无**(代码 0 命中; product.md 核心场景已承诺"熔断/成交通知"但未实现; ricow alert 曾列后续评估) | grep 实测, product.md §三.4 |
| AI 入口 | 不内置 LLM(AI 在外侧客户端); 002-ai-quant-researcher spec = 草稿未实施(面向 Binance bStocks) | specs/changes/002 |
| 高级订单 | 冰山/OCO/bracket/trailing **未实现**(引擎 0 命中; trailing 可由 Lua 自写逻辑) | grep 实测 |
| 密钥 | OS Keyring + headless 加密文件 fallback(领先行业) | architecture.md |
| 多交易所 | 2 所(BN + HL), 原生 adapter 非 ccxt | architecture.md |
| 形态 | 纯 CLI(2026-08-24 D12), systemd user service 部署 | product.md |

## 二、开源竞品 star/活跃度刷新(2026-09-06 GitHub API 实测)

| 项目 | 2026-08-16 | 2026-09-06 | 变化 | 活跃度 |
|:--|:--|:--|:--|:--|
| freqtrade/freqtrade | 53.3k | 54.1k | +0.8k | push 09-05 活跃 |
| nautechsystems/nautilus_trader | 25.6k | 28.4k | +2.8k 最快 | push 09-06, v2.0.0rc 重构中 |
| hummingbot/hummingbot | 19.5k | 19.9k | +0.4k | push 09-05 |
| NoFxAiOS/nofx | 12.7k | 12.8k | +0.1k | push 09-05;**repo 描述 2026-09 前移美股/商品/外汇**(实为 HL 代币化永续包装, 见 §4.2) |
| jesse-ai/jesse | 8.3k | 8.4k | +0.1k | push 09-04 |
| Drakkar-Software/OctoBot | 6.4k | 6.5k | +0.1k | push 09-05, node mode 成默认 |
| enarjord/passivbot | 2.0k | 2.1k | +0.1k | push 09-05, v8.1.0 破坏性重构 |
| HammerGPT/Hyper-Alpha-Arena | 1.1k | 1.16k | ~0 | **push 停 2026-05-13(~4 个月), 实质停更** |
| alsk1992/CloddsBot | 695 | 885 | +190 | push 09-01 活跃 |
| wquguru/nof0 | 2.75k | 2.75k | 0 | 停更(2025-12) |
| Gajesh2007/ai-trading-agent | 538 | 540 | ~0 | 停更(2025-10) |
| moss-site/moss-trade-bot-skills | — | **381(新)** | 新 | 2026-04 创建, push 08-19 |
| 2026-06 后新项目扫描 | — | 最高 22★ | — | topic:crypto-trading-bot / hyperliquid-bot, **无新重磅** |

结论: 格局与 2026-08 一致, 无 star>200 新竞品; NOFX/Alpha Arena/CloddsBot 状态变化已核实(§4)。

## 三、开源竞品深度对比(6 传统框架 × 11 维度, 2026-09-06 取证)

来源: 组1(Freqtrade/Hummingbot/OctoBot)+ 组2(Jesse/Nautilus/Passivbot)子代理官方文档/源码取证, URL 索引见文末。

| 维度 | Freqtrade 54k | Hummingbot 20k | OctoBot 6.5k | Jesse 8.4k | NautilusTrader 28k | Passivbot 2.1k |
|:--|:--|:--|:--|:--|:--|:--|
| 1 策略编写 | Python IStrategy 回调, new-strategy 向导 3 档模板 | Python V1 模板 + V2 Controller/Executor 积木; create 问答向导 | Web 配置内置 modes + Script(类 Pine); 无代码门槛最低 | Python 回调 + GUI 一键生成模板; 多 routes | Python(PyO3)/Rust 事件驱动; 执行算法独立组件 | JSON 配置不写码; 策略=配置族(trailing_martingale 等) |
| 2 指标库 | ta-lib/pandas-ta/technical | pandas_ta 自算无内置清单 | 内置评估器 + Script tulipy | ~158-174 内置(Rust 加速), 300+ 为营销口径 | ~40 Rust 类型安全指标 | 无通用指标库 |
| 3 回测 | bar + **--timeframe-detail 次周期** + **lookahead/recursive 检测** + 资金费; 无滑点模型; HL 限 5000 根 | controller 回测(1m 默认, 每根至多 1 成交, 简化) | K 线回放; **数据受限 ~500 根**(FAQ 原文) | 1m-1D bar; 17+ 指标; 防 lookahead routes; **Optuna+Ray 优化 + 蒙特卡洛 + 显著性** | **tick/盘口 L1-L3 纳秒级(6 家中唯一)**; 四模型; 无内置优化(明示外置) | 1m bar Rust 回测; 多目标进化优化(pymoo NSGA2/3); 分片加权指标 |
| 4 paper | dry-run 内置(同代码路径) | paper 内置(*_paper_trade 后缀) | simulator 内置(模拟成交按 recent trades) | **付费插件才有**(Free 仅 testnet) | sandbox 适配器开源 | 无(fake-live 非 paper) |
| 5 订单/执行 | market/limit;**FAQ 明言无 OCO/iceberg/TWAP** | LIMIT/MAKER/MARKET + 三重屏障(TP/SL/时限)+trailing(应用层) | market/limit + 随单 SL/TP + Staggered 网格 | market/limit/stop + 部分成交 | **TIF×执行指令(冰山/OCO/OUO/OTO)引擎级全支持** | 限价网格 + trailing + 自动解套 + panic 市价 |
| 6 风控 | ROI 表/止损/4 类 Protections/max_open_trades | 三重屏障 + kill switch + inventory skew | 随单 SL/TP + profile risk 参数 | 清算建模, 无独立引擎 | **RiskEngine: 下单频率上限(默认 100/s)+ 单笔名义上限** | **敞口上限 + WEL 熔断 + 亏损闸门 + Equity HSL 三色**(运营级最全) |
| 7 数据 | 本地 feather/parquet 增量, funding/trades 可取 | WS 实时 + conf 落盘 | 交易所按需取(历史浅) | PostgreSQL + import-candles | ParquetDataCatalog + Tardis/Databento | 本地 OHLCV v2 + 校验和, 多实例锁 |
| 8 形态易用 | CLI + FreqUI Web + 文档最全 | CLI 五窗格 + Condor Web(Dashboard 已停维护) | **Web UI + 手机 App + 双击可执行文件**(最易上手) | Web GUI + 内置编辑器(LSP)+ 图表, 上手快 | **无 UI(roadmap 明示不做)**, 概念陡 | 纯 CLI + monitor relay, v8 配置陡坡 |
| 9 通知扩展 | **TG ~40 命令(默认 22)** + webhook 出站 + REST API | API/MCP + TG(Condor) | **TG + TV webhook 官方一等公民** + ChatGPT | TG/DC/Slack(随付费插件)+ **本地 MCP + JesseGPT** | 无(明示 out of scope) | 无 |
| 10 实时/高频 | bar 驱动, 主循环 5s 节流 | tick 循环**默认 1s 下限 0.1s**, sub-second 下单, "HFT"是使命层营销 | 分钟-小时级(asyncio) | bar 收盘驱动(WS K 线) | 事件驱动, tick/盘口回测需商业 tick 数据 | 1m bar + 限价单 |
| 11 密钥 | config.json **明文** | AES-128-CTR 加密 + 密码(最安全) | 加密存储 + SHA256 密码 | .env 明文 | env 明文, 供应链安全(SLSA/签名)最强 | api-keys.json **明文** |

### 3.1 功能统计(6 框架)

| 功能 | 具备 | ricow |
|:--|:--|:--|
| 回测 | 6/6 | ✅(bar 级, 严谨性第一档) |
| 参数自动优化 | **4/6 内置**(FT Hyperopt/Jesse Optuna/PB pymoo/OB optimizer; NT 外置; HB 无) | ❌ 无(旧"明确不做") |
| 蒙特卡洛/显著性 | 1/6(仅 Jesse) | ❌ |
| paper/dry-run | 5/6 | ✅(Dry Run 默认) |
| 通知(出站) | **4/6**(FT TG+webhook/HB Condor/OB TG/J) | ❌ 无 |
| TV/信号 webhook 入站 | 1/6 官方(OctoBot; 商业 4/4 标配) | ❌ 无 |
| 冰山/OCO 引擎级 | 1/6(仅 Nautilus) | ❌(BN API 可暴露, 未做) |
| tick/盘口级回测 | 1/6(仅 Nautilus, 数据靠商业商) | ❌(backtest.md §七.5 明确不集成) |
| 密钥加密静置 | 2/6(HB/OB) | ✅ Keyring 领先 |
| 桌面原生 GUI | 0/6 主流; **长尾萌芽 3 个**(§4.3) | ❌ 纯 CLI(D12) |
| 多交易所 | FT 100+(ccxt)/HB 30+原生/NT 18 原生/J 9/OB ccxt 15+/PB 10 | 2 所(BN+HL) |

## 四、专项复核

### 4.1 旧稿结论修正/刷新(2026-08 → 2026-09)

- Freqtrade "config.json 明文存 key" ✅成立; "HL 回测限 5000 根" ✅成立; "TG 40+ 命令" ➡️ 修正: 默认 allowlist 仅 22 条; "FreqAI 传统 ML" ➡️ 修正: ML 沙箱(默认 LightGBM + RL 模块 + 实时 retrain)
- Hummingbot "CLI+Dashboard" ➡️ Dashboard 已停维护, 接力 Condor(Web+TG); "HFT 定位" ➡️ 核实为使命层营销, 真实 tick 循环 1s(下限 0.1s)
- OctoBot 凭据 ➡️ 修正"也明文存 key"的常见印象: config.json 加密存储; 形态 ➡️ node mode + 手机 App 成默认入口
- Jesse "HL backtesting:False" ✅成立(官方现行列表 HL 仅在实盘侧, 原因=API 历史 K 线不足); 商业模式 ➡️ 实盘闭源买断 $899 起
- Hyper Alpha Arena ➡️ **实质停更(~4 个月零提交, 无 release)**, 不建议再作对标活体
- NOFX ➡️ 定位未转轨("the strategy is a language model" 原话仍在), repo 描述前移 TradFi 是营销; 美股/外汇 = **HL 代币化永续包装**, 无券商直连; 无回测(全文档 0 命中)
- 桌面 GUI "0/8 行业空白" ➡️ 需修正: "主流缺席、长尾萌芽"(§4.3)

### 4.2 AI 交易新品类(2026-09 实测)

| 项目 | 形态 | 回测 | 安全/审计 | 对 ricow 的参考 |
|:--|:--|:--|:--|:--|
| NOFX 12.8k★ | "LLM 即策略": 每 trader 连续循环读市场→决策→执行→留痕; Go 硬风控 clamp; 9 加密所 | **无**(0 命中; 验证靠 Autopilot 实盘小仓 + leaderboard) | 私钥加密留本机(本地化卖点); 每决策留痕 | 无回测门禁 = ricow 差异点仍成立; AGPL-3.0 |
| Hyper Alpha Arena 1.16k★ | AI Trader + Program Trader + 86 因子 + 测试网 paper | 有(Program Trader) | — | **已停更**, 退出对标 |
| CloddsBot 885★ | 自然语言聊天下单终端(21 平台); 118+ 策略/119+ skills | 无独立 paper | **Trade Ledger 审计**(reasoning + 置信度校准 + SHA-256 防篡改); AES-256-GCM; sandbox shell 批准 | 审计台账设计可借鉴; 无回测门禁无 paper, 实盘化弱 |
| moss-trade-bot-skills 381★ | **AI agent skill 寄生形态**(Hermes/Claude/Codex 可装); 自然语言 → 五支柱信号策略 | 本地 pandas 回测宣称 bit-exact(按真实 HL 费用/滑点/资金费) | 数据指纹校验; 模拟实盘 + leaderboard copy-trading | 唯一"NL → 本地可验证回测 → 模拟"闭环的产品化框架; 无桌面 GUI |

横向结论: 四家无一具备"回测门禁 + 私钥不出本机 + 本地终端"三者组合; "LLM 参与量化"与"LLM 全权下单"分化清晰, ricow 的"回测门禁 + LLM 可解释辅助"差异化仍无开源对手占据。

### 4.3 桌面原生 GUI 现状复核(旧稿表述修正)

- **"主流 8 框架桌面原生 GUI = 0" 复核通过**(Freqtrade/Hummingbot/Jesse/OctoBot/Passivbot/Nautilus/NOFX/Superalgos 等 2025-2026 无一把桌面 GUI 产品化, 仍 CLI/Web/库)
- 但"行业空白"需修正为 **"主流缺席、长尾萌芽"**: 2024-12 起 egui/iced/Tauri2 生态出现小批原生桌面交易客户端:
  1. **rust-trade(Erio-Harrison) 484★** — Tauri2; Binance WS L1/L2 + paper 引擎 + 回测引擎 + 桌面 GUI(原生壳量化闭环中 star 最高)
  2. **OpenBook 179★** — Rust egui; Binance 永续 Bookmap 深度热图/订单流; 纯看盘无下单
  3. **kerosene 12★(2026-05 创建)** — Rust iced; **BYOK Hyperliquid 永续桌面终端(私钥本机), 形态与 ricow 最像**, 极早期单所
- 结论: 需求与工具链已就位, 但**尚无万星级、多所、带风控/回测门禁的桌面原生开源平台** —— 该层空白对 ricow 依然成立, 窗口在收窄。

## 五、差距清单(竞品证据 | ricow 现状 | 等级 | 建议)

### 🔴 核心缺口(行业标准多数具备, 与两形态口径都相干)

| # | 差距 | 竞品证据 | ricow 现状 | 建议 |
|:--|:--|:--|:--|:--|
| G1 | **通知/告警缺失**(成交/熔断/异常推送) | 4/6 开源有(Freqtrade TG 40 命令 + webhook 出站为最完整模板; Hummingbot Condor; OctoBot TG) | 无任何通知代码; **product.md §三.4 核心场景已承诺"熔断/成交通知"但未实现** | 补最小闭环: 成交通知 + 策略停止/风控触发告警(CLI 口径=webhook/TG 出站; 架构上出站通知不违背本地定位, 无密钥托管) |
| G2 | **参数自动优化缺失** | 4/6 内置(FT Hyperopt optuna NSGAIII / Jesse Optuna+Ray / PB pymoo 进化 / OB optimizer) | 无; 旧决策"明确不做"(散户过拟合) | D2 拍板: 维持 / 分层(先做"参数对比/网格扫描"轻量级, 不做自动寻优) |

### 🟡 口径相关或候选(部分依赖 D1 形态拍板)

| # | 差距 | 竞品证据 | 建议 |
|:--|:--|:--|:--|
| G3 | 回测报告可视化(权益曲线/回撤图/月度热力图) | Jesse 全套交互图表 + 17+ 指标图; FT FreqUI 图表; Passivbot 分片指标 | CLI 口径 = 报告导出 PNG/CSV(SQLite 已有数据, 成本低); GUI 口径 = 图表面板 |
| G4 | 复盘/交易日志 journal | 商业 Gainium Trade Journal 自动记账(旧稿); Jesse 图表复盘 | 依赖 D1: CLI 口径=db 查询命令增强; 候选 |
| G5 | TV/信号 webhook 入站 | 商业 4/4(3Commas Signal Bot/OctoBot 主推/TV 专页 TOKEN 鉴权) | D3 拍板(需公网入口, 与"仅交易所 API"网络模型有冲突, 本地化定位下收益有限) |
| G6 | 多交易所广度(2 vs 10-100) | FT ccxt 100+/HB 30+原生/NT 18 原生/J 9/PB 10 | 按需接入, 与返佣主线绑定(返佣高平台优先); 不入本轮 |
| G7 | 运营级风控护栏(引擎级) | PB 敞口上限/WEL 熔断/亏损闸门/Equity HSL 最全; NT 下单频率上限 100/s + 单笔名义上限 | ricow RiskEngine 已有(持仓/日亏/最小单/滑点), 可抄补: 下单频率限制 + 连续亏损熔断; 低成本高价值, 建议列入 |
| G8 | lookahead/回测严谨工具 | FT lookahead-analysis/recursive 检测独有 | ricow 回测已防前视(文档), 加"偏差检测"工具低成本; 候选 |
| G9 | 桌面原生 GUI | 主流 0/6, 长尾萌芽(rust-trade 484★ 量化闭环先例 / kerosene BYOK 形态最像) | D1 拍板; 即便不做, "报告导出"等 CLI 增强共享同一数据层, 不白做 |

### 🟢 维持"明确不做"复核(有竞品但不符合定位, 或证据不支持)

| 项 | 竞品证据 | 复核结论 |
|:--|:--|:--|
| 冰山/OCO 引擎级订单 | 仅 NT 1/6(FT FAQ 明言不做); BN API 层可用 | 维持不做(散户场景低频; 需要时经 exec 组件暴露交易所 API 即可) |
| 蒙特卡洛/显著性检验 | 仅 Jesse 1/6 | 维持(研究深度派; 参数优化 D2 若做再加) |
| tick/盘口级回测与高频链路 | NT 1/6 且 tick 数据=商业数据商 Tardis/Databento | 维持(见 §六 D5 证据链) |
| 跟单/社交/策略市场 | moss copy-trading / Clodds 生态 / 商业 3/4 | 维持(本地化定位; moss 的 copy 是平台侧) |
| AI 直接决策下单 | NOFX/Clodds/HAA | 维持(不可回测不可审计 = 差异化支点) |

## 六、高频/WS 支持评估(D5 决策证据链)

### 6.1 现状事实(2026-09-06 代码实测)
- ricow run 链路 = **WS 盘口 @depth@100ms 事件驱动 on_tick**(crates/ricow_engine/src/command.rs:37-52), 非轮询
- 回测/指标全 bar 级; 本地 SQLite 无 tick/盘口历史; backtest.md §七.5 把 tick 重放列为"默认不集成, 高频/做市策略时再议"

### 6.2 竞品对照(事件粒度才是可比的硬指标)
- ricow 100ms 盘口事件驱动 > Freqtrade(5s 节流 bar)/Jesse(bar 收盘)/OctoBot(分钟-小时)/Passivbot(1m bar)——**ricow 实时性已领先 4/6 传统框架**
- Hummingbot"高频"= 秒级 tick 循环(默认 1s, 下限 0.1s)+ sub-second 订单执行 = 秒级做市自动化, "HFT" 为使命层营销
- NautilusTrader 是唯一 tick/盘口级回测引擎(纳秒), 但: ①历史 tick/L2 数据走**商业数据商**(Tardis/Databento 官方集成); ②官方 scope 自限 individual/small-team 单节点, tick 实盘优势需自备低延迟托管兑现
- Passivbot 实证: **bar 级 + 限价网格即可表达"准做市/逆势"策略族**(2.1k★ 活跃项目)

### 6.3 结论(证据链)
1. 散户主流生态(Jesse 8.4k/Passivbot 2.1k/Freqtrade 54k)全部运行在 ≥1m bar —— bar 级足以承载主流零售策略
2. tick 回测的门槛不在引擎在**数据资产**(币安主流币 aggTrade 单日数百 MB 级; NT 靠商业数据商), 自建采集/存储/清洗管线与 ricow"简单易用 + 本地 SQLite"定位冲突
3. ricow 回测门禁模型(策略须先沙箱回测可验证)与 tick 策略"无回测数据源"存在架构矛盾; tick 支持 = 数据管线 + tick 回测引擎 + tick 风控 + 撤单竞速的数量级工程
4. ricow 实时行情能力(100ms WS 事件驱动)已满足"准高频感知", 缺失的仅是 tick 级回测验证手段(而它没有合理数据源)
- **建议(D5 选项 A 为主)**: 维持现状 100ms 盘口 WS(低频/准高频策略足够, 实时性已领先多数开源); 轻量候选 = 逐笔 aggTrade 流感知 + 事件聚合(有真实用户诉求再评估); 完整 tick 链路不建议(数据门槛 + 与回测门禁矛盾 + 散户无竞争位置)

## 七、决策清单(2026-09-06 已拍板: 用户拍板 GUI 冻结, 其余 PM/架构师定稿授权)

> ⚠️ §五 各行"建议"为调研时草案, 以下列拍板结果为准。

- **D1 形态口径** ✅ **纯 CLI 维持; GUI 相关全部冻结**(用户拍板 2026-09-06)。G3(回测报告可视化/CSV 导出)/ G4(复盘 journal)/ G9(桌面 GUI)统一冻结, 由未来统一 GUI 计划承接, 本次不立项。
- **D2 参数自动优化** ✅ **维持"明确不做"**。理由: 4/6 开源内置但散户过拟合顾虑仍有效; 自动寻优需批量回测框架 + 搜索空间定义, 投入大; 现有 CLI 可手动多次回测对比。参数对比回测工具降为远期候选(有真实需求再评估)。
- **D3 差距候选去留** ✅ 逐项拍板:
  - **G1 通知 → 做**(实施走 SDD)。理由: product.md §三.4 核心场景已承诺"熔断/成交通知"但未实现(文档-实现缺口); 4/6 开源标配 = 行业标准; 出站 webhook/TG 不违背"私钥不出本机/仅交易所 API"网络模型。范围: 最小闭环 = 成交通知 + 策略停止/风控熔断告警(独立于策略)。
  - **G5 TV/webhook 信号入站 → 不做**。理由: 需公网入站, 与本地网络模型冲突; 开源仅 OctoBot 1/6 官方支持; 本地用户自配 ngrok 成本高收益低。远期入口评估时再议。
  - **G7 运营级风控补项 → 做**(实施走 SDD): RiskEngine 补两项 = 下单频率上限(参照 Nautilus 默认 100/s)+ 连续亏损熔断(Passivbot 亏损闸门简化版)。理由: 直接服务 P4 "不会让用户莫名亏损"验收; 纯引擎层参数 + 检查, 低成本。
  - **G8 lookahead 检测工具 → 不做**。理由: 回测引擎已按已收盘 bar 隔离(无前视); 对 Lua 沙箱策略做偏差检测工具成本高收益低。远期候选。
  - **G6 多交易所广度 → 维持按需接入**(与返佣主线绑定, 返佣高平台优先), 不立项。
  - **G3/G4/G9 → 冻结**(D1)。
- **D4 落盘** ✅: 拍板结果仅标注于本文档 §七; **product.md / roadmap.md 本次不动**——G1/G7 实施以 SDD change 立项(另起 /speckit-specify)时再更新 roadmap; 当前阶段 P4 dogfood 优先。
- **D5 高频支持分级** ✅ **维持现状**(100ms 盘口 WS 事件驱动)。理由(§六 证据链): 实时性已领先 4/6 传统框架(Freqtrade 5s/Jesse bar/OctoBot 分钟级/Passivbot 1m); tick 回测门槛在数据资产(商业数据商); 与回测门禁模型矛盾。完整 tick 链路与轻量 tick 感知均不立项(观察, 出现专业高频用户诉求再评估)。

## 来源 URL 索引(子代理 2026-09-06 直连核实)

- 组1: freqtrade.io(backtesting/hyperopt/configuration/telegram-usage/rest-api/webhook-config/freq-ui/data-download/exchanges/faq/stoploss/leverage/lookahead-analysis/freqai)、github.com/hummingbot/hummingbot + hummingbot-site docs(clock-tick/paper-trade/executors/hummingbot-api/mcp/exchanges/hyperliquid)、github.com/Drakkar-Software/OctoBot + OctoBot-Docs(Exchanges/Web-interface/Simulator/Backtesting/FAQ/Telegram/TradingView-webhook/ChatGPT)
- 组2: github.com/jesse-ai/jesse + docs.jesse.trade(strategies/events/supported-exchanges/exchange-limitations/backtest/optimize/livetrade/pricing/notifications/mcp) + jesse.trade/roadmap; github.com/nautechsystems/nautilus_trader(README/ROADMAP.md/docs/concepts/backtesting/fill-models/accounts-and-margin/docs/integrations/{hyperliquid,binance}.md/crates/adapters/sandbox/crates/live/src/node/config.rs/python indicators __init__.pyi); github.com/enarjord/passivbot(docs: backtesting/optimizing/metrics/risk_management/equity_hard_stop_loss/fake_live/live/monitor/tools/hyperliquid_guide) + api-keys.json.example
- 组3: github.com/NoFxAiOS/nofx(dev README/docs/architecture/STRATEGY_MODULE.md/commits)、github.com/HammerGPT/Hyper-Alpha-Arena(+commits)、github.com/alsk1992/CloddsBot(+docs)、github.com/moss-site/moss-trade-bot-skills、moss.site/agent; 桌面复核: GitHub search API 10+ 组关键词, 逐个 README 核实(Erio-Harrison/rust-trade、nilesjarvis/kerosene、DegenSugarBoo/OpenBook)
- 未核实项(不影响结论): OctoBot exchanges.octobot.info 支持矩阵(Cloudflare 拦截)、Jesse 资金费/滑点参数化文档、部分框架 WebSocket 细节
