# 2026 年加密货币交易 Telegram Bot 管理生态调研报告

> 调研日期: 2026-08-15。星数/时间均为当日 GitHub API 实测。

## 1) Freqtrade 的 Telegram 遥控

仓库:freqtrade/freqtrade,53,298 stars,最后 push 2026-08-15(极活跃);最新 release 2026.7(2026-07-31)。实现于 freqtrade/rpc/telegram.py(python-telegram-bot)。

**UI 形态(三种并存)**:
- 斜杠命令为主;另有内联按钮键盘(16 个 CallbackQueryHandler:/forceexit 时为每个持仓渲染 force_exit__{trade_id} 按钮+"Cancel";/status 带 Refresh 按钮)
- 自定义快捷键盘:config.json 的 telegram.keyboard 可配 3 行命令按钮(默认 [["/daily","/profit","/balance"],["/status","/status table","/performance"],["/count","/start","/stop","/help"]]),键盘仅允许白名单命令且不支持参数

**命令清单**:
- 系统:/start、/pause|/stopentry|/stopbuy(暂停不新开仓)、/stop、/reload_config、/show_config、/logs [limit]、/help、/version
- 状态:/status、/status <id>、/status table、/order <id>、/trades [limit]、/count、/locks、/unlock、/marketdir、/list_custom_data
- 交易操作:/forceexit <id>|/fx、/forceexit all、/forcelong <pair> [rate]、/forceshort <pair> [rate]、/forcebuy <pair>(forcelong 别名)、/delete <id>、/reload_trade、/cancel_open_order|/coo
- 指标:/profit [n]、/profit_long|short、/performance、/balance、/balance full、/daily、/weekly、/monthly、/stats、/exits、/entries、/whitelist、/blacklist [pair]
- ⚠️ forcelong/forceshort/forcebuy 需配置 force_entry_enable = true 才可用(默认关)
- 通知降噪:notification_settings 按消息类型设 on/silent/off

来源:freqtrade.io/en/stable/telegram-usage/;github.com/freqtrade/freqtrade/blob/develop/freqtrade/rpc/telegram.py

## 2) Hummingbot Condor 的 Telegram

关键事实:Condor 本身就是 Telegram bot——README 首句 "A Telegram bot for monitoring and trading with Hummingbot via the Hummingbot API"。2026-04-21 官方博客发布,定位"AI 交易代理开源 harness"。hummingbot/condor:153 stars,Python,最后 push 2026-08-13。

**命令清单**(condor-docs getting-started/telegram.mdx 原文):/start(主菜单+API 状态)、/portfolio、/agent(LLM 交易代理)、/executors、/bots(启停容器/日志/系统指标)、/new_bot、/routines、/trade(CEX 下单)、/swap(DEX)、/lp(CLMM 流动性)、/servers、/keys(交易所凭据)、/gateway、/web(5 分钟有效的 dashboard 登录链接)、/update(admin 专用)。

**交互形态**:会话式自然语言(接 Claude/Gemini/OpenRouter 等 LLM);语音命令(Whisper 本地转写,音频不出服务器);通知(executor 状态变化/风控触发/bot 状态/价格提醒);支持多用户团队(添加多个 user IDs)。

**安全**(文档 Security 节):User Whitelist(仅授权 TG 用户 ID)、API 密钥加密存服务端不进 TG、Session Isolation;README 强烈建议 Tailscale 保护 Hummingbot API(8000 端口)不暴露公网。

**与经典 Hummingbot 的关系**:主仓库 master 完整文件树(2,163 文件)已无任何 telegram 文件,docs.hummingbot.org/telegram 页面为空壳(SPA)——2026 年经典 Hummingbot 的 TG 交互已被 Condor 承接。

来源:github.com/hummingbot/condor;github.com/hummingbot/condor-docs/blob/main/getting-started/telegram.mdx;hummingbot.org/blog/posts/introducing-condor/

## 3) 其他值得参考的加密交易 TG Bot

- pvp.trade(已直连验证):TG 原生社交跟单平台,首页原文 "Long and short tokens together in your Telegram group."——TG 群即交易界面的代表。来源:pvp.trade
- Mizar:mizar.com Cloudflare 拦截,未查证。
- TradingBoat:域名探测被拒,未查证。
- OctoBot(Drakkar-Software/OctoBot,6,401 stars,push 2026-08-14):内置 Telegram 接口——显示运行时长/组合/未平仓单/盈利/市场理解/风险参数、改风险、停止、触发紧急交易,/help 看全命令;配置仅 chat-id+token;还能关 privacy mode 监听 TG 群消息当信号源。来源:github.com/Drakkar-Software/OctoBot 及 docs/content/guides/octobot-interfaces/telegram.mdx
- Passivbot(enarjord/passivbot,2,057 stars):主仓库文件树无 telegram 文件,无内置 TG 遥控。来源:github.com/enarjord/passivbot

## 4) Rust 生态 TG Bot 框架现状(2026-08 实测)

| 框架 | 星数 | crates 最新版 | crates 更新 | 仓库最后 push | 性质 |
|---|---|---|---|---|---|
| teloxide | 4,202 | 0.17.0 | 2025-07-11 | 2026-08-08 | 全功能框架 |
| frankenstein | 366 | 0.50.2 | 2026-07-03 | 2026-07-19 | API 客户端库 |
| grammY | 3,719 | —(非 Rust) | — | 2026-08-15 | TypeScript/JS |

- teloxide = 最成熟:dptree 声明式 dispatcher、长轮询+webhook、Dialogues 对话框子系统(Redis/Sqlite 持久化,天然做确认流程)、强类型命令(enum 自动解析)、Bot API 覆盖至 9.2、rustc≥1.85、crates 下载 178 万次。v0.17.0 为 2025-07 发布,2026 年仍在活跃开发。
- frankenstein:Bot API 10.2 全覆盖、类型 1:1 映射、blocking(ureq)+async(reqwest) 双 client;但只是客户端库——无 dispatcher/dialogue/命令解析,更新循环要自己写(适合交易系统遥控层这种"控制逻辑自持"场景)。下载 19 万次。
- grammY 澄清:grammyjs/grammY 是 TS/JS 框架,crates.io 上不存在 Rust 的 grammy crate——Rust 侧主流候选就是 teloxide 与 frankenstein。

来源:github.com/teloxide/teloxide;crates.io/crates/teloxide;github.com/ayrat555/frankenstein;crates.io/crates/frankenstein;github.com/grammyjs/grammY

## 5) TG Bot 控制交易系统的常见安全模式(源码/文档实证)

1. Chat/用户 ID 白名单:Freqtrade authorized_only 装饰器逐条校验 chat_id 必须等于配置值(拒绝时记日志),群场景另可用 authorized_users 用户 ID 列表(空=无人可控);Condor 同为 User Whitelist。
2. 危险操作默认禁用+配置开关:Freqtrade force* 开仓命令需显式 force_entry_enable=true 才注册。
3. 防误触:内联按钮选择+Cancel:/forceexit 用按钮选 trade(force_exit__{id})+独立 Cancel 按钮;但 force 命令一旦发出即 "Instantly" 执行、无二次确认弹窗——二次确认需自建(teloxide Dialogues 正合适)。
4. 群组风险提示:Freqtrade 文档明示群里每个成员都能控制 bot;支持 topic_id 限定话题线程。
5. 命令面收窄:自定义键盘仅白名单命令且不带参数。
6. 凭据与网络隔离:Condor API key 加密存服务端不进 TG;TG bot 与交易 API 分离,API 层独立鉴权+Tailscale 私有网络(不暴露 8000 端口)。
7. 消息降噪:notification_settings 按类型 on/silent/off。
8. 通用补充(建议非实证):webhook secret 校验、速率限制、危险操作留痕、dry-run 先行、admin-only 命令(Condor /update)。

来源:freqtrade.io/en/stable/telegram-usage/;github.com/freqtrade/freqtrade/blob/develop/freqtrade/rpc/telegram.py;github.com/hummingbot/condor;github.com/hummingbot/condor-docs/blob/main/getting-started/telegram.mdx

## 6) 核心发现摘要

① Freqtrade 命令体系完整(含 forcebuy 为 forcelong 别名、force_entry_enable 开关、内联按钮+Cancel、chat_id/authorized_users 双白名单);② Condor 本身就是 TG bot(LLM 会话+语音+命令三形态,2026-04 发布,经典 Hummingbot 主仓库已无 telegram 代码);③ grammY 是 TS 框架非 Rust,Rust 侧 teloxide(4.2k stars,框架级)最成熟、frankenstein(366 stars,纯客户端)次之;④ 安全模式五条主线已从源码实证。
