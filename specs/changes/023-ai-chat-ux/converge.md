# 023 收敛核验 (converge) — 2026-09-18

> 依据: `spec.md`(FR-001–FR-026 / SC-001–SC-010) + `plan.md`(D1–D14) + `tasks.md`(T001–T013) + `specs/constitution.md`。
> 方法: 逐条对照**当前代码与实测证据**; 不改 spec/plan; 新发现的缺口如实列入 §五。
> 范围: F0 双语 / F1 轮次可读性 / F2 去术语化 / F3 选项式交互 / F4 写操作全确认。
> 环境: Windows 10 · `cargo test -p ricow`(bin-only crate) · `RICOW_ROOT` 走临时目录。

## 一、结论

- **F0–F4 五项全部落地**; SC-001–SC-003 / SC-005–SC-007 / SC-009 / SC-010 达成, SC-004 与 SC-008 **按下列口径达成**(见各行说明, 不隐去落差)。
- 终端门禁 `commands/approve.rs` / `commands/ctrl.rs` **语义零改动**(仅补"对话渠道用口语词"注释), 与 plan D14 / FR-022 一致。
- 写操作唯一入口 = `request_write_confirmation`; `start_dry_run` / `stop_run` 两个"模型可直执行"工具已删除, 白名单收敛为 **只读 12 + 虚拟 3**。
- 本文件如实记录三处**字面与实测的落差**: ① SC-004 无同名测试函数; ② SC-010 grep 命中面比字面更宽(逐处定性); ③ 真 tty 双语手工冒烟与 `#[ignore]` 真机项本轮未跑。

## 二、FR 对照(FR-001–FR-026)

| FR | 状态 | 证据 / 说明 |
|---|---|---|
| FR-001 `[ui].lang` | ✅ | `commands/config_file.rs`: `UiSection{lang}` / `File.ui` / `UI_KEYS` / `load()` 取值校验硬失败 / `WRITABLE` 9→10 / `template_text()` 增段; 单测 `test_ui_lang_missing_is_none_and_invalid_value_fails`(L720)、`test_ui_lang_roundtrip_and_surgical_write`(L734)、`test_ui_section_appended_when_missing`(L747) |
| FR-002 向导首问语言 | ✅ | `commands/onboard.rs::ask_language()`(L113) 为 `run_if_needed` 第一步; `Gaps.lang`(L39) 并入 `any()`(L49), 故"只缺 lang"也进向导且只问这一句; 提示语单语呈现(`welcome_text(lang)` L133 / `provider_prompt(lang)` L171); 单测 `test_detect_gaps_lang_unset_is_gap_and_only_lang`(L608)、`test_welcome_text_follows_language_and_lists_presets`(L679)、`test_provider_prompt_has_default_and_follows_language`(L691) |
| FR-003 非交互不写 lang | ✅ | `onboard.rs` L189-193: `!stdin().is_terminal()` 时若 `gaps.only_lang()` → 既不写配置也静默放行(`Outcome::Skipped`); 运行时降级由 `i18n::resolve` 承担, 单测 `test_resolve_defaults_to_zh_when_unset` |
| FR-004 `i18n.rs` | ✅ | 新建 `crates/ricow/src/i18n.rs`: `Lang::{parse,code}` / `resolve(&File)` / `t(lang, zh, en)`; `main.rs` 登记 `mod i18n;`; 4 单测: `test_lang_parse_accepts_zh_en_and_rejects_others`(含 `"  EN "`/`"Zh"` 容错, 拒 `""`/`"fr"`/`"english"`/`"中"`)、`test_lang_code_round_trips`、`test_resolve_defaults_to_zh_when_unset`、`test_t_picks_by_language` |
| FR-005 `/lang` | ⚠ 部分 | 实现完整: `classify` L732-738(`/lang` → `LangCmd::{Show,Set,Bad}`)、`handle_lang`(L357) 写回 `[ui].lang` 并以**新语言**回执(非法值只提示不改)。**无直接单测**: `test_classify_input`(L1289) 未覆盖 `/lang` 与 `/history` 两条分支; 取值语义由 `i18n` 单测与 `config_file` 单测共同覆盖。缺口见 §五 F1 |
| FR-006 `system_preamble(lang)` | ✅ | `ai/prompt.rs::system_preamble(lang)`(L98): 开头即 `【会话语言: 中文】` / `【session language: English】`, 并明写"工具返回中文不是改用中文的理由"; 单测断言 `zh.starts_with("【会话语言: 中文】")`(L146) 锁"语言约束置顶" |
| FR-007 轮次分隔线 | ✅ | `ai/session.rs`: `reply()` 先 `self.turn += 1`(L600) → `sink.line(&turn_divider(...))`(L602) → 回答后补空行(L614 记 transcript); `turn_divider`(L784) 出 `── 第 N 轮 ──` / `── Turn N ──` |
| FR-008 `/history`(别名 `/log`) | ✅ | `LineInput::History` 分支(L206) → `render_history`(L337-350): 逐轮 `turn_divider` + `你:`/`助手:`; `clip_for_history`(L788) 超 `HISTORY_ENTRY_MAX_CHARS` **截断并标注**; 空历史给提示; **不计轮次、不写 transcript** |
| FR-009 斜杠命令/空行不计轮次 | ✅ | 轮次只在 `reply()` 递增(Ask 路径); `LineInput::{Exit,Help,History,Lang,Market,Keys,Unknown,Empty}` 全部早返回, 不进 `reply()` |
| FR-010 去术语化 | ✅ | 逐行 grep 定性(见 §三 SC-010): `session.rs` / `tools.rs` 的**用户可见文案**对 `ricow restart\|ricow stop\|ricow run ` **零命中**; `help_text`(L890) / `empty_state_hint`(L762) / `welcome`(L160) / `execute_confirmed`(L952) 回执全部改为"会发生什么"表述 |
| FR-011 `welcome()` 精简 | ✅ | `welcome()` 只留"数据目录 / 配置文件 / 供应商·模型 / 密钥来源"一行 + 边界说明; 已删"工具面(N 个)"与"每轮最多 N 次调用"罗列 |
| FR-012 `non_interactive_hint` 例外 | ✅ | `ai/tools.rs::non_interactive_hint`(L1744-1799) 保留终端命令(单次模式用户本就是命令行使用者), 头部注释明写"这是**唯一**允许出现终端命令的用户可见文案", 尾部补"想用对话方式操作, 直接运行 `ricow ai` 进交互模式"; 单测 `test_non_interactive_hint_gives_terminal_commands_only`(L2010) |
| FR-013 `Menu` / `MenuSlot` 单源 | ✅ | `ai/menu.rs`: `Menu{title, options}` / `MenuOption{label, request}` / `new_slot()`; `ToolCtx` 与 `ChatSession` 共享同一 `MenuSlot`(`session.rs` L80 / L123); 编号与文案由 `render`(L207) 单一来源产生 |
| FR-014 两套菜单 | ✅ | `strategy_ready`(L67) 5 项(回测/试跑/测试网/实盘/管理); `manage` 4 项(看状态/停止/改参数/删除); 单测 `test_strategy_ready_has_five_options_named_after_strategy`(L257)、`test_manage_has_four_options_named_after_strategy`(L268) |
| FR-015 菜单三个产生点 | ✅ | ① 对话内落盘部署成功后自动渲染(`session.rs` L408-430 写 `MenuSlot`); ② 模型经 `show_menu` → `tool_show_menu`(`tools.rs` L1807, 仅写槽); ③ 删除策略成功后清空(`session.rs` L333 `*self.active_menu.lock().await = None`); 生命周期只由"被选中"或"被新菜单替换"结束, 无 TTL |
| FR-016 序号解析与分支顺序 | ✅ | `parse_menu_choice`(L222) 正则 `^\s*([1-9][0-9]?)[).、]?\s*$`; `Ask` 分支序 = pending 确认优先 → 菜单序号 → 普通提问; 越界提示并保留菜单; 单测 `test_parse_menu_choice_accepts_number_with_optional_suffix`(L241)、`test_parse_menu_choice_rejects_non_numbers_and_zero`(L250)、`test_menu_choice_maps_to_the_same_option_the_user_saw`(L308) |
| FR-017 `VIRTUAL_TOOLS` = 3 | ✅ | `tools.rs` L85 `VIRTUAL_TOOLS: [&str; 3]` = `preview_strategy` / `request_write_confirmation` / `show_menu`; 漂移闸 `test_registry_count_and_write_tool_boundary`(L2040)、`test_registry_matches_whitelist_exactly`(L2056, 逐字比对 `build()` 注册名与白名单) |
| FR-018 菜单无写操作 | ✅ | `MenuOption.request` 是**一句自然语言请求**(见 L82-87 等), 不是终端命令、不是空 action; 写操作仍须 F4 确认; 单测 `test_menu_follows_language_and_has_no_terminal_commands`(L278) 断言菜单文案不含终端命令 |
| FR-019 `ActionKind` 13 变体 | ✅ | `ai/confirm.rs` L60-90: `Deploy`/`DeployReplace`/`UpdateParams`/`DeleteStrategy`/`StartDryRun`/`StopDryRun`/`StartDemo`/`StopDemo`/`AckRisk`/`StartLive`/`StopLive`/`CloseLive`/`RestartLive`; 单测 `test_action_kind_count_is_thirteen`(L367) |
| FR-020 删两个直执行工具 | ✅ | `test_allowed_is_exactly_the_registry`(L1884) 断言 `!is_allowed("start_dry_run")` / `!is_allowed("stop_run")`; `test_no_write_tool_names_are_allowed`(L1905) 用 14 个写实名/越权名 fail-closed |
| FR-021 只读动作不确认 | ✅ | `READ_ONLY_TOOLS: [&str; 12]`(L62) 直执行; `is_allowed` = `READ_ONLY_TOOLS ∪ VIRTUAL_TOOLS`; 依据 D4(`run_backtest` 不动资金/不落盘/不改配置) |
| FR-022 两套确认语义 | ✅ | 对话: `is_simple_confirmation` / `is_simple_rejection` / `classify_user_line(line, pending, lang)` — 只认当前语言, 刻意不含 `y`/`yes`/`ok`/空; 终端: `expected_phrase()` 保留, `approve.rs`/`ctrl.rs` 零改动; 单测 `test_classify_accepts_current_language_words_only`(L484)、`test_rejection_words_per_language`(L529)、`test_conversational_bare_confirm_is_the_only_confirmation`(L546)、`test_expected_phrases_match_terminal_gates`(L373) |
| FR-023 确认块文案 | ✅ | `request_write_confirmation` 的 13 类分支逐个写明"动作名 + 会发生什么 + 是否真实资金 + 是否不可逆"(如 L1451「后果(**真实资金**): …」/ L1144「真实向测试网下单…**不涉及真实资金**」); 回一句确认词或拒绝词、15 分钟有效由 `prompt.rs::GATES_GUIDE` L26-27 硬约束; 拒绝词表 `拒绝`/`取消`/`放弃` · `reject`/`cancel`/`abort` |
| FR-024 新写能力内核 | ✅ | `StartDryRun`/`StopDryRun` → `ctrl::start_daemon(..,false,false,true)` / `stop_daemon(..,false)` 并前置判实例模式; `UpdateParams` → `commands::update_strategy_params_in`(`.bak` 备份 + 原子写 + 逐键"改前 → 改后"; dry_run/demo 按原模式重启, live 不自动重启); `DeleteStrategy` → `commands::delete_strategy_files_in`(只删 `.toml`/`.lua`, 不删 `logs/`); `RestartLive` → stop → `live_preflight(accept_risk=false)` fail-closed → `start_daemon(live,confirmed)`; 单测 `test_update_strategy_params_keeps_other_fields_and_backs_up`(L810)、`test_update_strategy_params_rejects_bad_input_without_writing`(L848)、`test_delete_strategy_files_removes_toml_and_lua_only`(L874) |
| FR-025 不触碰 `live_enabled` | ✅ | 同一单测 L828 断言 `assert!(cfg.live_enabled, "live_enabled 不得被改参数顺手打开/关掉")`, 并断言 `enabled`/`market`/`position_mode` 与未提到的 `pair`/`script_path` 原样保留 |
| FR-026 `execute(&mut self)` + 菜单刷新 | ✅ | `session.rs::execute(&mut self, action)`(L635) → `execute_confirmed` → 之后按 `ActionKind` 写 `active_menu`(L644 `*self.active_menu.lock().await = next`); Reject 分支覆盖 13 变体全集 |

**FR 汇总**: 26 条中 **25 条 ✅ / 1 条 ⚠ 部分**(FR-005 实现完整但缺 `classify` 分支单测, 见 §五 F1)。

## 三、SC 对照(SC-001–SC-010)

| SC | 状态 | 证据 |
|---|---|---|
| SC-001 全绿零回归 + clippy 零警告 | ✅ | 本次实跑(见 §四): `FMT_EXIT=0` / `CLIPPY_EXIT=0` / `BUILD_EXIT=0` / `TEST_EXIT=0`; bin **166 tests → 165 passed / 0 failed / 1 ignored**, integration **12 tests → 3 passed / 0 failed / 9 ignored** |
| SC-002 漂移闸 = 只读 12 / 虚拟 3 | ✅ | `test_registry_count_and_write_tool_boundary`(L2040) 断言 `READ_ONLY_TOOLS.len()==12` 且 `VIRTUAL_TOOLS.len()==3`; `test_registry_matches_whitelist_exactly`(L2056) 用 `ToolCtx::new(root,true,confirm::new_slot(),menu::new_slot(),Lang::Zh)` 构造真实注册表, 逐字比对白名单; `test_no_write_tool_names_are_allowed`(L1905) 14 个写实名/越权名 fail-closed |
| SC-003 确认语义单测 | ✅ | `test_classify_accepts_current_language_words_only`(zh 放行 `确认`/`确定`/`同意`、`confirm` 不放行; en 反之; `y`/`yes`/`ok`/空一律不放行)、`test_rejection_words_per_language`、`test_conversational_bare_confirm_is_the_only_confirmation`; 终端侧 `test_expected_phrases_match_terminal_gates`(L373) 逐字断言 `确认部署 eth-grid-1` / `确认覆盖 eth-grid-1` / `确认启动测试网 …` / `确认停止测试网 …` / `确认实盘 …` / `确认停止实盘 …` / `确认平仓停止 …` / `确认风险`, 与 `approve.rs`/`ctrl.rs` 字面量一致; `test_terminal_phrases_are_unique_across_all_kinds`(L394)、`test_no_cross_action_phrase_unlocks_another_action`(L430) 证互不放行 |
| SC-004 13 个 action 全覆盖 | ⚠ **口径达成, 无同名测试** | spec 点名的 `test_every_write_action_requires_confirmation` **不存在同名函数**(全仓 grep 仅命中 spec 与工作稿)。实际覆盖由等价结构承担: `confirm.rs::tests::all_kinds(name)` 构造全部 13 类 `PendingAction`(`test_action_kind_count_is_thirteen` 断言 `len()==13`), 再经 `test_terminal_phrases_are_unique_across_all_kinds` / `test_labels_are_unique_in_both_languages` / `test_no_cross_action_phrase_unlocks_another_action` 证明"13 类各自需确认且互不串门"; 工具侧由 `test_no_write_tool_names_are_allowed` / `test_registry_count_and_write_tool_boundary` 证明"无任何直执行写路径"。**无 mock、无跳过**。落差如实记录, 见 §五 F2 |
| SC-005 菜单单测 | ✅ | `test_parse_menu_choice_accepts_number_with_optional_suffix`(接受 `1`/`2.`/`3)`/`4、`)、`test_parse_menu_choice_rejects_non_numbers_and_zero`(拒 `0`/`12`/`1a`/空)、`test_strategy_ready_has_five_options_named_after_strategy`、`test_manage_has_four_options_named_after_strategy`、`test_render_numbers_options_from_one`(L299)、`test_menu_choice_maps_to_the_same_option_the_user_saw`(L308)、`test_unknown_kind_or_blank_name_is_none`(L293)、`test_menu_follows_language_and_has_no_terminal_commands`(L278) |
| SC-006 双语单测 | ✅ | `i18n.rs` 4 单测(parse 容错与拒绝 / code round-trip / resolve 默认 zh / t 选词); `config_file.rs` 3 单测(缺失=None 且非法值硬失败 / 读写保注释的外科写回 / `[ui]` 段缺失时追加) |
| SC-007 确定性集成 A 组 | ✅ | `cargo test -p ricow` 里 integration **3 passed / 0 failed / 9 ignored**: `approve_requires_interactive_tty`(L119)、`run_demo_without_credentials_fails_fast`(L144)、`piped_confirm_phrases_never_reach_the_host`(L186, 管道喂 9 种短语全部不进宿主分支) |
| SC-008 真实 demo 启停(宪法三, 禁 mock) | ⚠ **内核闭环已验证, tty REPL 端到端未跑** | 见下方"SC-008 证据链与边界" |
| SC-009 文档同步 7 份 | ✅ | `SECURITY.md`(L25)、`specs/architecture.md`(L100 确认渠道注明 + L149 R5-023 修订记录)、`specs/product.md`(L57)、`specs/roadmap.md`(L128-130 立项与约束)、`README.md`(L93 对话单词 vs 终端逐字短语)、`README_zh.md`(L96)、`specs/constitution.md`(L69 安全要求段追加修订) —— 均说明"对话确认词为口语词, 终端仍为逐字长短语" |
| SC-010 grep 核对 | ⚠ **子条 (b) 达成; 子条 (a) 命中面比字面宽(逐处定性)** | 见下方"SC-010 grep 逐处定性" |

### SC-008 证据链与边界(如实)

**边界(先说清楚)**: SC-008 要求"**对话内** `start_demo` / `stop_demo` 各一次确认"。本轮 agent shell **非 tty**(`IsInputRedirected=True`), 本机无 pty 工具(`winpty` / 真 `python` / `node` 均不可用), 而 `commands/chat.rs` 对非终端 stdin 打印"非交互式输入: 已退出会话。"后 `return Ok(())` —— **对话内确认无法由 agent 侧驱动**, 这是 019-R2 起就有的 D5 tty 门禁(设计使然, 非缺陷)。

**取证方式**: 走**终端渠道**, 但用**与对话内 `start_demo`/`stop_demo` 完全相同的内核**:
- 对话内 `StartDemo` ⇒ `ctrl::start_daemon(root, name, live=false, demo=true, confirmed=false)`(见 `session.rs::execute_confirmed`)
- 对话内 `StopDemo` ⇒ `ctrl::stop_daemon(root, name, close_all=false)`
- 终端 `ricow start --demo` / `ricow stop` 走的是同一批 `commands::ctrl` 函数 —— 与 `constitution.md` "对话渠道不得比终端宽"一致。

**实测结果(真实币安 demo, `https://demo-api.binance.com`)**:

| 步骤 | 证据 |
|---|---|
| 启动 demo 实例(真实下单) | 交易所回报 `orderId=65828939573`, `status=NEW`, `symbol=BTCUSDT`, `price=78523.67000000`, `origQty=0.00100000` |
| 停机 | `ricow stop sc008ladder` → `exit=0`, 用时 2107ms |
| 引擎停机日志 | `cancelled=1 cancel_failed=0 residual_orders=0 residual_position=0.00000445`; `停机清理: 已撤挂单=1 撤单失败=0 平仓单=未执行 平仓失败=0 残留挂单=0 残留持仓=0.00000445` |
| 外部签名查询(独立核对) | `openOrders` **1 → 0**(下单后 1 笔在挂, 停机后归零) |
| 三向一致 | 引擎回执 / 停机日志 / 交易所查询 三者一致; 残留 0.00000445 为 dust(引擎如实提示人工核对) |

**口径备注**: 上述为 demo 侧真实下单/撤单闭环,**未以 mock 顶替**; 差的是"从 REPL 会话里触发"这一层, 归入 §六 未跑项(真 tty 人工项)。

### SC-010 grep 逐处定性

**子条 (b)** — `session.rs` / `tools.rs` **用户可见文案**零命中 `ricow restart|ricow stop|ricow run `:

| 命中位置 | 定性 |
|---|---|
| `ai/confirm.rs:17` | 模块文档注释(两渠道对照表) |
| `ai/prompt.rs:21` / `:46` | `GATES_GUIDE` 常量 —— 供 `read_doc("commands")` 与 agent-kit 手册按需取, **不是对话固定文案**(019-FR-060 已确立"不常驻") |
| `ai/tools.rs:1662` | 代码注释(说明与终端同内核) |
| `ai/tools.rs:1777/1780/1783/1785/1794/1795/1796` | `non_interactive_hint` 内部 —— **FR-012 明文唯一例外** |
| `ai/session.rs:1224` | 代码注释 |

结论: `session.rs` / `tools.rs` 的**用户可见文案**零命中 ✅(命中项全部为注释 / 按需文档常量 / 明文例外)。

**子条 (a)** — `确认部署|确认启动测试网|确认风险|确认实盘|确认停止测试网|确认停止实盘|确认平仓停止|确认覆盖` 实际命中 **24 个文件**, 多于字面"仅 `approve.rs`/`ctrl.rs` + SDD 档案"。逐类定性:

| 类别 | 文件 | 定性 |
|---|---|---|
| 终端门禁(字面预期) | `commands/approve.rs`、`commands/ctrl.rs` | ✅ 逐字长短语的**权威字面量**, FR-022 要求保留 |
| 短语构造器 | `ai/confirm.rs`(`expected_phrase()` L215-229 + L373-475 断言) | 终端渠道短语的**单一构造源**, 供终端门禁与"互不放行"单测使用, 非对话文案 |
| 按需文档常量 | `ai/prompt.rs`(`GATES_GUIDE` L28/40-42 + L187-188 测试断言) | 供 `read_doc` / agent-kit 手册, 非对话固定文案 |
| 测试断言 | `ai/tools.rs`(L2148/2154/2253/2260/2267/2276/2363)、`tests/ai_live_smoke.rs` | 断言"终端短语仍唯一且与门禁一致", 非用户可见文案 |
| 回执子串误命中 | `ai/session.rs:1175` | 文案是 `已确认风险: … (一次确认长期有效…)`, 命中的是**回执里的动词短语**, 不是要求用户敲终端命令 |
| 注释 | `ai/session.rs:1190` | 代码注释 |
| 终端命令实现 | `commands/deploy.rs`、`commands/run.rs` | CLI 侧文档/提示(标题为终端命令), 不属对话文案 |
| 文档说明 | `README.md`、`README_zh.md`、`SECURITY.md` | **SC-009 要求同步**的说明文字(正是"终端仍逐字、对话用口语"的对照), 必然含短语 |
| SDD/历史档案 | `specs/{architecture,product,roadmap,constitution}.md`、`specs/changes/019-*`、`specs/research/competitors.md`、`.trae/documents/*` | 规格/历史档案, 字面预期内 |

结论: 子条 (a) 的**意图**(对话内不出现"请逐字输入终端短语"的指引)满足; 字面"仅命中 2 个源文件"**不成立** —— 因短语构造器、测试断言、按需文档与 SC-009 要求的文档对照天然含这些字符串。**这是口径落差, 不是残留**, 如实记录。

## 四、门禁与实测基线(本次实跑, 2026-09-18 Windows)

```
cargo fmt --all -- --check                                → FMT_EXIT=0
cargo clippy -p ricow --all-targets -- -D warnings        → CLIPPY_EXIT=0
cargo build -p ricow                                      → BUILD_EXIT=0
cargo test -p ricow                                       → TEST_EXIT=0
```

`test.log`:
```
running 166 tests
test result: ok. 165 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
running 12 tests
test result: ok. 3 passed; 0 failed; 9 ignored; 0 measured; 0 filtered out
```

- bin 单测 = `crates/ricow/src/**`(bin-only crate, `cargo test -p ricow --bin ricow`); integration = `tests/ai_live_smoke.rs`。
- 本轮为达成 clippy 零警告, 修复 5 处(`needless_borrow` / `useless_format` / `question_mark` / `unused_variables` / `bool_comparison`), 随后 `cargo fmt --all` 并**重跑整条门禁链**取得上表这一份一致记录。

## 五、发现清单

| ID | 类型 | 严重度 | 来源 | 证据 | 处置 |
|---|---|---|---|---|---|
| F1 | partial | LOW | FR-005 / FR-007 / FR-008 / FR-009 | `session.rs::tests` 仅 8 条, 未覆盖 `classify("/history")`、`classify("/lang en")`、`render_history` 截断标注、`turn_divider` 文案与"斜杠命令不计轮次" | 记为**后续单测补齐项**(非阻塞); 本轮不动代码 |
| F2 | contradicts | LOW | SC-004 vs 测试命名 | spec 点名的 `test_every_write_action_requires_confirmation` 不存在; 覆盖由 `all_kinds` 13 类 + 唯一性/互斥性 + 工具白名单结构测试承担 | 如实记录口径(见 §三 SC-004); 建议后续把该名收敛到一处或修订 SC 表述 |
| F3 | partial | LOW | SC-010 子条 (a) | grep 命中 24 文件, 与"仅 approve/ctrl + SDD"字面不符(短语构造器/测试/按需文档/SC-009 文档必然含) | 如实记录逐处定性(见 §三 SC-010); 无需改代码 |
| F4 | missing | LOW | FR-013/FR-017 | `tool_show_menu` 本身**无行为单测**(只被"白名单逐字一致"漂移闸间接覆盖) | 记为后续单测补齐项 |
| F5 | defect(已修复) | **HIGH** | FR-024(`StartDryRun`) / `ricow_strategy::DryRunContext` | Dry Run 下**虚拟持仓 side 记账错误**: 平仓归零时 `size=0` 但 `side` 停留为**平仓方向**, 随后**按原持仓方向再次开仓**(新单方向与残留 `side` 相反)走 `else`(非平仓)分支只累加 `size`、**不补 `side`** → `(side=Sell, size>0)` 的伪空头且 `size` 单调放大; 消费者按 `pos.side` 报 `short`。**缺陷画像收窄(024 红验证实测)**: 平仓/反手分支本就带 `entry.side` 赋值, 未修复也正确 —— 缺陷只在 `else` 分支暴露(实测 3 红 4 绿)。活体证据见 §七 | **已修复**: 单开变更 [024-dryrun-position-side](../024-dryrun-position-side/spec.md) —— 抽纯函数 `apply_fill_to_net_position` 并在两个 `else` 分支补 `entry.side`([context.rs:267-330](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/context.rs#L267-L330)) + 抽 `position_side_label`([lua.rs:290-302](file:///d:/sunhuazhu/ricow/crates/ricow_strategy/src/lua.rs#L290-L302)) + 8 条单测(红→绿), 对照证据见 §七.1 |

**汇总**: 检查 26 条 FR / 10 条 SC / 14 条 plan 决策(D1–D14) / 宪法 MUST 原则。
- D1–D14 全部落地; 其中 D14(`approve.rs`/`ctrl.rs` 一行不改)由 `git diff` 结论 = 仅注释佐证。
- 宪法测试纪律: 真实调用不打桩 ✅(demo 真实下单撤单) / 提交纪律 ✅(本轮未提交, 待用户明确指示) / unsafe forbid → **未新增**。
- 未跟踪缺口 **0 项**; F1–F4 为**已跟踪范围内的口径/补测项**; **F5 为 2026-09-19 复测新发现的引擎缺陷, 已由变更 [024-dryrun-position-side](../024-dryrun-position-side/spec.md) 修复**(修复后对照证据见 §七 末)。

## 六、遗留与风险(如实)

**未跑(如实, 不以 mock 顶替)**:

1. **真 tty 双语手工冒烟**(plan 动作 4: 语言选择 / 分隔线 / 菜单 / 确认词只认当前语言 / `/history` / `/lang` 即时生效) —— agent shell 非 tty, 且本机无 pty 工具; 留作人工项。
2. **对话内确认的真机端到端**(从 REPL 里触发 `start_demo`/`stop_demo` 各一次确认) —— 同上 tty 约束; 内核闭环已由终端渠道同内核取证(§三 SC-008)。
3. **`#[ignore]` 真机场景 S1–S13**(`tests/ai_live_smoke.rs`) —— 需 `RICOW_AI_API_KEY` 与公网; 具备条件时按 `cargo test -p ricow --test ai_live_smoke -- --ignored --test-threads=1 --nocapture` 执行。

**风险(仍有效, 已由本次证据收敛)**:

- R1(口语确认词削弱防误触): 由"单槽 pending + 15 分钟 TTL + 过期优先 + tty 门禁 + 只认当前语言 + 不含 `y`/`yes`/`ok`/空 + 确认块写明后果"共同兜底 —— 其中前五项有单测, 最后一项由 `GATES_GUIDE` 硬约束(模型侧, 非机械校验)。
- R2(菜单序号误触): 菜单内无任何一项直接产生写操作(`MenuOption.request` 是自然语言句子), 最坏后果 = 多走一步确认。
- R5(白名单与 `build()` 漂移): `test_registry_matches_whitelist_exactly` 是漂移闸, 两处必须同时改。

**门禁基线(本次实跑)**: bin **165 passed / 0 failed / 1 ignored**(166 collected), integration **3 passed / 0 failed / 9 ignored**(12 collected); `cargo fmt --all -- --check` 0 差异; `cargo clippy -p ricow --all-targets -- -D warnings` 0; `cargo build -p ricow` 0 警告。

## 七、2026-09-19 复测追加(当前 debug 构建, 隔离沙箱 `%TEMP%\ricow-e2e`)

对上文 §二/§三/§四 的关键结论做**独立复跑**, 结论一致; 并在 Dry Run 实测中新发现 F5。

**复跑基线(与 §四 一致)**: `cargo test -p ricow` → `TEST_EXIT=0`(bin 165 passed / 0 failed / 1 ignored; integration 3 passed / 0 failed / 9 ignored); `cargo clippy -p ricow --all-targets -- -D warnings` → `CLIPPY_EXIT=0`; `cargo fmt --all -- --check` → `FMT_EXIT=0`; `cargo build` 产物 = `target\debug\ricow.exe`(2026-09-19)。

**已复测通过的功能链路**(均用当前构建, 真实行情/真实成交, 无 mock):

| 链路 | 证据 |
|---|---|
| 生成(create) | `ricow create --name e2e_create --pair ETHUSDT --script e2e_ema.lua --days 30 --interval 1h` → `CREATE_EXIT=0`; 编译门禁 ✓ / 沙箱回测 ✓(720 根 1h K 线, 35 笔成交, 胜率 41.18%, 拒单 0) / 输出 `preview_id` 且**未写任何策略文件** |
| 生成(approve/deploy 负向) | `approve <id>` → `EXIT=1`「确认必须在交互终端输入」(019-R2 tty 门禁, **设计**); `deploy <id> --token bogus` → `EXIT=1`「preview 状态不是 approved」; `deploy 不存在 --token x` → `EXIT=1`; strategies 目录**零新增**(写门禁成立)。正向路径由引擎测试佐证: `cargo test -p ricow_engine --lib execute_strategy`(4 passed, 含"未批准不得落盘"/"token 一次性")、`--lib confirm`(5 passed)、`cargo test -p ricow --bin ricow r3s5`(4 passed) |
| 回测 | ETHUSDT 90 天 1h(2160 根): `shannon_grid` 42 笔 / 净盈亏 +2662.58 / 胜率 100% / 拒单 0; `dca` 2160 笔 / 净盈亏 -43.85; 已部署策略名 `e2e_ob` 720 笔 / 净盈亏 -9.1123 / 最大回撤 0.09%; 三份均含风险·收益与资产变化段 |
| Dry Run | `ricow start e2e_ob` → `EXIT=0`(模式: Dry Run); 35s 后 `status`/`info` 显示运行中、成交 542/578 笔、手续费 14.06 |
| demo 真实运行 | 真实币安 demo(`demo-api.binance.com`)回报 `orderId=65828939573 status=NEW BTCUSDT 0.001`; `stop` → `exit=0`; 停机日志 `cancelled=1 cancel_failed=0 residual_orders=0`; 外部 `openOrders` 1 → 0(**三向一致**) |
| 实盘门禁 | 018 `--accept-risk` / 002 `min_dry_run_hours` / FR-008 时钟偏移 三条链路按序 fail-closed,**拒绝而非降级** |
| 管理策略 | `list`/`status`/`info`/`fills`/`logs` ✓; `restart e2e_ob` → `EXIT=0`; `stop e2e_ob` → `EXIT=0`; `db stats`(K 线 2000 / 成交 629)、`db sync ETHUSDT` → `EXIT=0`、`db export ETHUSDT --interval 1h` → `EXIT=0`; `daemon status` 运行中且托管 0/3 |

**F5 活体证据(Dry Run 虚拟持仓记账)**:

- 策略诊断行(每 30 tick): `side=short` **恒真** 且 `size` 单调增长 `0.27 → 1.37`; `usdt=99999.9968` 冻结; `eth=0.0`。
- `ricow fills e2e_ob` 首 12 行**全部为 sell**(@2598.49 等), 无一笔平仓性质的成交。
- 日志侧正常: `strategy.dryrun: dry run order placed ... side=Buy ... status=Filled` 与 `lua_strategy: e2e_ob: 成交 ... buy 0.01` 说明**撮合与回灌无误, 错在记账**。
- 静态根因见 §五 F5; `BacktestContext`(`backtest.rs::close_position`)多空分离正确, 该缺陷**仅存在于 `DryRunContext`**; `context.rs` 现有单测仅覆盖净仓聚合与 side 标签, **未覆盖本路径**。
- 影响面: 任何以 `ctx:position_size` / `ctx:position_side` 判仓的 Lua 策略, 在 Dry Run 下会**永不平仓且仓位单调放大** → FR-024 `StartDryRun`("试跑") 的实证价值失效; 不涉真实资金, 但会误导用户对策略的判断。

**处置**: 按宪法 line 58(所有变更走 SDD)+ line 39(未确认不修改文件), 本次复测**未改动任何代码**; F5 当时记为待修缺口, 建议单开变更(修复 + 纯逻辑单测, 覆盖"平仓后同向再开仓 side 必须归位")。**2026-09-19 经用户确认后已单开变更 [024-dryrun-position-side](../024-dryrun-position-side/spec.md) 修复**(本段保留为修复前基线)。

### 七.1 F5 修复后对照证据(变更 024, 同夹具 `e2e_ob` / 同沙箱 `%TEMP%\ricow-e2e`)

| 观测项 | 修复前(上文 §七) | 修复后(024) |
|---|---|---|
| `ricow fills e2e_ob` | 首 12 行**全部为 sell** | 首 12 行 **buy/sell 交替**(buy 2604.55×0.27 / sell 2604.59×0.27 / buy 2604.60×0.27 …) |
| 策略诊断 `side` | `short` **恒真** | `none` ↔ `long` **交替**(各 118 次), 平仓归零后按原持仓方向再开不再残留伪方向 |
| 策略诊断 `size` | 单调放大 `0.27 → 1.37` | 恒 `0.0` / `0.27` 二值, **无放大** |
| 虚拟余额 `usdt` | `99999.9968` 冻结 | `99999.86`(空仓)↔ `99296.51`(持仓)摆动, 手续费正常递减 |
| 回测零变化(SC-004) | `shannon_grid` 42 笔 / +2662.5808 / 胜率 100%; `dca` 2160 笔 / -43.8504 | **逐项一致, 零变化** |
| 单测门禁(SC-001) | 修复前红: `cargo test -p ricow_strategy` 3 failed / 6 passed | 修复后绿: **149 passed / 0 failed**; `clippy`/`fmt`/`ricow` 测试全 EXIT=0 |

详见 [024-dryrun-position-side/converge.md](../024-dryrun-position-side/converge.md)。
