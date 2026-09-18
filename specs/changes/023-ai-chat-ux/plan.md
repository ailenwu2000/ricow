# 023 实施计划

## 一、决策

| # | 决策 | 理由 |
|:--|:--|:--|
| D1 | 确认面 = **所有写操作**, 但确认词分渠道: 对话用口语词, 终端保留逐字长短语 | 用户明确两条要求; 真正的风险源是模型擅自动作, 而非用户误回一个字 |
| D2 | 确认词随语言且**只认当前语言**(en 下「确认」不放行) | 避免双语混杂削弱门槛; 且可测 |
| D3 | 确认词刻意不含 `y`/`yes`/`ok`/空 | 保留"防肌肉记忆误触"的最低门槛, 与既有 `is_explicit_confirmation` 纪律一致 |
| D4 | 回测与预览**不算**写操作 | `tool_run_backtest` 自述"不动资金、不落盘、不改配置"(已核实); 否则"所有写操作"会被无限扩大到查询类, 与"直接运行"要求冲突 |
| D5 | 语言持久化进 `ricow.toml` 的 `[ui].lang`, 不进新文件 | 用户要求"记得住"; 代价是同步 `File`/`UI_KEYS`/`WRITABLE`/`template_text`/`load()` 两处报错文案 |
| D6 | `lang` 缺失不静默假定为中文, 而是触发首次询问 | 用户要求"一开始让用户选择" |
| D7 | 非交互式环境不写 `lang`, 运行时降级默认 `zh` | 管道/CI 无人可选, 也不该改写用户配置 |
| D8 | 菜单编号与文案由宿主单一来源渲染, 模型只能经 `show_menu` 请求 | 避免"模型自造编号 + 宿主序号映射"错位 |
| D9 | 菜单不设 TTL、不做多菜单栈/历史 | 防过度设计; 陈旧菜单最坏后果仅是"多走一步确认" |
| D10 | `UpdateParams` 写盘与条件重启共用**同一个确认** | 用户确认的是"改参数"这件事; 但 **live 实例不在此确认内重启**, 另起 `RestartLive`, 避免"一次确认换来实盘换血" |
| D11 | `DeleteStrategy` 只删 `.toml`/`.lua`, 不删 `logs/` | 保留追溯证据 |
| D12 | 删除 `start_dry_run`/`stop_run` 两个直执行工具 | 写操作收敛到唯一入口, 模型无法绕开确认 |
| D13 | 不引 i18n 框架, 就地双语字面量 | 防过度设计; 需占位符的文案由调用点 `format!` 组装 |
| D14 | 终端门禁(`approve.rs`/`ctrl.rs`)一行不改, 仅补注释 | 用户明确"终端保留长短语" |

## 二、改动清单

| 文件 | 改动 |
|:--|:--|
| `crates/ricow/src/i18n.rs` | **新建**: `Lang` / `parse` / `code` / `resolve(&File)` / `t(lang, zh, en)` + 2 单测 |
| `crates/ricow/src/main.rs` | 登记 `mod i18n;` |
| `crates/ricow/src/commands/config_file.rs` | `UiSection{lang}` + `File.ui`; `UI_KEYS`; `load()` 增 `"ui"` 分支(取值校验硬失败); 两处报错文案加 `[ui]`; `WRITABLE` 9→10; `template_text()` 增段 |
| `crates/ricow/src/commands/mod.rs` | `require_simple_confirmation(context, lang)`; `update_strategy_params_in`; `delete_strategy_files_in`; `require_interactive_terminal` 提示语跟随 `Lang` |
| `crates/ricow/src/ai/confirm.rs` | `ActionKind` 7→13; `label()` 双语; 13 个构造器; `PendingAction.params`; `is_simple_confirmation`/`is_simple_rejection`; `classify_user_line(line, pending, lang)`; **保留 `expected_phrase()`** 并重写文档为"两套语义" |
| `crates/ricow/src/ai/tools.rs` | `VIRTUAL_TOOLS` 4→3; `ToolCtx.menu: MenuSlot`; 新增 `tool_show_menu`; `prepare_*` 增删改; 删两个直执行工具; 文案双语化; `non_interactive_hint` 补对话入口提示 |
| `crates/ricow/src/ai/session.rs` | `lang`/`turn`/`transcript`/`active_menu` 字段; `reply()` 分隔线 + transcript + 菜单渲染; `LineInput::{History,Lang}`; `Ask` 分支插入序号解析; `execute()` 改 `&mut self`; 重写 `help_text`/`empty_state_hint`/`welcome`; `execute_confirmed` 增 5 分支并去终端命令 |
| `crates/ricow/src/ai/prompt.rs` | `system_preamble(lang)`; `RULES` 增语言硬约束与表述纪律强化; `GATES_GUIDE` 短语段替换; `TRAPS_GUIDE` 补两条; 保持 slim(<3000 字节断言不变) |
| `crates/ricow/src/commands/onboard.rs` | `Gaps.lang` 并入 `any()`; `run_if_needed` 最前插入语言选择; 提示语跟随语言; `welcome_text(lang)` |
| `crates/ricow/src/commands/chat.rs` | 提示符/退出语/欢迎流程按语言呈现; `StdioSink` 不改 |
| `crates/ricow/src/commands/approve.rs` / `ctrl.rs` | **零改动**, 仅补"对话渠道用口语确认词, 见 `ai/confirm.rs`"注释 |
| 文档 | `SECURITY.md` / `architecture.md` / `product.md` / `roadmap.md` / `README.md` / `README_zh.md` / `constitution.md` |

## 三、风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 口语确认词削弱防误触 | 单槽 pending + 15 分钟 TTL + 过期优先 + tty 门禁 + 只认当前语言 + 不含 `y`/`yes`/`ok`/空 + 确认块必须写明后果 |
| R2 | 菜单序号误触 | 仅纯数字行命中; 菜单内无任何一项直接产生写操作, 最坏后果 = 多走一步确认 |
| R3 | `lang` 缺失导致老用户被"打扰" | 仅一次性; 非交互环境不写且降级默认 `zh` |
| R4 | 策略 TOML 重写丢注释 | 写前留 `.toml.<时间戳>.bak`; 平台生成的 TOML 本无用户注释 |
| R5 | 白名单与 `build()` 漂移 | `test_registry_matches_whitelist_exactly` 是漂移闸, 两处必须同时改 |
| R6 | `[ui]` 段被老版本配置判为"未知段" | 未知段报错文案同步列出 `[ui]`; 该报错本身就是硬失败, 不会静默 |

## 四、验收动作

1. `cargo test -p ricow` + `cargo clippy -p ricow` 零警告。
2. `cargo test -p ricow --test ai_live_smoke`(A 组确定性用例全绿)。
3. 真实币安 demo 走一次对话内 `start_demo` / `stop_demo`(各一次确认), 核对订单可撤、回执与实际一致。
4. 中文与英文各走一遍手工冒烟(语言选择 / 分隔线 / 菜单 / 确认词只认当前语言 / `/history` / `/lang` 即时生效)。
5. grep 核对用户可见文案无终端命令残留。
6. 文档同步 + `converge.md`。
