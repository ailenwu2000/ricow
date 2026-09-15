# 竞品调研报告(2026-08-16)

> 调研时间: 2026-07 ~ 2026-08-16。方法: 3 子代理并行 + 父代理 GitHub API 实测, 抓取官方文档/README 原文。
> 标注 "未查证" = 页面未给出该项, 非 "不存在"。
> 原始资料: research/competitor-features-2026.md(功能对照总表)、research/competitor-orders-2026.md(订单策略)、research/ai-mcp-2026.md、research/telegram-bot-2026.md(原始 HTML 已随 backup 移除, 结论已提取)。

---

## 一、开源框架总览(2026-08-16 star 实测)

| 项目 | Stars | HL 支持 | 界面 | 内置策略 | 回测 | AI | 密钥存储 |
|:-----|:-----|:-----|:-----|:-----|:-----|:-----|:-----|
| Freqtrade | 53.3k | ccxt 现货+永续 | CLI + FreqUI Web + Telegram | 无成品, 信号框架 | ✅(HL 限 5000 根) | FreqAI 传统 ML | config.json 明文 |
| NautilusTrader | 25.6k | 原生 Rust adapter | 无 UI(明确不做) | 无成品, 事件驱动 | 纳秒级 | 无 | 环境变量/配置 |
| Hummingbot | 19.5k | 原生现货+永续 | CLI + Dashboard Web + Condor TG + MCP | PMM/网格/XEMM/套利/TWAP/LP | ✅ + paper | MCP(接 Claude/Gemini) | 启动密码加密 |
| NOFX | 12.7k | 原生 | 本地 Web 终端 | "LLM 即策略" | 未查证 | 核心即 AI | 加密静置不出本机 |
| Jesse | 8.3k | 原生, **HL backtesting:False** | 自托管 Web GUI | 研究框架 300+ 指标 | 高精度 + Optuna | JesseGPT | 自托管 |
| OctoBot | 6.4k | ccxt HL 现货 | Web UI + App + Telegram | 网格、DCA、TV 信号、AI | 回测 + paper | ChatGPT + Ollama | 未查证 |
| Passivbot | 2k | ccxt 层 | 纯 CLI | 逆势马丁网格 + trailing + unstucking | Rust 回测 + 进化 | 无 | 明文 |
| Hyper Alpha Arena | 1.1k | 原生 | Web UI(Docker) | AI + Program Trader | 测试网 paper | LLM 自主交易 | 未查证 |

> 2026-08 变化: NOF0(2.8k)半年无提交已搁置;ai-trading-agent(538★)创建即停更;CloddsBot 695★(2026-01 创建, 增速最快);**无 star>200 新竞品** — 格局稳定。

## 二、商业产品总览(12 个)

| 产品 | HL 支持 | HL 接入 | 形态 | 策略/bot | AI/MCP | 收费 | 安全模型 |
|:-----|:-----|:-----|:-----|:-----|:-----|:-----|:-----|
| Tealstreet | ✅ | API key/HL 钱包, 密钥只存本机 | Web + 桌面 + PWA | 手动 pro 终端(chaser/快捷单/宏) | 未查证 | 免费(返佣) | 无服务器, key 不离开设备 |
| HyperDash | ✅ | 钱包登录 | Web SaaS + TG 提醒 | Copytrade、TWAP | 未查证 | 未查证 | 钱包链上非托管 |
| pvp.trade | ✅ | Telegram bot | TG 群即交易界面 | 社交跟单 | 未查证 | 未查证 | 疑托管钱包 |
| Coinrule | ✅ 链上原生 | CEX API key; HL 链上 | Web SaaS | 无代码 IF-THEN 规则、模板、跟单、回测 | **MCP ~20 工具** + AI Agentic | Freemium | 未查证 |
| Gainium | ✅ | 未查证 | Web SaaS; **2026 整体开源** | Grid/DCA/Combo + Webhook | Max Gain AI + MCP + Vibe Trading | Free + credits | 可自托管 |
| WunderTrading | ✅ | **钱包连接** | Web SaaS | Signal/Grid/DCA/市场中性/跟单/终端 | **MCP 15 工具** | 订阅制 | HL 钱包制 |
| Katoshi AI | ✅ | 未查证 | Web SaaS | 算法策略构建/部署 | **MCP 6 分组工具** + Agent API | 未查证 | 未查证 |
| Insilico Terminal | ✅ | 未查证 | 专业 OEMS 终端 | Chase/Swarm/TWAP/Scale | 未查证 | 免费 | 未查证 |
| Dexari | ✅ | 自托管钱包(Turnkey) | 移动 App | 手动 perps ≤40x; TWAP | 未查证 | 低费率 | 自托管无 KYC |
| Mizar | 未查证 | — | Web SaaS | DCA/sniper/anti-rug | 未查证 | 按次付费 | CEX API key |
| 3Commas | ❌ | — | Web + 移动 | Grid/DCA/Signal/Smart Trade + 跟单市场 | QuantPilot(agentic) | 订阅 $20-140 | API key 托管 |
| Cryptohopper | ❌ | — | Web + 移动 | 自动/DCA/做市/套利/Trailing/Copy/组合 | MCP(只读行情 3 工具) + Hopper Arena | 订阅 $24-108 | API key 托管 |

## 三、商业 MCP 工具清单(2026-08-16 一手)

| 产品 | 端点 | 工具/能力 | 认证 |
|:-----|:-----|:-----|:-----|
| Coinrule | cloud.coinrule.com/mcp | **~20 工具最全**: 策略(list/create/update/start/stop/validate + `create_strategy_from_prompt` 自然语言建策略)、组合(get_portfolio_balances/holdings)、篮子(create/launch_basket)、回测(run_backtest/scenarios) | OAuth 2.1, Read/Write 分级 |
| WunderTrading | wundertrading.com:2083/mcp | **15 工具**: 行情/账户/策略下单(place_strategy_trade/market_enter/swing)/策略管理/历史导出 | HMAC header, ≤10 key |
| Katoshi AI | mcp.katoshi.ai | **6 工具**(按 Signal API 分组, 19 action: open/close_position、market/limit/stop/scale/grid/move/cancel、TP-SL、set_leverage 等) | Bearer / URL key |
| Cryptohopper | mcp-data.cryptohopper.com/mcp | **仅 3 个只读行情工具**: get_orderbook/get_ticker/get_candles — 无交易类 | OAuth 2.0 / Bearer |
| Pionex | pionex-mcp.mcpb 文件 | 装进 Claude Desktop; API 交易仅 Web | — |
| Gainium | "speaks MCP" | 连 Claude/ChatGPT/Cursor/n8n; Vibe Trading | — |
| Hummingbot(开源) | 本地 stdio (uv/Docker) | 12 模块: account/bot_management/controllers/executors/gateway/history/market_data/portfolio/trading; 面向 Claude Code/Gemini CLI; 经 API server 桥接 | 本地 |

**关键结论**: 六家商业 MCP 全是**云端账号侧**, 无一家本地/自托管; Hummingbot 是唯一自托管但定位是"管理 bot 实例"非写策略 → **"本地私钥 + 本地 MCP + 只读/写分离 + 回测门禁"无直接竞品**。

## 四、商业平台功能对照(2026-08-16 一手)

| 功能类别 | 3Commas | Pionex | Cryptohopper | Gainium |
|:-----|:-----|:-----|:-----|:-----|
| 核心机器人 | DCA/Grid/Signal/Smart Trade/Terminal | 16+ 免费(Grid 家族/DCA/TWAP/Rebalancing) | 自动/DCA/Trailing/Copy/Portfolio/套利/做市 | Grid/DCA/Combo + Terminal |
| 回测 | ✅ 1m K 线 11 指标 | 未查证 | ✅ | ✅ 手动逐 bar + 自动化 |
| 模拟盘 | ✅ 全档 Demo | 未查证 | ✅ 每订阅送模拟器 | ✅ 免费无限 |
| 信号/Webhook | ✅ TV 信号 + Pine Script | ✅ Signal Bot | ✅ Trading Signals | ✅ Webhook JSON |
| 跟单/社交 | 未查证 | ✅ KOL 复制 + 排行榜 | ✅ Social + Copy Bot | 未发现 |
| 策略市场 | 内置模板卡 | ✅ KOL 隐式市场 | ✅ Marketplace 三分类 + 卖家 | 未发现 |
| AI | QuantPilot agentic | Pionex.AI 4 agent + token 计费 | AI Trading + Hopper Arena | Max Gain 免费内置 |
| MCP | Developers API | ✅ .mcpb 文件 | ✅ 只读行情 | ✅ + 开源 |
| 移动端 | ✅ iOS | ✅ App | ✅ 全设备 | 未查证 |
| 收费 | 订阅 $20-140 | 交易所模式(机器人免费只收交易费) | 订阅 $24-108 | 免费 + credits + 开源 |

## 五、Telegram 生态

| 项目 | 形态 | 命令 | 安全 |
|:-----|:-----|:-----|:-----|
| Freqtrade | 斜杠 + 内联按钮 + 自定义键盘 | 40+: /status(含 table)/order/trades/count/forceexit(按钮)/forcelong/forcebuy(需开关)/profit/performance/balance/locks/logs | chat_id + authorized_users 双白名单; force 默认禁用 |
| Hummingbot Condor | TG bot 本身(LLM 会话 + 语音) | /portfolio/agent/executors/bots/new_bot/trade/swap/keys/web | User Whitelist; key 不进 TG |
| OctoBot | 内置 TG | 运行状态/盈利/改风险/停止/紧急交易 | chat-id + token |

**安全模式五条**: ① ID 白名单(空=无人可控) ② 危险命令默认禁用+开关 ③ 内联按钮选择+Cancel ④ 群组风险提示+topic 限定 ⑤ 通知降噪 on/silent/off。
**框架选型**: teloxide(4.2k★, 框架级 + Dialogues)首选; frankenstein(366★, 纯客户端)次之。

## 六、竞争洞察(对 ricow)

1. **"本地私钥 + 量化策略/回测 + AI 写策略"组合无直接竞品**: 本地密钥阵营(Tealstreet/Dexari)是手动终端无量化; 量化+回测阵营密钥在云端; 开源框架有量化但全是 CLI/Web 且无本地 MCP(我们暂不做本地 MCP, 2026-08-24 决策)。
2. **MCP 已是行业标配动作(2026-08 新证据)**: Pionex .mcpb、Cryptohopper MCP、Gainium "speaks MCP"+开源 — 竞品标配事实; 我们暂缓跟进(2026-08-24 决策)。
3. **AI 三派路线分化**: agent 自主构建(QuantPilot)/ agent 矩阵(Pionex.AI)/ 对话助手(Max Gain, 免费内置) — "AI 生成可回测代码 + 回测门禁 + 确认部署"无人占位。
4. **HL 回测是行业普遍弱项**: Jesse 关闭 HL 回测、Freqtrade 5000 根限制 → 本地 SQLite 增量 K 线库 + 本地回测是明确卖点。
5. **模拟盘+回测是商业标配**(3Commas/Gainium/Cryptohopper) → 我们 Dry Run 已覆盖, 不落后。
6. **信号/Webhook 是 4/4 商业标配** → TradingView 信号应排上(最终排期 v1 之后评估, 见第七节)。
7. **Gainium Hyperliquid 无限免费档** → HL 生态免费工具是被验证的获客打法, 与我们"免费+零费率"一致。
8. **AI 助手体验基准 = Max Gain**: 免费内置、对话式、读账户/查行情/建策略 — 远期入口评估时对标。

## 七、功能差距: 我们要增加什么

### 已规划(重构 D1-D8 覆盖)
pullback、ladder、CLI — 全部获得竞品新证据支撑; MCP server、TG 双向后置(2026-08-24 决策)。

### 建议新增评估(竞品普遍具备)
| 功能 | 竞品参照 | 建议 |
|:-----|:-----|:-----|
| TradingView 信号 webhook | 商业 4/4 标配 | 调研期建议 P1; 最终排期: v1 之后评估(product-plan/requirements 定稿) |
| 市场扫描器 ricow scan | Gainium Screener/Cryptohopper Scanner | 调研期建议 P1; 最终排期: P2(product-plan D10 定稿) |
| 冰山/括号/追踪止盈 | Bybit/OKX/全行业 | 调研期建议 P1; 最终排期: v1 之后评估 |
| AI 对话式运营助手(只读工具集) | Gainium Max Gain | 远期入口评估时实现 |
| 策略分享(只读 TOML 导出) | Pionex KOL/Cryptohopper Marketplace | 低成本, 列入评估 |
| 价格/信号提醒 | Triggers/Smart Alerts | 已有通知管道, 成本低 |

### 明确不做(维持)
跟单/社交/marketplace、云端 SaaS/移动端、LLM 直接下单、参数自动优化、拖拽编辑器、三角套利、做市(远期)。
