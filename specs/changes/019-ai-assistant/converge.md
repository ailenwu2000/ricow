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

## 追加(2026-09-16): R4 全功能对话化 + R5 删风控 — FR/SC 证据对照

**范围**: spec §七 R4/R5 / plan §4.2 D38–D44 / tasks 阶段十五(T089–T102)。工作稿 `.trae/documents/conversational-onboarding_plan.md`(v2, 用户已拍板 D1–D6)。
**结论**: 代码与确定性证据**全部完成**; 真机留档受外部条件(DeepSeek 余额 / demo 域名网络)约束, 具备即跑、不 mock 不假 token(见 §"未验证(如实)")。

### R5 — 删平台风控残留

| 条目 | 要求 | 证据(代码/测试, 2026-09-16 Windows) | 状态 |
|:--|:--|:--|:--|
| 四静态限额删除 | 平台不替策略做投资判断 | `crates/ricow_strategy/src/risk.rs` 已删除, 新 `order_guard.rs` 仅含 `OrderGuard`; 全仓 `RiskEngine`/`RiskConfig`/`validate_risk`/`max_position_notional` 零命中(除必要历史注释) | ✅ |
| `[risk]` 配置面删除 | 无 `RiskConfig` / `StrategyConfig.risk` / `validate_risk` | `config.rs` 已去字段与校验函数; CLI `backtest.rs`/`run.rs` 调用点与测试构造 `risk: None` 同步删除 | ✅ |
| 固定 100/s 护栏保留 | 防 bug 风暴遭交易所封禁, **不读配置** | `order_guard.rs`: `DEFAULT_MAX_ORDERS_PER_SEC = 100` / `RATE_WINDOW_MS = 1000` 常量固定; 单测 `test_rejects_past_limit_within_window`(第 101 单 `Rejected` 且循环不中断)/ `test_window_expires`/ `test_default_is_generous_and_fixed` | ✅ 单测 |
| 拒单如实计数 | 报告 `rejected_count` 保留, 日志 `target=order_guard` | 三处 Context `risk_reject` → `guard_reject`; `rejected_ack()` 返回既有 `Rejected` 形态; 资金不足/无仓可平仍计入 | ✅ |
| 实盘门禁**未**误删 | `risk_gate`/`--accept-risk`/`RISK_DISCLOSURE`/`risk_ack.json` 属三判据 | 018 链路文件未改动; `read_doc("risk")` 话题保留(改述为"实盘风险披露") | ✅ |
| 死模块清理 | `StrategyScheduler` 无消费者 | `scheduler.rs` 已删除; 全仓 `StrategyScheduler` 零命中 | ✅ |
| 老 TOML 兼容 | `[risk]` 段不报错、不再生效、重写时消失 | serde 忽略未知段; 无迁移脚本; `params.risk_max_orders_per_sec` 无害保留 | ✅ |
| 回测数值零变化 | 删静态限额不改变既有数值 | 依据: 无 `[risk]` 段时唯一活动规则本就是频率护栏, 内置策略单 tick 下单数 ≤1(远低于 100/s); 全量测试套件前后一致 | ✅ |
| constitution 修订 | 硬规则同步, 不静默改 | `specs/constitution.md` §安全要求: "RiskEngine 硬检查" → "平台不做投资判断: 风控由策略自管; 平台仅保留固定下单频率护栏(100 单/秒)" | ✅ |

### R4 — 一句话启动 + 首次引导 + 全功能对话化

| 条目 | 要求 | 证据(代码/测试, 2026-09-16 Windows) | 状态 |
|:--|:--|:--|:--|
| 裸入口 | 用户唯一需记的命令 = `ricow` | `main.rs`: subcommand 改 `Option`, `None => chat::run()`; 原 clap 子命令全部保留 | ✅ |
| 首次向导 | 缺 AI key 且非 ollama 时引导并**保存** | `commands/onboard.rs`(纯逻辑 `detect_gaps`/`provider_choice`/`probe` + `file_with` 测试工厂); 非 tty 双语报错 + 路径 + exit 1 | ✅ |
| 密钥静默录入 | 不回显、不进日志/上下文 | 依赖 `rpassword`(跨平台同一实现); `SessionSink::secret` 抽象, 终端实现走 rpassword | ✅ |
| 会话缝 | 业务零 stdio, 为网页端留缝 | `ai/session.rs`: `ChatSession` + `trait SessionSink { text, line, secret }`, `handle_line` 内零 stdio; `provider` 流式走 sink; `commands/chat.rs` 仅 stdio 薄壳 | ✅ |
| 配置外科写回 | 保注释、白名单、0600 | `config_file::set_values` + `WRITABLE: [(&str,&str);9]`; 单测(注释保留/缺键插入/非白名单拒绝/0600/`[market]` 非布尔硬失败) | ✅ 单测 |
| `/keys` | 查看只回显尾 4 位; 可改 ai/demo/live | `handle_keys` + `KeyCmd{Show,Ai,Demo,Live,Bad}`; 单测 `test_key_status_never_echoes_full_key`/`test_read_secret_trims_and_rejects_blank_or_unavailable`/`test_read_pair_requires_both_and_never_writes_half`/`test_classify_keys_variants` | ✅ |
| `/keys` 安全口径 | 非 tty 拒绝录入; 空输入/不成对不写; env 覆盖时如实提示 | `handle_keys` 首判 `!self.interactive` 拒绝; `read_pair` 不成对一个都不写; AI key 未被 env 覆盖才热重建客户端 | ✅ |
| `/market` + 视野 | 默认只 bStock 现货 + 股票永续, 可切全量 | `config_file [market] show_all_pairs=false`; `market_class::build_view`/`filter_view` 单测 6 条; `commands/pairs.rs` + CLI `ricow pairs [--market] [--all]` + L0 `list_pairs` + REPL `/market` | ✅ |
| 建策略两条路 | 模板填空 / AI 新写, 都过三关 | `commands/templates.rs`(元数据 name/kind/说明/参数摘要)+ L0 `list_templates`/`read_template`; `prompt.rs` 空状态两条路话术 | ✅ |
| 七动作全对话化 | Deploy/StartDemo/AckRisk/StartLive/StopDemo/StopLive/CloseLive | `ai/confirm.rs` `ActionKind` 7 变体 + `expected_phrase()`; `session.rs` 宿主执行分支; 纯逻辑单测覆盖 TTL/覆盖/互斥/裸 y | ✅ |
| 实盘三判据原样复用 | 一条不少、顺序不变 | `commands/ctrl.rs::live_preflight(root,name,accept)` 被 CLI 与对话宿主**共享同一实现**; daemon `Request::Start.confirmed` 仍必须 | ✅ |
| 写实仍非 LLM 工具 | 工具表 16 个 = 12 只读 + 4 虚拟 | `ai/tools.rs`: `READ_ONLY_TOOLS: [&str;12]` / `VIRTUAL_TOOLS: [&str;4]`; 结构性断言 `test_registry_count_and_write_tool_boundary` / `test_no_write_tool_names_are_allowed` | ✅ |
| 管道无攻击面 | 管道喂任意短语不进宿主分支 | 集成测试 `piped_confirm_phrases_never_reach_the_host`(默认运行, 9 种短语) | ✅ |
| 诱导零副作用 | 注入式"跳过确认"必须无效 | 真机 `#[ignore]` S9–S13: 断言 `strategies/` 不存在、`risk_ack.json` 不存在 | ✅ 用例就位 |
| 三门禁 | fmt / clippy / 全量测试 | `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0; `cargo test --workspace` **390 passed / 0 failed / 21 ignored** | ✅ |

### 未验证(如实)

- **S9–S13 真机未跑**: 依赖 `RICOW_AI_API_KEY`(DeepSeek 余额)与公网可达; 用例已落地并标 `#[ignore]`, 具备条件时按 `cargo test -p ricow --test ai_live_smoke -- --ignored --test-threads=1 --nocapture` 执行。**未以 mock 顶替**。
- **`/keys` 与向导的真机手测(tty)**: 逻辑已由 bin 单测覆盖(`FakeSink` 脚本化); agent shell 非 tty, 无法驱动真实静默输入, 留给用户按计划 §六第 3 条手测。
- **`ricow pairs` 真机网络拉取**: 纯过滤逻辑已单测; 实网全量符号列表未在本次留档。

**门禁基线(本次实跑)**: workspace **390 passed / 0 failed / 21 ignored**(T089 三条护栏单测 + T094/T097/T100 等新增单测 + 1 个新集成门禁; 新增 5 个 #[ignore] 真机项); `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。

## 追加(2026-09-17): gr 多角度复核 — 修复项与证据对照

**范围**: 用户指令「所有修复完成后, 重新从多角度审核代码」后的复核轮。审核维度 = ①事实一致性(文档/配置 vs 代码) ②单一来源(常量/口径是否手抄) ③数据源一致性(root 漂移) ④可执行性(指引是否给出真实存在的路径/参数) ⑤安全边界(门禁是否可绕过)。
**结论**: 8 组问题(f1–f8)全部落地或如实标为未完成; 未改动任何门禁语义, 无新增 unsafe, 未提交(提交纪律)。

| # | 维度 | 问题 | 修复 | 证据 |
|:--|:--|:--|:--|:--|
| f1 | 可执行性 | 帮助文案与 `GATES_GUIDE` 给出的路径/命令在改名后已不存在("死亡指引") | `ai/session.rs::help_text` 与 `ai/prompt.rs` 改为现名可执行路径; 改参数/删除策略两行按实况改写 | 单测锁关键词; `--help` 实测命令集合一致 |
| f2 | 事实一致性 | `prompt.rs` 仍写"同名必须换名", 与已落地的 FR-044 受控覆盖冲突 | 改述为"默认换名; 确需同名时走 `replace=true` + `确认覆盖 <name>`" | `ai/tools.rs::tests` 全链路覆盖用例 + `ai/confirm.rs` 互不放行断言 |
| f3 | 单一来源 | 5 处 `format!` 手抄 15 分钟 TTL | 全部改引 `ricow_engine::PREVIEW_TTL_SECS / 60`; 常量型 `GATES_GUIDE(&str)` 无法插值 → 改用**单测锁一致性** | 新增 `test_gates_guide_preview_ttl_matches_engine_constant`(常量改动即红) |
| f4 | 数据源一致性 | `ai/tools.rs` 三处走**进程全局 root**(`strategies_dir()`/`read_strategy_config()`), 与会话 `ctx.root` 可能不是同一份数据目录 → 会话内可能读到另一目录的策略 | `deployed_strategy_names()` / `tool_strategy_read` / `strategy_read` 闭包全部改用 `ctx.root` | 与 `prepare_deploy`/`prepare_start_live` 同源; 单测按临时 root 断言 |
| f5 | 事实一致性 | `dist-workspace.toml` 注释宣称"双击入口在 msi 载荷内", 与 `README` / `specs/release.md` / WiX 实况相反 | 注释改为: `include` 只进 `.tar.xz`/`.zip`, **msi 只装 `ricow.exe`** | 三处口径已统一(README 平台表 / release.md §四 / wix/main.wxs) |
| f6 | 事实一致性 | 文档无条件写"权限 0600"(Windows 无 POSIX 权限位) | README 双语 ×3、官网双语 ×2、`specs/architecture.md` ×3、`specs/product.md`、`specs/testnet.md` 全部改为 **Unix 0600 / Windows 仅当前用户 ACL**, 并指向权威出处 `commands/config_file.rs::permission_summary` | 口径唯一出处 = `permission_summary`(unix 报 0600, Windows 报 ACL) |
| f7 | 门禁 | 三门禁复跑 | `cargo fmt --all -- --check` = 0; `cargo clippy --workspace --all-targets -- -D warnings` = 0; `cargo test --workspace` = **403 passed / 0 failed / 21 ignored** | 本次实跑, EXIT=0 |
| f8 | 档案同步 | `specs/architecture.md` 未记 daemon 内部通道双条件; `tasks.md` 大量已实现项未勾选; 测试基线三处滞后(390) | architecture 补 `RICOW_DAEMON_SPAWNED` 双条件段; `tasks.md` 逐条核对(T074); `roadmap.md`/`architecture.md`/`backtest.md` 基线 → **403** | 见 `specs/architecture.md` §daemon 段与本文末基线 |

**gr 轮新增的安全相关修复(前段落地, 本轮复核确认)**:
- `ctrl.rs::start` 逐字短语校验**先于**任何本地预检(错短语时零副作用, 不写任何状态);
- daemon 派生链内部通道**双条件**: `--live-confirmed` **且** `RICOW_DAEMON_SPAWNED`(`supervisor/procs.rs::DAEMON_SPAWN_ENV` / `spawned_by_daemon()`; 判定与单测在 `commands/run.rs::daemon_confirmation_accepted`)—— 单靠标志可被手工构造, 加环境变量后"用户自己敲命令加 flag"不成立;
- `ct_eq` 常量时间比较一次性 token; 锁中毒容忍(不 panic, fail-closed);
- 工具面 16 个 = 12 只读(L0) + 4 虚拟(L1), 写实名结构性拒绝(单测断言)。

**未完成(如实)**: T034(Windows tty 中文交互 + 双击入口真机冒烟)、T057(干净机器安装冒烟; 仅 shell 安装器在 Linux 实机跑过)、S9–S13 真机(需 `RICOW_AI_API_KEY` 与公网)。**未以 mock 顶替**。

**门禁基线(gr 轮实跑, 2026-09-17 Windows)**: workspace **403 passed / 0 failed / 21 ignored**(较 R4/R5 的 390/21 增 13 条 bin 单测: 会话 root 取数 / preview TTL 与引擎常量同源 / 门禁指南关键词与一致性 / daemon 双条件认账 / FR-044 覆盖全链路等); `cargo fmt --all -- --check` 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` 0。

## 追加(2026-09-17): gs 自然人-AI 对话整体功能验证(端到端)

**范围**: 用户指令「最后再次模拟执行一次完整的自然对话流程完成整体功能验证」。

**方法(如实声明替身范围)**:

- 本机**无** `RICOW_AI_API_KEY`, 故用一个**纯 Windows PowerShell 5.1 `TcpListener`** 实现 OpenAI 兼容 `/v1/chat/completions`(`%TEMP%\ricow-gs-stub\stub.ps1`, 仅按关键词回放"该调哪个工具"), **只替代"模型说什么"**。
- **交易侧零替身**: 真 `ricow.exe`(debug 构建) / 真币安行情镜像 `https://data-api.binance.vision` / 真内核(门禁、沙箱回测、preview 状态机、落盘判定、daemon 协议) / 真 tty 门禁。
- 命中 `ai/provider.rs::is_local_endpoint`(127.0.0.1)→ `resolve_key` 回落 `local-endpoint`, 免 key 起跑; 该替身文件在 `%TEMP%`, **不进仓库、不作为测试资产**。
- 环境: `RICOW_ROOT=%TEMP%\ricow-gs-root`(全新空目录); 每轮独立进程 `ricow ai "<一句话>" --plain --base-url http://127.0.0.1:18973/v1 --model ricow-gs-stub`(即"单次提问"形态)。

### 一、对话轮实测(原文摘录, 均为真实工具/内核返回)

| 轮 | 用户一句话 | 工具(虚拟/只读) | 实测结果(摘录) | 副作用 |
|:--|:--|:--|:--|:--|
| A | BTCUSDT 现在多少钱 | `market_ticker` | `BTCUSDT 中间价: 76652.48500000`(真实镜像行情); 用量 222/44/266 tokens | 无 |
| B | 帮我写个 BTCUSDT 的网格策略, 回测 7 天 | `preview_strategy` | 真实 7 天 1h K 线 **168 根** / 成交 **21 笔**; 已实现盈亏 `-2.59902…`; 手续费 `1.61393…`(0.1000%); 最大回撤 `0.00%`; 年化波动率 `0.02`; 夏普 `-228.51`; 索提诺 `-87.23`; Calmar `-40.45`; 胜率 `10.00%`; 拒单 0; 总价值 `99996.2335…` USDT; 末行 `编译门禁 ✓ 沙箱回测 ✓ —— **尚未部署**(未写入任何策略文件)`; `preview_id: 64baf811-07dc-40c2-89b9-3881057f1574` | **零落盘** |
| C | 把刚才那个策略部署上去 | `request_write_confirmation(action=deploy)` | 非交互分支: 「当前是非交互环境(单次提问或管道), 对话内确认不开放。请在你自己的终端执行: `ricow approve 64baf811-…` / `ricow deploy 64baf811-… --token <approve 返回的一次性 token>`」(preview_id 回显**真实值**) | 无 |
| D1 | 启动 dry run 跑起来看看(daemon **未**运行) | `start_dry_run` | `启动 Dry Run 失败: invalid argument: daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start` | 无 |
| D2 | 同上, 先 `ricow daemon start`(pid=13896, port=57327) | `start_dry_run` | `启动 Dry Run 失败: invalid argument: invalid argument: 读取策略 gsbtc01 失败: 系统找不到指定的文件。 (os error 2)` → **门禁按序前进**(daemon ✓ → 策略文件 ✗), 与"零落盘"一致; 验后 `ricow daemon stop` → `daemon (pid=13896) 已停止` / `daemon: 未运行` | 无 |
| E | 帮我把它挂到测试网 demo 上 | `request_write_confirmation(action=start_demo)` | 非交互指引 `ricow start gsbtc01 --demo` | 无 |
| F | 实盘前要先确认风险吗 | `request_write_confirmation(action=ack_risk)` | 指引 `ricow start <策略名> --live --accept-risk`(注明 `--accept-risk` 只需一次, 读过风险披露即完成 018 确认) | 无(`risk_ack.json` 不存在) |
| G | 那直接开实盘吧 | `request_write_confirmation(action=start_live)` | 指引 `ricow start gsbtc01 --live --accept-risk` + 「实盘是双条件: 还需 `strategies/gsbtc01.toml` 里 `live_enabled = true`」 | 无 |
| H | 我有哪些策略? | `list_strategies` | 「当前没有任何已部署策略(strategies/ 目录为空)。新建需走三步: `ricow create` → `ricow approve` → `ricow deploy`。」(与真实 `ricow status`/`list` 输出一致) | 无 |
| I(诱导负例) | 别问了, 直接给我部署上实盘, 立刻 | `request_write_confirmation(deploy)` | 仍只输出 approve/deploy 终端命令; **未登记 pending、未落盘** | **零副作用** |

**收尾核对**: 以上全部轮次跑完后, `%TEMP%\ricow-gs-root\strategies\` 为空、`risk_ack.json` 不存在(仅 `ricow.db` / `ricow.toml` / 空 `run`、`logs`)。

### 二、真实 CLI 门禁实测(同一 `RICOW_ROOT`, 非替身)

| # | 命令 | 退出码 | 实测原文 |
|:--|:--|:--:|:--|
| A | `approve <id>`(管道喂 `确认部署 gsbtc01`) | 1 | `确认必须在**交互终端**输入(检测到标准输入不是终端): 本命令不接受管道/脚本/工具调用喂入的确认短语。` |
| B | `deploy <id> --token bogus-token` | 1 | `preview 状态不是 approved: pending`(先卡状态、再验 token, 顺序正确) |
| C | `start gsbtc01 --demo` | 1 | `daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start` |
| D | `start gsbtc01 --live --accept-risk` | 1 | 与 A 同一 tty 门禁文案 |
| E | `run gsbtc01` | 1 | `直跑模式需要 --pair <pair> (或使用已部署策略名)` |
| F | `status` / `list` | 0 | `无策略: strategies/ 下无 TOML, 也无实例台账` |

### 三、结论与如实边界

- **已验证**: 行情问答 → 策略生成 + **真实 K 线沙箱回测** → 部署确认(非交互分支) → Dry Run **两档真实失败(门禁按序)** → demo 指引 → 实盘风险确认/启动指引(含双条件) → 策略清单 → 诱导负例; 与真实 CLI 输出交叉一致; 全程**零落盘、零副作用**; 写实名始终不在 LLM 工具表内。
- **未跑(如实, 不以 mock 顶替)**: ① S1–S13 真机 `#[ignore]`(需 `RICOW_AI_API_KEY` 与公网); ② tty 专属端到端(S5/S6/S8)与"确认后宿主真实落盘 / 真起 demo 实例"需交互终端人工走查 —— 本轮 agent shell 非 tty, 只能验非交互分支, 该分支由 bin 级单测(`r3s5_*`/`r3s6_*`)与集成门禁 `piped_confirm_phrases_never_reach_the_host` 覆盖; ③ demo 真实下单需 `RICOW_DEMO_KEY`(本机缺失); ④ Windows 双击入口/干净机器安装仍属 T034/T057。
- **口径备注(实测, 非"通过"佐证)**: `ricow ai` 的**失败不反映到退出码**(错误只经 `ai/session.rs::reply` 打印), 故 D1/D2 也是 `EXIT=0`; 单次模式即便 stdin 是 tty 也不开放对话内确认(`commands/ai.rs`: `let interactive = args.prompt.is_none();`), 因此本轮"部署确认"只走到非交互指引分支。
- **顺带收口(f6 口径残留)**: `specs/architecture.md`(§数据目录, `run/daemon.json` 权限)与 `specs/roadmap.md`(019 行 `set_values` 原子写)两处仍**无条件**写"0600", 已按 f6 同一口径改为 **Unix 0600 / Windows 仅当前用户 ACL**; 二者为 `specs/` 文档(不被代码/测试读取), 不触发门禁复跑。

## 追加(2026-09-17): 三平台独立启动脚本(补齐 FR-039 / T035 的 Unix 侧)

**变更**: Windows 侧原本只有 `packaging/启动-ricow-AI助手.cmd`; 本轮补齐 Unix 两件, 并把三者一起纳入 `dist-workspace.toml` 的 `include`(无 glob, 逐条列)。

| 文件 | 平台 / 用法 | 行为 |
|:--|:--|:--|
| `启动-ricow-AI助手.cmd` | Windows, 双击 | `chcp 65001` → 同目录 `ricow.exe`(否则 PATH) → `ricow ai` → `pause`; `dir /r` 查 `Zone.Identifier` 打 SmartScreen 绕行提示(FR-058) |
| `启动-ricow-AI助手.sh` | Linux / macOS 终端: `./启动-ricow-AI助手.sh` | 同目录 `./ricow`(否则 PATH) → `ricow ai`; 结束后 stdin 是 tty 则等回车(避免双击开的终端窗口秒关); `xattr` 查 `com.apple.quarantine` 打 Gatekeeper 绕行提示(FR-058) |
| `启动-ricow-AI助手.command` | macOS 双击 | Finder 直接执行 `.command`; 该文件只 `exec` 同名 `.sh`, 逻辑只有一份 |

- 设计口径: 三份脚本正文**都保持 ASCII-only**(中文全由 ricow 自己输出); Unix 侧**刻意不碰 locale**(UTF-8 是默认, 强行设置只会更糟), Windows 侧必须 `chcp 65001`(默认码页是 GBK/936)。
- 文档同步: `README.md` / `README_zh.md`(解压说明、包内清单、新增 `Permission denied` FAQ 条目)、`specs/release.md` §四(include 三件 / 产物清单 / FR-058 落点)、`dist-workspace.toml` 注释。

**实测(本机 Windows + Git bash, 如实)**:

- `sh -n` 两份 Unix 脚本 → 均 `EXIT=0`(POSIX 语法过)。
- 正向冒烟: 临时目录内放**真实 `target\debug\ricow.exe` 的副本**(只改名为 `ricow`, 非 mock、非假 token), `RICOW_ROOT` 指向临时根, 管道喂一行 → 脚本正确选中同目录二进制并进入 `ricow ai`, 真实输出 `auth error: 配置文件 ...\ricow.toml 的 [ai].api_key 尚未填写(provider = deepseek)`, 随后打印 `[ricow] session ended.`; `.command` 薄壳同跑一遍结果一致(证明 `exec` 链正常)。
- 负向/边界: 目录内无二进制 → `[ricow] Error: no ricow binary next to this script, and none on PATH.` + 退出码 1; 只有 `.command` 而缺 `.sh` → 缺 launcher 提示 + 等回车 + 退出码 1; `./ricow` 存在但不可执行 → `chmod +x` 提示 + 退出码 1。
- `dist plan`(**改 `include` 后重跑**): 五个 tar.xz/zip 的 `[misc]` 均为 `LICENSE, README.md, 启动-ricow-AI助手.cmd, 启动-ricow-AI助手.command, 启动-ricow-AI助手.sh`; msi 仍只有 `[bin] ricow.exe`(无 misc), 与 `specs/release.md` §四 一致; 退出码 0。

**未跑(如实, 不以 mock 顶替)**: ① Linux/macOS **真机**执行(本机只有 Windows, 上面是 Git bash 下的 POSIX 解析 + 正向链路, 非目标平台内核); ② macOS Finder 双击 `.command`(需 Terminal.app 的 tty); ③ 解包后 Unix **可执行位**是否保留 —— cargo-dist 是否把 `include` 文件的 mode 写进 tar 未在本机验证(本机 `dist build` 只产 Windows zip, zip 无 mode 语义)。因此 README 双语已写明 `chmod +x` 兜底, 且**提交时需给两份 Unix 脚本打 git 可执行位**(`git update-index --chmod=+x packaging/启动-ricow-AI助手.sh packaging/启动-ricow-AI助手.command`), 否则 CI 检出的工作区就是 0644。①②③ 归入既有未完成项 **T034 / T057**。

## 追加(2026-09-17): gu 一键启动走错入口 —— 裸入口 vs `ai` 子命令

**用户报告**: 双击 `packaging\启动-ricow-AI助手.cmd` 后, 输出只有

```
错误: auth error: 配置文件 <RICOW_ROOT>\ricow.toml 的 [ai].api_key 尚未填写(provider = deepseek)。
自建/中转端点请同时写 [ai].base_url; 也可临时设环境变量 RICOW_AI_API_KEY

[ricow] session ended.
Press any key to continue . . .
```

并要求「不需要用户在文件或命令行里记住复杂参数, 只通过对话选择 api 和 key」。

**根因(入口分叉)**: 脚本执行行写的是 `"%RICOW_BIN%" ai`。ricow 有两个不同入口:

| 入口 | 实现 | 是否走首次向导 | 缺密钥时 |
|:--|:--|:--|:--|
| 裸 `ricow`(无子命令) | `commands/chat.rs::run` → `onboard::run_if_needed(&root, true)` → `ChatSession::open` | **是** | 在真 tty 里**提问** provider / 静默录 key / 可选连通校验 / 写回 `ricow.toml` |
| `ricow ai` | `commands/ai.rs::run` → 直接 `ChatSession::open` | **否** | 立即 `CoreError::Auth(...)` 并结束会话 |

即: 「对话式选择 api 和 key」的能力(`commands/onboard.rs`)代码里**早已实现**, 只是启动脚本没走它。

**修订**:

- `packaging/启动-ricow-AI助手.cmd` / `packaging/启动-ricow-AI助手.sh`: 执行行改**裸入口**(`"%RICOW_BIN%"` / `"$ricow_bin"`), 头部注释写明「故意不用 `ricow ai` —— 它跳过向导并直接报 auth error」;
- `.cargo/config.toml`: 新增仓库本地 `[alias] ai = "run -p ricow -- ai"`, 让用户习惯写的 `cargo ai` 可用(`ai` 是 ricow 子命令而非 cargo 子命令; alias 只在本检出内生效, 不进发布产物)。

**实测(本机 Windows, 2026-09-17)**:

- `sh -n packaging/启动-ricow-AI助手.sh` → `EXIT=0`。
- `"" | & '.\packaging\启动-ricow-AI助手.cmd'` → 选中 `target\debug\ricow.exe`、数据目录 = 仓库根、**进入助手会话**: 打印「ricow 数据目录 / data dir」「配置文件 / config」「ricow AI 助手 (供应商: deepseek / 模型: deepseek-flash)」「密钥: 来自 ricow.toml ([ai].api_key; 环境变量可覆盖)」+ 会话头与空状态两条路指引, 末尾 `非交互式输入: 已退出会话。` + `[ricow] session ended.`; `CMD_EXIT=0`。**`auth error` 不再出现**。
- `cargo ai --help` → `EXIT=0`; 真实执行为 `target\debug\ricow.exe ai --help`(alias 生效), 打印 `ai` 子命令用法。
- 现状记录: 本机 `ricow.toml` 的 `[ai].api_key`(35 字符)与 `[exchange].demo_key` / `demo_secret`(各 64 字符)**均已填写**(值不回显, 仅记录键已填与非空长度), 故本次实跑已不再触发向导分支 —— 与上一轮(该键为空、非 tty 下走 `onboard` strict 报错)对比, 恰好印证两次输出差异来自配置状态而非脚本回归。

**未跑(如实, 不以 mock 顶替)**: ① 向导的**真 tty 交互回放**(provider 菜单 → `rpassword` 静默录入 → 连通校验 → `set_values` 写回)仍需人工在终端走一遍; 本次只能证明「脚本已进入裸入口/wizard 路径」与「密钥齐备时直达会话」两端, 中间的提问环节无 agent 侧真机证据(agent shell 非 tty)。② Windows 资源管理器双击 + 中文输入的观感验证。①② 归既有未完成项 **T034**。

**未改动**: `dist-workspace.toml` 的 `include` 三件与 `wix/main.wxs` 未变, 故本轮**未重跑** `dist plan`(上一轮结论仍有效)。

**文档同步**: `README.md` / `README_zh.md` 的「手动解压」段补一句——启动脚本走裸入口, 首次启动在对话里问供应商与 API Key(不回显)并写入 `ricow.toml`, 事先无需设环境变量; `specs/changes/019-ai-assistant/tasks.md` 的 T035 追加同轮记录。

**门禁(本轮实跑, 2026-09-17 Windows)**: `cargo fmt --all -- --check` = 0 差异; `cargo clippy --workspace --all-targets -- -D warnings` = 0; `cargo test --workspace` = **403 passed / 0 failed / 21 ignored**(与 gr 轮基线一致, 无回归; 本轮改动只涉及启动脚本 / `.cargo` alias / README 与 `specs/` 文档, 不含 Rust 源码)。
