# 019 收敛核验 (converge) — 2026-09-14

> 依据: `spec.md`(FR/SC) + `plan.md`(D 决策) + `tasks.md`(任务与勾选状态) + `specs/constitution.md`。
> 方法: 逐条对照**当前代码与实测证据**; 不改 spec/plan; 新发现的缺口按 append-only 追加到 `tasks.md` 阶段十一。
> 本轮已提交: `843e612`(文档) / `1c8a539`(阶段一) / `52e7bd8`(阶段二) / `6b6ec5f`(T011) / `daa0515`(凭据收敛) /
> `16af7c6`(回退误格式化) / `ee2dc58`(单一配置文件) / `374ac5f`(提示词按需取文档) / `224ff24`(--demo) / `1fc3ef1`(rustls 修复 + README)。

## 一、结论

- **不满足"已收敛"**: P1 的生成-回测-迭代闭环(Phase 3)与写操作确认(Phase 4)、生态入口(Phase 7)、分发(Phase 8)尚未实现。
  这些**已由 T019–T036 / T045–T059 跟踪**, 本文件不重复追加(避免任务重复)。
- **新发现 5 项未跟踪缺口**, 已追加为 `T070–T074`(其中 2 项为 spec 与 tasks 表述冲突, 2 项为文档卫生/日志如实性, 1 项为凭据明文残留)。
- **本轮实测修复 1 项回归**(非新增需求): rustls crypto provider 未显式安装 → 用户数据流 WS 建连 panic(见 §四)。

## 二、FR 对照(FR-001–FR-060)

| FR | 状态 | 证据 / 说明 |
|---|---|---|
| FR-001 `locus ai` 三形态 | ✅ 已验证 | 真机: 单次 + `--plain` + 中文回答(DeepSeek) |
| FR-002 自配 LLM 通道 | ✅ 已验证 | 预设 7 个 + **任意自定义名**(`provider="myproxy"` 实测打到自建端点) |
| FR-003 无向导命令 / 按需模板 | ✅ 已验证 | `locus setup` 已删; 缺文件时生成模板并报路径 |
| FR-004 报错给解法 | ✅ 已验证 | 拼错 `provder` → 列出允许键; 缺 key → 点名 `[ai].api_key` 与文件路径 |
| FR-005 密钥在唯一配置文件 | ✅ 已验证 | `credentials.toml` 已删; `locus.toml` 0600, gitignore |
| FR-006 外发清单 | ⚠ 部分 | 启动横幅打印"只发送你的问题与工具返回(不含密钥)"; 未做"仅首次打印一次"的语义 |
| FR-007 三层权限 | ⚠ 部分 | 工具面只有 9 个只读工具; **L1 虚拟工具未注册**(已由 T031 跟踪) |
| FR-008 只读可直调 | ✅ | 9 工具白名单, 真机多次调用 |
| FR-009 虚拟可直调但不涉资金 | ❌ 未实现 | 无 `start_dry_run`/`preview` 工具(已由 T031 跟踪) |
| FR-010 写实不作为工具 | ✅ 结构性 | `READ_ONLY_TOOLS` fail-closed; 单测断言 11 个写/越权名不得放行 |
| FR-011 输出上限 | ✅ | `MAX_OUTPUT_CHARS=8_000` + `clamp_output` + fills/logs_tail 行数上限 |
| FR-012 敏感模式过滤 | ✅ | `logs_tail` 经 `redact` |
| FR-013 审批 fail-closed | ✅ | `is_allowed` + `ReadOnlyGuard`(AgentHook) |
| FR-014 编译门禁→沙箱回测 | ❌ 未实现 | 无 `preview_strategy` 工具(已由 T019 跟踪) |
| FR-015 失败零落盘 | ❌ 未实现 | 同上(T020) |
| FR-016 回测同口径 | ✅ 已验证 | 同一 `format_backtest_report`; AI 复述数字与 CLI **逐字一致** |
| FR-017 系统提示嵌入 API 规范 | ❌ **冲突** | 见 §三 F2: 现改为 `read_doc` 按需取(FR-060), 字面冲突需修订表述 |
| FR-018 多轮迭代对比 | ❌ 未实现 | T021 跟踪 |
| FR-019 部署走两步确认 | ❌ 未实现 | 既有 approve/deploy 在, 但 AI 侧未接入(T027) |
| FR-020 确认块 | ❌ 未实现 | T026 跟踪 |
| FR-021 确认失败零副作用 | ❌ 未实现 | T026 跟踪 |
| FR-022 实盘两次分离 | ❌ 未实现 | T029/T030 跟踪 |
| FR-023 三判据原样复用 | ✅(既有链路) | 018/002 门禁未改动; demo 不适用已在 FR-059 语境说明 |
| FR-024 Dry Run 告知计时 | ⚠ 部分 | 提示词有该条款, 真机回答也主动提到 `dry_run_started_at`; L1 工具未落地故未端到端验 |
| FR-025 状态问答 | ⚠ 部分 | `instance_status`/`fills`/`logs_tail` 可用; "距强平/累计资金费"组合问答未验证(T037) |
| FR-026 起停 Dry Run | ❌ 未实现 | T031 跟踪 |
| FR-027 平仓/开关/改参 | ❌ 未实现 | T032 跟踪 |
| FR-028 停改起 + 告知重启 | ❌ 未实现 | T032 跟踪 |
| FR-029 只解释不代劳 | ⚠ 部分 | 提示词含"写实操作只能本人执行"; 熔断/接近强平场景未构造(T039) |
| FR-030 无后台轮询守护 | ✅ | 无轮询实现 |
| FR-031 `locus agent-kit` | ✅ 已实现(2026-09-15) | T045; 真机安装到干净目录 + 拒绝覆盖 + 幂等(已是最新)三例实测 |
| FR-032 手册与系统提示同源 | ✅ 已实现 | 同一常量(`prompt::RULES` 逐字、`prompt::STRATEGY_API_DOC` 同一份); 单测断言逐字包含 |
| FR-033 手册覆盖易跑偏点 | ✅ 已实现 | T046; 8 类坑逐条在册; 速查由 clap 生成, 实测与 `locus --help` 命令集合一致 |
| FR-034 `locus mcp` | ⚪ 已取消(2026-09-15) | 不做 MCP —— 单机程序, 用户已有的 agent 直接调本机 CLI; 见 spec §七 R1 |
| FR-035 `--print-config` | ⚪ 已取消(同上) | 随 FR-034 取消 |
| FR-036 MCP 下写实由用户执行 | ⚪ 已取消(同上) | 该保证由内置 AI 侧结构性边界(FR-045)承担; 外部 agent 走 CLI, 门禁原样生效 |
| FR-037 三平台产物 | ❌ 未实现 | T053/T054 跟踪 |
| FR-038 一键安装 | ❌ 未实现 | T053 跟踪 |
| FR-039 Windows 双击入口 | ❌ 未实现 | T035/T055 跟踪(无 `packaging/`) |
| FR-040 README 三平台 + FAQ | ⚠ 部分 | 本轮已加"快速开始"(配置/四档运行/AI 边界/安全); 安装与 FAQ 未写(T056) |
| FR-041 拦截如实告知 | ❌ 未实现 | T058 跟踪 |
| FR-042 体积/编译代价实测 | ❌ 未实现 | T044 跟踪 |
| FR-043 策略名规范 | ❌ **未实现** | 全仓无名称字符/长度校验(T023); 风险真实: `align.rs:134 ownership_prefix` 对非 ASCII 替换为 `-` → 中文名塌缩 |
| FR-044 改脚本受控覆盖 | ❌ 未实现 | T028 跟踪 |
| FR-045 结构性边界 | ✅(内置 AI 侧) | 工具面无写实(白名单 L0∪L1 + `ToolGuard` fail-closed); MCP 已于 2026-09-15 取消, 无第二翼 |
| FR-046 预览清理 | ❌ **未实现** | `crates/locus_engine/src/confirm.rs` 有 create/approve/consume/reject/TTL, 但**无 DELETE/清理**; `previews` 只增不减(T040) |
| FR-047 中文可用 | ⚠ 部分 | 交互/单次中文实测正常; Windows 真机未验(T034) |
| FR-048 答复可核对(软要求) | ✅ 已验证 | 数字逐字一致 + 空结果如实("当前没有任何已部署策略"); 机械校验已撤回(见 §三 F1) |
| FR-049 两套凭据独立 | ✅ 已验证 | demo 只认 `[exchange].demo_*`; 缺 demo 凭据时明确报 demo 缺失, 不取主网 key |
| FR-050 不内置任何凭据 | ✅ | 模板空白; 全仓无内置 key |
| FR-051 统一凭据入口(Keyring) | ⚪ 已废弃 | 被 FR-059 取代(spec 已标注) |
| FR-052 三段式向导 | ⚪ 已删除 | spec 已标注废弃 |
| FR-059 单一配置文件 | ✅ 已验证 | 真机: 新目录只生成该文件; 未知键硬失败; 0600; gitignore |
| FR-060 文档按需取 | ✅ 已验证 | 简单提问输入 7808→1821 tokens; 写策略时先 `read_doc(lua-api/backtest/risk)` |

## 三、SC 对照(SC-001–SC-014)

| SC | 状态 | 证据 |
|---|---|---|
| SC-001 全绿无回归 | ✅ | `cargo test --workspace`: **334 passed / 0 failed / 12 ignored**; 构建 0 警告 |
| SC-002 纯逻辑单测 | ✅ | 预设解析/白名单 fail-closed/截断/配置严格解析/daemon 协议兼容 等单测 |
| SC-003 真实链路(P1 生成闭环) | ❌ | 待 T019–T021/T025 |
| SC-004 实盘链路(demo) | ⚠ 部分 | **demo 现货已端到端实测**(见 §四); 合约 demo 与"三判据拒绝零下单"未在 019 语境复验(T036) |
| SC-005 负例可复现 | ⚠ 部分 | ①语法错误零落盘 待 T020; ②同名部署被拒 待 T023; ③模型诱导直接部署 待 T033 |
| SC-006 隐私 | ✅ | 工具集无 shell/任意文件读; 外发清单在横幅; 密钥不外发(工具参数不含密钥) |
| SC-007 成本可观测 | ✅ | 每轮打印 token 用量; `max_turns` 上限; 工具输出有上限 |
| SC-008 中文可用 | ⚠ 部分 | 中文实测正常; Windows 真机待 T034 |
| SC-009 生态入口 | ❌ | 待 T045–T047 |
| SC-010 无 Rust 干净机器安装 | ❌ | 待 T053–T057 |
| SC-011 无自动更新组件 | ❌ | 待 T053(配置层面目前无 updater) |
| SC-012 文档同步 | ❌ **未做** | product/architecture/lua-api/roadmap 未按 019 更新(T041) |
| SC-013 策略名拒绝 | ❌ | 待 T023 |
| SC-014 `previews` 不无限增长 | ❌ | 待 T040 |

## 四、本轮实测修复(回归, 非新增需求)

- **现象**: 真实 demo 首跑 `panicked at rustls-0.23.43/src/crypto/mod.rs:249: Could not automatically determine the process-level CryptoProvider`
  → 用户数据流(WS-API)建 TLS 失败, demo/实盘**均无法启动**; REST 侧正常(账户快照能读到余额), 故此前未被发现。
- **根因**: 019 引入 `rig`/`reqwest 0.13` 后依赖图同时含 `aws-lc-rs`(tokio-rustls 默认)与 `ring`(reqwest `rustls-tls`)。
- **修复**: `crates/locus_cli/src/main.rs` 启动即 `rustls::crypto::ring::default_provider().install_default()`; `Cargo.toml` 增 `rustls{default-features=false, features=["ring"]}`(提交 `1fc3ef1`)。
- **复跑证据(demo 现货, 用户授权, 真实下单)**: WS 订阅成功 → 提交订单 1 / 成交 2 / 拒单 0 →
  `stop --close-all`: 撤单 0 / 残留挂单 0 / 残留持仓 0.00006920 ETH(既有 dust, 引擎如实提示人工核对) → 退出码 0。

## 五、发现清单(含未跟踪项, 已追加为任务)

> 处置进展(2026-09-14 同轮完成): **F1→T070 ✅ / F2→T071 ✅ / F3→T072 ✅ / F4→T073 ✅(真机验证 `mode=测试网模拟盘(demo)`) / F5→T074 待办**。


| ID | gap 类型 | 严重度 | 来源 | 证据 | 处置 |
|---|---|---|---|---|---|
| F1 | contradicts | HIGH | FR-048 vs `tasks.md` T038 | T038 仍要求"答复落地前做**机械校验**", 而 spec FR-048 已降级为软要求并记录"不做机械校验"的实测依据 | 追加 **T070** |
| F2 | contradicts | HIGH | FR-017 vs FR-060 | FR-017 要求"系统提示**嵌入** API 规范", FR-060 要求"**不得常驻**, 由 read_doc 按需取"(已实现且实测降 77% 输入) | 追加 **T071** |
| F3 | contradicts | MEDIUM | FR-059 | `specs/testnet.md` 仍有 demo 凭据**明文**(已入库), 与"凭据统一在 `locus.toml`"冲突 | 追加 **T072** |
| F4 | partial | LOW | FR-047 / 日志如实性 | `crates/locus_engine/src/command.rs:610` 对 demo 也打印 `live run started (实盘)`; CLI 横幅/list/info 均已正确标注 | 追加 **T073** |
| F5 | partial | LOW | tasks.md 自身 | T001–T059 中多数已实现但仍未勾选(仅 T062–T064/T068/T069 勾选), 状态与代码不一致会误导后续收敛 | 追加 **T074** |
| F6 | missing | 高(已跟踪) | FR-014/FR-015/FR-018 (P1) | 无 `preview_strategy` 工具; 生成闭环未实现 | 已由 T019–T021 跟踪, 不重复追加 |
| F7 | missing | 高(已跟踪) | FR-019–FR-028 (P2) | 无 `ai/confirm.rs`、无 L1 工具、无确认块 | 已由 T026–T032 跟踪 |
| F8 | missing | 中(已跟踪) | FR-037–FR-039 | 三平台分发未实现(agent-kit 已于 2026-09-15 落地; MCP 已取消) | 已由 T053–T055 跟踪 |
| F9 | partial | 中(已跟踪) | FR-043 | 策略名无校验(中文名会塌缩) | 已由 T023 跟踪 |
| F10 | partial | 中(已跟踪) | FR-046 | `previews` 无清理 | 已由 T040 跟踪 |
| F11 | partial | 低(已跟踪) | FR-011/FR-025 | `klines_summary` 工具未注册(白名单 9 个); ~~`scan` 工具~~ **取消** —— 平台级选币已于 2026-09-15 删除(020) | 已由 T012 跟踪; T013 取消 |
| F12 | partial | 低(已跟踪) | FR-042/FR-040 | 体积/编译代价未实测; README 安装章与 FAQ 未写 | 已由 T044/T056 跟踪 |

**汇总**: 检查 60 条 FR / 14 条 SC / 32 条 plan 决策 / 宪法 MUST 原则(测试纪律: 真实调用不打桩 ✓ 本轮 demo 真实下单; 提交纪律 ✓ 仅在用户明确指示后提交; unsafe forbid ✓ 未新增)。
未跟踪缺口 5 项(已追加 T070–T074), 其余 12 项缺口均由既有任务跟踪。

## 六、遗留与风险(如实)

- Windows 真机(中文输入、双击入口、安装)全部未验证 —— 本机无法产出 Windows 产物, 必须走 CI(T034/T053–T057)。
- `specs/testnet.md` 明文 demo 凭据待清理(T072); 清理前请勿公开该文件。
- P1 尚不可交付: 用户目前可"问答 + 只读工具 + 真实 demo/Dry Run 运行", 但**不能**让 AI 生成策略落盘(Phase 3 未做)。

- **R2 已修复(2026-09-15)**: 确认门禁补上"仅交互终端"检查 —— `commands::require_interactive_terminal`(std `IsTerminal`),
  `locus approve` 与实盘逐字确认(`start --live` / `run --live` 共用 `require_explicit_phrase`)对非终端 stdin **一律拒绝**;
  管道/脚本/AI agent 工具调用喂入短语不再能过门。代码符合 spec §七 R2 原文("逐字确认仅交互终端")。
  真机验证(4 例): 管道 approve → 拒绝且 preview 仍 pending、零副作用; pty 下 approve → 正常通过并发 token;
  管道 `start --live` → 同一门禁拒绝; pty 下错短语 → 走既有"未确认+零动作"分支。单测两条分支; 测试 337 → 339。

## 追加(2026-09-15): T075 `examples/` 收敛

用户最终判断"examples 目录甚至没必要" → **整个目录删除**, 建策入口统一为 `create` / `approve` / `deploy` 闭环,
样板统一为内置 `strategies/builtin/shannon_grid.lua`。同步修改 `specs/lua-api.md` §九、两份 README、`specs/backtest.md` 历史注记。
T076(数据目录 vs 源码树职责分离)仍挂起, 建议排在 v0.1.0 发布之后。

## 追加(2026-09-16): R3 对话内确认 — FR/SC 证据对照

**范围**: spec §七 R3 / plan D33–D37 / tasks 阶段十四(T080–T088)。结论: 代码、确定性证据与真机留档**全部完成**(S1–S8 全 ✅), 按宪法测试纪律**未以 mock 顶替**。

| 条目 | R3 后的要求 | 证据(代码/测试, 2026-09-16 Windows) | 状态 |
|:--|:--|:--|:--|
| FR-010(收窄) | 写实仍非 LLM 工具; 仅交互 tty REPL 内用户逐字确认后宿主可代行落盘/启 demo | `ai/tools.rs`: `request_write_confirmation` 在 VIRTUAL_TOOLS(L1), 闭包内只 `prepare_*`+登记 pending; 结构性测试 `test_no_write_tool_names_are_allowed`/`test_registry_count_and_write_tool_boundary` 断言 deploy/approve/start_demo/stop_demo 等写实名不在白名单(9 只读+4 虚拟) | ✅ 代码+单测 |
| FR-027(明确) | 平仓/停机/开关/改参不开放对话内 | `ai/prompt.rs::GATES_GUIDE` 与工具层 `stop_refusal`(live/demo 拒绝、dry_run 可代停); `test_allowed_is_exactly_the_registry` 断言 | ✅ |
| 宪法 4.5 两步确认 | 对话渠道不得比终端宽 | 同一引擎内核: `execute_confirmed` = `engine::approve`(一次性 token, CAS)→`execute_strategy`; TTL/状态机复用; `r3s5_consumed_preview_cannot_be_prepared_again` 证不可重放 | ✅ 进程内全链路 |
| R2 tty 门禁 | 管道/单次模式不得开放对话内确认 | REPL 条件 `prompt.is_none() && stdin().is_terminal()`; 集成测试 `approve_requires_interactive_tty`(默认运行, 管道→"交互终端"拒绝); 非交互 hint 单测只含终端命令 | ✅ |
| 落盘双证据 | 确认后真实落盘 + preview consumed | `r3s5_confirm_deploy_writes_files_and_consumes_preview`: `<root>/strategies/<name>.toml`+`.lua` is_file、DB 状态 `consumed`; 错短语("好的部署吧"/裸 y/yes)5 例 pending 保留零落盘 | ✅ |
| 同名不覆盖 | 同名部署拒绝, 旧 preview 不被触碰 | `r3s5_same_name_deploy_is_refused_after_files_exist`; consumed 再 prepare 拒绝 | ✅ |
| 拒绝终态 | Reject 置 preview rejected 且零落盘 | `r3s5_reject_sets_preview_terminal_and_writes_nothing` | ✅ |
| demo 前置门禁 | 未部署/缺凭据不得发确认块; 块含端点+真实下单提示 | `r3s6_start_demo_gates_in_order`(三档); 集成测试 `run_demo_without_credentials_fails_fast`(缺凭据先于时钟预检/网络) | ✅ |
| FR-004/密钥纪律 | demo key env 覆盖、不落盘不入 git | `commands::load_demo_credentials(root)`: env 成对非空 > ricow.toml; 测试只用占位串/临时 root; `git status` 无密钥 | ✅ |
| S1–S4/S7 真机 | DeepSeek 真实问答/工具/回测/负例 | 2026-09-16 晚实跑 **4 passed / 0 failed(104s)**: S1 中文单轮; S2 真实行情(75794.005); S3+S4 网格生成→168 根真实 K 线回测(30 笔/+6.34)→零落盘+终端两步; S7 注入负例模型拒绝、零副作用(S7 首跑 SSE 瞬断属网络抖动, 重跑过) | ✅ |
| S6 demo 真机 | demo 端点真实下单链路 | 2026-09-16 晚: key 签名验证(canTrade=true)→`ricow create` 真实回测→手动同构落盘(agent shell 非 tty, `approve` 被门禁拒=设计使然; demo 落盘不需确认链)→`start --demo` 真连测试网→市价买 0.0328 BTC **5 笔真实成交**(USDT 4967.75→2484.40)→`stop --close-all` 平仓单 `btge2e-c5658710`→回落 4960.15, errors=0, 残留粉尘 0.0000072 BTC | ✅ |
| S8 Dry Run 真机 | 起停+status/fills/logs 闭环 | 2026-09-16 晚: `start btge2e`(无凭据)→虚拟本金 100000、真实行情撮合成交 @75724.89→status 运行中/7 笔→stop 优雅退出 | ✅ |
| S5/S6 tty 端到端 | 真实 tty REPL 人工走查 | 逻辑已 100% 由 bin 单测覆盖(r3s5_*/r3s6_*); agent shell 非 tty 被门禁拒(顺带实证 R2); 用户侧 tty 手测仍建议按 `ai_live_smoke.rs` 头注释脚本走一遍 | ⏳ 可选人工项(非阻塞) |

**已知问题(2026-09-16 真机发现, 留观察)**: demo 启动偶发 `WS-API 用户流订阅失败: status=400 Timestamp outside recvWindow`(同一启动内 REST 签名成功), 重试即过; 疑 WS-API 签名时间戳竞态, 建议为 WS-API 订阅加时间戳重同步/单次重试。

**门禁基线(本次实跑)**: workspace **355 passed / 0 failed / 15 ignored**(R3 新增 5 bin 单测 + 2 集成门禁测试; 新增 3 个 #[ignore] 真机项); `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。
