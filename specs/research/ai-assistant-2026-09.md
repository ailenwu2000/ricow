# 019 调研档案: 内置 AI 助手的技术可行性(实测, 2026-09-14)

> 性质: 调研档案(证据与结论留档, 已定稿不回改)。结论已落入 `specs/changes/019-ai-assistant/spec.md` 与 `plan.md`。
> 方法: 本机实测(cargo / curl / crates.io API / 官网文档原文) + 历史实现考古(git 历史 + 既有 skill 参考)。
> 复现: 探针目录 `/tmp/rigprobe`(不在仓库内); 命令见 §六。

## 一、框架选型: rig 0.42.0(采纳)

| 项 | 实测结果 | 证据来源 |
|:--|:--|:--|
| 版本 | `rig` / `rig-core` / `rig-agent` / `rig-derive` 均 **0.42.0**(2026-08-17, MIT) | crates.io API |
| 工具链兼容 | rustc **1.96.1 stable**(项目 `rust-toolchain.toml` = stable) 下 **编译通过**;rig-core 为 edition 2024 | 探针 `cargo check` → `Finished dev profile in 54.95s`(冷, 单 crate) |
| Agent 编排 | `AgentBuilder::new(model){ preamble / tool / dynamic_tools / max_tokens / default_max_turns / add_hook / build }`;`agent.prompt(..).max_turns(n)`;`agent.stream_chat(prompt, &history)` | rig-agent `src/agent/builder.rs`、`src/agent/prompt_request/mod.rs`(`max_turns` 276/882, `usage()` 539) |
| 工具定义 | ① `impl Tool`(NAME / Args / Output / Error + `parameters()` JSON Schema + `async fn call`);② `DynamicTool::new(name, desc, schema, closure)` 运行时注册 | rig-core 例 `examples/agent_with_tools`(v0.42.0 tag) |
| **工具审批门** | `AgentHook::on_tool_call(&self, ctx, ToolCall) -> ToolCallAction`;`ToolCallAction::{Run, Rewrite(args), Skip(reason), Stop(reason)}`;**fail-closed 语义**, `Skip` 的原因回传给模型 | rig-agent `src/agent/hook.rs`(trait 1206 行起, `ToolCallAction` 1044);例 `agent_with_human_in_the_loop` / `agent_with_approval_policy` |
| 供应商 | 原生支持 DeepSeek / Moonshot / Z.ai(GLM) / MiniMax / Ollama / OpenRouter / Perplexity / OpenAI / Anthropic 等;另有通用 OpenAI 兼容通道(`Client::builder().api_key(..).base_url(..).build()`) | rig-core `src/lib.rs` §Model Providers;`src/providers/*.rs` |
| 遥测 | 依赖清单中**无上报/分析类 crate**;GenAI 语义约定属可选 OTel 集成, 不启用不外发 → 与宪法原则一"无遥测"一致 | `crates.io/api/v1/crates/rig-core/0.42.0/dependencies`(34 个 normal deps) |
| 依赖代价(实测) | rig 依赖树 **246 包** vs 等价基线(tokio+serde+serde_json+reqwest 同功能面)**164 包** → 新增 **~82 包**;`cargo tree -d` 显示 `reqwest v0.12.28` 与 `v0.13.5` **并存**(rig-core 依赖 ^0.13) | `/tmp/rigprobe` 与 `/tmp/ctlprobe` 的 `cargo tree` |

**通道口径**: 统一走 **OpenAI 兼容 Chat Completions** —— 必须显式 `openai::Client::builder().api_key(k).base_url(url).build()?.completions_api()`(rig 的 openai 客户端**默认走 Responses API**, 而兼容端点普遍不实现 `/responses`)。

**端到端真实验证(本机 Ollama, 零云端/零 key/零外发)**:

```
RICOW_AI_BASE_URL=http://127.0.0.1:11434/v1 RICOW_AI_API_KEY=ollama RICOW_AI_MODEL=qwen3:0.6b cargo run
[prompt] 回测结果：净盈亏 0.0，…     ← 模型主动调用 backtest 工具并使用其返回值(中文输入→中文回答)
[stream] (空)                        ← 流式路径可用(工具返回空策略列表)
```

> 说明: `qwen3:0.6b` 为 0.6B 小模型, 其质量**不代表**正式使用效果;该冒烟证明的是框架可用性与通道可行性(工具循环 + 流式 + 审批门 + 中文)。

## 二、备选方案与取舍

| 方案 | 评估 | 结论 |
|:--|:--|:--|
| 自建 reqwest 调 `/chat/completions` + 手写工具循环 | 依赖最轻, 但工具循环/流式/审批门/错误重试都要自研并长期维护 | 不选(用户点名 rig;且 rig 审批门正好对上写操作模型) |
| `async-openai` 0.42 | 仅 OpenAI 一家, 无编排 | 不选 |
| `genai` 0.6.5 | 多供应商 chat/stream/tools, 无编排/审批门 | 不选 |
| MCP server 让外部客户端驱动(见 §四) | P3 已有跑通实现 | ~~**采纳**(以 `ricow mcp` 子命令复活)~~ **取消(2026-09-15)**: 单机程序, 用户已有的 agent 直接调本机 CLI 即可 —— 见 019 spec §七 R1 |
| LLM 直接决策下单 | product §六 明确不做;不可回测不可审计 | 不选 |

## 三、rig 生态与"UI/中文"两个问题的实测回答

- **rig 无 UI**:`api.github.com/repos/0xPlaygrounds/rig/contents/crates` → 28 个 crate 全为后端/运行时/向量库/连接器,**无任何 UI/CLI/TUI**;生态里的 TUI 是第三方项目(rok-code / rok-tui 等)自建 → 界面必须 ricow 自己出。
- **中文**:rig 为编排层, 字符串按 JSON/UTF-8 透传, 不做字符集假设;实证见 §一(中文输入→中文回答)。真正的编码风险在 **Windows 控制台**(zh-CN 输入代码页默认 936 而 Rust `read_line` 期望 UTF-8)→ 处置: 明确报错 + `.cmd` 内 `chcp 65001` + 单次模式兜底(详见 spec FR-047 / plan 风险 R9)。项目 `unsafe_code = "forbid"`, **不能自行调用 `ReadConsoleW`**, 只能第三方 crate 或文档兜底。

## 四、"已有自己 AI agent"的接入标准(实测事实)

| 事实 | 证据来源 |
|:--|:--|
| `AGENTS.md` 已是事实标准:Linux Foundation(Agentic AI Foundation)托管, 6 万+ 仓库采用, **24+ 工具原生读取**(Codex / Cursor / Copilot / Gemini CLI / Aider / Zed / Windsurf / Jules / goose / opencode …) | agents.md 官网;OpenAI Codex 官方文档(发现顺序 `AGENTS.override.md` → `AGENTS.md` → 回退名, 32 KiB 上限) |
| **Claude Code 不读 `AGENTS.md`**, 读 `CLAUDE.md`;官方推荐 `@AGENTS.md` 导入或符号链接 | code.claude.com/docs/en/skills 与公开实践 |
| **Agent Skills(`SKILL.md`)是跨工具标准**(agentskills.io), Claude Code 与 **Codex 均已支持**;特点是**渐进式披露**(正文仅在被使用时加载) | code.claude.com/docs/en/skills(`.claude/skills/<name>/SKILL.md`);openai/codex `docs/skills.md` |
| MCP 是跨客户端标准:官方列出的支持方含 Claude / ChatGPT / VS Code Copilot / Cursor 等;Gemini CLI 走 `settings.json` 的 `mcpServers`(stdio/SSE/HTTP);Claude Code 用 `claude mcp add` | modelcontextprotocol.io/clients;gemini-cli `docs/tools/mcp-server.md`;code.claude.com/docs/en/mcp |
| **Codex 的 MCP 支持未在其官方 docs 目录见到专页**(仅 agents_md / skills / config 等) → 实现时逐个客户端核对, **不预先承诺** | openai/codex `docs/` 目录实测列举 |

## 五、P3 的 MCP 实现(可复活的既有资产)

- 历史: `crates/ricow_mcp`(rmcp **2.2.0**, 二进制 `ricow-mcp`), 暴露 tools = `create_strategy` / `execute_strategy`, resources = 策略 API 文档(`ricow://…`), **已按 stdio JSON-RPC 冒烟验证**(initialize → resources/list → tools/call → 用户侧 `ricow approve` → consume token → 落盘断言)。
- 删除: commit `4b4d961` "chore: 删除 ricow_mcp 残留目录(入口已移除)" —— 属 2026-08-24 D12 入口收敛的一部分。
- 复活口径(本次): 以 **`ricow mcp` 子命令**形式(不再单列 crate/binary → 用户只装一个二进制), 只暴露只读/虚拟工具;须**重新核对 rmcp 3.3.0 的 API**(跨大版本)。
- 已知陷阱: **MCP 协议的 stdout 是协议通道** → 该子命令的日志/提示必须走 **stderr**, 否则污染 JSON-RPC。
- 参考: skill `rmcp-mcp-server` 的 `references/ricow-p3-example.md`(资源实现范式、schemars 工具、参数 JSON→enum 必须显式按类型映射的坑、Python stdio 冒烟脚本)。

## 六、分发工具实测

| 事实 | 证据来源 |
|:--|:--|
| `cargo-dist` **0.32.0**(现名 `dist`)支持安装器: `shell`(curl\|sh)、`powershell`(irm\|iex)、`npm`(npx)、`homebrew`(formula, 发布到 tap 仓库)、`msi`(Windows 安装包, 依赖 WiX v3, GitHub CI 预装);并能**生成自己的发布 CI**(plan → build → host → publish → announce) | axodotdev 官方文档(installers 索引 / config 参考 / msi 指南) |
| 配套自我更新器 `axoupdater` 0.10.2 存在, 但**宪法原则一"无自动更新"要求显式关闭**他 | crates.io API + 宪法原则一 |
| winget / scoop **不在** cargo-dist 安装器列表 → 首版不做, 如实说明 | 同上(installers 列表) |
| 本机不可产出三平台产物: `x86_64-pc-windows-msvc` target 已安装但**无 MSVC 链接器**, macOS SDK 亦不可得 | `rustup target list --installed` + 008 spec §四 的历史结论(Windows 编译级验证不可复现) |
| 仓库分发现状(缺口证据): 顶层**无 `.github/`**、**无安装脚本/打包配置**;**README 无安装章节**(仅简介/竞品摘要/免责声明) | 仓库 `ls -a`、`grep README*.md` 实测 |
| 运行时**不读**仓库内非嵌入文件(`presets/`、`examples/` 均无引用) → 单二进制可直接分发;内置脚本为 `include_str!` 编译期嵌入 | `grep` 全 crate 源码实测 |

## 七、复现命令(摘要)

```bash
# 探针(仓库外)
cd /tmp/rigprobe && cargo check            # 编译 rig 0.42 探针(Tool + DynamicTool + AgentHook + 流式)
RICOW_AI_BASE_URL=http://127.0.0.1:11434/v1 RICOW_AI_API_KEY=ollama \
  RICOW_AI_MODEL=qwen3:0.6b cargo run      # 本地 Ollama 端到端(需先 ollama pull 一个支持工具调用的模型)
cargo tree --prefix none | sort -u | wc -l # 依赖包数(246 vs 基线 164)
cargo tree -d                              # 重复依赖(含 reqwest 0.12 / 0.13 双版本)

# 事实核查(只读)
curl -s https://crates.io/api/v1/crates/rig-core | ...        # 版本/时间/描述
curl -sL https://raw.githubusercontent.com/0xPlaygrounds/rig/v0.42.0/examples/<例>/src/main.rs
curl -sL https://code.claude.com/docs/en/skills.md            # Agent Skills 标准与目录约定
curl -sL https://modelcontextprotocol.io/clients.md           # MCP 客户端生态
```

## 八、未验证/留待实现期的事(不编造结论)

1. **Windows 真机**:中文交互输入与双击入口的实测结论(本次未在 Windows 真机验证;已列为 Phase 4.7 / SC-008 动作)。
2. **云端 provider 的工具调用质量**:需用户自己的 key 与目标模型复测(本地 0.6B 小模型不作证据)。
3. **rmcp 3.3.0 API 形态**:P3 用的是 2.2.0, 跨大版本需重新核对(Phase 7.3)。
4. **Codex 之外的客户端 MCP 支持矩阵**:逐个实测后再写进 README, 不预先承诺。
5. **编译时间/二进制体积增量**:实测后如实记录(禁止预判)。

> **环境变更记录 (2026-09-14)**: 本档案中的冒烟证据来自开发期临时拉取的本机 Ollama 模型
> `qwen3:0.6b`(522MB)。该模型**已在验证完成后删除**(`ollama rm qwen3:0.6b`); Ollama 服务为本机既有
> (2026-07-26 安装, 开机自启), 非本次新增。冒烟用例端点可由环境变量覆盖, 正式验证指向用户 provider。
