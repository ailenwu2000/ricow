# 019 实施计划: 内置 AI 助手 + 生态入口 + 开箱即用分发

**分支**: `master`(本项目不使用 feature 分支; 变更档案目录 = `specs/changes/019-ai-assistant/`) | **日期**: 2026-09-14 | **规格**: [spec.md](spec.md)

**输入**: `spec.md` 的功能规格(FR-001–FR-047 / SC-001–SC-014)+ 调研实测 [`specs/research/ai-assistant-2026-09.md`](../../research/ai-assistant-2026-09.md)+ 审核报告 [review.md](review.md)

**前置**: 用户 2026-09-14 已拍板方向(D1–D16 通过、D17 不花钱做签名、D18/D19 按建议执行)。

---

## 一、摘要

给 Locus 加**内置 AI 入口** `locus ai`(中文自然语言 → 生成/迭代 Lua 策略 → 真实 K 线沙箱回测 → 用户按键确认 → 落盘 → Dry Run → 用户确认后实盘 → AI 继续管理运行中的策略), 同时按"用户手上有什么"提供**三档接入**(内置 AI / agent kit / MCP), 并补上**三平台开箱即用分发**。

核心设计: **一套工具层 + 三个前端**;工具分三级权限(只读 / 虚拟 / 写实),**写实动作不作为工具注册** —— AI 无法自批、无法自行下单, 安全边界是结构性的而非提示词约束。既有安全链路(编译门禁 / 沙箱回测 / 两步确认 / 018 风险确认 / 时长门禁 / 时钟预检 / RiskEngine)全部原样复用。

## 二、技术上下文

**语言/版本**: Rust(stable, 项目 `rust-toolchain.toml`;实测 rustc 1.96.1)

**主要依赖**: `rig = "0.42"`(caret, 锁 0.42 线; `Cargo.lock` 入库保证可复现)、`rmcp = "3.3"`(MCP, 待核对 API)、复用既有 tokio/serde/serde_json/futures/sqlx/mlua/ta/rust_decimal

**存储**: 既有 SQLite(`previews` / `fills` / `pnls`)不变;AI 会话仅进程内不落库;LLM 配置 = `$LOCUS_ROOT/ai.toml`(**不含密钥**);密钥走既有 keyring(兜底 env)

**测试**: `cargo test --workspace`(纯逻辑单测)+ `#[ignore]` 真实联调(本机 Ollama 冒烟 / demo 账户实盘链路 / stdio JSON-RPC 冒烟);**不 mock LLM**

**目标平台**: Linux / macOS / Windows(三平台产物经 CI;本机实测无法产出 Windows/macOS 产物)

**项目类型**: CLI(单二进制)+ 本地引擎

**性能/成本约束**: 提示词约 20KB(≈5-7k tokens/轮);工具输出必须有长度上限;默认轮次上限 8

**规模/范围**: 1 个变更, 5 个 crate 不动 + `locus_cli` 内新增 `ai/` 模块;新增 4 个子命令(`ai` / `mcp` / `agent-kit` / `setup`)

## 三、宪法检查

| 原则 | 检查 | 结论 |
|:--|:--|:--|
| 一、完全本地化 | 网络仅交易所 API + 美股行情 + **用户自配 LLM API**(原则一明文允许);rig 依赖含无上报组件;沿用"无自动更新"(强制关闭 dist 的 updater) | ✅ 通过 |
| 二、策略层统一 Lua | AI 只产出 Lua;不产出 Rust 策略本体 | ✅ 通过 |
| 三、测试纪律 | 交易流程用 demo 账户真实调用, 禁 mock;纯逻辑单测;AI 链路真实冒烟用本机 Ollama(零成本零外发) | ✅ 通过 |
| 四、产物一律中文 | 命令/提示/文档/提交信息全中文 | ✅ 通过 |
| 五、少而精 | 不新建 crate、不新增 DB 表、不造热更新机制、不造平台内 MCP 协议、复用既有 daemon 控制通道与确认通道;新增依赖与"是否值得"见 §六 复杂度追踪 | ✅ 通过(有 1 项需说明, 见下) |
| 安全要求 | 写操作强制确认链路**不绕过**;Lua 沙箱不变;Dry Run 默认不变 | ✅ 通过 |

**复杂度追踪(仅列需说明项)**

| 项 | 为何需要 | 被否的更简替代方案 |
|:--|:--|:--|
| 引入 `rig`(~82 个新增依赖、与项目 reqwest 双版本并存) | 需要"工具循环 + 审批门 + 流式 + 多供应商"四件套;自研需长期维护等价物 | 自建 reqwest 工具循环(依赖最轻但需自研并维护循环/重试/流式);`async-openai`/`genai` 无编排与审批门 |
| 复活 MCP(`locus mcp`) | 服务"已有自己 agent"的用户(本次明确诉求);MCP 是跨客户端标准 | 只做 agent kit(无需协议实现, 但外部 agent 只能靠 CLI 命令, 无法"发现工具") |
| 新增 `previews` 清理与策略名规范 | 二者是**既有缺陷被本变更放大**(AI 迭代放大 preview 写入;AI 面向中文用户必然生成中文名 → 订单归属前缀塌缩) | 不修则: 表缓慢膨胀 + 停机兜底可能误撤同机另一实例挂单 |

## 四、已确认决策(D1–D19)

| # | 决策 | 状态 |
|:--|:--|:--|
| D1 | 入口 = `locus ai`(交互 + 单次);不新增 TG/Web | ✅ |
| D2 | 实现放 `locus_cli` 内 `ai/` 模块, **不新建第 6 个 crate** | ✅ |
| D3 | **工具三级权限**: L0 只读(AI 直调)/ L1 虚拟且有副作用(Dry Run 起停、预览;预览不落盘但 Dry Run 会**开始计时**)/ L2 写实(**不作为工具注册**, 只能用户触发) | ✅ |
| D4 | 实盘确认两次分离: 首次 018 披露知情确认 + 每次短语 `确认实盘 <name>`;三判据顺序不变 | ✅ |
| ~~D5~~ 已取代(D31) | (原) LLM 配置 `$LOCUS_ROOT/ai.toml`(不含密钥)+ 密钥走既有 keyring(兜底 env) | ✅ |
| **D31** ✅ | **唯一配置文件** `$LOCUS_ROOT/locus.toml`(0600): `[ai]` 段同时含 provider 与 api_key, `[exchange]` 段含演示/实盘凭据; 无 keyring / 无向导 / 无凭据命令 / 无多档案 / 无枚举 key 槽(2026-09-14 用户拍板) | 竞争品惯例(llm 按供应商名存 key、LiteLLM 密钥与模型同条目、freqtrade 密钥在 config.json); 用户只需理解一个文件 |
| **D32** ✅ | 权威文档**不常驻**系统提示, 由 `read_doc` 按需取; 对话开始即声明"写策略前必须先读文档" | 实测: 简单提问输入 7808→1821 tokens; 写策略时才付文档 token; 且模型读原文而非我转述(无转述失真) |
| D6 | 策略知识编译期嵌入(`include_str!("../../../../specs/lua-api.md")`), 与 agent kit 手册**同源** | ✅ |
| D7 | 默认护栏: 轮次上限 8;每轮打印 token 用量;不自动重试;不做参数寻优 | ✅ |
| D8 | 会话历史仅进程内;审计留痕复用 `previews` 表 + `logs/` | ✅ |
| D9 | **不 mock LLM**: 纯逻辑单测 + **真实端点冒烟**(默认本机 Ollama;可用 `LOCUS_AI_BASE_URL/MODEL/API_KEY` 指向用户的正式通道)。正式验证用用户 provider(deepseek 等), 小模型仅证链路 | ✅ |
| D10 | 不做热更新: 改参数/风控 = 停 → 改 → 起(一个确认块可含该动作集) | ✅ |
| D11 | 明确不做: LLM 直接下单 / 参数寻优 / 多智能体 / 语音 / GUI / 后台轮询 / 密钥外发 | ✅ |
| D12 | 生态入口三档(agent kit / MCP / 内置 AI)**共用同一套工具与命令实现** | ✅ |
| D13 | ~~MCP 以 `locus mcp` **子命令**复活(不再单列 binary)~~ ⚪ **已取消(2026-09-15, 见 `spec.md` §七 R1)** | ❌ |
| D14 | ~~MCP 只暴露 L0/L1;L2 由用户在自己终端执行~~ ⚪ **已取消(同上)**; L0/L1/L2 分级本身继续适用于内置 AI(L2 不作工具注册) | ⚪ |
| D15 | 分发用 cargo-dist 生成 shell / powershell / homebrew / msi, 平台矩阵走 CI, **显式关闭自我更新器** | ✅ |
| D16 | Windows 随包双击入口 `启动-Locus-AI助手.cmd`(`chcp 65001` + `ai`), msi 可入开始菜单 | ✅ |
| D17 | **不做**代码签名/公证(不花钱), README 如实写绕行方式 | ✅ 2026-09-14 |
| D18 | **策略名规范**: 仅 `[A-Za-z0-9_-]`、长度 ≤24、同前缀冲突拒绝;create 阶段拒绝并给解法 | ✅ 2026-09-14("按建议") |
| D19 | **改脚本路径**: 默认新名;受控覆盖 `--replace`(确认块 + 先备份 `<name>.lua.<ts>.bak`) | ✅ 2026-09-14("按建议") |
| ~~D20~~ 已取代(D31) ✅ | **不内置任何人的 demo 凭据**; 每个用户配置自己的 demo / 实盘 / AI 凭据(2026-09-14 用户拍板) | 共享凭据会导致多人持仓互相干扰, 且把凭据固化进产品等于替用户做安全承诺 |
| ~~D21~~ 已取代(D31) ✅ | 凭据分演示/实盘**两套独立集合**, 模式决定用哪套, **不跨套回落**(fail-closed) | 现状两套共用 `binance_key/binance_secret` 只能靠环境变量硬切, 用户无法同时配好两套 |
| ~~D22~~ 已取代(D31) ✅ | 凭据统一入口: OS Keyring 优先 → 不可用回落 `$LOCUS_ROOT/credentials.toml`(0600 明文) | 架构承诺的 headless 回落此前**未实现**(只有 trait 声明), WSL/headless 下用户根本存不进凭据 |
| ~~D23~~ 已取代(D31) ✅ | `locus setup` 三段式(演示/实盘/AI) + **粘贴即存**; 不再要求"临时文件 + keyring set" | 后者是开发者口味, 对目标用户过重(且实测在本机跑不通) |

### 4.1 R3 批次(2026-09-16 v2, 对话内确认; 对应 spec.md §七 R3)

| # | 决策 | 状态 |
|:--|:--|:--|
| D33 | LLM 侧只新增一个 L1 虚拟工具 `request_write_confirmation`: 仅做前提校验 + 渲染确认块 + 登记会话内 pending; **不落盘、不起进程、不发 token**; 白名单结构性断言同步禁止 deploy/start_demo/approve 等写实名 | ✅ 已实现 |
| D34 | 对话内仅开放**落盘部署**与**启动测试网 demo**两种代行; 实盘启动/停机(含 close-all)/实盘开关/改参改风控不开放, 模型代请求必须拒绝并给终端命令 | ✅ 已实现 |
| D35 | pending 状态机: 会话级 `Arc<Mutex<Option<PendingAction>>>` 单槽、TTL 15 分钟、过期优先拦截、新覆盖旧、错短语与普通提问保留; 短语判定复用 `is_explicit_confirmation`(裸 y/yes/ok 不认) | ✅ 已实现 |
| D36 | 宿主执行进程内直调同一引擎内核(`engine::approve`→`execute_strategy`;`ctrl::start_daemon(demo=true)`), 不经 shell、无第二条路径; deploy 拒绝连带 `engine::reject` 终态 | ✅ 已实现 |
| D37 | 仅交互式 tty REPL 开放对话内确认; 单次 prompt / 管道 / 非终端 stdin 一律只回终端命令(与 R2 同一 `IsTerminal` 门禁); demo 凭据新增 env 成对覆盖 `RICOW_DEMO_KEY/SECRET` | ✅ 已实现 |

## 五、改动清单(文件 → 改动)

| 文件 | 改动 |
|:--|:--|
| `Cargo.toml`(workspace) | `rig = "0.42"`;dist 配置(矩阵 + 安装器, **不含 updater**);`rmcp = "3.3"` |
| `crates/locus_cli/Cargo.toml` | 加 `rig` / `rmcp`(features 待 Phase 7 核对) |
| `crates/locus_cli/src/main.rs` | `Command::{Ai, Mcp, AgentKit, Setup}` 分发 |
| `crates/locus_cli/src/commands/{ai,mcp,agentkit,setup}.rs` | 四个子命令入口 |
| `crates/locus_cli/src/ai/{mod,config,provider,prompt,tools,confirm,session,guard}.rs` | 配置/适配层/提示词/工具注册表/确认块/会话循环/审批门 |
| `crates/locus_engine/src/strategy.rs` | 策略名规范(D18)校验: 字符集 + 长度 + 前缀冲突 |
| `crates/locus_engine/src/confirm.rs` | 预览过期/终态清理(FR-046) |
| `crates/locus_engine/src/*.rs`(按需) | 只读查询补"返回数据"形态, 供 CLI 打印 / 内置 AI / MCP **三处共用同一口径** |
| `packaging/启动-Locus-AI助手.cmd` | Windows 双击入口 |
| `.github/workflows/release.yml` | cargo-dist 生成的发布流水线 |
| `README.md` / `README_zh.md` | 三平台快速开始 + 常见问题(编码/签名/换 provider/离线) |

## 六、实施阶段

阶段划分与逐项任务见 [tasks.md](tasks.md)(阶段一~九, T001–T059), 摘要:

| 阶段 | 内容 | 主要产出 |
|:--|:--|:--|
| 一 | 通道与骨架 | 依赖、配置、`locus setup`、provider 适配、REPL 骨架、系统提示、Ollama 冒烟 |
| 二 | L0 只读工具 | 工具注册表 + 审批门(fail-closed)+ 状态/行情/回测/成交/日志/文档工具 + 输出上限 |
| 三 | 生成-回测-迭代闭环 | 预览工具(零落盘)、自修重试、多轮对比、策略名规范(D18) |
| 四 | 写操作确认 + 运行管理 | 确认块、部署、`--replace`(D19)、018 披露、实盘短语确认、Dry Run 计时告知、平仓/开关/改参 |
| 五 | 实盘问答闭环 | 状态问答、告警解释不代劳、表述纪律 |
| 六 | 文档/测试/收敛 | 预览清理、全量测试、真实链路验收、文档同步(含 `product.md` D12 行)、converge |
| 七 | 生态入口 | agent kit(AGENTS.md/SKILL.md 同源手册)~~、`locus mcp`(stdio, 只读工具)、`--print-config`、客户端实测~~(**2026-09-15 修订: MCP 部分取消, 见 `spec.md` §七 R1**) |
| 八 | 分发与上手 | dist 配置与 CI、Windows 双击入口、README 三平台、干净环境冒烟 |
| 九 | 审核修正项收口 | 审核 F1–F11 的逐项落地核对(映射表) |

**执行前置约定**: 本计划是方案级分解;执行时按 `plan-execution` skill 展开为逐项可验证清单(每项含精确文件、命令、期望输出), 经用户确认后逐项实施。

## 七、风险与处置(摘录; 完整表见工作稿归档)

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 模型幻觉产出不可用/有害策略 | 既有三重门禁不动;提示词禁止承诺收益;产出必须过回测才进入确认块 |
| R2 | 账户数据外流担忧 | 首次配置打印外发清单;输出白名单;本地 Ollama 可全离线(已实测) |
| R3 | 成本失控 | 打印 token 用量 + 轮次上限 + 各工具输出上限 |
| R4 | rig 0.x 破坏性变更 | `rig = "0.42"` + `Cargo.lock` 入库;rig 耦合只在 `ai/provider.rs` |
| R5 | 依赖体量与编译代价上升 | 实测记录后如实写进文档, 不预判 |
| R6 | 弱模型工具调用不稳 | 预设标注要求 + 可选探测;如实提示不自动降级 |
| R7 | AI 与 daemon 状态"说了没做" | 全部经现有 daemon 通道;只复述真实返回 |
| R8 | **实盘被 AI 侧误触发** | 结构性: L2 不注册为工具(内置 AI 与 MCP 都没有)+ 非裸 `y` 短语 + 披露与启停确认分离 + 三判据顺序不变 |
| R9 | Windows 控制台中文输入编码 | 明确报错 + `.cmd` 内 `chcp 65001` + 单次模式兜底;必要时再引入 `rustyline` |
| R10 | 用户误以为"有 AI 在盯盘" | 不做后台轮询;主动告警沿用既有出站通知;setup/文档如实说明 |
| R11 | MCP 扩大攻击面 | 只暴露 L0/L1;L2 由用户执行;输出不含凭证;手册写明不可信内容边界 |
| R12 | 手册与实际命令漂移 | 手册与系统提示**同源同常量**;改命令必须同步 |
| R13 | 无签名被系统拦截 | 如实提示绕行;不夸大为"一键无感安装" |
| R14 | 分发渠道维护成本 | 首版只做 cargo-dist 直接支持的渠道;winget/scoop 不做并说明 |
| R15 | macOS/Windows 产物本机不可构建 | 一律走 CI 矩阵 |
| R16 | **中文策略名 → 订单归属前缀塌缩**(误撤同机另一实例挂单) | D18 命名规范(结构性地在 create 拒绝)+ 提示词/手册同约束 |
| R17 | `previews` 表随迭代堆积 | 过期/终态清理 + 前后行数对照 |
| R18 | 工具输出无上限 → 上下文与成本失控 | 各工具输出上限 + 长会话裁剪/提示 `/new` |
| R19 | (2026-09-14 实测发现) "headless 加密文件回落"在架构/product 有承诺但**代码未实现**(`FallbackStore` 仅 trait 声明), 导致无 Secret Service 环境(WSL/headless/Docker)无法存任何凭据 | D22 落地实测闭环: 真机写入 → 权限 0600 → 读回 → `keyring list` 显示已配置 |

## 八、验收

以 [spec.md](spec.md) §五 SC-001–SC-014 为准(测试全绿 / 生成闭环真实链路 / 实盘链路 demo 验证 / 负例含结构性拒绝 / 隐私 / 成本可观测 / 中文与 Windows 结论 / 生态入口双验证 / 干净环境安装 / 无自动更新 / 文档同步 / 策略名规范 / 预览表不膨胀)。

## 九、参考资料

- 调研实测: [`specs/research/ai-assistant-2026-09.md`](../../research/ai-assistant-2026-09.md)
- 审核报告: [review.md](review.md)(F1–F11 已落地本版)
- 工作稿(未归档, 已在 `.hermes` gitignore 区): `.hermes/plans/2026-09-14_102752-locus-ai-assistant.md`(r4, 含完整叙述与附录)
- 既有安全模式: [`specs/research/ai-mcp-2026.md`](../../research/ai-mcp-2026.md) §5
- P3 的 MCP 实现参考: skill `rmcp-mcp-server` → `references/locus-p3-example.md`
