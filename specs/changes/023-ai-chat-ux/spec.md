# 023 功能规格: AI 对话体验重构(双语 · 轮次可读性 · 去术语化 · 选项式交互 · 写操作全确认)

**状态**: 已实现待验收 (2026-09-18) | **依据**: 用户三轮评审结论 + `specs/product.md`「易于部署使用」原则 + `specs/constitution.md` 安全条约 | **收敛核验**: 见 [`converge.md`](./converge.md)

## 一、为什么

`019-ai-assistant` 落地后, 对话入口能用了, 但按**非技术用户**标准复审仍有四个硬伤:

1. **轮次不可读**: `reply()` 输出与下一轮提示符紧贴, 用户看不出"上一句是谁说的"; `history` 只喂模型, 从不给用户看, 也没有回看命令。
2. **表述术语化**: `help_text()` / `empty_state_hint()` / 各动作回执都在教用户敲 `ricow run` / `ricow stop` / "逐字输入确认短语", 违反"适合对电脑技术不熟悉的用户"这一根本原则。
3. **无选项式交互**: 策略就绪后, 用户必须自己想清楚"下一步能做什么、怎么说"; 违反用户要求的"AI 只告诉用户怎么选、选哪一项, 用户说序号或原话即可"。
4. **无语言选择**: 只服务中文用户; 而用户可能是英文使用者, 且要求"一开始就让用户选, 选了英文则所有对话交互呈英文"。

同时, 二轮评审追加两条硬约束: **所有写操作都要一次确认**(不只是实盘), 但**终端 CLI 的逐字长短语必须保留**。

## 二、范围

**做**:
- 新增 `[ui].lang` 配置(写入 `ricow.toml`), 首次向导先问语言, 对话内 `/lang` 可改; 语言持久化并立即生效。
- 对话宿主固定文案按语言呈现; AI 回复语言由提示词硬约束跟随会话语言。
- 每轮加分隔线与轮次号; 新增 `/history` 回看本会话往返。
- 面向用户文案去术语化: 不再要求用户敲任何终端命令。
- 宿主持有菜单状态, 渲染"策略就绪"与"管理策略"两套菜单; 用户可用序号或原话选择。
- `ActionKind` 由 7 变体扩为 **13 变体**, 覆盖**全部写操作**(落盘/覆盖/改参数/删除/启停 dry_run 与 demo/实盘全部动作)。
- 删除 `start_dry_run` / `stop_run` 两个"模型可直接执行"的 L1 工具, 写操作收敛到唯一入口 `request_write_confirmation`。
- 对话渠道确认词 = 当前语言的**单个口语词**; 终端渠道 = **逐字长短语(一行不改)**。

**不做**:
- 不引入 i18n 框架依赖(无 Fluent/gettext、无资源文件、无构建期生成), 仅就地双语字面量。
- 不改 `PENDING_TTL`(15 分钟)、不改 pending 单槽模型、不改 tty 门禁、不改"过期优先"。
- 不改 `approve.rs` / `ctrl.rs` 的任何确认短语与门禁逻辑。
- 不做菜单 TTL / 多菜单栈 / 菜单历史; 不做 `/history` 跨会话持久化。
- 不改 `ricow ai "..."` 单次模式为对话模式。
- 不新增回测历史库等未被要求的功能。

## 三、功能需求

### 双语(F0)

- **FR-001**: `ricow.toml` 新增 `[ui]` 段, 键 `lang` ∈ {`zh`,`en`}; 缺失 = "尚未选择"(不静默假定), 非法值 → 硬失败, 报错风格与 `[market].show_all_pairs` 一致。
- **FR-002**: 首次向导 `run_if_needed` **最前**插入语言选择一问(`请选择界面语言 / Choose your language: 1. 中文 2. English`); 选定后向导后续提示语按该语言呈现(不再中英叠排); 即使无其它缺口, 也因 `lang` 缺口进入向导只问这一句。
- **FR-003**: 非交互式 stdin(管道/CI)不写 `lang`, 运行时按默认 `zh` 处理。
- **FR-004**: 新增 `crates/ricow/src/i18n.rs`, 作为宿主固定文案唯一来源: `Lang::{parse,code}` / `resolve(&File)` / `t(lang, zh, en)`。
- **FR-005**: 新增 `/lang` 命令: 无参 → 显示当前语言 + 双语选项; `/lang zh|en` → 写回 `[ui].lang` 并**立即生效**(回执用新语言); 非法值 → 提示可选值, 不改。
- **FR-006**: `system_preamble(lang)` 增参数, 提示词硬约束"始终用会话语言回答, 即使工具返回中文"。

### 轮次可读性(F1)

- **FR-007**: `reply()` 开头递增 `turn`、输出空行 + 分隔线(`── 第 N 轮 ──` / `── Turn N ──`), 回答后补空行; 同时记录 `transcript`。
- **FR-008**: 新增 `/history`(别名 `/log`): 渲染本会话全部往返(轮次分隔 + `你:`/`助手:` 或 `You:`/`AI:`); 单条超 2000 字符截断并标注; 空历史给提示; 该命令不计轮次、不写 transcript。
- **FR-009**: 斜杠命令与空行不计入轮次。

### 去术语化(F2)

- **FR-010**: 面向用户文案只讲"会发生什么", 不出现"敲什么命令"; `help_text()` / `empty_state_hint()` / `welcome()` / `execute_confirmed()` 回执 / `tools.rs` 用户可见文案一律清除 `ricow xxx` 指引。
- **FR-011**: `welcome()` 精简: 只留供应商/模型/密钥来源一行 + 一句边界说明; 删除"工具面(N 个)"与"每轮最多 N 次调用"的罗列。
- **FR-012**: `non_interactive_hint` 例外保留终端命令(单次模式用户本身就是命令行使用者), 仅补一句"想用对话方式操作直接运行 `ricow ai`"。

### 选项式交互(F3)

- **FR-013**: 宿主渲染菜单: `Menu { title, options: Vec<MenuOption{label, request}> }`; `ToolCtx` 与 `ChatSession` 共享 `MenuSlot`; **编号与文案由宿主单一来源产生**。
- **FR-014**: 菜单 A「策略就绪」5 项(回测 / 试跑 / 测试网 / 实盘 / 管理策略); 菜单 B「管理策略」4 项(看状态 / 停止 / 改参数 / 删除)。
- **FR-015**: 产生菜单三个点: ① 对话内落盘部署成功后自动渲染菜单 A; ② 模型调用 `show_menu(kind,name)` 时宿主渲染; ③ 删除策略成功后清空菜单。菜单生命周期仅由"被选中"或"被新菜单替换"结束, 不设 TTL。
- **FR-016**: 序号解析 `parse_menu_choice`(`^\s*([1-9][0-9]?)[).、]?\s*$`); `Ask` 分支顺序: **pending 确认优先** → 菜单序号 → 普通提问(保留菜单); 越界提示 `请选 1..N` 且保留菜单。
- **FR-017**: `VIRTUAL_TOOLS` 收敛为 3: `preview_strategy` / `request_write_confirmation` / `show_menu`; 新增 `show_menu` 工具(仅写 `MenuSlot`, 无盘面副作用, 返回值不罗列选项)。
- **FR-018**: 菜单内**无任何一项直接产生写操作** —— 序号只转成一句自然语言请求, 写操作仍需 F4 确认; 误触最坏后果是"多走一步确认"。

### 写操作全确认(F4)

- **FR-019**: **写操作 = 会改变磁盘内容或进程运行态的动作**。`ActionKind` 13 变体: `Deploy` / `DeployReplace` / `UpdateParams` / `DeleteStrategy` / `StartDryRun` / `StopDryRun` / `StartDemo` / `StopDemo` / `AckRisk` / `StartLive` / `StopLive` / `CloseLive` / `RestartLive`。
- **FR-020**: 删除 `start_dry_run` / `stop_run` 直执行工具, 其能力改由 `request_write_confirmation` 承载 —— 写操作只有一个入口, 模型无法绕开确认。
- **FR-021**: 只读动作(回测 / 预览 / 全部查询)不需确认, 直接执行(依据: `run_backtest` 自述"不动资金、不落盘、不改配置", 已核实)。
- **FR-022**: 两套确认语义: **对话** = 当前语言口语词(`确认`/`确定`/`同意` | `confirm`/`confirmed`), **只认当前语言**、刻意不含 `y`/`yes`/`ok`/空; **终端** = 逐字长短语, `expected_phrase()` 保留, `approve.rs`/`ctrl.rs` 零改动。
- **FR-023**: 确认块文案必须写明: 动作名(含策略名) / 会发生什么 / (若涉资)真实资金 / (若不可逆)不可撤销 / 回一句确认词或拒绝词 / 15 分钟有效且只能本人回。拒绝词: `拒绝`/`取消`/`放弃` | `reject`/`cancel`/`abort`。
- **FR-024**: 新增写能力宿主内核:
  - `StartDryRun`/`StopDryRun`: `ctrl::start_daemon(..,false,false,true)` / `stop_daemon(..,false)`; 停止前判实例模式, 不匹配则拒绝并指向对应动作。
  - `UpdateParams`: 已部署 + `params` 非空 → `backtest::parse_param` 解析 → 读配置 → 合并 `[strategy.params]` → 时间戳 `.bak` 备份 → 原子写 → 回执逐键"改前 → 改后"; **dry_run/demo 按原模式自动重启**(`restart_flags` 语义, 不静默降级); **live 不自动重启**, 需另起 `RestartLive`。
  - `DeleteStrategy`: 运行中先 `stop_daemon(close_all=false)`; 删 `strategies/{name}.toml` 与 `.lua`(存在才删); **不删 `logs/`**。
  - `RestartLive`: `stop_daemon(false)` → `live_preflight(accept_risk=false)`(fail-closed) → `start_daemon(..,true,false,true)`。
- **FR-025**: `UpdateParams` / `DeleteStrategy` 只动 `[strategy.params]` 与策略文件, **不触碰顶层 `live_enabled`**。
- **FR-026**: `execute()` 改 `&mut self`, 成功后按 `ActionKind` 渲染菜单 A / 刷新菜单 B / 清空菜单; Reject 分支适配 13 变体全集。

## 四、验收标准

- **SC-001**: `cargo test -p ricow` 全绿零回归; `cargo clippy -p ricow` 零警告。
- **SC-002**: 漂移闸: `test_registry_matches_whitelist_exactly` 与 `test_registry_count_and_write_tool_boundary` 同步为 **只读 12 / 虚拟 3**, 且不存在任何"模型直写"工具名。
- **SC-003**: 确认语义单测: zh 下 `确认`/`确定`/`同意` 放行且 `confirm` 不放行; en 反之; `y`/`yes`/`ok`/空一律不放行; 终端短语唯一且与 `approve.rs`/`ctrl.rs` 字面量一致。
- **SC-004**: 13 个 action 名全覆盖 —— `test_every_write_action_requires_confirmation` 证明无任何直执行写路径。
- **SC-005**: 菜单单测: `parse_menu_choice` 接受 `1`/`2.`/`3)`/`4、`, 拒绝 `0`/`12`/`1a`/空; 菜单编号与 request 一一对应且含策略名。
- **SC-006**: 双语单测: `Lang::parse` 接受 `zh`/`en`(大小写与空白容错)并拒绝其它; `resolve` 未设置时默认 `zh`; `[ui].lang` 读写保注释、非法值硬失败。
- **SC-007**: 确定性集成 `cargo test -p ricow --test ai_live_smoke` A 组全绿: 管道喂确认词不触发、非交互不登记 pending、`approve_requires_interactive_tty` 仍用终端长短语。
- **SC-008**: 测试网真实调用(宪法三, 禁 mock): 真实币安 demo 凭据跑一次对话内 `start_demo` / `stop_demo`(各需一次确认), 确认订单可撤、停机回执与实际一致。
- **SC-009**: 文档同步: `SECURITY.md` / `specs/architecture.md` / `specs/product.md` / `specs/roadmap.md` / `README.md` / `README_zh.md` 说明"对话确认词为口语词, 终端仍为逐字长短语"; `specs/constitution.md` 安全要求段追加修订记录。
- **SC-010**: grep 核对: `确认部署|确认启动测试网|确认风险|确认实盘|确认停止测试网|确认停止实盘|确认平仓停止|确认覆盖` 仅命中 `approve.rs` / `ctrl.rs` 与 SDD 档案; `session.rs` / `tools.rs` 用户可见文案零命中 `ricow restart|ricow stop|ricow run `。
