# ricow 路线图与进度

> 本文件是里程碑与进度的**唯一权威来源**。
> `specs/product.md` §八 的里程碑表引用本文件, 不各自维护。
> 改进度只改本文件一处。
> 历史记录(CHANGELOG.md)后置到 P5 公开发布时再建, 按 keepachangelog 格式补写; 发布前不建空壳。

## 状态总览

| 阶段 | 交付物 | 验证点 | 状态 |
|:--|:--|:--|:--|
| P0 | 备份基线 | 旧版代码/文档/调研全量归档 | ✅ 完成 (tag v2.0-gui-final) |
| P1 | headless 引擎 + CLI | 命令行跑通策略全生命周期(回测/Dry Run/实盘) | ✅ 完成 (6 crate + 63 测试) |
| P2 | pullback + ladder + ~~ricow scan~~ | pullback/ladder 落地, 选币工具齐 (rhai 属 P3; testnet 冒烟后置 P3 前补) | ✅ 完成 (76 测试); `ricow scan` 已于 2026-09-15 删除(020-platform-scope-trim) |
| P3 | CLI 全流程 + daemon 化 | 回测/选币/策略管理命令齐全; daemon 化运行 | ✅ 完成 (125 测试) |
| P4 | dogfood 实盘 → 公开发布 | "可用且不会让用户莫名亏损" 才发布 | 🔄 进行中 (回测引擎 v0.2 已交付; 实盘 dogfood **执行清单已备**(无主网资金期间: demo 验交易链路 + 主网 Dry Run 预演, 仅"实盘首日"待有资金): [`specs/research/p4-dogfood-runbook-2026-09.md`](research/p4-dogfood-runbook-2026-09.md), 待用户执行) |

图例: ✅ 完成 · 🔄 进行中 · ⏳ 未开始

## 项目更名与开源(2026-09-15)

- **更名**: 项目由 **locus** 正式更名 **ricow** —— 仓库名 / 二进制 / crate 名 / 环境变量(`RICOW_ROOT` / `RICOW_DB` / `RICOW_*`)/ 配置与数据文件名(`ricow.toml` / `ricow.db`)/ `clientOrderId` 前缀(`ricow-*`)全部同步;
  刻意保留的历史名: `locus_hl`(009 已删除的 crate)、`packaging/locus@.service`(008 已删除的单元)仅作为史实出现在文档中。
- **历史**: git 历史不继承 —— 旧 60 次提交留在 gitee 与本地归档(`/mnt/d/mywork/ricow-old-git-history-20260915.tar.gz`), GitHub 仓库从 `v0.7.0` 重新开始。
- **开源落地**: 仓库 <https://github.com/ailenwu2000/ricow>(public, Apache-2.0); 已落地 LICENSE / 双语 README(含语言切换)/ CONTRIBUTING / CODE_OF_CONDUCT / SECURITY / issue+PR 模板 / CODEOWNERS / dependabot(仅自动提 patch/minor)。
- **CI 硬门禁 (2026-09-15)**: `rustfmt --check` + `clippy -D warnings` + `test (ubuntu-latest)` + `test (windows-latest)`, 任意分支推送与 PR 均触发; 工具链由 `rust-toolchain.toml` 钉死 **1.96.1**(CI 以该文件为唯一版本来源, 避免 stable 浮动导致"本地绿、CI 红"); 217→221 告警清零与全仓格式对齐见 `specs/changes/021` / `022`。
- **首个公开发布 (2026-09-15)**: **v0.7.0**(项目进度约 70%)—— 用 dist(cargo-dist 0.32.0)发布 5 平台产物(linux x64/arm64、macOS x64/arm64、windows x64)+ SHA256 + shell/powershell 安装脚本 + source 包, 共 16 个资产; 已实测下载 linux x64 产物、校验 sha256、运行得 `ricow 0.7.0`。产物平台 ≠ 宣称支持: 除 Linux x86_64 外均未实机跑交易流程。
- **官网**: 落地页源码 `website/`(中英双语纯静态页,含 OG/Twitter 卡片元信息),由 `.github/workflows/pages.yml` 部署。
  **已上线**: <https://ailenwu2000.github.io/ricow/>(2026-09-15 实测 HTTP 200,中英文页均正常);自定义域名已配置为 **ricow.xyz**(裸域,2026-09-15):`https://ricow.xyz` 实测 200 且内容正确(证书 SAN 同时覆盖 `ricow.xyz` 与 `www.ricow.xyz`),`www.ricow.xyz` 由 GitHub 自动 301 → 裸域(方向由 custom domain 决定)。
  **DNS 已配置正确**(2026-09-15 实测):权威 NS(ns71/ns72.domaincontrol.com)返回 `185.199.108–111.153` 四条,Google/阿里/114 解析器一致;`curl --resolve` 直连 GitHub Pages 实测 **HTTPS 200 且证书校验通过**(SAN 含 ricow.xyz 与 www.ricow.xyz),访问 www 得 301 → 裸域。
  遗留仅为**缓存与开关**:①公开解析器(Google/Cloudflare/阿里/114)与权威 NS 现已全部返回四条 GitHub IP;仅本机 WSL/Windows 路径仍解析到旧停放 IP(`ipconfig /flushdns` 后仍在,须 `wsl --shutdown` 重启或等上游缓存过期),浏览器如仍见停放页请开无痕/换网络验证;②Pages 的 **Enforce HTTPS 已可用但尚未勾选**(实测 `http://ricow.xyz` 仍 200 而非 301);③GitHub 侧 `NotServedByPagesError` 为其自身检查结果缓存,在 Settings → Pages 点 **Check again** 即应消失(证书已签发说明校验曾通过)。
  Pages 的 Source 已设为 "GitHub Actions"(仓库管理员已启用);Actions 发布方式下**不需要 CNAME 文件**。
- **历史变更档案**: `specs/changes/**` 正文保留旧名不回改(宪法「定稿不回改」)。
- 发布流程唯一权威: `specs/release.md`。

## 测试基线

- **当前基线 (2026-09-25, Linux, 内置策略收敛 + paired_grid 等比化后实跑)**: `cargo test --workspace` = **529 passed / 0 failed / 22 ignored**; 较上次 544/22: **−15 通过 / ignored 不变**; 变化 = 删除 shannon_rebalance(4 例) + executors/{dca,twap,vwap,pullback,ladder}(约 10 例) + paired_grid ATR 未就绪(1 例); paired_grid 改名「现货动态非对称网格」并等比化(买价 = ref÷(1+间距), 卖价 = 栈顶买入价×(1+间距))。
- **前基线 (2026-09-19, Windows, 026 落地后实跑)**: `cargo test --workspace` = **504 passed / 0 failed / 21 ignored**; `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。
  较上次记录的 472/21: **+32 通过 / ignored 不变(仍 21)**; 增量全部来自 026(两张新表与幂等迁移、`recent_fills_with_mode` 的 mode 关联、`web/tail.rs` 尾读与轮转、交易/日志端点与只读红线、AI 工具三态与 Confirm 结果注入), 逐条见 `specs/changes/026-trade-visibility/tasks.md`。
- **前基线 (2026-09-19, Windows, 023 + 025 落地后实跑)**: `cargo test --workspace` = **472 passed / 0 failed / 21 ignored**; `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。
  较 2026-09-17 的 403/21: **+69 通过 / ignored 不变(仍 21)**; 增量来自 2026-09-18 起的对话体验重构(023)与 Web UI(025: axum 鉴权与回环绑定、会话缝第二个 sink、会话历史持久化与回放、术语表、启动脚本站位), 另含 025 实机收口补的 2 例(Web 输入「一次入站只投一行」回归、daemon 前置校验回归), 逐条见 `specs/changes/023-ai-chat-ux/` 与 `specs/changes/025-web-ui/tasks.md`。
- **前基线 (2026-09-17, Windows, 019 R5 + gr 复核修复后实跑)**: `cargo test --workspace` = **403 passed / 0 failed / 21 ignored**; `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。
  较 2026-09-16 的 390/21: **+13 通过 / ignored 不变** —— 全为 gr 复核修复引入的 bin 单测(会话 root 取数 / preview TTL 与引擎常量同源 / 门禁指南关键词 / daemon 双条件认账等), 逐条见 `specs/changes/019-ai-assistant/converge.md`。
- **前基线 (2026-09-16, Windows, 019 R4/R5 落地后实跑)**: `cargo test --workspace` = **390 passed / 0 failed / 21 ignored**。
  较同日 R3 后的 355/15: **+35 通过 / +6 ignored** —— R4 新增会话缝 `ai/session.rs`、七动作确认状态机 `ai/confirm.rs`、裸入口 `chat` / 首次向导 `onboard`、`/keys` `/market`、`pairs` 的对应用例; R5 删平台风控后 `risk.rs` / `scheduler.rs` 及其用例随文件删除, 频率护栏用例改挂 `order_guard`(逐条见 `specs/changes/019-ai-assistant/tasks.md`)。
- **前基线 (2026-09-16, Windows, 019 R3 对话内确认落地后实跑)**: `cargo test --workspace` = **355 passed / 0 failed / 15 ignored**; 较 2026-09-15 的 348/12(Windows): +7 通过 = R3 对话内确认 bin 单测 5(`r3s5_*`×4 真实落盘/preview consumed/同名拒绝/reject 终态, `r3s6_*`×1 demo 三档门禁)+ `tests/ai_live_smoke.rs` 默认门禁 2(管道 approve 被 tty 门禁拒、缺 demo 凭据先于网络失败); +3 ignored = 真机 S1/S2/S3+S4/S7(DeepSeek, 已于 2026-09-16 全量跑绿; 旧 ollama smoke 1 ignored 已被该文件取代)。
- **更早基线 (2026-09-15, 021 clippy 清零后实跑)**: `cargo test --workspace` = **339 passed / 0 failed / 12 ignored**(Linux; 文档此前记 306/308 均已过时, 以本次实跑为准)。
  ignored 全部为需真实外部环境的用例 (BN demo 现货 / 合约 / 用户流、Nasdaq 冒烟、019 AI 真机与 demo 联调), 不 mock 替代; 其中 008 的 3 例已于 2026-09-12 跑绿, 011 新增 2 例现货用户流用例与实盘闭环(CLI 探针)已于 2026-09-13 真实跑绿 —— 记录见 `specs/testnet.md`。
  220 → 204 的差额 = 009 移除 `locus_hl`(16 个内联测试); 204 → 216 = 008 新增 supervisor / CLI 命令面用例; 216 → 233 = 004 风控用例(频率窗口/两级熔断/装配与参数校验/接线); 233 → 270 = 011 用例(下单参数对齐 / 时钟预检判定 / 归属与停机清理编排 / 门禁 / proto 往返 / 现货事件解析 / 交易所过滤器解析 / 归属前缀长度约束)。
- 验证方式: `cargo test --workspace`(纯逻辑) + 带 env key 的 `#[ignore]` 真实联调 (见 `specs/testnet.md`)。
- 历史基线: P1 63 → P2 76 → P3 125 → 回测重构 173 → 001-vwap 154(分支基线) → 220 → 204(009 移除 locus_hl) → 216(008 实施后) → 233(004 实施后) → 270(011 实施后) → 282(012 实施后) → 286(013 实施后) → 289(014 实施后) → 294(003 实施后) → 301(002 实施后) → 304(015 实施后) → 306(016 实施后) → 308(018 实施后) → 339(021/022 告警清零与格式化后) → 348(019 R1/R2) → 355(019 R3) → 390(019 R4/R5) → 403(019 R5 + gr 复核修复) → 472(023 对话体验 + 025 Web UI) → **504(026 交易可见性, 当前)**。

## 变更档案状态 (specs/changes/)

| 编号 | 变更 | 状态 | 结论 / 备注 |
|:--|:--|:--|:--|
| 001-vwap | VWAP 执行模式示例 | ✅ 已收敛 (2026-09-01) | 6 回调 API 之外的 `ctx.klines.volume` 字段落地; 见该档案 converge.md |
| 002-ai-quant-researcher | AI 量化研究员(Binance bStocks) | ✅ 已实施 (2026-09-13) | 暴露建策略闭环 CLI: `ricow create`(编译门禁 → 真实 K 线沙箱回测 → preview, **不落盘**)→ `ricow approve`(人工批准, 一次性 token)→ `ricow deploy`(落盘 `<name>.toml` + `<name>.lua`, 同名拒绝覆盖/路径穿越拒绝); 引擎侧新增 `execute_strategy`, 报告打印与 `backtest` 共用同一口径(顺带修初始余额硬编码 USDC); **并接管 `dry_run_started_at`**: 首次 Dry Run 启动写入(ISO8601), 实盘启动按 `params.min_dry_run_hours`(默认 24h = 覆盖三时段一个完整日周期, 设 0 关闭)核验, 不足即拒绝启动(不静默降级); 实测: 语法错误代码被拒且零落盘、真实 K 线回测出报告、部署物 Dry Run 真跑通(tick=50 成交=1)、二次/同名部署被拒、`run/start --live` 双双被时长门禁拒绝且未下任何单、`min_dry_run_hours=0` 放行后仍被时钟预检拦住; 测试 294 → 301 |
| 003-notifications | 出站通知(成交 + 熔断告警) | ✅ 已实施 (2026-09-13) | 补齐 product.md §三.4 承诺; 通道=用户自选 webhook(`params.notify_webhook`, 一个 POST 覆盖 Telegram/飞书/Slack/自建, 守住"数据不出本机"), 四类事件=成交/熔断/接近强平/停机残留; 事件白名单 + 同类限速(默认 5s, 被压制数如实附带)+ 强平按 (pair,方向) 去重; 投递 spawn+5s 超时+失败只 warn(不重试不排队, 绝不反压交易循环); 默认关闭; 实测: 本地回环端点真实收到 4 条 POST(成交×2/接近强平/平仓成交, 内容与事件一致)、不可达端点时仅 warn 且 tick 照常、成交 1 笔→通知 1 条(修掉一处重复投递); 测试 289 → 294 |
| 004-risk-guards | RiskEngine 补项(下单频率上限 + ~~两级亏损熔断~~) | ✅ 已实施 (2026-09-12); **两级亏损熔断已于 2026-09-15 删除**(020) | 频率上限(滑动窗口 1s, 默认 100/s) + 两级熔断(连续 3 日净亏损停开仓 / 峰值回撤 30% 停新交易); **并补上关键缺口: 风控此前只挂在无消费者的 `StrategyScheduler` 上, 回测/Dry Run/实盘三条下单路径从未过风控** → 现三条路径统一 `RiskEngine::from_config` 接入; 测试 216→233; 实测: 默认参数回测与"护栏关闭"逐位一致(零误伤), `limit=1` 冒烟在真实行情 Dry Run 下拒绝 715 单(有据可查); 见该档案 plan/tasks |
| 005-market-filter | 市场分类层 + ~~scan 池参数化~~ | ✅ 已实施 (2026-09-07); **`scan` 已于 2026-09-15 删除**(020; `market_class`/`us_tickers` 按明示保留) | 交付物: `market_class`/`us_tickers`/`nasdaq` 适配 + `scan --pool bstock-spot`; 策略消费者见 006 |
| 006-bs-momentum-lua | bs_momentum 策略 Lua 化合规整改 | ✅ 已实施 (2026-09-09) | 宪法原则二整改(撤销 Rust 版策略本体); 策略本体已于 2026-09-11 下线 |
| 007-bs-momentum-v2 | bs_momentum 正期望改造 | ✅ 已实施 (2026-09-09) | 市场门 / 周频 / 持仓下限; 真实成交轨期望 ≈0 → 判定不合格, 策略下线 |
| 008-platform-process-model | 跨平台进程模型重构(策略启停/状态/日志) | ✅ 已收敛 (2026-09-12) | 常驻 daemon(pm2 模型)+本机 TCP(token)+stdin 管道优雅停机(EOF 自愈)+实例台账+实例/成交查看; 实测: 全链路冒烟(daemon/list/start/info/fills/logs/stop)、崩溃自愈(kill -9 daemon → 子进程 4s 自愈退出)、testnet 真实调用 3 例跑绿(现货/合约 openOrders 闭环 + 拒单如实上报)、测试 204→216/0/9; 收敛遗留: 实盘撤单兜底与持仓明细随实盘运行器落地(见档案 Phase 7) |
| 009-remove-hyperliquid | 移除 Hyperliquid 适配与文档承诺 | ✅ 已收敛 (2026-09-12) | 删 `locus_hl` crate(1696 行) + 全部引用点 + 文档承诺; 测试 220 → 204; 见该档案 converge.md |
| 011-live-runner | 实盘运行器(让 `live_enabled=true` 真正生效) | ✅ 已实施 (2026-09-13) | 门禁双条件(`live_enabled` **且** `--live`)+ 下单前时钟预检 + 真实账户装配 + 双流(盘口 + WS-API 用户流)+ 成交回写落库 + 停机清理(撤单兜底/可选平仓/残留复查, 幂等)+ `start --live`/`stop --close-all`/`info` 实盘快照; 实测(demo 现货): 用户流订阅就绪并发到 `executionReport`、T014 挂单→停机撤单兜底→交易所零残留、T015 市价买入→`stop --close-all` 兜底平仓(2 笔成交回写落库)、时钟预检拒绝超限(自然漂移 1142ms 实测拒绝) ; 测试 233 → 270; **合约实盘为 Phase 2 未做**(需新建 `BnFuturesExchange`); 见该档案 plan/tasks/converge |
| 012-futures-live-adapter | 合约实盘适配层 (USDT-M) | ✅ 已实施并真实验证 (2026-09-13) | `BnFuturesExchange` 实现 `Exchange`(REST + `fstream` WS 盘口增量合并 + listenKey 用户流就绪语义); 引擎按 `market` 分派装配/持仓语义, hedge 定向持仓(缓存键 `pair|side`)与 one-way/hedge 平仓参数; 实测: 挂单→撤单兜底零残留、开仓→`stop --close-all` 归零、hedge 双向两侧分别平掉; 顺带修平仓单号超 36 字符被拒 与 `ricow info` 无成交策略 panic + 手续费 f64 噪音; 测试 270→282 |
| 013-futures-margin-model | 合约逐仓记账校准(按侧独立钱包/强平 + 结算时点) | ✅ 已实施 (2026-09-13) | 依据 2026-09-05 真实清算实测(价格下行只清 LONG 侧、SHORT 侧原样保留; 两侧保证金独立占用)修正回测: 钱包按侧寻址 + hedge 按侧独立强平 + 两侧资金费独立结算(不跨侧抵消); 关闭 L1(结算点与 interval 无关, 1d 由欠结 2/3 修正为恰 3 次)与 L2(尾 bar 补结算 `finalize()`); 报告新增 hedge 按侧明细(各侧强平次数/收盘钱包余额)便于与交易所账单对照; one-way 与现货路径行为不变; 测试 282 → 286 |
| 014-live-funding-and-liq | 实盘资金费记账 + 强平风险可见(合约) | ✅ 已实施 (2026-09-13) | 资金费以**交易所账单**为准(`GET /fapi/v1/income`, 不自算费率×名义)→ `funding_fees` 表(`tran_id` 幂等, Rust 侧 Decimal 精确聚合) + 启动/30min/停机前增量拉取(水位 = `MAX(funding_time)`); `Exchange` trait 增 `funding_income`(默认空, 现货零改动); 距强平距离纯函数 + 阈值告警(实测: 50x 小仓 liq=2443.31 / mark=2479.89 → 距离 1.48% 触发 warn, 与公式一致); 实测 `income` 端点签名调用返回 `[]`(demo 账户无跨 8h 持仓 → 如实显示 0 笔); 测试 286 → 289 |
| 015-backtest-noop-fill | 回测撮合零动作单口径修复 (L4) | ✅ 已实施 (2026-09-13) | hedge `sell + position_side="long"` 而多仓为 0 这类"成交但零动作"的请求, 此前照收手续费并记一笔成交(成本/笔数虚增); 现判定于**成交那一刻**: 市价单拒单计数、限价单保持挂单(与"资金不足"同一约定), 均不计费不记笔数; 真实对照(同窗口)成交 1→0 / 手续费 0.0482688→0 / 拒单 0→1, 且不触发 L4 的策略逐位一致; 测试 301 → 304 |
| 016-dryrun-initial-cash | Dry Run 虚拟本金可配 (`params.initial_cash`) | ✅ 已实施 (2026-09-13) | 原 Dry Run 本金硬编码 100k, 使"小资金预演"不成立 —— 同一份 `[risk]` 限额不可能同时适配 100k 与真实几百 USDT(实测: 小资金限额下 Dry Run `tick=66 提交订单=65 拒单=65 成交=0`); 现 `params.initial_cash` 可配(缺省 100000 保持既有行为), 非法值报错**不静默回落**, 启动打印本金额; 实测 `initial_cash=300.0` → 建仓 150 USDT/拒单 0(对比修复前 65 单全拒); 测试 304 → 306 |
| 017-spot-live-snapshot-fix | 现货实盘快照一致性修复 (dogfood 实测) | ✅ 已实施 (2026-09-13) | demo 实盘预演暴露三处: ① **`stop --close-all` 静默不平仓**(012 起清理改走 `get_positions_directional`, 现货实现恒空 → 报告"无持仓"而账户仍持 2.016 ETH); ② **现货成交后只刷持仓不刷现金** → 策略按 equity 决策以为"只有币没有钱", 每 tick 再卖一半, 几何级数清仓(`1.0079→0.5039→0.252→…` 全卖光); ③ 成交回写异步窗口内重复下单(1.4s 内 3 次同单)。修复: 清理/残留持仓源按市场分支(现货复用 `spot_position_of`)+ 现货补刷 base/quote 余额 + 下单后按 `any_filled` 立刻对齐快照。实测修复前 `提交订单=216/拒单=209/成交=8/持仓清空` → 修复后 `提交订单=1/拒单=0/成交=2/平仓单执行/残留 0.000058`, 交易所侧 `openOrders=0`; 测试 306 不变(接线/时序缺陷无 mock 单测, 以真实链路为证) |
| 018-first-use-risk-ack | 首次使用风险确认 | ✅ 已实施 (2026-09-13) | 补齐 `product.md` §十 风险披露的第二条(README 免责声明早已有, 代码侧确认**完全缺失** —— 克隆仓库配好 key 即可用真钱开跑且无任何告知)。实盘启动最前置判定: 未确认 → 拒绝 + 打印四条披露要点与确认方式; `--accept-risk` → 记录到 `$RICOW_ROOT/risk_ack.json`(含 schema 版本, 披露变更可递增触发重新确认)后放行, 之后不再要求; 判定顺序 风险确认→时长门禁→时钟预检(未确认时零交易所往返), Dry Run/回测不受影响; 实测三步(拒绝且无记录文件 / 确认落地 / 再跑不再要求并正常启动); 测试 306 → 308 |
| 019-ai-assistant | 内置 AI 助手 + 生态入口 + 开箱即用分发 | 🔄 **主线已打通, 做文档/测试收口 (2026-09-16)** | 已落地并真机验证: `ricow ai`(交互/单次/`--plain`, 只读 12 + 虚拟 4 工具)、策略生成闭环(`preview_strategy` → 编译门禁 → 真实 K 线沙箱回测 → 零落盘)、`ricow create/approve/deploy` 两步确认(逐字短语)、**`ricow agent-kit`**(AGENTS.md / SKILL.md / CLAUDE.md / lua-api.md, 与内置 AI 系统提示同源; 命令速查由 clap 生成; 拒覆盖)、`--demo` 测试网运行、实盘二次分离(逐字 `确认实盘 <name>`)、策略名规范、提示词按需取文档(8k→1.8k tokens)、**单一配置文件 `ricow.toml`**(provider+api_key 同段, 删 keyring/setup/写凭据命令)。**R4(2026-09-16)全功能对话化**: 裸 `ricow` 即入口(默认 `chat`, 首次跑走向导 `onboard`)、会话缝 `ai/session.rs`(`ChatSession` + `SessionSink`, 业务零 stdio)、对话内七动作确认状态机(逐字短语:`确认部署` / `确认启动测试网` / `确认风险` / `确认实盘` / `确认停止测试网` / `确认停止实盘` / `确认平仓停止`; 实盘仍原样跑 `ctrl::live_preflight` 三判据)、`/keys` `/market` 外科式改 `ricow.toml`(`set_values` 白名单 9 键, 保留注释 + 原子写; 权限 Unix 0600 / Windows 仅当前用户 ACL)、`pairs` 命令与交易对视野。**R5(2026-09-16)删平台风控残留**: 删 `risk.rs` 静态限额(`RiskEngine` / `[risk]` 配置面 / 装配器)与 `scheduler.rs`, 只留固定 100 单·秒⁻¹ 工程护栏 `order_guard` —— 平台不做投资判断, 风控由策略自管(`constitution.md` 已同步修订)。未完成: 三平台分发(CI)、文档/测试收口、agent-kit 的真机第三方 agent 验证(T047)(`ricow mcp` **不做** —— 2026-09-15 定案: 单机程序, 用户已有的 agent 直接调本机 CLI, 生态入口 = agent-kit 手册); Phase 1–4 主体完成(仅 Windows 双击入口 T034/T035 未做, 本机无法产出 Windows 产物), 见 `specs/changes/019-ai-assistant/tasks.md`(T001–T078) |

> **023-ai-chat-ux(2026-09-18, 实施中)**: 对话体验重构 —— 中英双语(`[ui].lang` + `/lang`, AI 回复语言跟随)、每轮分隔线与轮次号 + `/history`、去术语化(面向用户不出现 `ricow xxx` 命令)、宿主持有菜单的选项式交互(菜单 A 5 项 / B 4 项)、**写操作全确认**(7 → 13 类, 唯一入口 `request_write_confirmation`)。确认词**分渠道**: 对话 = 当前语言**口语词**(`确认`/`确定`/`同意` · `confirm`/`confirmed`), 终端 = **逐字长短语**(`approve.rs` / `ctrl.rs` 一行不改); 见 `specs/changes/023-ai-chat-ux/`。

> **025-web-ui(2026-09-19, 已实施)**: 增加 Web UI 模式 —— `ricow web` 在 `127.0.0.1` 上起内置网页(axum + SSE, 一次性 token 鉴权, 无 token / 错 token 一律 `401` 且不回会话内容), 浏览器里完成全部对话与操作(**与 CLI 同一会话引擎**: 会话缝 `ai/session.rs` 的第二个 sink `web/sink.rs`, 不复制任何 LLM 路径)。左侧会话历史持久化在 `ricow.db`(新建/切换/删除, 重开恢复最近 20 轮上下文)、右侧对话输出区 + 输入区; 关键字/警告/出错按宿主标注的 `Severity` 着色; 量化术语点击弹解释; 界面中英双语并与 CLI 共用 `[ui].lang`; 明文密钥走独立通道, 不落库、不进对话流、不进浏览器存储。**既有 CLI 功能零改动**(原三个启动脚本逐字节不动, Web 启动脚本为新增文件); 见 `specs/changes/025-web-ui/`。

> **026-trade-visibility(2026-09-19, 已实施)**: 让交易与日志**可见**(起因: 用户实测"跑测试网看不到交易信息与日志")。① **落库**: 引擎侧新增 `orders` / `positions` 两表(`CREATE TABLE IF NOT EXISTS` 幂等追加, 既有 8 张表**零改动**)并在下单提交 / 订单状态变化 / 成交回报 / 持仓刷新时写入; `pnl_snapshots` 由死表改为**每笔成交后写一条**且永久保留。② **Web 面板**: 新增只读交易面板(持仓 / 挂单 / 最近成交)+ 日志面板(策略切换 + 尾读 SSE 实时追加), 共 4 条交易端点(`/api/trades/{fills,orders,positions,pnl}`)与 3 条日志端点(`/api/logs`、`/tail`、`/stream`), 全部挂在**同一道 token 中间件之内**, 页面**无任何直连交易所的写按钮**(撤单 / 平仓 / 停机仍走对话确认)。③ **口径**: 数据源**唯一来源 = 本地库**(不直连交易所, 停机后仍能看最后状态并标注"截至 <时间>"); 每条回复带 `source` 三态(`ok` / `daemon_down` / `unreadable`), **不把"连不上 daemon"说成"没有交易"**; 成交的 `mode` 由 `exchange_order_id` ⟕ `orders` 带出, 关联不到即如实显示"未知"不猜。④ **修两处假阴性**: AI `instance_status` 把"连不上 daemon"说成"策略已经停着/没有可停止的对象"; demo 测试网停机误印「实盘停机」文案(现按**真实运行模式**措辞)。⑤ **AI 回流**: 新增三个只读工具(`positions` / `open_orders` / `pnl`), 写操作执行结果注入对话 history。测试 472 → **504 passed / 0 failed / 21 ignored**, 三门禁全绿; 端到端以**真实 demo 测试网**验证(禁 mock); 见 `specs/changes/026-trade-visibility/`。

> SDD 产物规范: 计划与任务分解应存于 `specs/changes/<feature>/{spec,plan,tasks}.md`。
> 005/006/007 的计划当时落在 `.hermes/plans/`(临时区, 已 git 忽略), 未回填档案 —— 后续变更须归档到位。

## 已终止的探索 (记录结论, 避免重复投入)

1. **bs_momentum 动量轮动 (2026-09-11 下线)**
   - 判定: 真实 bStock 成交轨期望 ≈0 (spot 91 天每 bar −0.0198%、期末 −5.38%; futures 220 天每 bar +0.0367%、期末 −1.08%),
     十年 Nasdaq 近似轨的 +15,088% 含幸存者偏误, 不作证据 → 用户判定"期望值为负, 不合格"。
   - 已删: `strategies/builtin/bs_momentum.lua` + 组合回测 CLI 入口。
   - 保留(通用能力, 暂无内置消费者): 组合回测路径 `run_portfolio_backtest` + 组合信号模式(`universe` / `signal_klines` / `SIGNAL_TAIL`)。
   - 证据: `specs/research/bs-momentum-attribution-2026-09.md` §七。
2. **bStock 日内 Top5 相对强度 (2026-09-11 放弃)**
   - 判定: 算术否决 —— 日频 100% 换手下成本 0.2%/日 > 最优毛期望 +0.122%/日; 合约样本(148 会话)超额 ≈0。
   - 已删: `strategies/builtin/bs_intraday_top5.lua` + CLI 入口 + 一次性调研脚本。
   - 保留(通用能力): `build_interval_ticks`(任意 interval tick 对齐) / `ctx:now()` / `ctx:klines` 行 `ts` 字段。
   - 证据: `specs/research/bstock-intraday-top5-abandoned-2026-09.md`。
3. **bStock 横截面选股族 `bs_rs_rotation` (2026-09-12 未实施即终止)**
   - 判定: 用户 2026-09-12 —— "横截面选股策略长期期望值为负。不做了。产品中增加的相关功能保留，以后可能会用。"
     本变更**未实施, 无新实测数字**(不编造); 判定依据 = 前两条实测(bs_momentum 真实成交轨 ≈0 / 日内族被换手成本算术否决)
     + 短线横截面族落在反转区且样本仅 63/150 个交易日无法证明长期有效。
   - 产物: `specs/changes/010-bs-rs-rotation/`(spec.md + checklists, 未实施即终止留档); 上游计划 `.hermes/plans/2026-09-11_222951-…` 已标注作废。
   - **保留(用户明示"相关功能保留, 以后可能会用")**: 美股数据层(`nasdaq` / `us_tickers` / `market_class` / `us_klines` 缓存)、
     ~~`ricow scan --pool bstock-spot` 横截面选币~~(**2026-09-15 用户改判删除** —— 020-platform-scope-trim; 其余保留项不变)、组合回测路径 `run_portfolio_backtest` + 组合信号模式(`universe` / `signal_klines` / `SIGNAL_TAIL`)、
     `build_interval_ticks`、`ctx:now()`。这些能力**当前无内置策略消费者, 但不算死代码, 不删**(宪法原则五"死代码必删"不适用于用户明示保留的通用能力)。

## 当前阶段: P4 (dogfood 实盘 → 公开发布)

- 内容: "可用且不会让用户莫名亏损" 才发布
- 现状: 实盘 dogfood 未开始。P4 前置(2026-09-04)已交付:
  - **回测引擎重构**(specs/backtest.md v0.2, D1-D11): 现货/合约统一回测 —— 修复 Bug A(余额不足拒单如实计数)/Bug B(费用实扣)、账户模型 (pair,方向) 双仓 + 逐仓钱包、合约开平仓/8h 资金费/按对强平(先平浮亏仓)、配置三层、fapi 公共数据源、Lua 方向仓查询(pos_size/pos_entry + position_side 字段),全量测试 + 真实数据冒烟通过。
  - CLI 直跑 `--param` 覆盖顺序修复(此前被内置默认值静默覆盖)。
- **币安 demo 测试网联调 (2026-09-04, P4 前置)**: 现货(demo-api.binance.com)+ 合约(demo-fapi.binance.com)真实交易链路打通 —
  新增 fapi 签名客户端 `FuturesClient`(下单/账户/持仓/杠杆/双向持仓, ricow_binance)与真实联调集成测试
  (crates/ricow_binance/tests/, #[ignore] 需 env key): 现货 3 随机对 挂单/撤单/市价买/平仓闭环、合约 one-way 3 随机对 开平仓闭环、
  hedge 双向同对 LONG+SHORT 并存、真实强平观察;凭据与端点见 specs/testnet.md。
- 阻塞项: dogfood 实盘验证(主网小资金, 由用户执行); 回测引擎遗留项仅剩 L4(hedge 空方向单不计费), L1/L2 已于 2026-09-13 随 013 关闭; 逐仓钱包语义已按真实清算实测校准(013)。

## 下一步 (按优先级)

0. **产品重构 (2026-09-12 用户指令)**
   - 已完成: **009 移除 Hyperliquid**(未落地却写进文档, 已全量移除 crate + 文档承诺, 测试 204)。
   - 已收敛: **008 跨平台进程模型**(2026-09-12) —— 常驻 daemon(pm2 模型) + 本机 TCP(token) + stdin 管道优雅停机(EOF 自愈) + 实例台账 + list/status/info/fills/logs;
     实测: 全链路真实行情冒烟(37 笔成交落库)、kill -9 daemon 后子进程 4s 自愈退出、testnet 真实调用 3 例跑绿(现货/合约 openOrders 闭环 + 拒单如实上报)、测试 204→216/0/9;
     收敛遗留(不阻塞归档): 引擎撤单兜底 / `info` 持仓快照随实盘运行器落地; Windows 编译级验证当前不可复现已在 spec §四 如实降级。
     档案见 `specs/changes/008-platform-process-model/`(spec/plan/tasks, 含 Phase 7 收敛)。
     关键实测依据(detached 存活 / tokio spawn 陷阱 / 管道 EOF 自愈 / SQLite 并发写): `specs/research/process-model-probe-2026-09.md`。
   - 已澄清(2026-09-12): 用户判定「横截面选股策略长期期望值为负。不做了。产品中增加的相关功能保留，以后可能会用。」
     → ①计划中的 `bs_rs_rotation` **未实施即终止**(已建 spec 留档, 见"已终止的探索" 3);
     ②`ricow scan` 横截面选币与 bStock/美股数据层(含组合回测路径、信号轨机制、`build_interval_ticks`、`ctx:now()`)**保留不删**。
1. ~~**回测引擎遗留项**~~: L1/L2 已随 013 关闭, **L4 已随 015 关闭**(零动作单不再计费/记笔数) —— 见 `specs/backtest.md` §十。
2. ~~**CLI 残留清理**~~: ✅ 已随 008 一并处理(`backtest --market` 帮助文本、`--dump-dir` 删除、`scan` 帮助文本) —— 见 `specs/architecture.md` §十一。
3. **backlog 变更**: ~~003-notifications~~(2026-09-13 实施) → ~~002-ai-quant-researcher~~(2026-09-13 实施); backlog 已清空; **019-ai-assistant 已定稿(2026-09-14)进入实施**, P4 dogfood 仍待执行。
4. **P4 dogfood**: 上述前置已完成, 以真实小资金跑通"回测 → Dry Run → 实盘"闭环 —— **执行清单见 [`specs/research/p4-dogfood-runbook-2026-09.md`](research/p4-dogfood-runbook-2026-09.md)**(含一次性准备、四阶段命令与判据、立即停手条件、回滚、实测坑清单、结果留档表)。
5. ~~**全仓 rustfmt 对齐**~~ ✅ 已随 **022**(2026-09-15)完成: 工具链钉死 1.96.1 后一次性对齐 41 个文件(+612 −518), CI 已开 `cargo fmt --all -- --check` 硬门禁。原记录(2026-09-14):: 实测 `cargo fmt --all` 会改动 **34 个文件 / 约 +6069 −1139 行**, 且**非纯空白差异**(结构体字段换行、长表达式换行等), 即现有代码与当前 `rustfmt.toml`(`max_width = 100`, `use_small_heuristics = "Max"`)的输出不一致。**已全部回退, 未纳入 019**。开工前需先确认团队基准 rustfmt 版本/配置; 单独立项(否则 review 无法区分语义改动与格式噪声), 不与其他变更混做。
6. ~~**AI 对话流程仍需简化**(2026-09-17 用户反馈, 原话: 「AI对话流程还需要简化。目前有点过于复杂」)~~ → **已立项 023-ai-chat-ux(2026-09-18)**: 向导首问语言、对话内确认改为随语言的**口语词**、去术语化表述、菜单选项式交互, 三项"用户要记的东西"都被收掉。
   - 现象(当时自查): 用户要理解的东西偏多 —— 首次向导 4 步(供应商 / 模型 / 密钥 / 可选连通校验, 外加可跳过的 demo 凭据)、对话内七类**逐字**确认短语、以及"裸入口 / `ai` 子命令 / 一堆 CLI 子命令"多档入口并存。(023 收敛了前两项; 入口档次未动。)
   - **约束(仍然有效)**: 逐字短语在**终端**是**安全机制**(不是可删的复杂度) —— 023 的解法是**分渠道**: 对话用当前语言口语词, 终端保持逐字长短语一行不改, 不削弱"写实必须本人确认"。

## 文档-实现缺口 (2026-09-11 审计)

本轮对照代码逐份复核 specs, 已就地修正的条目:

| 文档 | 问题 | 处置 |
|:--|:--|:--|
| architecture.md | 测试基线写 125; CLI 参数写 `--initial-cash`(实为 `--cash`); 直跑策略表漏 vwap; scan 缺 `--pool`; 未记录美股数据源/市场分类模块 | ✅ 已修正 |
| backtest.md | 验收测试数 173 过时; D2(MMR 取 exchangeInfo)与 D10 修正冲突未就地标注; L5 实际已修 | ✅ 已修正 |
| lua-api.md | §八"当前只支持 Binance 现货"过时(已支持合约 USDT-M); create_strategy 通道描述与实现不符(引擎能力在, CLI 入口未暴露) | ✅ 已修正 |
| product.md | D3 "v1 = 6 策略"与现状不符(实为 1 策略样板 + 5 执行模式示例) | ✅ 已修正 |
| changes/001-vwap | 档案状态仍写"草稿", 实际已收敛; 4 份 checklists/requirements.md 的 `spec.md` 链接指向同级(断链) | ✅ 已修正 |
| testnet.md | 合约下单/账户端点写"计划中联调", 实际 2026-09-04 已完成 | ✅ 已修正 |
| changes/005,006,007 | 计划产物只落 `.hermes/plans/`, 档案缺 plan.md/tasks.md | ⏳ 规范澄清入 constitution(§文档体系); 历史档案不回填 |
| roadmap.md | 进度停留在 2026-09-04, 未记录 001/005/006/007 与两次策略终止 | ✅ 本次重写 |

> 本轮同时按 §十一 记录了三处 CLI 残留(不改代码, 列入"下一步"第 3 项):
> `backtest --market` 帮助文本含不可用的 `us`、`--dump-dir` 无消费方、`scan` 帮助文本仍写"P2 骨架"。
> 真实命令冒烟已复核: `ricow --help` 命令集与 §五 一致; `ricow backtest --strategy shannon_rebalance --pair ETHUSDT --days 20` 真实数据跑通
> (480 根 1h K 线、8 笔成交、报告字段与 backtest.md §六 一致), 同时发现文档示例 `--pair ETH` 无效(交易所 `Invalid symbol`)已修正。
