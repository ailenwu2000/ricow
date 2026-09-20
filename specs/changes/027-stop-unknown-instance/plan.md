# 027 实施计划: 停机动作对"不存在的策略名"误报成功

**规格**: [spec.md](./spec.md) | **来源**: CLI 全功能验收确认的唯一功能性缺陷(停机假成功回执) + 两项已拍板决策(**D1 名字存在性取并集** / **D2 CLI 与 AI 合并为单一判定来源**) | **日期**: 2026-09-20 | **分支**: `027-stop-unknown-instance`

## 摘要

缺陷: `ricow stop <不存在的名字>` 回 `策略 X 已停止` + `该策略未在运行 (无需停止)`, **exit=0**; 同源的 `restart` 先印两行假成功后报错。停机是风险动作里最需要确定性的一类, "假阴性回执"会让用户误判风险已解除。

技术方案一句话: **把"名字是否存在"的判定收敛成一个共享函数**(策略文件 ∪ 实例台账, 与 `ricow status` 同口径), 在**daemon 侧**执行判定并三分支答复 —— 不存在 → 非零失败(不出现"已停止/无需停止"字样); 存在但未运行 → 成功且只陈述"未在运行 (无需停止)"单一事实; 运行中 → 既有停机语义与文案**一行不动**。

- 判定单点: 新增 `commands::instances::strategy_name_exists()` + `unknown_name_message()`; daemon 停机内核与 AI 停机工具**共用同一份**。
- 协议: `StopReport` 只新增一个 `already_stopped: bool`(`#[serde(default)]`, 旧 daemon 缺省 false = 旧行为), 让 CLI 能把"本来就没在跑"与"刚刚停掉"分开措辞。
- 不新增依赖 / 不新增 CLI 命令或参数 / 不改控制通道 `Request` 形状 / 不改任何停机执行语义。

## 技术上下文

**语言/版本**: Rust 1.88(workspace `rust-version = "1.88"`, edition 2021)

**主要依赖**: **零新增**。复用 tokio(控制通道异步 TCP)、serde / serde_json(协议与回执)、clap(CLI); 判定本身只用 `std::path` 与既有 `ledger::list_instances`。

**存储**: 文件, 两类来源(均为既有格式, 本次不改): `strategies/<name>.toml`(已部署清单) 与 `run/<name>.json`(实例台账)。不涉及 SQLite。

**测试**: `cargo test -p ricow --bin ricow`(本 crate 为 bin crate)。daemon 侧沿用既有**真实 TCP 回环集成测试**先例([server.rs](../../../crates/ricow/src/supervisor/server.rs#L494-L548), 非 mock); 判定函数与文案分派用单元测试。

**目标平台**: 跨平台(本次验收在本机 Windows 执行); 不引入任何平台相关分支。

**项目类型**: CLI(bin crate `ricow`) + 常驻 daemon(控制通道 TCP 回环)。

**性能目标**: 不适用 —— 判定成本 = 一次 `Path::is_file()` + 一次 `run/*.json` 目录扫描, 与既有 `ricow list` 同量级, 不进入任何热路径。

**约束**: 不改停机执行语义(指令下发 / `--close-all` / 等待上限 / 超时如实报告 / 清理提示); 不改控制通道 `Request` 形状; 不改 `status` / `info` / `logs` / `list` / `fills` 的输出与退出码; 不新增绕过开关。

**规模/范围**: 生产代码 4 个文件(另 3 个测试同文件内); 新增判定函数 1 个、文案函数 1 个、协议字段 1 个; **不**新增模块 / 命令 / 参数 / 依赖。

## 宪法检查

*门禁: Phase 0 前通过; Phase 1 后复查。*

| 宪法条款 | 检查结论 |
|---|---|
| 原则三 · 测试纪律 | ✅ 本次**不触碰交易流程**(下单 / 撤单 / 成交 / 持仓 / 风控一行不动), 只改控制通道的名字判定与回执文案 → 属"纯逻辑", 用单元测试; 端到端验收仍走既有 **demo 测试网真实停机**(不 mock、不假 token) |
| 原则四 · 产物一律中文 | ✅ 本变更全部文档与代码注释中文 |
| 原则五 · YAGNI | ✅ 只加 1 个 bool 字段 + 1 个判定函数 + 1 个文案函数; **不做** `--force`、批量停机、新错误类型、`restart` 重构 |
| 变更纪律 · 先审后执行 / 只改相关代码 / 不顺手重构 | ✅ 本计划即审查件; [restart](../../../crates/ricow/src/commands/ctrl.rs#L265-L283) 与 [format_info](../../../crates/ricow/src/commands/instances.rs) **明确不写**(见 D7/D11), 无附带清理 |
| 开发流程 · SDD 五步 | ✅ 当前处于 plan 阶段(第 2 步), 未写任何实现 |
| 安全要求 · 确认门禁 | ✅ **不新增写操作、不改确认门禁**(终端逐字短语 / 对话确认词原样); 判定收紧方向更保守, 不降低任何门槛; Dry Run 默认与实盘三判据不动 |
| 文档体系 · 现状与代码一致 | ⏳ converge 时复核 [architecture.md](../../../specs/architecture.md#L56) 停机章节与第 117 行的 `start\|stop\|restart` 描述是否需要同步; **预期无需改动**(本次是行为修正而非架构变化), 以实测决定 |

**Phase 0 后复查**: 见 §一 决策表末尾的"复查记录"。**Phase 1 后复查**: 见 §六 复杂度追踪末尾。

## 一、决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | 名字存在性判定口径 = **`strategies/<name>.toml` 存在 ∪ 实例台账 `run/<name>.json` 存在**(取并集); 空串 / 纯空白一律判否 | spec §八 D1 拍板; 与 `ricow status` / `info` 的既有口径逐字一致(它们同样"TOML 或台账任一存在即认为存在")。**半存在**(有 TOML 无台账)归入"存在但未运行", 不是错误 |
| D2 | 判定与文案**各收敛成单点**: 新增 `commands::instances::strategy_name_exists()` 与 `unknown_name_message()`; daemon 与 AI 两侧**只准从这里取** | spec §八 D2 拍板 + FR-015; 双口径是用户故事 3 的痛点本身, 靠"同一份实现"从结构上消除"一边报错一边成功" |
| D3 | 判定在 **daemon 侧**执行([server.rs](../../../crates/ricow/src/supervisor/server.rs) 的 `stop` 内核), CLI 侧**不重复实现** | daemon 是唯一同时持有运行中子进程句柄与数据根的一方; CLI 要走上停机通道**必须先 `Client::connect` 成功** → daemon 未运行时 `connect` 已报错, **FR-009 的优先级天然成立**, 无需额外分支。同时避免"CLI 判一次 + daemon 判一次"的双口径 |
| D4 | 名字不存在 → `Response::err(...)`, 复用既有 `CoreError::InvalidArgument` 通道(CLI 打印 `错误: invalid argument: <msg>` 且 exit≠0) | 与 `ricow status X` 的既有报错**同一前缀同一措辞** (FR-014); **不新增 `CoreError` 变体**(YAGNI, 且新变体会让前缀与 status 分家) |
| D5 | `StopReport` 新增 `already_stopped: bool`(`#[serde(default)]` = 旧 daemon 缺省 false) | CLI 必须能区分"本来就没在跑"与"刚刚停掉"; 缺省 false 使**新 CLI 遇旧 daemon 时行为与今日完全一致**(不产生反向假阴性)。**不动** `exited` / `graceful` / `exit_code` / `waited_ms` / `note` 的既有语义 (FR-016) |
| D6 | "存在但未运行"分支的报文保持现状(`exited=true, graceful=true, exit_code=None, waited_ms=0, note="该策略未在运行 (无需停止)"`), 只额外置 `already_stopped=true`; CLI **不再给这条回执套"已停止"头衔**, 只印 note 一行 | FR-006: 只陈述单一事实、不含"已停止"字样; 同时真实停机路径的 report 形状零变化 (SC-005) |
| D7 | **`restart` 一行不改** —— 现有 `stop(...).await?` 已是第一步且 `?` 已短路 | FR-010 / FR-011 由"收紧 stop"自动获得; `prev_mode` 的 `views()` 预读对不存在的名字返回 `None`(无副作用)。改动它是**顺手重构**, 违反宪法行 40 |
| D8 | AI 侧 [prepare_stop](../../../crates/ricow/src/ai/tools.rs#L1833-L1858) 的"没有名为 X 的实例"判定改为调用同一 `strategy_name_exists()`: 判否 → 用同一 `unknown_name_message()` 基底 + 指向 `instance_status`; 判是但视图里没有(= 有 TOML 无台账) → 归入"未在运行…无需停机"分支 | D2 落地; 修掉"AI 只认台账"与 CLI 的口径分叉。**保留** `ensure_daemon_running()` 的**优先级**(它先于名字判定执行, FR-009) |
| D9 | 既有测试 [server.rs:534-539](../../../crates/ricow/src/supervisor/server.rs#L534-L539) 断言 `Request::Stop{name:"nope"}` → `ok` + `exited=true` 的 4 行, **断言的正是本缺陷**, 必须同步改为断言失败, 并补"仅有 TOML → 成功"的正向用例 | 该断言是"把缺陷固化成期望"的典型; 不改它就无法交付 SC-001/SC-002。这是**修正期望**, 不是测试倒退 |
| D10 | 新增测试: ① `strategy_name_exists` 纯函数 5 用例(空串 / 纯空白 / 仅 TOML / 仅台账 / 皆无 / 皆有); ② `stop_daemon` 文案分派 4 组合(重点断言"存在未运行"路径**不含**"已停止"); ③ daemon 真实 TCP 回环: 不存在 → `ok=false` 且 error 含"未找到策略或实例", 仅 TOML → `ok=true` + `already_stopped=true`; ④ proto: 旧格式(缺 `already_stopped` 字段)解析为 `false` | 全部落在"纯逻辑"与"控制通道", 不触碰交易流程(宪法原则三) |
| D11 | `status` / `info` / `logs` / `list` / `fills` 与 `format_info` 的文案**逐字不动** | FR-017: 它们是本次**对齐基准**, 不是改造对象 |
| D12 | **不做**: `--force` / `--yes` 之类绕过开关; `stop --all` 批量停机; 新 CLI 子命令或参数; 新协议类型(枚举 / 新 `Request` 变体); `restart` 重构; 名字字符集或长度校验的新规则; 多语言文案改造 | FR-018 + 原则五 + 宪法行 40 |
| D13 | converge 时按实测复核 `specs/architecture.md` 第 56 / 117 行停机描述与 `specs/roadmap.md` 记录; 若无与本次冲突的现状描述则**不改文档** | 文档体系要求"现状与代码一致"; 但本次非架构变化, 不预先制造改动 (YAGNI) |

**Phase 0 复查记录**(调研后回看本表): 无 `NEEDS CLARIFICATION` 残留; D1/D2 来自用户拍板, D3/D5/D7 来自代码实证(见 [research.md](./research.md) 的 Alternatives), 决策表无需修订。**Phase 1 后复查**: 决策与 [contracts/](./contracts/) 中的退出码表 / 文案矩阵逐条一致, 无冲突。

## 二、改动清单

| 文件 | 改动 |
|---|---|
| `crates/ricow/src/commands/instances.rs` | 新增 `pub(crate) fn strategy_name_exists(root: &Path, name: &str) -> bool`(**唯一判定来源**, D1/D2: 空/纯空白 → false; `strategies/<name>.toml` `is_file()` → true; `ledger::list_instances(root)` 含该名 → true)与 `pub(crate) fn unknown_name_message(name: &str) -> String`(**唯一文案基底**: `未找到策略或实例 {name} (strategies/{name}.toml 不存在, 也无实例台账)`, 与既有 `format_info` 措辞同口径)。既有 `snapshot` / `views` / `format_info` / `format_table` **零改动**(D11) |
| `crates/ricow/src/supervisor/proto.rs` | `StopReport` 新增 `#[serde(default)] pub already_stopped: bool`(D5, 含 doc 注释说明"名字存在但本来就没在跑"); 更新 `stop_report_roundtrip` 覆盖新字段 + 新增"旧格式缺字段 → false"断言(D10④)。`Request` / `Envelope` / `Response` / `InstanceView` **零改动** |
| `crates/ricow/src/supervisor/server.rs` | `stop()` 的"句柄缺失"分支(D3/D4/D6)由"无条件返回成功"改为: 不满足 `strategy_name_exists(&root, name)` → `Response::err(format!("{}; 可用 ricow list 查看现有实例", unknown_name_message(name)))`; 满足 → 返回既有的 `StopReport` 但置 `already_stopped: true`。**运行中路径(拿到句柄后的全部代码)一行不动**。测试: 修正 [L534-539](../../../crates/ricow/src/supervisor/server.rs#L534-L539) 的缺陷期期望 + 新增"仅 TOML → 成功未运行"用例(D9/D10③) |
| `crates/ricow/src/commands/ctrl.rs` | `stop_daemon()` 文案分派(D6): 先判 `report.already_stopped` —— 为真则**跳过**"已停止 / 未观测到退出"头衔, 只走既有 `if let Some(note)` 输出; 为假则三条既有分支逐字不变。`restart()` / `start()` / `restart_flags()` / 实盘三判据内核 **零改动**(D7)。新增文案分派单测(D10②) |
| `crates/ricow/src/ai/tools.rs` | `prepare_stop()`(D8): 把 `views()` 找不到即报"没有名为 X 的实例"改为 —— 找不到 **且** `strategy_name_exists()` 判否 → `unknown_name_message()` + `可用 instance_status 确认现状`; 找不到**但**判是(有 TOML 无台账) → 归入既有"当前未在运行…无需停机"分支; 找到但 `!running` → 既有分支不动。`ensure_daemon_running()` 位置与优先级**不动**; `prepare_restart_live()` / 确认块文案**不动** |

> 明确的**不改动**面(D11/D12): `commands/mod.rs`(`read_strategy_config_in` / `deployed_strategy_names` / `validate_new_name`)、`supervisor/client.rs`、`supervisor/ledger.rs`、`supervisor/procs.rs`、`ai/session.rs`(5 处 `stop_daemon` 调用点全部无需改 —— 它们拿到的就是同一份文案)、`commands/daemon.rs`、`commands/run.rs`、`commands/instances.rs` 的既有函数体。

## 三、项目结构

### 文档(本次功能)

```text
specs/changes/027-stop-unknown-instance/
├── spec.md                  # 功能规格 (已定稿, §八 D1/D2)
├── checklists/
│   └── requirements.md      # 需求清单 (全绿, 阻塞已解除)
├── plan.md                  # 本文件
├── research.md              # Phase 0 输出: 4 个技术选型的 Decision / Rationale / Alternatives
├── data-model.md            # Phase 1 输出: 判定来源并集 + 停机回执三分支 + 文案矩阵
├── contracts/
│   ├── cli-stop.md          # 终端契约: `ricow stop` / `ricow restart` 的 stdout 与退出码
│   └── control-channel.md   # 控制通道契约: `Request::Stop` 语义 + `StopReport` 形状与兼容规则
├── quickstart.md            # Phase 1 输出: 可照跑的验证场景 (4 类名字 × 2 条通道)
└── tasks.md                 # Phase 2 输出 (/speckit-tasks 生成, 不由本命令创建)
```

### 源代码(仓库根)

```text
crates/
└── ricow/                      # bin crate: CLI + daemon (本次唯一改动的 crate)
    └── src/
        ├── commands/
        │   ├── instances.rs    # ← 新增 strategy_name_exists() / unknown_name_message()
        │   ├── ctrl.rs         # ← stop_daemon() 文案分派 (restart/start 不动)
        │   └── ...
        ├── supervisor/
        │   ├── proto.rs        # ← StopReport 新增 already_stopped
        │   ├── server.rs       # ← stop() 名字判定三分支
        │   ├── client.rs       #   (不改)
        │   └── ledger.rs       #   (不改, 判定的台账来源)
        └── ai/
            └── tools.rs        # ← prepare_stop() 改用同一判定
```

**结构决策**: 沿用既有模块划分, **不新建文件、不新建模块**。判定与文案落在 `commands/instances.rs` —— 该模块的职责说明本身就是"daemon 控制通道 ∪ 实例台账 ∪ 已部署清单,**三者并集**才是有哪些策略的完整答案", 与 D1 的口径同源; 且 `supervisor::server` 已经在调用 `crate::commands::*`(见 [server.rs:151](../../../crates/ricow/src/supervisor/server.rs#L151) 与 [server.rs:459](../../../crates/ricow/src/supervisor/server.rs#L459)), 不会引入新的依赖方向。

## 四、风险

| # | 风险 | 处置 |
|---|---|---|
| R1 | 判定收紧后**误伤真实场景**(如台账写入失败、实例只在 `children` 里) | daemon 侧判定只在"`children` 未命中(句柄已摘除) + 台账无记录 + TOML 不存在"三者同时成立时报错 —— 运行中的实例根本进不到该分支(D3); AI 侧 `views()` 已涵盖台账 ∪ 运行中。单测覆盖"仅有台账 → 不报错"与"仅 TOML → 不报错" |
| R2 | 版本混跑(新 CLI + 旧 daemon)导致行为漂移 | `already_stopped` 默认 `false` → 新 CLI 走旧三条分支, 与今日行为逐字一致; `Response::err` 由**新** daemon 才有, 旧 daemon 不会发错 → 无反向假阴性(D5) |
| R3 | 误伤 `restart`: 收紧 stop 后第一步即失败, 会不会退化成"只启动不停止" | `restart` 零改动(D7), `stop(...).await?` 的 `?` 在失败时**立即返回**; 验收动作第 5 条断言"失败后 daemon `children` 仍为空、台账未新增" |
| R4 | 文案调整波及 AI 侧既有测试 | 已核对: AI 侧"无需停机 / daemon 未运行"断言在 [tools.rs:2661-2697](../../../crates/ricow/src/ai/tools.rs#L2661-L2697) 的 daemon 未运行路径, 本次**不动该路径文案**; 改后跑全量测试确认 |
| R5 | 破坏既有 CLI 文案与退出码(最高优先级) | 真实停机路径(运行中 → 停止)的 `StopReport` 与 `stop_daemon` 三条分支**逐字不动**(D6/D11); 验收动作第 3/8 条逐条比对 |
| R6 | 过度设计(新协议类型 / 绕过开关 / 新错误变体) | D12 明确禁列; 本变更只加 1 个 bool 字段、1 个判定函数、1 个文案函数, 零新依赖 |
| R7 | `stop_daemon` 在 `already_stopped` 分支只输出 note, 若 note 缺失会输出空 | 契约层保证: 该分支的 `note` 由 daemon 固定填 `"该策略未在运行 (无需停止)"`, 且该字段只在**新** daemon 下才可能为 `true`(旧 daemon 无此字段) → 空输出不可达; 契约写入 [contracts/control-channel.md](./contracts/control-channel.md) 并由 D10② 单测锁定 |

## 五、验收动作

1. `cargo fmt --all -- --check` 零差异; `cargo clippy --workspace --all-targets --locked -- -D warnings` 零警告。
2. `cargo test -p ricow --bin ricow` 全绿, `failed = 0`, 且 passed 数相对改动前基线**不倒退**(基线以改动前实测为准)。
3. **真实停机路径零回归**(SC-005): 对**运行中**的实例执行 `ricow stop <name>` 与 `ricow stop <name> --close-all`, 输出与退出码与改动前**逐字一致**(含模式感知清理提示)。
4. **不存在名字**(SC-001/SC-002, FR-002~FR-005): `ricow stop no-such-strategy` 与 `ricow stop no-such-strategy --close-all` → 退出码非 0, 输出含"未找到策略或实例"与 `ricow list` 指引, **不含**"已停止"、"未在运行"、"无需停止"(后两者不得出现于不存在分支)。
5. **重启继承**(FR-010/FR-011): `ricow restart no-such-strategy` → 退出码非 0, 输出**不出现**任何"策略 X 已停止"式回执, 且事后 `ricow list` 无该名字、台账无新增。
6. **存在但未运行**(SC-003, FR-006): 对已部署未运行的名字(含"仅有 `strategies/<n>.toml`、从未启动"这一半存在情形)执行 `ricow stop <name>` → 退出码 0, 回执**只**陈述"未在运行 (无需停止)", **不含**"已停止"。
7. **daemon 未运行优先级**(FR-009): 停掉 daemon 后 `ricow stop <任意名字>` → 仍先如实报"daemon 未运行 (无 run/daemon.json)", 不得改口成"策略未在运行"。
8. **口径统一**(SC-004, FR-013): 对同一组名字(不存在 / 仅 TOML / 存在未运行 / 运行中)分别走终端与对话内 AI 停机工具, 两边结论逐条一致; daemon 未运行时 AI 仍先报 daemon 未运行。
9. **边界**(FR-008): 空串 / 纯空白名字 → 报错, 不落入"未在运行"分支。
10. **不回归**(FR-017): `ricow status` / `ricow status <name>` / `ricow info` / `ricow logs` / `ricow list` / `ricow fills` 的未知名字行为与输出逐字不变。
11. **协议兼容**(D10④): 缺 `already_stopped` 字段的旧格式 `StopReport` JSON 可解析且值为 `false`。
12. 端到端验收用 **demo 测试网真实实例**(不 mock、不假 token)。**不做** `git commit`(宪法行 42: 用户明确说"提交"才可)。

## 六、复杂度追踪

> **仅当宪法检查出现违规且必须说明时填写**

无违规。本次未引入新项目 / 新模式 / 新抽象: 判定与文案各一个函数、协议一个字段, 全部落在既有模块与既有分层内; 无被否的更简替代方案(更简的做法是"只改 CLI 文案不判定", 但那会保留 FR-002 的假阴性, 不满足 spec 的核心价值)。

**Phase 1 后复查记录**: 交付物 [research.md](./research.md) / [data-model.md](./data-model.md) / [contracts/](./contracts/) / [quickstart.md](./quickstart.md) 与 §一 决策表、spec FR-001~FR-018 逐条对齐; 宪法检查各条结论不变, 无新增违规项。
