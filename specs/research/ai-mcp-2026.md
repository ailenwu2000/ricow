# 2026 AI-first 交易平台与 MCP 协议调研报告

> 调研时间: 2026-08-15 | 面向: hyper_local AI-first 重构
> 数据来源: doc/competitors.md(2026-07 两轮子代理实测)、doc/ai-assistant.md、doc/requirements.md、research/*.json(2026-08-15 GitHub API 原始响应)。
> 子代理会话网络请求被拒;Rust MCP 框架数据由父代理独立实测补充(见第五节)。

## 1) Coinrule / WunderTrading / Katoshi AI / Cryptohopper 的 MCP server 能力

- Coinrule:Web SaaS,无代码 IF-THEN 规则 bot(模板/跟单/回测)。提供 MCP + "AI Agentic bot",官网宣称可让 ChatGPT/Claude/Gemini/Grok 通过 MCP 建策略(自然语言生成交易规则)。HL 为链上原生(HL/Base/Arb/ETH),CEX 用 API key。来源:coinrule.com
- WunderTrading:Web SaaS,Signal(TV)/Grid/DCA/市场中性/跟单/交易终端。提供 MCP Server / AI bot;HL 用钱包连接。来源:wundertrading.com/en/hyperliquid-trading-bot
- Katoshi AI:Web SaaS "HL 自动化引擎",算法策略构建/部署;集成列表含 TradingView 信号 / Webhooks / AI Agents / MCP。来源:katoshi.ai
- Cryptohopper:Web+移动,自动 bot/DCA/做市/套利/Trailing/Copy/组合 bot;提供 AI Trading + MCP + AI 对战(Hopper Arena)。未支持 HL;API key 托管。来源:cryptohopper.com/features
- 各家 MCP 具体工具列表:未查证。
- 已查证共性:四家 MCP 全部是云端 SaaS 形态——AI 经各家自建 MCP 端点操作云端账户,无一家提供本地/自托管 MCP;核心卖点是"自然语言建策略/管理 bot"。
- 启示:商业端 MCP 与 hyper_local "本地私钥不出机"叙事相反;其安全模型依赖云端托管+API key 权限,无法直接借鉴为本地方案。

## 2) Hummingbot 的 MCP 集成

- hummingbot/mcp 仓库真实存在:59★/40 forks/5 open issues/Python/创建 2025-04-04/最后 push 2026-03-23/未 archived。近半年无新提交,维护活跃度存疑。来源:github.com/hummingbot/mcp
- 主仓库 hummingbot/hummingbot:19,466★,最后 push 2026-08-13(活跃);40+ CEX/DEX connector,HL 原生现货+永续(测试网/Vault/API key)。
- 界面矩阵:CLI + Dashboard(Streamlit Web)+ Condor(Telegram)+ REST API + MCP(接 Claude/Gemini)。
- 能力边界:hyper_local 设计文档明确评估——"Hummingbot MCP 面向运维、不面向写策略":定位是让 Claude/Gemini 管理 bot 实例(部署/配置/监控),与"LLM 生成可回测策略代码"是两条路线。来源:doc/ai-assistant.md
- MCP 具体 tool 列表:未查证。
- 凭证安全(已查证):Hummingbot 凭证经启动密码加密(config_crypt),本地存储。

## 3) 2026 年 AI 交易 agent 开源项目

- NOFX(NoFxAiOS/nofx):12.6k★。"strategy is a language model"——LLM 决策循环+Go 运行时不可覆盖硬风控;本地 Web 终端(127.0.0.1:3000);凭证加密静置、不出本机;无传统策略库/回测。
- NOF0(wquguru/nof0):2.8k★。仅 HL;AI 竞技场——多 LLM 各持 $10k 实盘竞赛;Go+Next.js 自部署。
- Hyper Alpha Arena(HammerGPT/Hyper-Alpha-Arena):1.1k★。HL+BN;AI Trader+Program Trader;86 因子+表达式引擎+AI 因子挖掘;测试网 paper;Apache-2.0。
- CloddsBot(alsk1992/CloddsBot):498★。预测市场+7 永续所+DEX;118+ 模板;自然语言驱动下单("核心即 Claude 代理");WebChat+21 种消息平台;自托管。
- ai-trading-agent(Gajesh2007/ai-trading-agent):出现在竞品来源清单,星数未记录 → 未查证。
- 其他参照:Nocturne 529★(仅 HL,LLM+TAAPI);OctoBot 6.2k★(ChatGPT+本地 Ollama LLM 交易模式);Jesse 8.2k★(JesseGPT 写/优化/调试策略);NautilusTrader 24.7k★(官方明确不做 UI/AI);Freqtrade 53,298★(2026-08-15 API 实测,FreqAI 传统 ML)。
- 通用 AI 助手经 MCP 操作交易系统的公开成熟案例:未查证(最接近的是商业 SaaS MCP 端点)。
- 行业观察:2025-2026 AI 原生新品类共同点 = LLM 直接决策下单、无沙箱脚本、无回测门禁;"AI 生成可回测代码"路线目前无人占位——hyper_local 的机会点。

## 4) 开源 Rust MCP server 框架(父代理 2026-08-15 实测)

| 项目 | 星数 | 状态 | 说明 |
|:-----|:-----|:-----|:-----|
| modelcontextprotocol/rust-sdk | ⭐3803 | 2026-08-15 活跃 | **官方 Rust SDK,首选** |
| rust-mcp-stack/rust-mcp-sdk | ⭐190 | 2026-08-14 活跃 | 社区异步工具包,备选 |
| 4t145/rmcp | ⭐72 | 2026-07-22 | 自称 "BEST Rust SDK for MCP",星数低 |
| finite-sample/rmcp | ⭐209 | 2026-08-10 | R 语言 MCP Server(非 Rust) |

结论:用 modelcontextprotocol/rust-sdk(官方,活跃),不建议自研(协议细节多)。

## 5) AI 操作交易系统的常见安全模式

- 只读工具与写工具分离:能——生成/修改策略脚本、生成策略配置、解读回测报告、只读状态问答;不能——直接下单、启停策略、读密钥、修改任何配置不经用户确认、主动联网抓取资讯。来源:doc/ai-assistant.md 能力边界表
- 操作确认(三道门禁):① Rhai AST 白名单静态校验(只允许注册过的 API;拒绝 eval/未知符号;操作数上限防死循环)→ ② 沙箱回测门禁(本地 K 线,自动出收益/回撤/交易数报告,不达标继续迭代)→ ③ 用户 diff 确认后才保存部署,默认先进 Dry Run。来源:doc/ai-assistant.md、doc/requirements.md
- 实盘保护层:风控默认启用;onboarding 门禁;下单二次确认;单日亏损熔断;密钥 OS Keyring 加密、HL Agent/API Wallet(主钱包私钥不进软件)、BN 禁提现只交易权限 key;发送给 LLM 的内容绝不含密钥/钱包地址/完整账户快照。来源:doc/requirements.md
- 竞品对照:OctoBot(LLM 直接决策、不可回测审计)与 CloddsBot(对话式下单)被列为反面参照——hyper_local 的差异化即"AI 只产出代码和建议、永不直接下单"。
- 竞品安全实践:Hummingbot config_crypt 启动密码加密凭证;NOFX 凭证加密静置不出本机;Tealstreet 无服务器、API key 不离开设备;Freqtrade config.json 明文私钥(负面案例)。
- 双人复核:交易领域 AI 操作的双人复核案例未查证。hyper_local 以"用户确认+默认 Dry Run+熔断"替代(个人桌面软件场景)。
- 对 AI-first 重构的安全设计结论:写工具(下单/启停策略/部署)必须走确认流程;只读工具(行情/回测/状态)可放开给 AI;凭证永不出本机、不送 LLM。

## 6) 对 hyper_local AI-first 重构的直接启示

1. 现 doc/requirements.md "5.6 明确不做"中明确排除 MCP server(roadmap.md 同列)——本次调研为推翻该决策提供依据。
2. 可占位空白:商业端 MCP 全是云端托管,开源端 Hummingbot MCP 面向运维,AI 原生新品全是"LLM 直接下单、无回测门禁"——"本地私钥+本地 MCP server+只读/写工具分离+回测门禁+Dry Run 默认"组合目前无直接竞品。
3. Rust 侧 MCP 框架选型已确认官方 rust-sdk,可直接开工。

## 7) 调研缺口

- [ ] 四家商业 MCP 的工具清单页(Coinrule/WunderTrading/Katoshi/Cryptohopper docs 站)
- [ ] hummingbot/mcp README(工具列表、支持的 LLM 客户端)
- [ ] NOFX / CloddsBot / ai-trading-agent 最新 star 与 README(2026-08 现状)
- [ ] "Claude Code 经 MCP 操作自建交易系统"公开案例检索
