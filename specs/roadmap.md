# ricow 路线图与进度

> 本文件是里程碑与进度的**唯一权威来源**。
>   
> `specs/product.md` §八 的里程碑表引用本文件, 不各自维护。
>   
> 改进度只改本文件一处。
>   
> 历史记录(CHANGELOG.md)后置到 P5 公开发布时再建, 按 keepachangelog 格式补写; 发布前不建空壳。

## 状态总览

| 阶段 | 交付物                                | 验证点                                                       | 状态                                                                                                                                                                                               |
| :- | :--------------------------------- | :-------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P0 | 备份基线                               | 旧版代码/文档/调研全量归档                                            | ✅ 完成 (tag v2.0-gui-final)                                                                                                                                                                        |
| P1 | headless 引擎 + CLI                  | 命令行跑通策略全生命周期(回测/Dry Run/实盘)                               | ✅ 完成 (6 crate + 63 测试)                                                                                                                                                                           |
| P2 | pullback + ladder + ~~ricow scan~~ | pullback/ladder 落地, 选币工具齐 (rhai 属 P3; testnet 冒烟后置 P3 前补) | ✅ 完成 (76 测试); `ricow scan` 已于 2026-09-15 删除(020-platform-scope-trim)                                                                                                                             |
| P3 | CLI 全流程 + daemon 化                 | 回测/选币/策略管理命令齐全; daemon 化运行                                | ✅ 完成 (125 测试)                                                                                                                                                                                    |
| P4 | dogfood 实盘 → 公开发布                  | "可用且不会让用户莫名亏损" 才发布                                        | 🔄 进行中 (回测引擎 v0.2 已交付; 实盘 dogfood **执行清单已备**(无主网资金期间: demo 验交易链路 + 主网 Dry Run 预演, 仅"实盘首日"待有资金): [`specs/research/p4-dogfood-runbook-2026-09.md`](research/p4-dogfood-runbook-2026-09.md), 待用户执行) |

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
    
  **DNS 已配置正确**(2026-09-15 实测):权威 NS(ns71/ns72.domaincontrol.com)返回 `185.199.108–111.153` 四条,Google/阿里/114 解析器一致;`curl --resolve` 直连 GitHub Pages 实测 **HTTPS 200 且证书校验通过**(SAN 含 ricow.xyz 与 [www.ricow.xyz),访问](http://www.ricow.xyz\),访问) www 得 301 → 裸域。
    
  遗留仅为**缓存与开关**:①公开解析器(Google/Cloudflare/阿里/114)与权威 NS 现已全部返回四条 GitHub IP;仅本机 WSL/Windows 路径仍解析到旧停放 IP(`ipconfig /flushdns` 后仍在,须 `wsl --shutdown` 重启或等上游缓存过期),浏览器如仍见停放页请开无痕/换网络验证;②Pages 的 **Enforce HTTPS 已可用但尚未勾选**(实测 `http://ricow.xyz` 仍 200 而非 301);③GitHub 侧 `NotServedByPagesError` 为其自身检查结果缓存,在 Settings → Pages 点 **Check again** 即应消失(证书已签发说明校验曾通过)。
    
  Pages 的 Source 已设为 "GitHub Actions"(仓库管理员已启用);Actions 发布方式下**不需要 CNAME 文件**。
- **历史变更档案**: `specs/changes/**` 正文保留旧名不回改(宪法「定稿不回改」)。
- 发布流程唯一权威: `specs/release.md`。


## 测试基线

- **当前基线 (2026-10-07, Windows, 044 Web UI 体验补强 — AI 对话 Markdown 渲染 + 日志面板搜索/过滤/跟随开关 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **838 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 836(043): **+2 通过 / ignored 不变(22)**。增量构成: ricow bin **432 → 434**(+2: `web::assets` 的「markdown 渲染器仅产 DOM、不碰 innerHTML」与「`markdown.js` 排在 `chat.js` 之前」); 其余 target 一字未变(ricow_binance 62 / ricow_core 18 / ricow_engine 119 / ricow_strategy 199 / `architecture_guard` 3 / `ai_live_smoke` 3 通过 + 9 ignored)。**门禁五件**: `fmt --check` 0 差异 / `clippy --workspace --all-targets -- -D warnings` **exit 0、零代码告警**(只剩已知的 Windows 增量锁 `os error 5` 噪声) / `ci_grep_gates.sh` **五条红线全绿** / `node --check` 16 份资产全过 / 测试全绿。**真机取证**: 官方冒烟 `e2e_web.py` **两种形态各 181 PASS / 0 FAIL**(内嵌 / `--web-assets-dir`); CDP(headless Edge + Node 22 内建 `WebSocket`, 零依赖)**问题 0 条** —— 15 条 markdown 断言(`<h1>/<pre>/<code>/<table>/<strong>/<li>` 均产出;**三条注入负例** `<img src=x onerror=...>` 与 `<script>` **全部未执行**、`<img>` 以字面串出现; 宿主 `line` 帧 `mdChildren == 0` 即**不套 markdown**) + 日志面板断言(搜索命中 2 / `error` 1 / `warn` 1; 暂停时 `scrollTop` 保持 0 且显「已暂停 · 1 条新」; 点跟随回底并恢复)。**如实**: 本轮两次「167 PASS」起初是**假绿** —— 044 的 14 条断言补在了 skill 目录的脚本副本里, 而实际执行的是仓库 `tmp/e2e_web.py` 那份; 同步后才是真实的 **181**。已把「跑前先 `cp <skill>/e2e_web.py tmp/`、跑完复核日志里确实出现新增断言字样」写进 `SKILL.md`。
- **前基线 (2026-10-07, Windows, 043 token 303 换发后 SSE/WS 静默 401 修复 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **836 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 835(042): **+1 通过 / ignored 不变(22)**。增量构成: ricow bin **431 → 432**(+1: `web::assets` 锁「meta 载体存在 + 读取方真读它 + 替换后内容正确」); 其余 target 一字未变。同口径冒烟 **167 PASS / 0 FAIL**(较 042 的 159 增加 **B2 段 8 条**)。**缺陷性质**: 页面本身 200 而**所有 `EventSource`/WebSocket 静默 401** —— 中间件「Bearer → 查询串 → cookie」中**查询串优先且不判空**, 导航 303 换发后地址栏的空 `?token=` 直接盖掉新种的 cookie; 而 Python 冒烟每次都显式带 token, **结构上走不到这条组合**, 只有真浏览器能进。修复取最小面(服务端一行未动, 前端加 meta 载体 + `R.TOKEN` 两级取值)。见 `specs/changes/043-web-token-sse-fix/`。
- **前基线 (2026-10-07, Windows, 042 策略视图拆分 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **835 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 834(041): **+1 通过 / ignored 不变(22)**。增量构成: ricow bin **430 → 431**(+1: `test_strategy_shell_loads_before_its_parts` —— **比对下标**而非字符串); 其余 target 一字未变。同口径冒烟 **159 PASS / 0 FAIL**(较 041 的 144 增加 15 条)。**纯重构零行为变化**, 但真机走查抓到一处四道静态门全绿的漏导出(`onUndo` → 打开策略页整片 `ReferenceError`), 已固化为跨文件符号静态审计 + 真机走查常规化。见 `specs/changes/042-strategy-js-split/`。
- **前基线 (2026-10-07, Windows, 041 前端开发回路(`--web-assets-dir` 热读资产) 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **834 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 829(040): **+5 通过 / ignored 不变(22)**。增量构成: ricow bin **425 → 430**(+5: 资产表自检 / 目录启动校验 / 默认内嵌 / 两条端到端); 其余 target 一字未变。同口径冒烟 `e2e_web.py` **两种形态各 144 PASS / 0 FAIL** + 开发回路探针 **19 PASS / 0 FAIL**。
- **前基线 (2026-10-07, Windows, 040 行情实时化(K 线/盘口 SSE 实时推送) 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **829 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 812(039): **+17 通过 / ignored 不变(22)**。增量构成: ricow bin **411 → 425**(+14: `web::realtime` 的订阅复用 / 租约与生命周期 / 死条目不复用 / 就绪窗口与按路隔离 / 帧口径与 20 档截断) + ricow_binance lib **59 → 62**(+3: `parse_kline_frame` 现货帧 / 合约帧 / 拒非 kline 与非对象); 其余 target 一字未变。
- **前基线 (2026-10-06, Windows, 039 Web 图表能力补强 — 回测基准线/回撤曲线/平仓盈亏明细 + 市场 K 线均线与成交量 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **812 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 806: **+6 通过 / ignored 不变(22)**。增量构成: ricow bin **407 → 411**(+4: `commands::backtest` 的基准序列 2(建仓口径 / 三个入参缺失一律空白) + 回撤序列 1(与标量 `max_drawdown` 同口径) + `Option` 抽稀透传 1) + ricow_strategy lib **197 → 199**(+2: `PnlTracker` 平仓明细与聚合一致 / 超上限保留最近且总数不丢); 其余 target 一字未变(ricow_binance 59 / ricow_core 18 / ricow_engine 119 / `architecture_guard` 3 / `ai_live_smoke` 3)。**真机取证**: 临时 root `tmp/verify-039` 起 web 服务, 官方冒烟脚本 **144 PASS / 0 FAIL**(把脚本放到仓库 `tmp/` 使 root 解析正确 + 打开 `show_all_pairs` 让回测段真跑到图表载荷); 另加针对性探针 `tmp/probe_039_chart.py` **14/14 PASS** —— 五序列与 `times` 等长、`closed_total` 与明细一致、`drawdown` 逐点等于由载荷自身重算值且 `min == −max_drawdown_pct/100`、`benchmark` 的 `None` 是连续前缀且 `benchmark[i]/price[i]` 恒为常数、`Σ closed.pnl == realized_pnl`。**如实**: 浏览器像素级渲染未截图取证(未装 agent-browser); 以 `node --check` 语法 + 内置 LWC v4.2.3 的 API 面(`addAreaSeries`/`addHistogramSeries`/`priceFormat type=custom|volume`/`scaleMargins`)核对替代。
- **前基线 (2026-10-06, Windows, 038 P1 工程健壮性加固 — 回测去乐观 / 缺口检测 / 延迟度量 / Prometheus 只读端点 / 崩溃自动重启 / 实盘回测对齐 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **806 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 761: **+45 通过 / ignored 不变(22)**。增量构成: ricow bin **382 → 407**(+25: `commands::align` 汇总/窗口/渲染 8 + `supervisor::server` 重启判定/退避/配置读取/停机收摊 6 + `web::metrics` 渲染/转义/空快照/token 门 6 + `supervisor::procs` 子进程 stdin 停机链路的跨平台覆盖 4 + 其余 1) + ricow_strategy lib **184 → 197**(+13: `gaps.rs` 缺口检测 8 + `backtest.rs` 限价穿透 5) + ricow_engine lib **112 → 119**(+7: `latency.rs` 延迟分位数); 其余 target 一字未变(ricow_binance 59 / ricow_core 18 / `architecture_guard` 3 / `ai_live_smoke` 3)。
    
  门禁五件: `cargo fmt --all -- --check` **0 差异** / `cargo clippy --workspace --all-targets -- -D warnings` **exit 0、零代码告警** / `cargo deny --locked check` 全绿(advisories/bans/licenses/sources 全 ok) / `bash scripts/ci_grep_gates.sh` **五条安全红线全绿** / `architecture_guard` 三条用例全绿。端到端以**临时数据目录 + 手工灌库 + 真二进制**模拟验证(`ricow align` 三档 mode/异常路径、`GET /metrics` 有 token 200 / 无 token 401 空体、daemon 实读 `[supervisor]`), 见 `specs/changes/038-p1-robustness-hardening/converge.md`。
    
  **实测接口变化的边界**: 既有 CLI **用户可见文案零变化**(新能力一律新子命令 / 新参数 / 新端点); 既有回测**数字零变化**(`limit_fill_penetration_bps` 默认 `0`); `RunEvent` schema **未动**(延迟度量只进 `RunOutcome`); 写确认面**未动**。
  **同期补掉的测试覆盖缺口 (2026-10-06, 承接 231 归因复核)**: `supervisor::procs` 的两条用例原先挂 `#[cfg(unix)]`(要 `sh`), 于是「停机指令 → 优雅退出」这条链**在 Windows 上零覆盖**, 而 CI 是三平台矩阵 —— 平台不同就少测一半, 恰是最容易漏缺陷的形状。现已改为两平台各起一份原生替身(`sh` / `cmd /V:ON` 的 `set /p` 读一行; **不能**用 `findstr` —— 它要读到 EOF 才退, 与"生产靠子进程读到那一行就退"不同构), 平台分支写 `#[cfg(not(windows))]` 而非 `#[cfg(unix)]`(门开得更宽, 不会再出现某平台被悄悄排除); `wait_exit` 超时用例改成"让子进程**阻塞在 stdin 上**"(不再依赖平台自带的计时命令 —— Windows 上没有干净的单进程 `sleep`)。子进程 stdin 一律优先走 **`std::io::pipe()`**(Win32 `CreatePipe`)而非 `Stdio::piped()`: 后者那条 NT 具名管道路径在受限进程树里会被注入 DLL 拦掉 231, 一旦只能用它这条链就永久测不到。只有 `request_stop` 的**生产路线**(`Stdio::piped()` → `ChildStdin`)必须留着, 故给它设了三道闸的**精确跳过** —— 只认 231 这一个 errno / 跳过前做正对照(同进程 `std::io::pipe()` 必须可用) / 明确打印跳过说明, CI 三平台与用户普通终端会真跑其断言。另新增 `write_stop` 直测: 指令原文、**flush 必被调用**(漏了它停机指令会卡在缓冲里直到超时)、写失败原样冒泡。`ai_live_smoke` 的两个门禁用例也从"文件句柄当 stdin"改回**真管道**(同一 `std::io::pipe()` 路线) —— 与测试名里的 "piped" 从此名实相符, 临时夹具文件一并去掉。逐条见 `specs/changes/038-p1-robustness-hardening/converge.md` §三。
- **前基线 (2026-10-06, Windows, 037 实盘资金安全加固 — 启动接管 / 引擎级订单登记 / 组合敞口只读视图 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **761 passed / 0 failed / 22 ignored** —— **全目标零失败**。较同口径上次 736: **+25 通过 / ignored 不变(22)**。增量构成: ricow bin **370 → 382**(+12: `commands::exposure` 组合敞口视图的渲染 / 过滤 / 显示宽度对齐) + ricow_engine lib **99 → 112**(+13: `exposure.rs` 的只读聚合, 含跨策略对冲 / 模式隔离 / 名义低估标记 / 输出顺序稳定); 其余 target 一字未变(ricow_binance 59 / ricow_core 18 / ricow_strategy 184 / `architecture_guard` 3 / `ai_live_smoke` 3)。
    
  **如实**: 本文件上次记的 670 是 2026-10-05; 670 → 736 的 +66 来自 **2026-10-06 的四笔审计加固提交**(安全 / 资源泄漏 / 稳定性三梯队 + 中危 ×2 + 低危收尾, `git log` 自 `8bbecd3` 起), 那几轮只写了提交说明、**未单独记基线**, 故 037 的起点按实跑 736 计。
    
  门禁五件: `cargo fmt --all -- --check` **0 差异** / `cargo clippy --workspace --all-targets -- -D warnings` **exit 0、零代码告警** / `cargo deny --locked check` 全绿 / `bash scripts/ci_grep_gates.sh` **五条安全红线全绿** / `architecture_guard` 三条用例全绿。端到端以**临时数据目录 + 手工灌库**模拟验证(不涉 testnet; 本变更不新增下单语义), 见 `specs/changes/037-live-safety-hardening/converge.md`。
- **前基线 (2026-10-05, Windows, Web 策略创建/回测可视化 + 策略日志透出 + 目录诚实性护栏 落地后实跑)**: `cargo test --workspace --no-fail-fast` = **670 passed / 0 failed / 22 ignored** —— **全目标零失败**(此前每轮都带 `ai_live_smoke` 2 例环境性失败)。较同口径上次 667: **+3 通过 / ignored 不变(22)**。增量构成: ricow bin **325 → 341**(+16: 策略创建/回测可视化 6 + 策略日志透出与 P2-8 清单编辑 3 + Web 审查修复 1 + 目录诚实性护栏 6) + ricow_strategy lib **181 → 182**(+1, `LogBuffer` 有界头/尾单测) + `ai_live_smoke` 目标由 `FAILED` 转 `ok`(1 passed/2 failed → **3 passed/0 failed**, 见下条); 其余 target 一字未变: ricow_binance 48 / ricow_core 16 / ricow_engine 77 / `architecture_guard` 3。  **`ai_live_smoke` 2 例已修 (2026-10-05)**: 两个门禁用例(`approve_requires_interactive_tty` / `piped_confirm_phrases_never_reach_the_host`)原先靠 `spawn` + **stdin 管道**, 在 agent 进程树内撞 `ERROR_PIPE_BUSY(231)`; 现改用**文件句柄**作 stdin(`spawn_with_non_tty_stdin`, 内容预写进临时文件)。D5 门禁判的是 `std::io::stdin().is_terminal()` —— **管道与文件都非终端**、走同一分支, 语义等价, 断言强度不变(仍要求进程非 0 退出 / 文案命中 / 零落盘)。(2026-10-06 再改: 已换回**真管道** `spawn_with_piped_stdin` —— `std::io::pipe()` 走的 `CreatePipe` 不受那条 NT 具名管道拦截影响, 于是"用例名写着 piped、夹具却是文件"的名实不符也一并消掉; 见上方"同期补掉的测试覆盖缺口"。)
    
  **2026-10-05/10-06 归因复核(多轮更正, 前两版都是误判)**: 拦截方的正确定位 = **agent 沙箱注入的 DLL `tsbx.dll`**, 作用范围 = **仅 agent 进程树**。逐条: ① "WorkBuddy 沙箱"原判**方向对、依据错** —— 当时拿"关掉 `dangerouslyDisableSandbox` 后错误一字不变"来否掉它, 而**策略开关并不会卸载已注入的 hook DLL**, 那个对照其实证明不了任何事; ② "360 主动防御(系统级)"**排除** —— 用户在自己终端跑**零 ricow 代码**的探针 `pipe_probe.exe` **四行全 OK(含 `stdin piped`)**, 且 2026-10-06 用 `mod_probe.rs` 枚举本进程模块, **非 System32 的只有 `tsbx.dll` 一个、没有任何 360 模块**。触发条件精确为"**stdin 走 `Stdio::piped()`**": stdout/stderr piped 或 stdin 用文件/null 全部正常。机制: Rust std 给子进程 stdin 建管道早已**不走** Win32 `CreatePipe`, 改用 NT 层 `NtCreateNamedPipeFile` + `NtOpenFile`(`library/std/src/sys/process/windows/child_pipe.rs`, 注释原话 "we specifically do *not* use `CreatePipe` here"), 故 Python(`CreatePipe`)/.NET(命名管道, 含可继承)同构造均正常。**2026-10-06 再订正两点**(判别性探针 `pipe_probe2.rs` / `pipe_probe3.rs`, 零 ricow 代码, 同进程逐项跑): ① **不是"匿名管道被拦"** —— 同一进程里 `std::io::pipe()`(Win32 `CreatePipe`)拿**读端当子进程 stdin 完全正常, 子进程真收到写入的字节(退出码 0)**; `stdin = null` 先跑 OK 而紧随其后的 `stdin = piped` 必 231、连做 3 次全 231(排除"第一次 spawn / 偶发")。**被拦的只有 Rust std 为子进程 stdin 走的那条 NT 具名管道路径**。② **与 Rust 版本无关** —— rustc **1.83 / 1.85 / 1.96.1 三版实测行为一致**, 故"1.96 才改的"说法也不准。Rust 上游 issue **#143078** 记录的正是这类第三方 hook 对 NT 文件/管道调用的干扰(上游为此加过缓解补丁); **注入者已定位**: `mod_probe.rs` 枚举本进程模块, **非 System32 的只有 agent 自带的沙箱 DLL `tsbx.dll`**, **一个 360 模块都没有**(360 `ZhuDongFangYu` / `SafeWrapper.dll` 只管机器, 未注入本进程)。**产品代码不改**: `supervisor/procs.rs:80` 的 `stdin(Stdio::piped())` 是**写入**通道(给子进程发 `stop`), 只读文件句柄替不了; 且它在用户终端本就正常 —— 属环境差异; **但"本机无法验证 stdin 通道"这条旧结论要修正**: `std::io::pipe()` 的读端**可以**当子进程 stdin 且不被拦(`pipe_probe3.rs` 实测子进程收到字节) —— 这是 agent 树内唯一可行的"喂 stdin"路线, 只是它绕开了 `ChildStdin`, `procs::request_stop`(形参即 `Option<ChildStdin>`)的 happy path 仍无法覆盖。(2026-10-06 更新: 该 happy path **已补**用例 `request_stop_writes_through_real_child_stdin` —— CI 三平台与用户终端真跑, 本机 agent 树内按三道闸精确跳过; 同模块另两条原先 `#[cfg(unix)]` 的用例也已恢复跨平台覆盖。见上方"同期补掉的测试覆盖缺口"。)故此前"本机 `ricow start` 大概率失败"的推论**一并撤回**(用户普通终端里它正常)。
    
  **门禁**: `clippy --workspace --all-targets -- -D warnings` **exit 0、零代码告警**(仅刷 Windows incremental 锁文件 `os error 5` 环境告警); `cargo fmt --all -- --check` **已通过(exit 0 / 0 处 diff)** —— 首跑曾报 28 处 diff / 7 文件(`ai/provider.rs`、`commands/backtest.rs`、`strategies/catalog.rs`、`web/backtest_jobs.rs`、`web/mod.rs`、`web/strategy_io/{mod,tests}.rs`), 已于 2026-10-05 用 `cargo fmt --all` 修好(**纯格式, 不改语义**); 修测试文件后又复跑一轮: **fmt --check 0 差异 / clippy -D warnings exit 0 / 全量 test 670 passed / 0 failed** —— 三门禁齐备, 提交不会红。本基线取自在建工作区(该轮改动尚未提交)。
- **前基线 (2026-10-03, Windows, 036 回测敏感性 + Web 工程债加固落地后实跑)**: `cargo test --workspace --no-fail-fast` = **650 passed / 22 ignored**(另有 2 例环境性失败, 见下); 较上次 642: **+8 通过 / ignored 不变(22)**。增量构成: 敏感性阶梯与报告 4(`commands/backtest.rs`) + 来源门变体矩阵 2(`web/auth.rs`) + SSE 断线续传 1(`web/logs.rs`) + 会话入站 1(`web/sessions.rs`)。`fmt --check` 0 差异 / `clippy --all-targets -D warnings` 零代码告警 / 安全红线门禁 4 条全绿; `web::` 全组 102 用例未改断言全绿(拆文件零行为变化取证); 逐条见 `specs/changes/036-backtest-sensitivity-web-hardening/`。
- **前基线 (2026-10-03, Windows, 035 工程底座加固落地后实跑)**: `cargo test --workspace --no-fail-fast` = **642 passed / 22 ignored**(另有 2 例环境性失败, 见下); 较上次 611: **+31 通过 / ignored 不变(22)**。增量构成: 2.1 币安 REST 重试 4(`ricow_binance/src/retry.rs`) + 2.2 K 线缓存 1 + 2.3 回测 run card 2(`commands/backtest.rs`) + 2.4 编号迁移与锁抖动 3(`ricow_strategy/src/db.rs`) + 3.3 运行事件流 9(`ricow_strategy/src/{events,context}.rs`) + 3.4 LLM 重试与历史预算 6(`ai/provider.rs`) + 3.5 配置 schema 版本 6(`commands/config_file.rs` + `ai/config.rs`)。`fmt --check` 0 差异 / `clippy --all-targets -D warnings` 0 / `cargo deny --locked check` = advisories/bans/licenses/sources **全 ok**; 逐条见 `specs/changes/035-engineering-hardening/`。
    
  **如实**: 2.4 顺带根治了本机**高频抖动**的 `ai::tools` 部署类用例 `SQLITE_BUSY(database is locked)` —— 根因是 `Database::open` 每次跑全量建表写事务, 而 `PRAGMA journal_mode=WAL` 建池需独占锁、SQLite 对"另一连接正在用"**不调用 busy handler** 直接返回 `SQLITE_BUSY`, 竞争者只活几十毫秒。修法双管: ① 迁移改为 `PRAGMA user_version` 驱动、版本已最新时**零写事务**; ② `open` 加 10 次线性退避重试(仅锁错误, `tokio::time::sleep` 以免卡死 current_thread 运行时)。连续 3 次 `cargo test -p ricow --bin ricow` 全绿(`317 passed / 0 failed / 1 ignored`)。
- **前基线 (2026-10-03, Windows, 034 UI 主题+折叠落地后实跑)**: `cargo test --workspace` = **611 passed / 22 ignored**(另有 2 例环境性失败, 见下); 较上次 608: **+3 通过 / ignored 不变**; ricow bin 用例 **300 → 303(+3)**, 增量全部来自 034 —— `[ui].theme` 配置解析 2 例 + `/api/theme` 端点 1 例。`fmt --check` 0 差异 / `clippy --all-targets -D warnings` 0。三主题(深/白/红)与历史消息两行折叠以真实浏览器截图取证, 见 `specs/changes/034-ui-themes-fold/converge.md`。
    
  **如实**: `ai_live_smoke` 的 2 例(`approve_requires_interactive_tty` / `piped_confirm_phrases_never_reach_the_host`)在**本机 agent 进程树内**(**不是** agent 沙箱 —— 归因订正见上条 2026-10-05/10-06)报 `ERROR_PIPE_BUSY(231)` —— 它们只是 `spawn ricow` 并喂 stdin 管道, 用一个不含任何 ricow 代码的 20 行 Rust 程序即可复现同一错误, 故为**环境侧注入 DLL 对 Rust std 那条 NT 具名管道路径**的拦截(不是泛泛的"环境对管道创建的拦截", `CreatePipe` 实测正常), 非代码缺陷(032 基线在真机终端为全绿)。另在**高负载轮次**偶见 `ai::tools` 部署类用例 `SQLITE_BUSY(database is locked)` 抖动 —— **该抖动已于 035 根治**(见上条"当前基线")。
- **前基线 (2026-10-02, Windows, 033 密钥管理页落地后实跑)**: `cargo test --workspace` = **608 passed / 22 ignored**(另有 2 例环境性失败, 见下); 较上次 589: **+19 通过 / ignored 不变**; ricow bin 用例 **279 → 300(+21)**, 增量全部来自 033 —— `commands/config_file.rs` 密钥环 12 例 + `web/keyring.rs` 密钥端点 9 例。`fmt --check` 0 差异 / `clippy --all-targets -D warnings` 0。
- **前基线 (2026-09-29, Windows, 032 Web UI 工作台化落地后实跑)**: `cargo test --workspace` = **589 passed / 0 failed / 22 ignored**; 较上次 538: **+51 通过 / ignored 不变**; 增量全部来自 032-web-ui-console(web 模块: 配置/密钥端点、策略保存与源码/AI 编辑/回测 job、运行面 ensure_daemon 与 start/stop/risk 门禁、401 矩阵与前端资产密钥扫描等单测)。三门禁全绿(fmt 0 差异 / clippy --all-targets -D warnings 0 / test 0 failed); 端到端以浏览器实测 + testnet 真实调用取证(场景 1/3/5 走查 + demo 实发下单 + live 拒绝路径), 见 `specs/changes/032-web-ui-console/quickstart.md` 执行记录。
- **前基线 (2026-09-25, Linux, 031 策略目录重构 + 清单 + 统一注册落地后实跑)**: `cargo test --workspace` = **538 passed / 0 failed / 22 ignored**; 较上次 529: **+9 通过 / ignored 不变**; 增量 = catalog.rs 清单解析/校验单测 8 例 + architecture_guard「清单键 == Lua 读取键」一致性 1 例。三门禁全绿(fmt 0 差异 / clippy -D warnings 0 / test 0 failed); 真实回测逐位一致验证通过: `--strategy {paired_grid,shannon_spot_grid}` 与迁移前(51ccb46 二进制)固定窗口报告逐位一致, 旧实例 TOML(`type=内置id`)加载行为不变。
- **前基线 (2026-09-25, Linux, 内置策略收敛 + paired_grid 等比化后实跑)**: `cargo test --workspace` = **529 passed / 0 failed / 22 ignored**; 较上次 544/22: **−15 通过 / ignored 不变**; 变化 = 删除 shannon_rebalance(4 例) + executors/{dca,twap,vwap,pullback,ladder}(约 10 例) + paired_grid ATR 未就绪(1 例); paired_grid 改名「现货动态非对称网格」并等比化(买价 = ref÷(1+间距), 卖价 = 栈顶买入价×(1+间距))。
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
- 历史基线: P1 63 → P2 76 → P3 125 → 回测重构 173 → 001-vwap 154(分支基线) → 220 → 204(009 移除 locus_hl) → 216(008 实施后) → 233(004 实施后) → 270(011 实施后) → 282(012 实施后) → 286(013 实施后) → 289(014 实施后) → 294(003 实施后) → 301(002 实施后) → 304(015 实施后) → 306(016 实施后) → 308(018 实施后) → 339(021/022 告警清零与格式化后) → 348(019 R1/R2) → 355(019 R3) → 390(019 R4/R5) → 403(019 R5 + gr 复核修复) → 472(023 对话体验 + 025 Web UI) → 504(026 交易可见性) → 538(031 策略目录重构) → 589(032 Web UI 工作台) → 608(033 密钥管理页) → 611(034 UI 主题+折叠) → 642(035 工程底座加固) → 650(036 回测敏感性 + Web 工程债加固) → 670(2026-10-05 Web 策略创建/回测可视化 + 策略日志透出 + 目录诚实性护栏; 含 `ai_live_smoke` 2 例环境性失败修复, 全目标 0 failed) → 736(2026-10-06 审计加固四提交 — 安全/资源泄漏/稳定性三梯队 + 中危×2 + 低危收尾; 未单独记基线, 由 037 起点反推) → 761(2026-10-06 037 实盘资金安全加固: 启动接管遗留挂单 / 引擎级最小订单登记 / 组合敞口只读视图; 全目标 0 failed) → **806(2026-10-06 038 P1 工程健壮性加固: 回测限价去乐观 / 数据缺口检测 / 下单延迟度量 / Prometheus 只读端点 / 崩溃自动重启 / 实盘回测对齐工具, 并收口"平台门控导致某平台零覆盖"的测试缺口; 全目标 0 failed) → 812(2026-10-06 039 Web 图表能力补强: 回测基准线/回撤曲线/平仓盈亏明细 + 市场 K 线 MA7/25/99 与成交量副图 + K 线涨跌配色改回"涨红跌绿"; 全目标 0 failed) → **829(2026-10-07 040 行情实时化: 后端币安 WS(K 线 + 盘口) → SSE 实时下发, 上游按 `(市场,标的,周期)` 共享并引用计数回收, 按路隔离 + 12s 就绪窗口如实报错; 全目标 0 failed) → **834(2026-10-07 041 前端开发回路: 资产清单收敛为单一 `ASSETS` 表并派生路由, `--web-assets-dir` 从磁盘热读同一批资产(路径从不来自请求 → 无目录穿越), 给错目录启动即失败、缺文件 500 不静默退回; 全目标 0 failed) → 835(2026-10-07 042 策略视图拆分: `strategies.js`(2626 行)拆为外壳 + 三份子文件, `R.sg` 共享命名空间 + 状态袋 `S.st` + 跨文件延迟别名, 加载顺序由单测与首页对齐测试双向盯住; 纯重构零行为变化, 全目标 0 failed) → 836(2026-10-07 043 token 303 换发后 SSE/WS 静默 401 修复: 前端加 meta token 载体 + `R.TOKEN` 两级取值, 服务端 token 语义一行未动; 全目标 0 failed) → **838(2026-10-07 044 Web UI 体验补强: AI 对话受限 Markdown 渲染(只产 DOM, 零 innerHTML, 仅作用于助手气泡) + 日志面板搜索/级别过滤/跟随暂停开关(诚实计数); 全目标 0 failed, 当前)**。


## 变更档案状态 (specs/changes/)

| 编号                          | 变更                                     | 状态                                                                                      | 结论 / 备注                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |                                                                                                                                                         |
| :-------------------------- | :------------------------------------- | :-------------------------------------------------------------------------------------- | :---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 001-vwap                    | VWAP 执行模式示例                            | ✅ 已收敛 (2026-09-01)                                                                      | 6 回调 API 之外的 `ctx.klines.volume` 字段落地; 见该档案 converge.md                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |                                                                                                                                                         |
| 002-ai-quant-researcher     | AI 量化研究员(Binance bStocks)              | ✅ 已实施 (2026-09-13)                                                                      | 暴露建策略闭环 CLI: `ricow create`(编译门禁 → 真实 K 线沙箱回测 → preview, **不落盘**)→ `ricow approve`(人工批准, 一次性 token)→ `ricow deploy`(落盘 `<name>.toml` + `<name>.lua`, 同名拒绝覆盖/路径穿越拒绝); 引擎侧新增 `execute_strategy`, 报告打印与 `backtest` 共用同一口径(顺带修初始余额硬编码 USDC); **并接管 `dry_run_started_at`**: 首次 Dry Run 启动写入(ISO8601), 实盘启动按 `params.min_dry_run_hours`(默认 24h = 覆盖三时段一个完整日周期, 设 0 关闭)核验, 不足即拒绝启动(不静默降级); 实测: 语法错误代码被拒且零落盘、真实 K 线回测出报告、部署物 Dry Run 真跑通(tick=50 成交=1)、二次/同名部署被拒、`run/start --live` 双双被时长门禁拒绝且未下任何单、`min_dry_run_hours=0` 放行后仍被时钟预检拦住; 测试 294 → 301                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |                                                                                                                                                         |
| 003-notifications           | 出站通知(成交 + 熔断告警)                        | ✅ 已实施 (2026-09-13)                                                                      | 补齐 product.md §三.4 承诺; 通道=用户自选 webhook(`params.notify_webhook`, 一个 POST 覆盖 Telegram/飞书/Slack/自建, 守住"数据不出本机"), 四类事件=成交/熔断/接近强平/停机残留; 事件白名单 + 同类限速(默认 5s, 被压制数如实附带)+ 强平按 (pair,方向) 去重; 投递 spawn+5s 超时+失败只 warn(不重试不排队, 绝不反压交易循环); 默认关闭; 实测: 本地回环端点真实收到 4 条 POST(成交×2/接近强平/平仓成交, 内容与事件一致)、不可达端点时仅 warn 且 tick 照常、成交 1 笔→通知 1 条(修掉一处重复投递); 测试 289 → 294                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |                                                                                                                                                         |
| 004-risk-guards             | RiskEngine 补项(下单频率上限 + ~~两级亏损熔断~~)     | ✅ 已实施 (2026-09-12); **两级亏损熔断已于 2026-09-15 删除**(020)                                     | 频率上限(滑动窗口 1s, 默认 100/s) + 两级熔断(连续 3 日净亏损停开仓 / 峰值回撤 30% 停新交易); **并补上关键缺口: 风控此前只挂在无消费者的 `StrategyScheduler` 上, 回测/Dry Run/实盘三条下单路径从未过风控** → 现三条路径统一 `RiskEngine::from_config` 接入; 测试 216→233; 实测: 默认参数回测与"护栏关闭"逐位一致(零误伤), `limit=1` 冒烟在真实行情 Dry Run 下拒绝 715 单(有据可查); 见该档案 plan/tasks                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |                                                                                                                                                         |
| 005-market-filter           | 市场分类层 + ~~scan 池参数化~~                  | ✅ 已实施 (2026-09-07); **`scan` 已于 2026-09-15 删除**(020; `market_class`/`us_tickers` 按明示保留) | 交付物: `market_class`/`us_tickers`/`nasdaq` 适配 + `scan --pool bstock-spot`; 策略消费者见 006                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |                                                                                                                                                         |
| 006-bs-momentum-lua         | bs_momentum 策略 Lua 化合规整改               | ✅ 已实施 (2026-09-09)                                                                      | 宪法原则二整改(撤销 Rust 版策略本体); 策略本体已于 2026-09-11 下线                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |                                                                                                                                                         |
| 007-bs-momentum-v2          | bs_momentum 正期望改造                      | ✅ 已实施 (2026-09-09)                                                                      | 市场门 / 周频 / 持仓下限; 真实成交轨期望 ≈0 → 判定不合格, 策略下线                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |                                                                                                                                                         |
| 008-platform-process-model  | 跨平台进程模型重构(策略启停/状态/日志)                  | ✅ 已收敛 (2026-09-12)                                                                      | 常驻 daemon(pm2 模型)+本机 TCP(token)+stdin 管道优雅停机(EOF 自愈)+实例台账+实例/成交查看; 实测: 全链路冒烟(daemon/list/start/info/fills/logs/stop)、崩溃自愈(kill -9 daemon → 子进程 4s 自愈退出)、testnet 真实调用 3 例跑绿(现货/合约 openOrders 闭环 + 拒单如实上报)、测试 204→216/0/9; 收敛遗留: 实盘撤单兜底与持仓明细随实盘运行器落地(见档案 Phase 7)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |                                                                                                                                                         |
| 009-remove-hyperliquid      | 移除 Hyperliquid 适配与文档承诺                 | ✅ 已收敛 (2026-09-12)                                                                      | 删 `locus_hl` crate(1696 行) + 全部引用点 + 文档承诺; 测试 220 → 204; 见该档案 converge.md                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |                                                                                                                                                         |
| 011-live-runner             | 实盘运行器(让 `live_enabled=true` 真正生效)      | ✅ 已实施 (2026-09-13)                                                                      | 门禁双条件(`live_enabled` **且** `--live`)+ 下单前时钟预检 + 真实账户装配 + 双流(盘口 + WS-API 用户流)+ 成交回写落库 + 停机清理(撤单兜底/可选平仓/残留复查, 幂等)+ `start --live`/`stop --close-all`/`info` 实盘快照; 实测(demo 现货): 用户流订阅就绪并发到 `executionReport`、T014 挂单→停机撤单兜底→交易所零残留、T015 市价买入→`stop --close-all` 兜底平仓(2 笔成交回写落库)、时钟预检拒绝超限(自然漂移 1142ms 实测拒绝) ; 测试 233 → 270; **合约实盘为 Phase 2 未做**(需新建 `BnFuturesExchange`); 见该档案 plan/tasks/converge                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |                                                                                                                                                         |
| 012-futures-live-adapter    | 合约实盘适配层 (USDT-M)                       | ✅ 已实施并真实验证 (2026-09-13)                                                                 | `BnFuturesExchange` 实现 `Exchange`(REST + `fstream` WS 盘口增量合并 + listenKey 用户流就绪语义); 引擎按 `market` 分派装配/持仓语义, hedge 定向持仓(缓存键 \`pair                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | side`)与 one-way/hedge 平仓参数; 实测: 挂单→撤单兜底零残留、开仓→`stop --close-all`归零、hedge 双向两侧分别平掉; 顺带修平仓单号超 36 字符被拒 与`ricow info\` 无成交策略 panic + 手续费 f64 噪音; 测试 270→282 |
| 013-futures-margin-model    | 合约逐仓记账校准(按侧独立钱包/强平 + 结算时点)             | ✅ 已实施 (2026-09-13)                                                                      | 依据 2026-09-05 真实清算实测(价格下行只清 LONG 侧、SHORT 侧原样保留; 两侧保证金独立占用)修正回测: 钱包按侧寻址 + hedge 按侧独立强平 + 两侧资金费独立结算(不跨侧抵消); 关闭 L1(结算点与 interval 无关, 1d 由欠结 2/3 修正为恰 3 次)与 L2(尾 bar 补结算 `finalize()`); 报告新增 hedge 按侧明细(各侧强平次数/收盘钱包余额)便于与交易所账单对照; one-way 与现货路径行为不变; 测试 282 → 286                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |                                                                                                                                                         |
| 014-live-funding-and-liq    | 实盘资金费记账 + 强平风险可见(合约)                   | ✅ 已实施 (2026-09-13)                                                                      | 资金费以**交易所账单**为准(`GET /fapi/v1/income`, 不自算费率×名义)→ `funding_fees` 表(`tran_id` 幂等, Rust 侧 Decimal 精确聚合) + 启动/30min/停机前增量拉取(水位 = `MAX(funding_time)`); `Exchange` trait 增 `funding_income`(默认空, 现货零改动); 距强平距离纯函数 + 阈值告警(实测: 50x 小仓 liq=2443.31 / mark=2479.89 → 距离 1.48% 触发 warn, 与公式一致); 实测 `income` 端点签名调用返回 `[]`(demo 账户无跨 8h 持仓 → 如实显示 0 笔); 测试 286 → 289                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |                                                                                                                                                         |
| 015-backtest-noop-fill      | 回测撮合零动作单口径修复 (L4)                      | ✅ 已实施 (2026-09-13)                                                                      | hedge `sell + position_side="long"` 而多仓为 0 这类"成交但零动作"的请求, 此前照收手续费并记一笔成交(成本/笔数虚增); 现判定于**成交那一刻**: 市价单拒单计数、限价单保持挂单(与"资金不足"同一约定), 均不计费不记笔数; 真实对照(同窗口)成交 1→0 / 手续费 0.0482688→0 / 拒单 0→1, 且不触发 L4 的策略逐位一致; 测试 301 → 304                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |                                                                                                                                                         |
| 016-dryrun-initial-cash     | Dry Run 虚拟本金可配 (`params.initial_cash`) | ✅ 已实施 (2026-09-13)                                                                      | 原 Dry Run 本金硬编码 100k, 使"小资金预演"不成立 —— 同一份 `[risk]` 限额不可能同时适配 100k 与真实几百 USDT(实测: 小资金限额下 Dry Run `tick=66 提交订单=65 拒单=65 成交=0`); 现 `params.initial_cash` 可配(缺省 100000 保持既有行为), 非法值报错**不静默回落**, 启动打印本金额; 实测 `initial_cash=300.0` → 建仓 150 USDT/拒单 0(对比修复前 65 单全拒); 测试 304 → 306                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |                                                                                                                                                         |
| 017-spot-live-snapshot-fix  | 现货实盘快照一致性修复 (dogfood 实测)               | ✅ 已实施 (2026-09-13)                                                                      | demo 实盘预演暴露三处: ① **`stop --close-all` 静默不平仓**(012 起清理改走 `get_positions_directional`, 现货实现恒空 → 报告"无持仓"而账户仍持 2.016 ETH); ② **现货成交后只刷持仓不刷现金** → 策略按 equity 决策以为"只有币没有钱", 每 tick 再卖一半, 几何级数清仓(`1.0079→0.5039→0.252→…` 全卖光); ③ 成交回写异步窗口内重复下单(1.4s 内 3 次同单)。修复: 清理/残留持仓源按市场分支(现货复用 `spot_position_of`)+ 现货补刷 base/quote 余额 + 下单后按 `any_filled` 立刻对齐快照。实测修复前 `提交订单=216/拒单=209/成交=8/持仓清空` → 修复后 `提交订单=1/拒单=0/成交=2/平仓单执行/残留 0.000058`, 交易所侧 `openOrders=0`; 测试 306 不变(接线/时序缺陷无 mock 单测, 以真实链路为证)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |                                                                                                                                                         |
| 018-first-use-risk-ack      | 首次使用风险确认                               | ✅ 已实施 (2026-09-13)                                                                      | 补齐 `product.md` §十 风险披露的第二条(README 免责声明早已有, 代码侧确认**完全缺失** —— 克隆仓库配好 key 即可用真钱开跑且无任何告知)。实盘启动最前置判定: 未确认 → 拒绝 + 打印四条披露要点与确认方式; `--accept-risk` → 记录到 `$RICOW_ROOT/risk_ack.json`(含 schema 版本, 披露变更可递增触发重新确认)后放行, 之后不再要求; 判定顺序 风险确认→时长门禁→时钟预检(未确认时零交易所往返), Dry Run/回测不受影响; 实测三步(拒绝且无记录文件 / 确认落地 / 再跑不再要求并正常启动); 测试 306 → 308                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |                                                                                                                                                         |
| 019-ai-assistant            | 内置 AI 助手 + 生态入口 + 开箱即用分发               | 🔄 **主线已打通, 做文档/测试收口 (2026-09-16)**                                                     | 已落地并真机验证: `ricow ai`(交互/单次/`--plain`, 只读 12 + 虚拟 4 工具)、策略生成闭环(`preview_strategy` → 编译门禁 → 真实 K 线沙箱回测 → 零落盘)、`ricow create/approve/deploy` 两步确认(逐字短语)、**`ricow agent-kit`**(AGENTS.md / SKILL.md / CLAUDE.md / lua-api.md, 与内置 AI 系统提示同源; 命令速查由 clap 生成; 拒覆盖)、`--demo` 测试网运行、实盘二次分离(逐字 `确认实盘 <name>`)、策略名规范、提示词按需取文档(8k→1.8k tokens)、**单一配置文件 `ricow.toml`**(provider+api_key 同段, 删 keyring/setup/写凭据命令)。**R4(2026-09-16)全功能对话化**: 裸 `ricow` 即入口(默认 `chat`, 首次跑走向导 `onboard`)、会话缝 `ai/session.rs`(`ChatSession` + `SessionSink`, 业务零 stdio)、对话内七动作确认状态机(逐字短语:`确认部署` / `确认启动测试网` / `确认风险` / `确认实盘` / `确认停止测试网` / `确认停止实盘` / `确认平仓停止`; 实盘仍原样跑 `ctrl::live_preflight` 三判据)、`/keys` `/market` 外科式改 `ricow.toml`(`set_values` 白名单 9 键, 保留注释 + 原子写; 权限 Unix 0600 / Windows 仅当前用户 ACL)、`pairs` 命令与交易对视野。**R5(2026-09-16)删平台风控残留**: 删 `risk.rs` 静态限额(`RiskEngine` / `[risk]` 配置面 / 装配器)与 `scheduler.rs`, 只留固定 100 单·秒⁻¹ 工程护栏 `order_guard` —— 平台不做投资判断, 风控由策略自管(`constitution.md` 已同步修订)。未完成: 三平台分发(CI)、文档/测试收口、agent-kit 的真机第三方 agent 验证(T047)(`ricow mcp` **不做** —— 2026-09-15 定案: 单机程序, 用户已有的 agent 直接调本机 CLI, 生态入口 = agent-kit 手册); Phase 1–4 主体完成(仅 Windows 双击入口 T034/T035 未做, 本机无法产出 Windows 产物), 见 `specs/changes/019-ai-assistant/tasks.md`(T001–T078) |                                                                                                                                                         |
| 037-live-safety-hardening   | 实盘资金安全加固(启动接管 / 订单登记 / 组合敞口)           | ✅ 已实施 (2026-10-06)                                                                      | 对标商用框架(NautilusTrader / LEAN / Hummingbot)**架构级**差距的 P0 三项(依据 [`specs/research/framework-vs-commercial-2026-10.md`](research/framework-vs-commercial-2026-10.md))。① **P0-A 启动接管遗留挂单**: 启动按归属前缀切分交易所挂单 → 本实例归属逐个撤销(逐笔落事件 `orphan_canceled`)→ 复查 → **live 仍有残留或枚举失败则拒绝启动**(带单号与手工撤单指引), demo 只警告; 非归属单只上报不撤; **无 `orphan_policy` 配置面**。② **P0-B 引擎级最小订单登记** `ricow_engine::oms`: 会话内订单生命周期 + **传输失败 = `Unknown`**(收尾提示手工核对, 不谎报结论) + 非终态重复单号检测 + 账目计入 `RunOutcome`; 纯内存无 IO, 失败不阻断交易。③ **P0-C 组合敞口只读视图**: 新增 `ricow exposure`, 逐 `(标的 × 模式)` 汇总净头寸 / 多空合计 / 总名义(按开仓均价估算, 低估则标 `*`)/ 未终结挂单数 / 活跃策略数, `--detail` 下钻; **只展示不拦截、不直连交易所、绝不跨模式相加**。**不做 P0-D**(单笔名义上限 / 速率熔断 —— 需可配置风险参数面, 与宪法 D15/D17 冲突, 待用户拍板)。测试 736 → **761 passed / 0 failed / 22 ignored**(+25), 门禁五件全绿; 端到端以临时数据目录 + 手工灌库模拟验证; 见 `specs/changes/037-live-safety-hardening/`                                                                                                                                                                                                                                                                                                                                                                                                           |                                                                                                                                                         |
| 038-p1-robustness-hardening | P1 工程健壮性加固(去乐观 / 可观测 / 自适应)            | ✅ 已实施 (2026-10-06)                                                                      | 同一份调研报告的 **P1 六项全做**, 主线是"把不可见的假设与失败摆到台面上": ① **回测限价成交去乐观**(`--limit-fill-penetration-bps` / TOML `[backtest]`, **默认 0 = 既有数字零变化**; 报告单列"限价单成交 N 笔 / 占比 / 穿透深度")。② **数据缺口检测**(`gaps.rs` 纯函数 + CLI 取数后硬报错, 列前 5 处并如实说明"另有 N 处未列出", 不再拿稀疏 K 线静默回测)。③ **下单延迟度量**(`latency.rs` nearest-rank 分位数, 只测 `place_order` 往返并在报告里写明不含行情推送与策略计算; 只进 `RunOutcome`, **不动 `RunEvent` schema**)。④ **Prometheus 只读端点**(`GET /metrics` 复用既有 web 服务与 token 门, 不引 crate, 不新开端口; 读库失败降级为空而非 500)。⑤ **崩溃自动重启**(`[supervisor]` 段默认 `none` = 零行为变更; 非法值硬拒并降级; 重启走既有 `Server::start` 于是**不绕过 037 的启动挂单接管**; 主动停机结构性不触发)。⑥ **实盘/回测对齐工具**(新增只读子命令 `ricow align`, 窗口由 run card 还原, 三条诚实性硬约束: 口径差异显式印出 / 样本不足直说无从对比 / 期货下现金流不叫盈亏)。测试 761 → **806 passed / 0 failed / 22 ignored**(+45), 门禁五件全绿; 见 `specs/changes/038-p1-robustness-hardening/`                                                                                                                                                                                                                                                                                                                                                                                                                                      |                                                                                                                                                         |
| 039-web-chart-upgrade       | Web 图表能力补强(基准线 / 回撤 / 平仓明细 / K 线指标)          | ✅ 已实施 (2026-10-06)                                                                      | 对标 FreqUI / FMZ / QuantConnect 的**图表能力**收口, 同时**更正**了上一轮分析报告的一处误判 —— 报告把"回测无可视化"列为最大差距, 复核代码后该判断有误: `BacktestOutcome::chart()` 早已下发 `{times,price,equity,fills}` 且前端已画权益曲线 + 买卖点。真实缺口收窄为三项: ① **基准对照线**(口径与指标卡 `benchmark_return_pct` 同源 = 首次成交价 + `entry_equity` 满仓持有, 建仓前空白不画 0); ② **逐点回撤曲线** (`(权益−历史峰值)/峰值`, 由**全分辨率**曲线算完再抽稀 —— 对抽稀序列重算会系统性低估); ③ **平仓盈亏明细表** (`PnlTracker` 增 `(时刻,已实现盈亏)` 记录, 与胜率/已实现盈亏**同源**故可机械核对; 回测时刻取**虚拟 bar 时间**而非墙钟, 否则图表时间轴对不上; **不做**开→平 round-trip 配对 —— 合约对冲下无唯一解)。市场页 K 线补 **MA7/25/99 + 成交量副图**(前端现算, 零后端改动/零新依赖; 数据不足处**断线**, 不补 0 冒充), 并**修正** K 线涨跌配色为项目自身约定「涨红跌绿」(此前用 accent 作涨色, 与指标卡/历史表相反)。诚实性硬约束: 明细超 1000 条时 **同时报总条数**并明说"仅显示最近 N 条", 只报显示部分的求和, 不拿部分和冒充总数; 无平仓事件不渲染空表。测试 806 → **812 passed / 0 failed / 22 ignored**(+6); 门禁五件全绿; 真机取证 = 官方冒烟 **144 PASS / 0 FAIL** + 针对性载荷探针 **14/14 PASS**。见 `specs/changes/039-web-chart-upgrade/`                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| 040-realtime-market         | 行情实时化(K 线 / 盘口 SSE 实时推送)                | ✅ 已实施 (2026-10-07)                                                                      | 市场页 K 线与盘口由「进入详情即拉一次的**死快照**」改为**真实时**, 路线 = **后端币安 WS → SSE 下发**(非前端轮询增强); 浏览器侧仍是 SSE(D18「前端不做 WebSocket」未被推翻), 零新依赖 / 不新开端口 / 无新表。① **上游新增 K 线流订阅**: `ws.rs` 新增 `parse_kline_frame`(纯函数, 只认 `e=="kline"`, 字段缺失或类型不符即 `None`, 不猜)+ `run_kline_ws`(与 `run_depth_ws` 同构: 退避重连 + 消费端退出即停; K 线帧自带整根 OHLCV 故**无需快照同步**), 现货 / 合约各一条; `futures_ws.rs` 第二份 `backoff_delay` 删除。② **订阅复用内核 `web/realtime.rs`**: 按 `(市场,标的,周期)`(K 线)/ `(市场,标的)`(盘口)**共享上游**, 同键 N 个页面共用**一条**币安连接(引用计数 + `AbortHandle` 租约, **不用 `receiver_count()`** —— Lease 与 Receiver 的 drop 顺序无保证会时而漏拆); 最后一个订阅者离开即 abort(**不设宽限期**, 空闲连接白占币安配额); **死条目不复用**(中止重建)。③ **SSE 端点** `GET /api/markets/{symbol}/stream?market=&interval=`(同一道 token 门; 首帧 `hello` 先发避免白屏; 盘口服务端截断 **20 档**; `interval` **必填**不继承兄弟端点 `1h` 默认; 标的不在行情视野 → **联网前 404**)。④ **按路隔离 + 12s 就绪窗口**(诚实性核心): K 线 / 盘口各自订阅各自报错(`error` 帧带 `scope` + 实情原文), 上游"订上了但一帧不给"按路如实报「12 秒内未推送任何数据」而**不打死整条连接**, 数据后来到了自行恢复; 依据是**实测** —— 币安对不存在的标的照常完成握手然后**一帧不发**(故"解析错误帧"是死代码, 真正需要的是**标的预校验**), 且**合约 WS 主机的非订单簿行情整体不下发**(同为 20s 窗口: `bookTicker` 9 105 帧而 `aggTrade`/`kline_*`/`ticker`/`markPrice@1s` **全 0**, 连 PING/CLOSE 都没有; 已排除我方 URL/参数、客户端库、单节点(8 IP 全同)、预热(90s 仍 0)、走错主机与中间人, 且平台**正常受理订阅**; 推断为币安侧按地区/出口 IP 的行情权限限制, **不臆造其内部原因**, 见 converge.md §3)。前端 **三态状态 chip + 最后更新时刻 + 手动重试**, **绝不静默降级成轮询**; **不做** Last-Event-ID 续传(行情无补发语义, 重连以 REST 快照重对齐); 零持久化 / 帧数据经 `textContent` 注入 / 无交易所写动作(面板只读不变)。测试 812 → **829 passed / 0 failed / 22 ignored**(+17 = ricow bin +14 / ricow_binance +3), 门禁五件全绿; 真机取证 = 针对性 SSE 探针 **23 PASS / 0 FAIL**(现货 22s 收到 189 帧且 `close` **真的在跳**、WS `open_time` 与 REST 逐位相等、`close` 相对差 **0.000000%**、盘口买一/卖一逐字相同; 合约盘口 191 帧正常而 K 线零帧 → 按路错误帧隔离; 缺 interval/缺 market/坏周期 → 400, `ZZZUSDT` → 404, 无 token → 401)。见 `specs/changes/040-realtime-market/` |
| 041-web-dev-loop             | 前端开发回路(`--web-assets-dir` 热读资产)          | ✅ 已实施 (2026-10-07)                                                                      | 开发效率收口(依据 `tmp/webui_analysis.md` P1-4 与 §六「第三刀」): 前端 11 份资产原经 `include_str!` **编译期内嵌**, 改一行 JS 也要 `cargo build` 全量重编(1~3 分钟)。**本变更只做开发回路这一件事**(`strategies.js` 2626 行拆分 diff 大且前端无单测, 另起变更)。① **清单收敛**: 原 11 个独立 handler + 路由 + test-only `ALL_JS` 合并为模块级 `const ASSETS`(文件名 / Content-Type / 内嵌副本), **运行时路由表由它循环生成** —— "清单 / 路由 / 磁盘白名单"三处手抄点归零(历史事故: 033 加 `keys.js` 时三处清单**一处都没追加**, 存储红线 / 只读红线 / 首页对齐三项断言**集体漏扫且全绿**)。② **两种来源**: `AssetSource{Embedded, Disk}` —— 默认内嵌(**发布行为一字未改**, 内嵌路径仍零拷贝返回 `&'static str`), `ricow web --web-assets-dir <DIR>` 时改从磁盘热读**同一批文件名**, **改一行 JS 刷新即见**。③ **安全形状**(用结构而不是过滤): 磁盘寻址一律 `规范化目录.join(编译期常量名)`, **请求里的路径从不参与拼路径** → 无 `..` 穿越 / 无任意文件读取(**结构上不可能**); 目录在**启动时**校验(存在 / 是目录 / 含 `index.html`), 否则 exit 1 并给可操作文案(不在浏览器白屏); 磁盘模式**缺文件 → 500 并点名文件**、**不静默退回内嵌**(同 040 诚实性口径); `Cache-Control: no-store` **只加在磁盘来源**(内嵌不加 —— 防后人"顺手对齐"的有意不对称, 连此差异本身也写进了断言); 走**同一批路由与同一道 token 门**, 不新开端口 / 不新增放行口。④ 显式 flag **本身即人工确认**(不按 debug/release 门控, 同 `[ai].allow_custom_base_url` 口径), 代价是启动打印双语开发模式警告。测试 829 → **834 passed / 0 failed / 22 ignored**(+5 = 资产表自检 / 目录校验 / 默认内嵌 / 两条端到端), 门禁五件全绿(clippy 本轮**连 Windows 增量锁噪音都是 0 条**); 真机取证 = 开发回路探针 **19 PASS / 0 FAIL**(改一行 JS 后**同一个进程立刻吐新内容**且内容确实变了、磁盘来源带 `no-store`、缺 `runs.js` → 500 点名、`/nope.js` 与 `/../Cargo.toml` → 404 且不泄漏仓库文件、首页 token 照旧替换、无 token 仍 401 空体) + 官方冒烟 `e2e_web.py` **两种形态各 144 PASS / 0 FAIL**。见 `specs/changes/041-web-dev-loop/` |
| 042-strategy-js-split       | 策略视图拆分(`strategies.js` → 外壳 + 三份子文件)   | ✅ 已实施 (2026-10-07)                                                                      | 前端最大单文件(2626 行)拆分, **纯重构零行为变化**。拆为 `strategies.js`(**外壳**: `R.sg` 共享命名空间 / 工具 / 常量 / **状态袋 `S.st`**(13 槽) / 页面装配) + `strategy_form.js`(新建·复制·参数表单) + `strategy_editor.js`(详情编辑器: Lua 高亮 / 撤销 / 保存 / 清单编辑 / AI 改写入口) + `strategy_backtest.js`(指标卡 / 权益曲线 / 平仓明细 / 策略日志 / 寻优 / 历史)。两条硬约定: ① 跨文件引用一律**延迟别名**(`const f = (...a) => S.f(...a)`, **禁值别名**) —— 外壳与子文件依赖**双向**, 值别名会把对方尚未导出时的 `undefined` 固化; ② **加载顺序即契约** —— `assets.rs::ASSETS` 与 `index.html` 两处下标顺序由单测 `test_strategy_shell_loads_before_its_parts` 与「首页对齐」测试双向盯住。仅 `R.sg` 一个共享命名空间, 13 个可变状态全进 `S.st`, 拆分后无跨文件自由变量。**仍无构建链**(拆的是源码文件, 无 `import`/`export`, 全部 `include_str!`)。**踩到的坑**: `strategy_editor.js` 漏了 `S.onUndo = onUndo` 导出(外壳 `mount()` 直接挂到 `#sg-undo`)→ 打开策略页整片 `ReferenceError`, 而 `node --check` / grep 扫描 / Rust 单测 / Python 冒烟**四道静态门全绿**(都进不了浏览器运行期); 固化为**跨文件符号静态审计**(`tmp/audit_042.py`)+ **真机走查列为常规环节**。E2E 断言改为按四份文件**并集**看能力(否则纯文件搬家造一片假红)。测试 834 → **835 passed / 0 failed / 22 ignored**(+1), 冒烟 **159 PASS / 0 FAIL**; 见 `specs/changes/042-strategy-js-split/` |
| 043-web-token-sse-fix       | token 303 换发后 SSE/WS 静默 401 修复              | ✅ 已实施 (2026-10-07)                                                                      | **起因**: 044 真机走查在动代码前抓到的既有回归。缺陷: 中间件取 token 的优先级是 `Bearer → 查询串 → cookie` 且**查询串优先、不判空**, 而顶部导航(`Sec-Fetch-Mode: navigate`)会 **303** 到去掉 token 的干净地址并种 `ricow_token` cookie —— 二次打开时地址栏带的是**空** `?token=`, 空串**盖掉**刚种的 cookie → **`EventSource`/WebSocket 全部 401, 而页面本身仍 200**(对话不流式 / 市场不实时 / 日志不刷新, 界面毫无异样 = 静默失效)。**为何全绿**: Python 冒烟每次请求都显式带 token, 永远走不到「空 query + cookie」这条组合 —— **这条路径只有真浏览器走得进去**。修复取**最小面**: 服务端一行未动, 改在**客户端记得住 token** —— `<head>` 加 `<meta name="ricow-token" content="__RICOW_TOKEN__">`(复用既有占位符替换), `common.js` 的 `R.TOKEN` 改为**查询串 → meta 载体**两级取值。刻意**不改**中间件语义(「新链接须能盖掉旧 cookie」是必要的, 否则进程重启换 token 后旧链接永不生效)。`e2e_web.py` 新增 **B2 段 8 条**(303 / Location 去掉 token / 种 HttpOnly cookie / **非导航不得重定向的反向对照** / 带 cookie 取首页 / meta 载体带 token); 真机以 CDP 取 `performance.getEntriesByType('resource')` 的 `responseStatus` 取证(修复前 401 / 修复后 200, 而带真实 token 的动态流为 **0** = 连接被挂着, 正是"看着像产品不响应"的成因)。`SKILL.md` 新增 §1.3 记录该坑与复跑方式。测试 835 → **836 passed / 0 failed / 22 ignored**(+1), 冒烟 159 → **167 PASS / 0 FAIL**; 见 `specs/changes/043-web-token-sse-fix/` |
| 044-webui-ux-polish         | AI 对话 Markdown 渲染 + 日志搜索/过滤/跟随开关      | ✅ 已实施 (2026-10-07)                                                                      | 补齐 `tmp/webui_analysis.md` §四 P1 第 6、7 项。① **AI 对话 Markdown 渲染**: 新增 `web/assets/markdown.js`(受限子集: 围栏代码块 / 行内代码 / 加粗 / 斜体 / 删除线 / 链接 / 标题 / 列表 / 引用 / 水平线 / 表格), **只产 DOM** 返回 `DocumentFragment`, **不碰 `innerHTML`** —— AI 输出是**不可信内容**, DOM 路线既不用动安全扫描白名单, 也没有"转义漏一处即 XSS"的窗口; 链接只放行 `http:`/`https:`(其余降级纯文本), 外链带 `noopener noreferrer`。**只作用于助手气泡**: `renderRich(el, md)` 先按既有规则切出**报告段**(仍走卡片), 剩下的散文段才按 `md` 选渲染方式 —— 宿主 `line` 帧是**对齐纯文本**, 套 markdown 会打散对齐且与报告结构化(FR-025)抢同一段文本; 术语浮层跳过 `pre`/`code`(代码里的"手续费"不该变成可点术语)。② **日志面板三件套**: 关键字搜索(不分大小写, 改词即时生效) / 级别过滤(**诚实口径**: 日志原样存储不解析, 级别只能是行内关键字启发式, UI 文案自陈「按关键字粗略匹配」, 宿主说明行**始终显示**——它解释"日志为什么断了一截") / **跟随↔暂停**(`follow` 意图 + `atBottom()` 客观位置两个量, 用户上滚自动暂停且只计待读、点按钮回底 —— 原先无条件 `scrollTop = scrollHeight` 会**抢用户的滚动**); 计数诚实:「显示 M / 共 N 行」+ 注明本地最多 500 行。只读红线**未松**: 面板按钮换成**显式允许清单**(`log-follow` 等视图控件)+ 仍禁内联事件、仍禁交易所写动作 —— 加交易按钮照样红, **例外显式化而非意图弱化**。**无新依赖 / 无构建链**(手写渲染器, 不引 marked.js)。测试 836 → **838 passed / 0 failed / 22 ignored**(+2), 门禁五件全绿; 真机取证 = CDP 走查**问题 0 条**(15 条 markdown 断言含**三条注入负例全部未执行**、`<img>` 以字面串出现、宿主输出不套 markdown; 日志搜索命中 2 / `error` 1 / `warn` 1、暂停时 `scrollTop` 保持 0 且显「已暂停 · 1 条新」、点跟随回底)+ 官方冒烟 **两种形态各 181 PASS / 0 FAIL**。**收敛期教训**: 断言补在 skill 目录的脚本副本里而实际执行的是仓库 `tmp/` 那份 → 两次 167 PASS **全是假绿**(044 的 14 条一条没跑); 修法是每次跑前 `cp <skill>/e2e_web.py tmp/` 并复核日志里**确实出现**新增断言字样。见 `specs/changes/044-webui-ux-polish/` |

> **023-ai-chat-ux(2026-09-18, 实施中)**: 对话体验重构 —— 中英双语(`[ui].lang` + `/lang`, AI 回复语言跟随)、每轮分隔线与轮次号 + `/history`、去术语化(面向用户不出现 `ricow xxx` 命令)、宿主持有菜单的选项式交互(菜单 A 5 项 / B 4 项)、**写操作全确认**(7 → 13 类, 唯一入口 `request_write_confirmation`)。确认词**分渠道**: 对话 = 当前语言**口语词**(`确认`/`确定`/`同意` · `confirm`/`confirmed`), 终端 = **逐字长短语**(`approve.rs` / `ctrl.rs` 一行不改); 见 `specs/changes/023-ai-chat-ux/`。

> **025-web-ui(2026-09-19, 已实施)**: 增加 Web UI 模式 —— `ricow web` 在 `127.0.0.1` 上起内置网页(axum + SSE, 一次性 token 鉴权, 无 token / 错 token 一律 `401` 且不回会话内容), 浏览器里完成全部对话与操作(**与 CLI 同一会话引擎**: 会话缝 `ai/session.rs` 的第二个 sink `web/sink.rs`, 不复制任何 LLM 路径)。左侧会话历史持久化在 `ricow.db`(新建/切换/删除, 重开恢复最近 20 轮上下文)、右侧对话输出区 + 输入区; 关键字/警告/出错按宿主标注的 `Severity` 着色; 量化术语点击弹解释; 界面中英双语并与 CLI 共用 `[ui].lang`; 明文密钥走独立通道, 不落库、不进对话流、不进浏览器存储。**既有 CLI 功能零改动**(原三个启动脚本逐字节不动, Web 启动脚本为新增文件); 见 `specs/changes/025-web-ui/`。

> **026-trade-visibility(2026-09-19, 已实施)**: 让交易与日志**可见**(起因: 用户实测"跑测试网看不到交易信息与日志")。① **落库**: 引擎侧新增 `orders` / `positions` 两表(`CREATE TABLE IF NOT EXISTS` 幂等追加, 既有 8 张表**零改动**)并在下单提交 / 订单状态变化 / 成交回报 / 持仓刷新时写入; `pnl_snapshots` 由死表改为**每笔成交后写一条**且永久保留。② **Web 面板**: 新增只读交易面板(持仓 / 挂单 / 最近成交)+ 日志面板(策略切换 + 尾读 SSE 实时追加), 共 4 条交易端点(`/api/trades/{fills,orders,positions,pnl}`)与 3 条日志端点(`/api/logs`、`/tail`、`/stream`), 全部挂在**同一道 token 中间件之内**, 页面**无任何直连交易所的写按钮**(撤单 / 平仓 / 停机仍走对话确认)。③ **口径**: 数据源**唯一来源 = 本地库**(不直连交易所, 停机后仍能看最后状态并标注"截至 <时间>"); 每条回复带 `source` 三态(`ok` / `daemon_down` / `unreadable`), **不把"连不上 daemon"说成"没有交易"**; 成交的 `mode` 由 `exchange_order_id` ⟕ `orders` 带出, 关联不到即如实显示"未知"不猜。④ **修两处假阴性**: AI `instance_status` 把"连不上 daemon"说成"策略已经停着/没有可停止的对象"; demo 测试网停机误印「实盘停机」文案(现按**真实运行模式**措辞)。⑤ **AI 回流**: 新增三个只读工具(`positions` / `open_orders` / `pnl`), 写操作执行结果注入对话 history。测试 472 → **504 passed / 0 failed / 21 ignored**, 三门禁全绿; 端到端以**真实 demo 测试网**验证(禁 mock); 见 `specs/changes/026-trade-visibility/`。

> **032-web-ui-console(2026-09-29, 已实施)**: Web UI **工作台化** —— 由单一对话视图演进为五视图工作台(左侧导航 + hash 路由, 无构建 vanilla JS): ① **设置页**(币安 API/demo/AI 密钥页内配置, 统一 `GET/POST /api/config/keys`, 密钥静默写 `ricow.toml` 只回显尾 4 位; 市场视野开关同端点); ② **市场**(现货/合约列表默认 bStock 可切全部 + 详情 + lightweight-charts K 线, UMD 内嵌编译进二进制); ③ **策略**(复制内置/新建/编辑落盘 — 用户亲手保存保留编译门禁、免沙箱回测+preview+approve 链; AI 修改产物须用户显式保存; 回测 202+job 轮询); ④ **运行**(Dry Run/demo/live 启停 + 状态 + 行内 SSE 日志; **live 全部动作不降级**: 逐字输 `确认实盘 <策略id>` + 首次 live 风险披露逐字 `确认风险` 落盘 `risk_ack.json`, 两道缺一不可); ⑤ **对话**(零回归, 与 CLI 同一会话引擎)。**写操作确认渠道由 2 个增至 3 个**(宪法 1.2.0: 新增 Web 渠道 — 页面显式交互承载普通写操作, live 不降级), 025 spec 已加演进注记。测试 538 → **589 passed / 0 failed / 22 ignored**, 三门禁全绿; demo 真实下单 + live 拒绝路径 testnet 实证; 见 `specs/changes/032-web-ui-console/`。

> **033-key-manager(2026-10-02, 已实施)**: **密钥管理页** —— 密钥从"唯一一份生效值"升级为**带别名的密钥环(vault)**。
>   
> 左侧导航新增第 6 个一级视图「密钥」(`#keys`, 位于「运行」与「设置」之间): **AI 通道** 与 **币安凭据** 各一组,
>   
> 组内**左列条目列表(按别名) + 右列详情表单**; 每条可保存 / **选用**(显式动作, 写入 `[ai]` 或
>   
> `[exchange].binance_*` / `demo_*`) / **删除**(删除"使用中"条目时同步清空生效字段, 防"已删除却仍生效"的幽灵凭据)。
>   
> 密钥仍只存**唯一配置文件** `ricow.toml`(新增 `[[ai_key]]` / `[[exchange_key]]` 两个数组表, 块级行式编辑保留注释 +
>   
> 原子写 + 0600 / icacls), 读接口只回 `***` + 末 4 位(032 FR-007 契约不变), 密钥框**留空 = 不修改**语义不变;
>   
> 「设置」页不再重复承载密钥配置, 只留市场视野 + 指向密钥页的入口(避免两处配置漂移)。
>   
> 新增 8 条 `/api/keys*` 端点(全部在 token 中间件之内), legacy `/api/config/keys` 保持可用。
>   
> **宪法未改**(未新增确认渠道: 密钥写入属 032 已确立的 Web 渠道普通写操作, live 逐字短语门禁一字未动)。
>   
> 测试: ricow bin 279 → **300 passed / 0 failed / 1 ignored**(+21), 工作区 608 passed / 22 ignored、
>   
> 三门禁(fmt / clippy -D warnings / test)在本机除 `ai_live_smoke` 2 例环境性失败外全绿;
>   
> 端到端以真实 HTTP + 临时数据目录 30 组对照取证(含全部拒绝路径)。见 `specs/changes/033-key-manager/`。

> **034-ui-themes-fold(2026-10-03, 已实施)**: **UI 主题 + 现代字体 + 历史消息折叠**。
>   
> ① **主题三选一**: 顶栏新增主题下拉(语言钮旁), `dark`(默认)/`light`(白色)/`red`(红色),
>   
> 值存 `ricow.toml` 的 `[ui].theme`(与 `[ui].lang` 同路径: 白名单 + 行式编辑保留注释, 非法值硬失败),
>   
> 新端点 `GET/POST /api/theme`; CSS 全面变量化(20 处硬编码颜色收敛为 `--raised` 等变量),
>   
> 三套主题各是一份变量表, K 线图配色随 `ricow:theme` 事件用缓存数据就地重绘(不重新拉数据)。
>   
> ② **现代字体**: 正文 15px/1.65 → 14px/1.6, 字体栈前置 Inter / Segoe UI Variable,
>   
> 中文回退苹方 / 微软雅黑 UI, 等宽栈前置 Cascadia Code / JetBrains Mono, 开抗锯齿。
>   
> ③ **历史消息两行折叠**: 对话流里除最新一块外, 更早的 `.msg`/`.line` 默认收成两行
>   
> (`-webkit-line-clamp`, 不受气泡内边距干扰), 淡出遮罩 + "··· 点击展开"提示(随语言);
>   
> 点击展开 / 再点收起; 宿主菜单与流式中的气泡不折, 手动展开过的块不被新消息重新收起,
>   
> 点术语先展开所在块, 选文字(复制)不触发手势。测试 608 → **611 passed / 22 ignored**(+3),
>   
> 三门禁全绿(除 2 例环境性失败); 三主题与折叠以真实浏览器截图取证。
>   
> 见 `specs/changes/034-ui-themes-fold/`。

> **035-engineering-hardening(2026-10-03, 已实施)**: **工程底座加固** —— 对标 Vibe-Trading 的工程做法, 只补底座、不扩产品面
>   
> (依据 `tmp/analysis/ricow-optimization-vs-vibe-trading-2026-10-03.md` 第二/三节)。
>   
> ① **2.1 币安 REST 统一重试/退避/限流**: 新建 `ricow_binance/src/retry.rs`, 现货/合约/fapi 三处客户端共用;
>   
> 重试按**请求语义**分档 —— 读操作可重试传输错误 + 429/418/408/5xx, **写操作仅连接层失败可重试**(安全红线:
>   
> 超时/5xx 可能"其实已成交", 重放 = 重复下单); 429/418 尊重 `Retry-After`(≤30s); 指数退避 + ±20% 抖动;
>   
> `CoreError::RateLimit` 由**死变体**接上。
>   
> ② **2.2 回测优先读本地 K 线缓存**: `klines` 主键加 `market` 维度(现货/合约同 pair 不互相污染),
>   
> 取数改"先读本地 → 未命中直连交易所 → 回填", **仅当窗口已全部收盘**才允许命中(防实时尾 bar 未收盘被固化),
>   
> 生效数据源写进 run card。
>   
> ③ **2.3 回测 run card**: 落 `run/backtest/<ts>-<策略>-<pair>.json` —— 策略源码 sha256 + 参数快照 +
>   
> 数据窗口(pair/interval/market/position_mode/根数/首末 open_time/数据源)+ 全部报告指标, 服务 P4 dogfood 证据留档与"逐位一致"机器化比对; 落盘失败只 warn。
>   
> ④ **2.4 数据库编号迁移**: 引入 `PRAGMA user_version` 驱动迁移, `migrate()` 拆"幂等基线建表 + 版本迁移",
>   
> **版本已最新时零写事务**; v0→v1 重建 `klines` 加 `market`(旧数据标 `spot`, 单事务幂等);
>   
> 并**根治**本机高频 `SQLITE_BUSY` 抖动(`Database::open` 加锁错误退避重试 + async sleep)。
>   
> ⑤ **2.5 CI 门禁补齐**: 新增 `deny.toml` + `cargo-deny` job(许可证/漏洞/重复版本/来源)、
>   
> `scripts/ci_grep_gates.sh` + `gates` job(四条安全红线: AI 工具层零落盘 / 明文密钥不进日志 / 无调试残留 /
>   
> `execute_strategy` 调用点白名单; 按 `#[cfg(test)]` 配平跳过测试代码)、`test` 矩阵加 **macOS**、
>   
> `dist-workspace.toml` 加 `pr-run-mode = "skip"` 并去掉 `release.yml` 的 PR 触发; 顺带修真实漏洞 **RUSTSEC-2026-0285**(rustls → 0.23.45);
>   
> 内部依赖收敛到 `[workspace.dependencies]` 以过 `wildcards = "deny"`。
>   
> ⑥ **3.1+3.2**: 锁毒化容忍(`lock_or_recover`)+ 新增 `CoreError::Db` 变体与 `SqlxResultExt::core()` 边界归一(约 16 处不再手工猜错误种类)。
>   
> ⑦ **3.3 结构化运行事件流**: 新增 `ricow_strategy/src/events.rs`, 实例运行写 `run/<name>/events.jsonl`
>   
> (一行一事件、写完即 flush、坏行跳过、**永不返回错误**不反压交易); 埋点启动/停止/下单/拒单/撤单。
>   
> ⑧ **3.4 AI 健壮性**: LLM 调用 3 次退避重试(**仅瞬时错误**, 状态码按数字边界匹配; 流式"已吐字即不重试")
>
> - 对话历史字符预算(24k)超限按**整轮**折叠 + 确定性摘记(刻意不调 LLM 做摘要)。
>     
>   ⑨ **3.5 配置 schema 版本**: `ricow.toml` 加顶层 `schema_version`(缺省=当前版本, 老文件**内存迁移不落盘**,
>     
>   未来版本拒绝并给解法), 默认值收敛到 `pub mod defaults` 单一来源。
>     
>   测试 611 → **642 passed / 22 ignored**(+31), 三门禁(fmt / clippy -D warnings / deny)全绿;
>     
>   见 `specs/changes/035-engineering-hardening/`。
>     
>   **未做(显式记录)**: walk-forward / 滑点敏感性(分析文档第四节, 属**决策项**, 用户未回应);
>     
>   Web 前端工程债(第五节, 低优先); `cargo-audit` 独立 job / 覆盖率 / `cargo-udeps` / bench(Vibe-Trading 有, ricow 当前不需要)。

> **036-backtest-sensitivity-web-hardening(2026-10-03, 已实施)**: 用户拍板补上 035 显式记录的两项。
>   
> ① **回测滑点/费用敏感性(分析文档第四节)**: `--sensitivity` / `--sensitivity-fee` 沿单一成本轴跑多档
>   
> 完整回测(默认阶梯 0,5,10 / 5,10,20, 可显式给档, ≥2 档硬校验), 定宽表 + 每轴"净盈亏符号在哪一档翻转"
>   
> 结论段; 抽 `BacktestOutcome` 结构化内核让报告/run card/扫描三共用, **不动撮合引擎**(YAGNI);
>   
> 扫描不落 run card。② **Web 工程债(第五节)**: 日志 SSE 断线续传(事件 id = 文件 offset,
>   
> `Last-Event-ID` 头优先 + `last_event_id` 查询参数兜底, 不重发已给过的行); 前端统一 SSE 打开器
>   
> `R.sse`(指数退避 1s→8s + ±20% 抖动, 连续 5 次放弃; `new EventSource` 收敛到一处);
>   
> 写方法 Origin/Referer 来源门(回环白名单, 非回环 403 空体, 与 token 同一道中间件);
>   
> **mod.rs 2178 → ~700 行**按端点族拆 7 个模块 + 共享测试脚手架 `test_support.rs`,
>   
> `strategy_io`/`runs` 目录化下沉测试体 —— `web::` 全组 102 用例未改断言全绿即零行为变化取证。
>   
> 测试 642 → **650 passed / 22 ignored**(+8); 见 `specs/changes/036-backtest-sensitivity-web-hardening/`。

> **037-live-safety-hardening(2026-10-06, 已实施)**: **实盘资金安全加固** —— 依据
>   
> [`specs/research/framework-vs-commercial-2026-10.md`](research/framework-vs-commercial-2026-10.md) 的差距分级, 只做 🔴 P0 三项(架构范式差距, 不扩功能面)。
>   
> ① **P0-A 启动接管遗留挂单**(`live::split_owned` / `must_refuse_start` + `command.rs` 接线): 修"进程崩溃/强杀/断电后重启,
>   
> 交易所仍挂着旧单而策略 `on_init` 再下一张同样的单 → 敞口翻倍"。启动按归属前缀切分 → 本实例归属逐个撤销(逐笔事件
>   
> `orphan_canceled` + `outcome.orphan`)→ 复查 → **live 仍有残留或枚举失败则拒绝启动**(错误带残留单号 + 手工撤单指引),
>   
> demo 只警告；非归属单只上报不撤。**刻意不加 `orphan_policy` 配置面**(固定工程不变量, 与 `order_guard` 同口径)。
>   
> ② **P0-B 引擎级最小订单登记**(`ricow_engine/src/oms.rs`): 修"`orders` 表只写不读、下单传输失败不落任何记录,
>   
> 事后无法回答这一单到底提交成功没有"。会话内生命周期 + **传输失败 = `Unknown`**(单独入账, 收尾醒目提示用户手工核对交易所,
>   
> 不谎报结论)+ **非终态重复单号检测**(策略 bug 信号)+ 账目计入 `RunOutcome`; 纯内存、无 IO、失败不阻断交易。
>   
> ③ **P0-C 组合敞口只读视图**(`ricow_engine/src/exposure.rs` + `ricow exposure`): 修"daemon 跑 N 个策略却看不到合起来的敞口"。
>   
> 逐 `(标的 × 模式)` 汇总净头寸 / 多空合计 / 总名义(按开仓均价**估算**, 算不出就标 `*` 并注明被低估)/ 未终结挂单数 / 活跃策略数,
>   
> `--detail` 下钻策略明细; **只展示不拦截**(守宪法"平台不做投资判断")、**不直连交易所**、**绝不跨模式相加**(模拟持仓混进实盘敞口比不给数字更危险)、
>   
> **不跨交易对汇总**(报价资产未必一致)。
>   
> **验证**: 纯逻辑单测 25 例(引擎 13 + CLI 12, 含跨策略对冲 / 模式隔离 / 名义低估标记 / 输出顺序稳定 / 显示宽度对齐)
>
> - 端到端以**临时数据目录 + 手工灌库**模拟验证(含 live 拒绝路径、空库、只剩已平仓行、`--pair`/`--mode` 过滤与拼错报错);
>     
>   **不引入 Exchange 替身**(宪法原则三"交易流程禁 mock Exchange 替身"不可协商, 本变更也不新增下单语义)。
>     
>   测试 736 → **761 passed / 0 failed / 22 ignored**(+25); 门禁五件(fmt / clippy -D warnings / cargo deny / 安全红线 / architecture_guard)全绿。
>     
>   **不做(显式记录)**: **P0-D 单笔名义上限 / 异常速率熔断** —— 需**可配置的风险参数面**, 与宪法 D15/D17(删除 `RiskEngine` 四条静态限额、
>     
>   只保留固定 `order_guard`)**直接冲突**, 属宪法级决策, 待用户显式拍板后再立项(见 spec §三/§六)。
>     
>   见 `specs/changes/037-live-safety-hardening/`。

> **038-p1-robustness-hardening(2026-10-06, 已实施)**: **P1 工程健壮性加固** —— 同一份调研报告
>   
> ([`specs/research/framework-vs-commercial-2026-10.md`](research/framework-vs-commercial-2026-10.md))的 P1 六项**全做**,
>   
> 主线是"**把不可见的假设与失败摆到台面上**", 六项互不耦合且**默认行为零变化**。
>   
> ① **P1-A 回测限价成交去乐观**: 旧撮合是"bar 内**触及**限价即按限价 100% 全成", 忽略排队位置 —— 对网格类策略**系统性高估**成交率与收益。
>   
> 新增 `limit_fill_penetration_bps`(CLI `--limit-fill-penetration-bps` / 策略 TOML `[backtest]` 段, **默认 `0` = 与加它之前一字不差**);
>   
> 给正值后买单价压到 `limit × (1−bps)`、卖单抬到 `limit × (1+bps)`, **成交价仍按原始限价记账**(穿透只用于判定, 不冒充额外滑点)。
>   
> 报告"成交笔数"之后单列 `限价单成交: N 笔 (占全部成交 X%; 穿透 N bps — <乐观提示 / 已要求穿透>)`, 并进 Web 术语表(双向核对测试覆盖)。
>   
> ② **P1-D 数据缺口检测**(`ricow_strategy/src/gaps.rs` 纯函数): 取数后按周期步长扫 `open_time` 连续性, 有缺口即**硬报错**
>   
> (列前 5 处 + 如实说明"另有 N 处未列出"), 不再拿稀疏 K 线静默回测出一份看着正常的报告。容忍毫秒抖动; 文档写明"输入须升序"的前提。
>   
> ③ **P1-F 下单延迟度量**(`ricow_engine/src/latency.rs`): 实盘主循环与停机兜底平仓两处下单点计时, 收尾打印 p50/p95/p99/max
>   
> (nearest-rank: `ceil(n × p).max(1)`, **纯函数不碰 `Instant`** —— CI 红线 5)。**只进 `RunOutcome`, 不动 `RunEvent` schema**;
>   
> `report_line` 明写"不含行情推送与策略计算"(标签比指标本身更重要)。
>   
> ④ **P1-B Prometheus 只读端点**: `GET /metrics` 挂在**既有 web 服务**上(**不新开端口**)、与全部端点同一道 token 门(无 token `401` **空体**);
>   
> 手写暴露格式**不引 `prometheus` crate**; 指标 `ricow_daemon_up` / `ricow_instance{name,mode}` / `ricow_open_orders{strategy}` /
>   
> `ricow_position_size{strategy,pair,mode}` / `ricow_net_pnl{strategy}` / `ricow_fills_total` / `ricow_backtest_jobs{state}`。
>   
> 任何一路读失败**降级为空而非 500**;**不暴露**对账修正次数(未持久化, 暴露即假数字)、单号、密钥、策略源码。
>   
> ⑤ **P1-C 崩溃自动重启**: `ricow.toml` 新增 `[supervisor]` 段(`restart_policy` 默认 `"none"` = 零行为变更 / `max_retries` 3 / `backoff_secs` 5 线性退避);
>   
> 非法值 / 类型错 / 读失败一律**硬拒并降级为不重启**(安全方向: 少做一次动作)。判定是纯函数 `restart_decision`; 跑满 60s 视为健康、计数清零;
>   
> **主动停机结构性不会被顶回来**(`stop_all` 先整体 drain `children`); 重启走既有 `Server::start` ——
>   
> 于是**不绕过** 037 定的启动挂单接管与实盘三判据。该键**不进 Web 可写面**(与 `ai.allow_custom_base_url` 同理: 改文件即人工确认)。
>   
> ⑥ **P1-E 实盘/回测对齐工具**(新增只读子命令 `ricow align <策略名> [--card] [--mode] [--limit]`): 把同一对照窗口内的回测指标(取该策略最新 run card)
>   
> 与实盘成交放一张表, 让偏差自己暴露。窗口由 run card 还原(未知周期**硬报错不猜**)。**三条诚实性硬约束**:
>   
> 口径差异显式印出 / 窗口内实盘 0 笔时**直说"无从对比"**&#x800C;不是给一张看起来完整的空表 / 期货下净现金流**不叫盈亏**而叫"现金净流入"。
>   
> **收敛期自查修掉的真 bug**: `check_keys` 的类型表漏了 `[supervisor]` 的两个整数键 —— 用户照模板写 `max_retries = 3` 会被回一句"必须是字符串"。
>   
> **验证**: 纯逻辑单测 45 例(CLI 25 + 策略层 13 + 引擎 7; 其中 4 例为承接 231 归因复核时补的 `supervisor::procs` 停机链路跨平台覆盖) + 端到端以**临时数据目录 + 手工灌库 + 真二进制**模拟
>   
> (`ricow align` 三档 mode 与四条异常路径、`GET /metrics` 200/401 空体与指标格式、daemon 实读 `[supervisor]` 三态: on-failure 生效 / 非法值降级 / 不阻断启动);
>   
> 测试 761 → **806 passed / 0 failed / 22 ignored**(+45), 门禁五件全绿。见 `specs/changes/038-p1-robustness-hardening/`。

> SDD 产物规范: 计划与任务分解应存于 `specs/changes/<feature>/{spec,plan,tasks}.md`。
>   
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
     - 短线横截面族落在反转区且样本仅 63/150 个交易日无法证明长期有效。
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

1. **产品重构 (2026-09-12 用户指令)**
   - 已完成: **009 移除 Hyperliquid**(未落地却写进文档, 已全量移除 crate + 文档承诺, 测试 204)。
   - 已收敛: **008 跨平台进程模型**(2026-09-12) —— 常驻 daemon(pm2 模型) + 本机 TCP(token) + stdin 管道优雅停机(EOF 自愈) + 实例台账 + list/status/info/fills/logs;
       
     实测: 全链路真实行情冒烟(37 笔成交落库)、kill -9 daemon 后子进程 4s 自愈退出、testnet 真实调用 3 例跑绿(现货/合约 openOrders 闭环 + 拒单如实上报)、测试 204→216/0/9;
       
     收敛遗留(不阻塞归档): 引擎撤单兜底 / `info` 持仓快照随实盘运行器落地; Windows 编译级验证当前不可复现已在 spec §四 如实降级。
       
     档案见 `specs/changes/008-platform-process-model/`(spec/plan/tasks, 含 Phase 7 收敛)。
       
     关键实测依据(detached 存活 / tokio spawn 陷阱 / 管道 EOF 自愈 / SQLite 并发写): `specs/research/process-model-probe-2026-09.md`。
   - 已澄清(2026-09-12): 用户判定「横截面选股策略长期期望值为负。不做了。产品中增加的相关功能保留，以后可能会用。」
       
     → ①计划中的 `bs_rs_rotation` **未实施即终止**(已建 spec 留档, 见"已终止的探索" 3);
       
     ②`ricow scan` 横截面选币与 bStock/美股数据层(含组合回测路径、信号轨机制、`build_interval_ticks`、`ctx:now()`)**保留不删**。
2. ~~**回测引擎遗留项**~~: L1/L2 已随 013 关闭, **L4 已随 015 关闭**(零动作单不再计费/记笔数) —— 见 `specs/backtest.md` §十。
3. ~~**CLI 残留清理**~~: ✅ 已随 008 一并处理(`backtest --market` 帮助文本、`--dump-dir` 删除、`scan` 帮助文本) —— 见 `specs/architecture.md` §十一。
4. **backlog 变更**: ~~003-notifications~~(2026-09-13 实施) → ~~002-ai-quant-researcher~~(2026-09-13 实施); backlog 已清空; **019-ai-assistant 已定稿(2026-09-14)进入实施**, P4 dogfood 仍待执行。
5. **P4 dogfood**: 上述前置已完成, 以真实小资金跑通"回测 → Dry Run → 实盘"闭环 —— **执行清单见 [`specs/research/p4-dogfood-runbook-2026-09.md`](research/p4-dogfood-runbook-2026-09.md)**(含一次性准备、四阶段命令与判据、立即停手条件、回滚、实测坑清单、结果留档表)。
6. ~~**全仓 rustfmt 对齐**~~ ✅ 已随 **022**(2026-09-15)完成: 工具链钉死 1.96.1 后一次性对齐 41 个文件(+612 −518), CI 已开 `cargo fmt --all -- --check` 硬门禁。原记录(2026-09-14):: 实测 `cargo fmt --all` 会改动 **34 个文件 / 约 +6069 −1139 行**, 且**非纯空白差异**(结构体字段换行、长表达式换行等), 即现有代码与当前 `rustfmt.toml`(`max_width = 100`, `use_small_heuristics = "Max"`)的输出不一致。**已全部回退, 未纳入 019**。开工前需先确认团队基准 rustfmt 版本/配置; 单独立项(否则 review 无法区分语义改动与格式噪声), 不与其他变更混做。
7. ~~**AI 对话流程仍需简化**(2026-09-17 用户反馈, 原话: 「AI对话流程还需要简化。目前有点过于复杂」)~~ → **已立项 023-ai-chat-ux(2026-09-18)**: 向导首问语言、对话内确认改为随语言的**口语词**、去术语化表述、菜单选项式交互, 三项"用户要记的东西"都被收掉。
   - 现象(当时自查): 用户要理解的东西偏多 —— 首次向导 4 步(供应商 / 模型 / 密钥 / 可选连通校验, 外加可跳过的 demo 凭据)、对话内七类**逐字**确认短语、以及"裸入口 / `ai` 子命令 / 一堆 CLI 子命令"多档入口并存。(023 收敛了前两项; 入口档次未动。)
   - **约束(仍然有效)**: 逐字短语在**终端**是**安全机制**(不是可删的复杂度) —— 023 的解法是**分渠道**: 对话用当前语言口语词, 终端保持逐字长短语一行不改, 不削弱"写实必须本人确认"。
8. **策略门禁覆盖面收口 + 运行态可见性**(2026-10-05 审计, 待用户定方案)
   - **已核实的结论**: "门禁只挂启动/回测层、已在跑的实例不受影响"是**正确**的边界 ——
       
     实例启动时 `load_strategy` 只调用一次、运行期不重读 `.lua`(`ricow_engine/src/command.rs` 的 640/1019/1529),
       
     故"运行中实例刚变可疑"在时序上不存在;
       
     唯一的"持续门禁"手段是热停正在跑的实盘实例, 与 FR-024 纪律相悖。详见
       
     [`specs/research/strategy-gate-coverage-audit-2026-10.md`](research/strategy-gate-coverage-audit-2026-10.md)。
   -
   - **缺口 1(可见性)**: `GET /api/runs` / `.../status` 不带 `declared` / `duplicate_of`, 用户看不到运行中实例用的
       
     是不是未声明副本 → "会自己停止再重启"的前提在 UI 上不成立。建议加只读标记 + 前端徽章(纯展示、不触 daemon)。
   - **缺口 2(覆盖面)**: `catalog::run_block` 目前只有 4 个调用点(CLI 回测 + Web 回测/寻优/启动),
       
     而启动路径的公共汇聚点 `ctrl::start_daemon`(`ctrl.rs:202`)自身**无闸**, 被 7 处调用
       
     (CLI start/restart + AI 五处 + Web start) → `commands/backtest.rs:919` 声称的"免得网页不让跑但命令行能跑"
       
     对**启动**这条线尚未成立(对话里让 AI "启动 X" 照样能起未声明副本)。建议收口到 `start_daemon` 一处盖全。
   - **收口的顺序陷阱**: `ctrl::restart` 是"先 stop 后 start", 若闸只加在 `start_daemon`, 会出现
       
     "实例被停掉、没起来"(还可能带未平仓敞口) → 收口则**必须**配 restart **先判后停**;
       
     不收口则维持现状(restart 不判)。两者绑定, 不允许只做一半。
   - **状态**: 截至 2026-10-05 **均未落码**, 待用户拍板是否实施。
9. **P0-D 操作级熔断 / 单笔名义上限**(2026-10-06, 037 显式不做, 待用户拍板)
   - **来源**: [`specs/research/framework-vs-commercial-2026-10.md`](research/framework-vs-commercial-2026-10.md) §四的 🔴 P0-D ——
       
     商用框架普遍有"单笔名义上限 + 异常速率熔断"作为**引擎级**的最后一层操作护栏; ricow 目前只有固定 100 单/秒的
       
     `order_guard`(防 bug 风暴被交易所封禁), **没有按金额的口径**。
   - **为什么没做**: 金额上限必须是**可配置的**才有意义(每个用户资金规模不同), 而"新增可配置的风险参数面"与宪法
       
     D15/D17(019 R5 删除 `RiskEngine` 四条静态限额、只保留固定 `order_guard`, 理由 = **平台不做投资判断**)直接冲突 ——
       
     属**宪法级决策**, 不能由实现者在变更里顺手绕开。
   - **可选方案(待选)**: ⓐ 维持现状(护栏只在策略侧, 用 `ctx:equity()` 自管); ⓑ 加**固定比例**的引擎级上限(如单笔名义 ≤ 权益×N,
       
     N 为固定常量不落配置 → 仍守"无配置面"); ⓒ 显式修订宪法、开一个受控的风险参数面。三者代价递增, 须用户拍板。
   - **状态**: 截至 2026-10-06 **未落码**, 待用户拍板。
   - **同报告的其他项已收尾**: 🔴 P0-A / P0-B / P0-C 随 **037** 落地; 🟡 **P1-A ~ P1-F 六项随 038(2026-10-06)全部落地** ——
       
     至此 `framework-vs-commercial-2026-10.md` 的差距清单里**只剩 P0-D 这一项未做**。

## 文档-实现缺口 (2026-09-11 审计)

本轮对照代码逐份复核 specs, 已就地修正的条目:

| 文档                  | 问题                                                                                              | 处置                                   |
| :------------------ | :---------------------------------------------------------------------------------------------- | :----------------------------------- |
| architecture.md     | 测试基线写 125; CLI 参数写 `--initial-cash`(实为 `--cash`); 直跑策略表漏 vwap; scan 缺 `--pool`; 未记录美股数据源/市场分类模块 | ✅ 已修正                                |
| backtest.md         | 验收测试数 173 过时; D2(MMR 取 exchangeInfo)与 D10 修正冲突未就地标注; L5 实际已修                                    | ✅ 已修正                                |
| lua-api.md          | §八"当前只支持 Binance 现货"过时(已支持合约 USDT-M); create_strategy 通道描述与实现不符(引擎能力在, CLI 入口未暴露)               | ✅ 已修正                                |
| product.md          | D3 "v1 = 6 策略"与现状不符(实为 1 策略样板 + 5 执行模式示例)                                                       | ✅ 已修正                                |
| changes/001-vwap    | 档案状态仍写"草稿", 实际已收敛; 4 份 checklists/requirements.md 的 `spec.md` 链接指向同级(断链)                        | ✅ 已修正                                |
| testnet.md          | 合约下单/账户端点写"计划中联调", 实际 2026-09-04 已完成                                                            | ✅ 已修正                                |
| changes/005,006,007 | 计划产物只落 `.hermes/plans/`, 档案缺 plan.md/tasks.md                                                   | ⏳ 规范澄清入 constitution(§文档体系); 历史档案不回填 |
| roadmap.md          | 进度停留在 2026-09-04, 未记录 001/005/006/007 与两次策略终止                                                   | ✅ 本次重写                               |

> 本轮同时按 §十一 记录了三处 CLI 残留(不改代码, 列入"下一步"第 3 项):
>   
> `backtest --market` 帮助文本含不可用的 `us`、`--dump-dir` 无消费方、`scan` 帮助文本仍写"P2 骨架"。
>   
> 真实命令冒烟已复核: `ricow --help` 命令集与 §五 一致; `ricow backtest --strategy shannon_rebalance --pair ETHUSDT --days 20` 真实数据跑通
>   
> (480 根 1h K 线、8 笔成交、报告字段与 backtest.md §六 一致), 同时发现文档示例 `--pair ETH` 无效(交易所 `Invalid symbol`)已修正。
