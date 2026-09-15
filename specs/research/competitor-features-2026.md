# 竞品功能文档(2026-08)

> 调研时间: 2026-07 ~ 2026-08-16 | 面向: hyper_local AI-first 重构(CLI + MCP + Telegram 三入口)。注: 2026-08-24 决策 MCP/TG 暂不做, 本调研仅作竞品情报留存。
> 素材: doc/competitors.md(2026-07 两轮子代理)、research/competitor-orders-2026.md、research/ai-mcp-2026.md、research/telegram-bot-2026.md(2026-08-15 一手 72 页官方文档)、research/ 下 100+ 原始 HTML/JSON。
> 标注 "未查证" = 页面未给出该项, 非 "不存在"。
> ⚠️ 本文为调研整理稿, 差距分析结论见末尾"功能差距分析"节。

---

## 一、竞品全景(2026-08)

### 开源框架(主流 8 个)

| 项目 | Stars(2026-08-16 实测) | HL 支持 | 界面 | 内置策略 | 回测 | AI | 密钥存储 |
|:-----|:-----|:-----|:-----|:-----|:-----|:-----|:-----|
| Freqtrade | 53.3k | ccxt 现货+永续 | CLI + FreqUI Web + Telegram | 无成品,信号框架 | ✅(HL 限 5000 根) | FreqAI 传统 ML | config.json 明文 |
| NautilusTrader | 25.6k | 原生 Rust adapter | 无 UI(明确不做) | 无成品,事件驱动 | 纳秒级 | 无(明确 out of scope) | 环境变量/配置 |
| Hummingbot | 19.5k | 原生现货+永续 | CLI + Dashboard Web + Condor TG + REST + MCP | PMM/网格/XEMM/套利/TWAP/LP 最全 | ✅ + paper | MCP(接 Claude/Gemini) | 启动密码加密 |
| NOFX | 12.7k | 原生(9 所之一) | 本地 Web 终端 | "LLM 即策略",无传统策略库 | 未查证 | 核心即 AI(任意 LLM) | 加密静置不出本机 |
| Jesse | 8.3k | 原生 driver, **HL backtesting:False** | 自托管 Web GUI + 内置编辑器 | 无成品,研究框架 300+ 指标 | 高精度回测 + Optuna + 蒙特卡洛 | JesseGPT 写/优化/调试 | 自托管 |
| OctoBot | 6.4k | ccxt HL 现货 | Web UI + 手机 App + Telegram | **网格、DCA**、指数篮子、TV 信号、AI/LLM | 回测 + paper | ChatGPT + Ollama 本地 LLM | 未查证 |
| Passivbot | 2k | ccxt 层 | 纯 CLI(第三方 pbgui Web) | 逆势马丁网格 + trailing + unstucking | Rust 回测 + 进化优化 | 无 | api-keys.json 明文 |
| Hyper Alpha Arena | 1.1k | 原生(测试网 paper + 主网) | Web UI(Docker)中英双语 | AI Trader + Program Trader,86 因子 | 测试网 paper | LLM 自主交易 + AI 因子挖掘 | 未查证 |

> 2026-08-16 GitHub API 实测: NOFX 12,693★/push 2026-08-15(活跃); NOF0 2,750★/push 2025-12-07(**半年无提交,搁置**); CloddsBot 695★(2026-01 创建,已超过 2026-07 的 498★); ai-trading-agent 538★/push 2025-10(**创建即停更**)。

### 商业产品(12 个, 2026-07)

| 产品 | HL 支持 | HL 接入 | 形态 | 策略/bot | AI | 收费 | 安全模型 |
|:-----|:-----|:-----|:-----|:-----|:-----|:-----|:-----|
| Tealstreet | ✅(含链上) | API key/HL 钱包,密钥只存本机 | Web + 桌面 + PWA | 无 bot,手动 pro 终端(chaser 追单/快捷单/宏) | 未查证 | 免费(返佣) | 无服务器,key 不离开设备 |
| HyperDash | ✅(HL 专属) | 钱包登录 | Web SaaS + TG 鲸鱼提醒 | Copytrade、TWAP | 未查证 | 未查证 | 钱包链上非托管 |
| pvp.trade | ✅ | Telegram bot | **TG 群即交易界面** | 社交跟单/反向跟单 | 未查证 | 未查证 | 疑托管钱包 |
| Coinrule | ✅(链上原生) | CEX API key;HL 链上 | Web SaaS | 无代码 IF-THEN 规则 bot、模板、跟单、回测 | **AI Agentic bot + MCP** | Freemium | 未查证 |
| Gainium | ✅(Spot+Futures) | 未查证 | Web SaaS;开源可 docker 自托管 | 网格/DCA/Combo/指标 bot/Webhook(TV) | Max Gain AI 助手 | Free + 付费 | 可自托管 |
| WunderTrading | ✅(HL 专页) | **钱包连接** | Web SaaS | Signal(TV)/Grid/DCA/市场中性/跟单/终端 | MCP Server / AI bot | 订阅制 | 未查证(HL 钱包制) |
| Katoshi AI | ✅(HL 自动化引擎) | 未查证 | Web SaaS | 算法策略构建/部署;TV 信号/Webhook | AI Agents + MCP | 未查证 | 未查证 |
| Insilico Terminal | ✅ | 未查证 | 专业 OEMS 终端 | 手动高级执行: Chase/Swarm/TWAP/Scale | 未查证 | 免费 | 未查证 |
| Dexari | ✅(powered by HL) | 自托管钱包(Turnkey) | 移动 App | 手动 perps/spot ≤40x;TWAP;高级 DCA 规划中 | 未查证 | 低费率零 gas | 自托管无 KYC MFA |
| Mizar | 未查证 | — | Web SaaS | DCA/sniper bot、anti-rug、跟单市场 | 未查证 | 按次付费 + $MZR | CEX=API key;DEX=ETH vault |
| 3Commas | ❌ 未支持 | — | Web + 移动 | Grid/DCA/Signal/套利/Smart Trade + 跟单市场 | AI Assistant | 订阅制 | API key 托管 |
| Cryptohopper | ❌ 未支持 | — | Web + 移动 | 自动 bot/DCA/做市/套利/Trailing/Copy/组合 | AI Trading + MCP + Hopper Arena | 订阅制 | API key 托管 |

> 子代理已派: 商业平台完整功能清单(3Commas/Pionex/Cryptohopper/Gainium)与商业 MCP 工具清单。

---

## 二、高级订单/策略功能对照(2026-08-15 一手,72 页官方文档)

| 平台 | 回调买/卖 | TWAP | 阶梯单 | 冰山 | 网格 | DCA/加仓 | 追踪止盈/损 |
|:-----|:-----|:-----|:-----|:-----|:-----|:-----|:-----|
| 币安 | 追踪止损(trailingDelta 已验证) | 算法单存在,反爬未查证 | 未查证 | ✅(icebergQty,GTC) | 无官方 bot | 无官方 DCA | ✅(trailingDelta) |
| OKX | 移动止盈止损(variance+激活价) | ✅(浮动比例/上限/间隔/均量/总量) | 无(算法单类型无 ladder) | ✅(浮动比例/上限/均量/总量) | ✅(上下界/网格数/等差等比/TP/SL) | ✅(price steps/TP target/加仓) | ✅ |
| Bybit | Trailing Stop(距离或%+激活价) | ✅(5m-24h、5s-120s、±20% 随机、触发/停止价) | **Scaled Order**(2-100 档、4 分布) | ✅(4 种追价、价格上限、7 天) | 无 | 无官方 DCA | ✅ |
| 3Commas | **Trailing Buy**(激活价+追踪幅度) | 无独立 TWAP | Step Sell(≤6 档) | 无 | ✅(Long/Neutral/Short/Hedge、Pump Protection) | ✅(6 大参数) | ✅(激活价%+追踪%) |
| Pionex | DCA Trailing Mode(Price scale + Max Rebound Rate) | ✅(固定间隔 10s-5min) | 无(靠 DCA 等比加仓) | 无 | ✅(网格数/等差等比/触发价/滑点/Trailing up) | ✅(Simple/DIY/Composite/Trailing 四模式) | ✅(追踪止盈 Max Drawdown%) |
| Gainium | 无独立命名(退出侧 Trailing TP/SL) | 无独立 TWAP | 买入阶梯 = Scaled DCA(%,ATR,ADR) | 无(Smart Orders 近似) | ✅(步长/算术几何/反向/Smart Orders) | ✅(Scaled/TA/Custom 三类) | ✅(Trailing TP/SL) |

**参数共识(直接指导 pullback/ladder 设计)**:

1. **回调单**: 激活价(可选) + 回调幅度(百分比或绝对价差),触发后市价/限价执行。→ hyper_local `pullback` 策略参数对齐。
2. **阶梯单**: 总数量 + 档数(2-100) + 价格区间/步进 + 数量分布(均分/递增/递减/自定义)。→ hyper_local `ladder` 策略参数对齐(Bybit Scaled Order 最完整)。
3. **TWAP**: 总数量 + 间隔(5s-120s 可调或固定档)+ 每单数量 + 可选随机化(±20%)+ 触发/停止价 + 时长/次数上限。
4. **共性约束**: 并行实例上限(Bybit TWAP 20/账户)、单子最小占比、策略 7 天自动终止、资金预冻结、网格全额资金覆盖。

---

## 三、AI / MCP 生态(2026-08-16 一手,含 MCP 工具清单)

### 商业端 MCP(全部云端 SaaS)

| 产品 | MCP 端点 | 工具/能力(已查证) | 认证 | 客户端 |
|:-----|:-----|:-----|:-----|:-----|
| **Coinrule** | `https://cloud.coinrule.com/mcp` | **~20 工具**, 最全: 策略类(list_strategies/get_strategy/get_strategy_status/get_strategy_activity/get_strategy_trades/get_strategy_pnl/validate_strategy/create_strategy/**create_strategy_from_prompt**(自然语言生成并上线)/update_strategy/start/stop_strategy)、组合类(get_portfolio_balances/get_portfolio_holdings/get_recent_signals/list_exchange_accounts)、模板(list_strategy_templates)、篮子(list_baskets/create_basket/launch_basket)、回测(list_backtests/run_backtest/**run_backtest_scenarios** 多参数对比/get_backtest_export) | OAuth 2.1, Read/Write 两级权限(写工具对只读连接隐藏) | Claude/ChatGPT/Grok |
| **WunderTrading** | `https://wundertrading.com:2083/mcp` | **15 工具**: 行情(get_supported_exchanges/get_exchange_markets)、账户(get_api_profiles)、策略下单(place_strategy_trade/place_strategy_market_enter/place_strategy_swing)、策略管理(get_strategy/edit_trade_strategy/cancel_strategy/close_strategy_market)、查询(get_live_strategies/get_strategies_history/get_strategy_orders_history/export_*) | Header X-API-Key/X-Secret-Key(HMAC), 每账户≤10 key | Cursor/VS Code(Copilot)/Windsurf/Antigravity |
| **Katoshi AI** | `https://mcp.katoshi.ai?id=USER_ID` (streamable-http) | **6 工具**(按 Signal API 分组, 19 个 action 全支持: open/close_position、market/limit/stop_market/**scale**/grid/move/cancel、TP-SL、close/cancel/clear/sell all、set_leverage、adjust_margin、start/stop_bot); 工具名 docs 未列出(未查证); 另有 Agent API 自然语言命令 | Bearer token / URL api_key | Cursor/n8n/LangChain/CrewAI/Claude Desktop |
| **Cryptohopper** | `https://mcp-data.cryptohopper.com/mcp` | **仅 3 个只读行情工具**(截至抓取): get_orderbook/get_ticker/get_candles — **无交易类 MCP**; 按订阅档配额计费 | OAuth 2.0 / Bearer key | Claude Code/Cursor/VS Code/Zed/Gemini CLI/Codex |
| **Pionex** | pionex-mcp.mcpb 文件 (v0.2.57) | 装进 Claude Desktop; API 交易仅限 Web 端 | — | Claude Desktop |
| **Gainium** | "Gainium speaks MCP" | 连 Claude/ChatGPT/Cursor/n8n; Vibe Trading = web app 内嵌对话式 AI 交易 | — | 任意 MCP 客户端 |

**已查证共性**: 六家 MCP 全是**云端 SaaS 形态** — AI 经各家自建 MCP 端点操作云端账户, **无一家提供本地/自托管 MCP**(Hummingbot 是唯一自托管,见下)。认证分化: OAuth(Coinrule/Cryptohopper) vs API key header(WunderTrading/Katoshi)。

### 开源端 MCP

- **Hummingbot MCP**(hummingbot/mcp): 59★/40 forks, 最后 push 2026-03(近半年无新提交)。**唯一自托管 stdio 型**(`uv run main.py` 或 Docker); 面向 Claude Code 与 Gemini CLI; 经 Hummingbot API server(localhost:8000)桥接, 不直连交易所; 工具按 12 模块分组(account/bot_management/controllers/executors/gateway/history/market_data/portfolio/trading 等), README 仅具名 `configure_server`。定位是**管理 bot 实例(部署/配置/监控), 不面向写策略**。
- Rust MCP 框架: modelcontextprotocol/rust-sdk ⭐3803(官方,活跃,首选) > rust-mcp-stack ⭐190 > rmcp ⭐72。

### AI 原生新品类(2025-2026)

- NOFX 12.7k★(push 2026-08-15,活跃): "strategy is a language model" — LLM 决策循环 + Go 运行时硬风控; 无传统策略库/回测。
- NOF0 2.8k★(**push 2025-12,半年无提交 — 搁置**): 仅 HL, AI 竞技场(多 LLM 各持 $10k 实盘竞赛)。
- CloddsBot 695★(2026-01 创建, 增速最快): 预测市场 + 7 永续所 + DEX, 118+ 模板, 自然语言驱动下单。
- ai-trading-agent 538★(2025-10 创建即停更): HL AI 交易 agent。
- **2026 新竞品扫描(GitHub API)**: topic:crypto-trading-bot / topic:hyperliquid-bot 中 2025-10 后创建的项目最高仅 174★(Polymarket bot), **无 star>200 的新重磅玩家** — 市场格局稳定, 竞品面无新增。
- 行业观察: **共同点 = LLM 直接决策下单、无沙箱脚本、无回测门禁**。

### AI 功能三派(商业平台, 2026-08)

| 派别 | 代表 | 做法 |
|:-----|:-----|:-----|
| Agent 自主构建策略 | 3Commas QuantPilot | AI 端到端构建/回测/优化策略(early access) |
| 专职 agent 矩阵 + 按 token 计费 | Pionex.AI(Beta) | 4 专职 agent(Support/Pi/Quant/Coder) + Agents Market; Free/Plus 8U/Pro 20U/Max 100U |
| 对话式操作助手(免费内置) | Gainium Max Gain | 自然语言建/改 bot、调试、查 screener、读账户 |
| AI 对战竞技场 | Cryptohopper Hopper Arena | AI 模型各持 $10,000 实盘对战, 每 5 分钟按 Crypto.com 价格下单(详情 Cloudflare 未查证) |

### 差异化空白(本重构的支点)

> "本地私钥 + 本地 MCP server + 只读/写工具分离 + 回测门禁 + Dry Run 默认" 组合目前**无直接竞品**。Pionex/Gainium 均已上线 MCP 但只做"账号侧"云端 MCP, 本地私钥+本地 MCP 仍是空白。(我们暂不做本地 MCP, 2026-08-24 决策)

---

## 四、Telegram 生态(2026-08-15)

| 项目 | 形态 | 命令/功能 | 安全 |
|:-----|:-----|:-----|:-----|
| Freqtrade | 斜杠命令 + 内联按钮 + 自定义键盘 | 40+ 命令:/start /stop /status(含 table 视图)/order /trades /count /forceexit(按钮选仓)/forcelong/forceshort/forcebuy(需 force_entry_enable 开关)/profit /performance /balance /daily /weekly /locks /unlock /reload_config /logs 等 | chat_id 白名单 + authorized_users 双白名单;force 命令默认禁用 |
| Hummingbot Condor | TG bot 本身(LLM 会话 + 语音 + 命令) | /start /portfolio /agent(LLM 代理)/executors /bots /new_bot /routines /trade /swap /lp /servers /keys /gateway /web(5 分钟 dashboard 链接)/update(admin) | User Whitelist;API key 加密存服务端不进 TG;建议 Tailscale |
| OctoBot | 内置 TG 接口 | 运行时长/组合/未平仓/盈利/市场理解/风险参数、改风险、停止、紧急交易,/help | 仅 chat-id + token;可关 privacy 听 TG 群消息当信号 |

**安全模式五条主线(源码实证)**:
1. Chat/用户 ID 白名单(空=无人可控)
2. 危险操作默认禁用 + 配置开关(force_entry_enable)
3. 防误触: 内联按钮选择 + Cancel;二次确认需自建(teloxide Dialogues 正合适)
4. 群组风险提示: 群成员都能控制 bot;支持 topic_id 限定
5. 消息降噪: notification_settings 按类型 on/silent/off

**Rust 框架选型(实测)**: teloxide ⭐4202(框架级,dptree dispatcher、Dialogues 对话框、Bot API 9.2 覆盖)首选;frankenstein ⭐366(纯客户端库)次之;grammY 是 TS 非 Rust。

---

## 五、商业平台完整功能对照(2026-08-16 一手)

| 功能类别 | 3Commas | Pionex | Cryptohopper | Gainium |
|:-----|:-----|:-----|:-----|:-----|
| **核心机器人** | DCA(100+ 对)、Grid、Signal Bot、Smart Trade、Terminal | 16+ 免费: Grid/Infinity/Leveraged/Margin/Reverse Grid、DCA 4 模式、Rebalancing、Smart Trade、TWAP、Trailing Up、Futures Grid/DCA/Moon、Signal Bot | Automatic Trading、DCA、Trailing、Manual/Copy/Portfolio Bot、三角套利/交易所套利/做市机器人 | Grid、DCA、Combo 三类零代码 + Smart Trading Terminal |
| **回测** | ✅ 1m K 线 + 11 指标 | ⚠️ 整体未查证 | ✅ Backtesting 功能页 | ✅ 手动逐 bar 回放 + 自动化;付费含无限客户端回测 |
| **模拟盘 Paper** | ✅ 全档含 Demo Account | 未查证 | ✅ 每订阅送 1 个模拟器 bot | ✅ 免费无限模拟(9 所现货&合约含强平) |
| **信号/Webhook** | ✅ Signal Bot 接 TV 信号 + webhook + Pine Script | ✅ Signal Bot;TV 细节未查证 | ✅ Trading Signals 订阅 | ✅ Webhook JSON 触发开/平/加仓,动作数组 |
| **跟单/社交** | 未查证 | ✅ KOL 复制交易 + Leaderboard + 分组 | ✅ Social Trading + Copy Bot(1:1) | 未发现 |
| **策略/信号市场** | ⚠️ 内置策略模板卡(Breakout/Swing/Scalping 等一键启动);独立市场未查证 | ✅ KOL bot 复制即隐式市场 + 排行榜 | ✅ Marketplace: Templates/Strategies/Signals 三分类 + 卖家体系 | 未发现独立市场(Bot Database 对比站) |
| **AI 功能** | QuantPilot(agentic,端到端构建/回测/优化) | Pionex.AI(Beta): 4 专职 agent + Agents Market, 按 token 计费 | AI Trading(Algorithm Intelligence) + Hopper Arena(实盘对战) | Max Gain AI(免费内置对话助手) + Vibe Trading |
| **MCP/开放接口** | Developers API Read&Write(Expert 档) | ✅ pionex-mcp.mcpb(v0.2.57,Claude Desktop) | ✅ Cryptohopper MCP(只读行情) | ✅ "speaks MCP" + n8n;**2026 已整体开源** |
| **移动端** | ✅ iOS App | ✅ 官方 App(API 交易仅 Web) | ✅ web/phone/tablet/手表 | ⚠️ 跨平台监控;原生 App 未查证 |
| **通知** | 未查证 | ✅ 公告中心 1802 篇 + 2FA 安全提醒 | ✅ Triggers 自定义动作 + App 通知页签 | ✅ Screener Smart Alerts + Chrome 扩展 |
| **风控** | ✅ 11 指标 + Trailing TP/SL + Multiple TP | ✅ Proof of Reserves/Merkle + Bot Pause | ⚠️ Trust/Security 承诺 | ✅ 多 TP/SL、SL 移 TP、跟踪 TP/SL、DCA 阶梯 |
| **收费** | 订阅 $20/$50/$140 | 交易所模式: 机器人全免费只收交易费(现货 0.05%、合约 0.02%/0.05%); Futures Moon 收 20% 利润 | 订阅 $24.16/$57.50/$107.50 | 免费 + credits($0 起, HL 专属无限免费档) + 开源自托管 |

**要点**:
- **3Commas**: 四大交易产品(DCA/Grid/Signal/Smart Trade)+ Terminal;回测 1m K 线 11 指标;Demo Account 全档含;Signal Bot 接 TV + Pine Script;支持 HL;QuantPilot agentic AI;新增 DEX/Stocks/Prediction market 入口;社区 118k+ 会员。
- **Pionex**: 交易所模式(机器人全免费,只赚交易费);16+ 机器人;KOL 跟单体系完整;Pionex.AI 4 专职 agent(Support/Pi/Quant/Coder)按 token 计费;发 .mcpb 文件接 Claude Desktop;Earn 理财 + RWA 股票代币 + Pionex 卡;月交易量 $60B+、500 万用户。
- **Cryptohopper**: Pro Tools(市场/交易所套利、做市)是高阶付费点;Strategy Designer 无代码 130+ 指标可上架 Marketplace;每订阅送模拟器;Copy Bot 1:1;AI Trading + Hopper Arena;MCP 只做行情;用户 115 万+。
- **Gainium**: **2026 整体开源**(docker-sh 微服务 + backtester TS 引擎);Screener 选币器 + 免费 Chrome 扩展;paper trading 免费无限;Max Gain AI 免费内置;Trade Journal 自动记账;5 个免费计算器;**Hyperliquid 专属无限免费档**(与 hyper_local 定位高度相关);credit 双轨制。

---

## 六、功能统计(2026-07)

### 开源 8 框架

| 功能 | 具备 | 备注 |
|:-----|:-----|:-----|
| 回测 | 7/8 | Jesse 对 HL 关闭,JFreqtrade 受 5000 根限制 — **HL 回测是行业普遍弱项** |
| Paper/Dry Run | 6/8 | 主流标配 |
| 开箱网格策略 | 4/8 | Hummingbot / Passivbot / OctoBot / chainstack |
| Web UI | 6/8 | 无一家桌面原生 |
| 桌面原生 GUI | 0/8 | 全行业空白 |
| AI(LLM) 功能 | 5/8 | 形态分裂: 生成 Python(Jesse)vs LLM 直接决策(OctoBot/NOFX 系) |
| 参数自动优化 | 4/8 | Hyperopt/Optuna/进化算法 |
| 密钥加密存储 | 2/8 | 仅 Hummingbot/NOFX;Freqtrade/Passivbot 明文 |
| HL Agent/API Wallet | 2/8 | Hummingbot/Freqtrade 文档均强调 |

### 商业 12 产品

| 特征 | 情况 |
|:-----|:-----|
| 支持 HL | 9/12(3Commas/Cryptohopper 未支持,Mizar 未查证)— HL 生态是新品主战场 |
| HL 接入方式 | 钱包/agent wallet 授权为主流;老牌 SaaS 仍是 API key 托管 |
| 量化 bot + 回测 | 有但全部云端托管 |
| 本地密钥 | 仅 Tealstreet(桌面)与 Dexari(移动)— 均无量化/回测 |
| AI/MCP | 6/12 具备,全在云端 |
| 桌面原生 | 仅 Tealstreet 1 家(手动终端) |

---

## 七、功能差距分析: hyper_local 需要增加什么

> ⚠️ 本节基于 requirements v2.1,最终排期以 v3.0 为准: TV webhook/冰山/括号/追踪止盈 → v1 之后评估; 扫描器 → P2(`ricow scan`); CLI 命令名 `ricow`(非 `hl`)。
> 对比基线: doc/requirements.md v2.1(已完成功能 + 5.2 CLI + 5.3 待实现策略)。
> 方法: 竞品普遍具备(行业标准)且我们缺失 → 建议新增;竞品有但不符合定位 → 维持"明确不做"。
> 本版已整合 2026-08-16 子代理补充(商业 MCP 工具清单 / 商业平台功能清单 / star 刷新)。

### 7.1 竞品有、我们已规划(重构计划 D1-D8 已覆盖)

| 功能 | 竞品参照 | 我们现状 | 状态 |
|:-----|:-----|:-----|:-----|
| 回调单 pullback | 3Commas Trailing Buy / Pionex Max Rebound / Bybit Trailing Stop | 未实现 | 重构 Phase 3 新增 |
| 阶梯单 ladder | Bybit Scaled Order / 3Commas Step Sell | 未实现 | 重构 Phase 3 新增 |
| CLI | Freqtrade / Passivbot / Hummingbot | 未实现(5.2 已设计) | 重构 Phase 2 |
| MCP server | Coinrule/WunderTrading/Katoshi/Pionex/Gainium 全上线 | **暂不做(2026-08-24 决策)** | 远期可选 |
| Telegram 双向管理 | Freqtrade 40+ 命令 / Condor | **暂不做(2026-08-24 决策)** | 远期可选 |

> **新证据(2026-08-16)**: MCP 已从"少数先行者"变为**行业标配动作** — Pionex 发 .mcpb 文件、Cryptohopper 上线 MCP(虽只读行情)、Gainium 宣布 "speaks MCP" 并整体开源。(我们暂缓跟进, 2026-08-24 决策)Gainium 的 Hyperliquid 无限免费档说明 HL 生态免费工具是被验证的获客打法。

### 7.2 竞品普遍具备、我们缺失(建议本轮评估新增)

| 功能 | 竞品参照 | 缺失现状 | 建议 |
|:-----|:-----|:-----|:-----|
| 冰山订单 Iceberg | Bybit/OKX/币安 API | 5.3 已列 P1 | 维持 P1 排期(重构后补) |
| 括号订单 Bracket(OCO) | Gainium 原生 / Coinrule | 5.3 已列 P1 | 维持 P1 排期 |
| 追踪止盈 Trailing TP | 全行业标配(3Commas/Bitsgap/Gunbot) | 5.3 已列 P1 | 维持 P1 排期(网格 trailing 已有止损侧) |
| TradingView 信号 webhook | OctoBot 主推 / WunderTrading / 3Commas Signal Bot / Gainium | 5.3 已列 P1 | 维持 P1 排期 — **商业平台 4/4 全有, 行业标准** |
| 市场扫描器 | Cryptohopper Scanner / 3Commas 发现工具 / Gainium Screener | 5.3 已列 P1 | 维持 P1 排期 |
| 模拟盘/Paper | 6/8 开源 + 3Commas/Gainium/Cryptohopper 商业标配 | ✅ 已有 Dry Run | 已覆盖 |
| 回测 | 7/8 开源 + 商业标配 | ✅ 已有 BacktestContext | 已覆盖(HL 5000 根限制是行业弱项, 本地 SQLite 是卖点) |

### 7.3 竞品有、我们明确不做(维持,不新增)

| 功能 | 理由 |
|:-----|:-----|
| Copy trading/跟单/社交/策略 marketplace | requirements 5.6 明确不做;定位不符(虽有 3/4 商业平台具备, 但本地化单机场景无此土壤) |
| 云端 SaaS/移动端/浏览器插件 | 本地化核心差异化 |
| LLM 直接决策下单 | 不可回测不可审计(差异化支点) |
| 参数自动优化(Hyperopt 类) | 散户过拟合风险,暂缓观察 |
| 无代码拖拽编辑器(Coinrule 类) | Lua + AI 生成已足够 |
| 三角套利扫描 | 超出当前定位(3Commas/Cryptohopper 均有, 但跨所接口基础在, 远期评估) |

### 7.4 值得新增评估的候选(竞品近期动向)

| 候选 | 竞品证据 | 分析 |
|:-----|:-----|:-----|
| **AI 对话式运营助手** | Gainium Max Gain(免费内置)、Pionex.AI Support agent、3Commas QuantPilot | 引擎已备 AI 生成策略 + 回测门禁;对话式入口远期可选时再评估 |
| **策略分享(只读导出)** | Pionex KOL 复制、Cryptohopper Marketplace | 完整做社交不符定位;只读分享(策略 TOML 导出即分享)低成本高价值, 列入评估 |
| 价格/信号提醒(独立于策略) | 商业平台标配通知(Triggers/Smart Alerts) | 通知管道后置(随入口评估), 做"价格提醒"成本低;列入评估 |
| **Screener 扩展为 CLI `ricow scan`** | Gainium Screener + Chrome 扩展、Cryptohopper Scanner | 5.3 已列市场扫描器;Gainium 免费 Chrome 扩展证明"轻量选币工具"是获客入口, 重构后补 |
| AI 策略竞技场/backtest 排行榜 | NOF0 / Hopper Arena / Gainium 社区 | 需服务端,与本地化冲突;不采纳 |
| 策略模板市场(内置模板库) | 3Commas Marketplace / Pionex 16+ 机器人 | 内置 6 策略 + 模板随版本发布(重构方案已含),不开放第三方市场 |
| 多账户/子账户管理 | Freqtrade 多 bot、商业多账户、Katoshi 子账户/vault 管理 | HL Vault 已支持;远期评估,不在本轮 |
| 做市/套利 Pro 工具 | Cryptohopper Pro Tools、Gainium Combo、Pionex 套利 | 高阶付费点;现有 Grid/DCA/TWAP 之外规划, 远期评估 |

### 7.5 结论(给重构的产品输入)

1. **竞品面确认无新增威胁**: 2026 年无 star>200 新竞品;NOF0 搁置、ai-trading-agent 停更;格局与 2026-07 调研一致。
2. **调研结论**: MCP 行业标配化、TG 双向(Freqtrade/Condor 模板)、pullback/ladder 参数共识、CLI 主流形态 — 竞品情报留存;入口收敛为纯 CLI(2026-08-24 决策), MCP/TG 暂不做。
3. **AI 助手对标 Max Gain**: 免费内置、对话式、读账户/查行情/建策略 — 远期入口评估时的体验基准。
4. **文档同步项(历史)**: requirements 5.6 / product 非目标 / roadmap 非目标 三处 "MCP server 明确不做" 已于 2026-08-16 推翻、2026-08-24 再次收敛为不做;5.2 CLI 命令表补 `ricow scan`(✅); `ricow alert` 列为后续评估。

---

## 八、研究缺口

- [x] 商业 MCP 工具清单(Coinrule ~20 工具/WunderTrading 15/Katoshi 6/Cryptohopper 3 只读/Hummingbot 12 模块)— 2026-08-16 子代理查证
- [x] 2026-08-16 最新 star 数与功能定位变化 + 新竞品扫描 — 父代理 GitHub API 实测(无新重磅竞品)
- [x] 商业平台完整功能清单(3Commas/Pionex/Cryptohopper/Gainium)— 2026-08-16 子代理查证
- [ ] "Claude Code 经 MCP 操作自建交易系统" 公开案例 — 未查证(非关键路径, 远期评估 MCP 时再查)
