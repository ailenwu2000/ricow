# 027 Phase 0 调研: 停机动作对"不存在的策略名"误报成功

**规格**: [spec.md](./spec.md) | **计划**: [plan.md](./plan.md) | **日期**: 2026-09-20 | **分支**: `027-stop-unknown-instance`

**结论**: 无 `NEEDS CLARIFICATION` 残留。四项技术选型全部有代码实证支撑, 无新增依赖、无新增模块。

## R-1 判定与文案放在哪一层

**Decision**: 判定与文案单点放在 [commands/instances.rs](../../../crates/ricow/src/commands/instances.rs), 新增 `pub(crate) fn strategy_name_exists(root: &Path, name: &str) -> bool` 与 `pub(crate) fn unknown_name_message(name: &str) -> String`。判定口径 = `strategies/<name>.toml` `is_file()` **或** `ledger::list_instances(root)` 含该名(并集); **空串 / 纯空白直接判否**(前置短路, 不查盘 —— 否则空名会拼出 `strategies/.toml`, 结果取决于磁盘上是否存在该文件)。

**Rationale**:

1. 该模块的模块注释原文就是"daemon 控制通道 ∪ 实例台账 `run/<name>.json` ∪ 已部署清单 `strategies/*.toml`。**三者并集**才是'有哪些策略'的完整答案"([instances.rs:1-4](../../../crates/ricow/src/commands/instances.rs#L1-L4)) —— 与 spec §八 D1 的口径同源。
2. `ricow status <name>` / `ricow info <name>` 的既有报错文案就在同一文件 ([instances.rs:253-258](../../../crates/ricow/src/commands/instances.rs#L253-L258)): `未找到策略或实例 {name} (strategies/{name}.toml 不存在, 也无实例台账)`。文案基底与判定同处一个文件, 不存在"两套说法"的空间。
3. **依赖方向零新增**: [server.rs:151](../../../crates/ricow/src/supervisor/server.rs#L151) 已在调用 `crate::commands::load_strategy_toml`, [server.rs:459](../../../crates/ricow/src/supervisor/server.rs#L459) 已在调用 `crate::commands::*` → `supervisor → commands` 是既有方向; [ai/tools.rs:1844](../../../crates/ricow/src/ai/tools.rs#L1844) 已在调用 `commands::instances::views` → `ai → commands::instances` 也是既有方向。放本文件不会引入循环依赖。
4. 两种来源的既有实现都现成: `strategies/*.toml` 的枚举方式可对照 [mod.rs:436-450](../../../crates/ricow/src/commands/mod.rs#L436-L450) 的 `deployed_strategy_names`; 台账枚举用 [ledger.rs:156-172](../../../crates/ricow/src/supervisor/ledger.rs#L156-L172) 的 `list_instances`。

**Alternatives considered**:

| 备选 | 结论 | 理由 |
|---|---|---|
| A) 新建 `supervisor/names.rs` 独立模块 | 否 | "已部署清单"来源 (`strategies/*.toml`) 归属 commands 层; 放 supervisor 会把文件路径拼接重复实现一遍, 且违反"不新建文件"的结构决策 |
| B) 放 `commands/mod.rs` | 否 | mod.rs 是命令装配层(已 400+ 行), 与"有哪些策略"的归属地 `instances.rs` 分工不同; 且会把 `format_info` 的文案基底与判定分开 |
| C) 在 `stop` 与 `restart` 各写一份 | 否 | 直接违反 FR-015"判定必须单点"; 且 `restart` 本来就复用 `stop`, 见 D7 |

## R-2 判定在哪一侧执行

**Decision**: 在 **daemon 侧**执行([server.rs#stop](../../../crates/ricow/src/supervisor/server.rs#L212-L225) 的"句柄缺失"分支), CLI 侧**不重复实现**。

**Rationale**:

1. daemon 是唯一同时持有运行中子进程句柄(`state.children`)与数据 root 的一方 —— "`children` 里没有它"这一事实只有 daemon 知道。运行中的实例根本进不到该分支, 所以判定只在"句柄已摘除"之后发生。
2. **FR-009 的优先级天然成立, 无需任何额外分支**: CLI 要走上停机通道**必须先 `Client::connect(root)` 成功**, 而它在无 `run/daemon.json` 时直接返回 `CoreError::InvalidArgument("daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start")`([client.rs:19-32](../../../crates/ricow/src/supervisor/client.rs#L19-L32))。即"daemon 未运行"这一步在名字判定之前就失败了。
3. 若 CLI 侧再判一次 → 双口径(违反 FR-015 / D2), 且"判完到发请求"之间存在 TOCTOU 窗口(判定为存在、请求到达时已被别的进程停掉)。

**Alternatives considered**:

| 备选 | 结论 | 理由 |
|---|---|---|
| A) CLI 侧预判(`stop_daemon` 里先查 strategies/ 与 run/) | 否 | 双实现违反 FR-015/D2; 且会把"daemon 未运行"与"名字不存在"两个优先级搅在同一段代码里, 违背 FR-009 |
| B) 两侧都判(CLI 兜底 + daemon 权威) | 否 | 明确的双口径; 两处口径一旦漂移就是本次要修的病本身 |

## R-3 如何在回执层面区分"不存在"与"存在但未运行"

**Decision**: 两条路分开:

- **不存在** → 不产出 `StopReport`, 直接 `Response::err(format!("{}; 可用 ricow list 查看现有实例", unknown_name_message(name)))`。
- **存在但未运行** → 仍产出既有形状的 `StopReport`(`exited=true, graceful=true, exit_code=None, waited_ms=0, note="该策略未在运行 (无需停止)"`), 只新增 `already_stopped: true`。
- 协议改动 = [StopReport](../../../crates/ricow/src/supervisor/proto.rs#L96-L107) 增加 `#[serde(default)] pub already_stopped: bool`, **仅此一处**。

**Rationale**:

1. `Response::err` 已是既有通道: [Client::call_ok](../../../crates/ricow/src/supervisor/client.rs#L66-L73) 把 `ok=false` 映射为 `CoreError::InvalidArgument(resp.error)`, CLI 在 [main.rs:117-120](../../../crates/ricow/src/main.rs#L117-L120) 打印 `错误: {e}` 并 `exit(1)`。→ FR-002 的"非零退出码"与 FR-014 的"与 `ricow status` 同前缀(`invalid argument: `)同措辞"**零新代码**即满足。
2. 成功回执需要被 CLI 分成两种措辞("刚刚停掉" vs "本来就没在跑"), 而 `StopReport` 现有字段都无法表达 —— `exited` 在两种情况下都是 `true`。必须有一个显式字段。
3. `#[serde(default)]` 使**旧 daemon 的响应(无该字段)解析为 `false`** → 新 CLI 走回既有三条分支, 与今日行为逐字一致, **不产生反向假阴性**; 反过来 `Response::err` 只可能由新 daemon 发出, 旧 daemon 不会误发。
4. 不动 `exited` / `graceful` / `exit_code` / `waited_ms` / `note` 的既有语义 → 真实停机路径的回执形状零变化(SC-005)。

**Alternatives considered**:

| 备选 | 结论 | 理由 |
|---|---|---|
| A) CLI 从 `note` 文本里匹配"无需停止"来判定 | 否 | 拿用户可见文案当协议是最脆的耦合 —— 改一个字文案就让分支失效; note 是自由文本, 不是判别位 |
| B) 新增 `Response` 变体或新协议类型(如 `StopOutcome` 枚举) | 否 | 违反 D12(不加新协议类型)与原则五; 且会破坏 `Response{ok, error, data}` 的极简形状 |
| C) 复用 `exited=false` 表示"没在跑" | 否 | `exited=false` 现有语义是"停机超时未观测到退出"([server.rs:245-250](../../../crates/ricow/src/supervisor/server.rs#L245-L250)), 语义冲突; CLI 会据此打印"未观测到退出", 与事实(没在跑)相反 |
| D) 让 daemon 直接返回面向用户的整段文本 | 否 | 会绕开 `StopReport` 的结构化字段, CLI 与 AI 两侧的既有消费方(含 `ai/session.rs` 5 处调用)都要改, 违反"不改动面"的最小化 |

## R-4 退出码映射与边界

**Decision**:

| 情形 | 通道 | 退出码 |
|---|---|---|
| 名字不存在 | `Response::err` → `CoreError::InvalidArgument` | **1** |
| 名字存在但未运行 | `StopReport{already_stopped:true}` | **0** |
| 名字存在且运行中, 优雅退出 | 既有 `StopReport` | **0**(现状) |
| 名字存在且运行中, 停机超时未观测到退出 | 既有 `StopReport{exited:false}` | **0**(现状, 本次不改) |
| daemon 未运行 | `Client::connect` 失败 → `InvalidArgument` | **1**(现状) |

**Rationale**: CLI 只有"`Err` → 打印 + exit 1"与"`Ok` → exit 0"两个出口([main.rs:117-120](../../../crates/ricow/src/main.rs#L117-L120))。复用它即可满足 FR-002, 不需要改错误模型、不新增 `CoreError` 变体(避免前缀与 `ricow status` 分家)。停机超时的退出码语义本次**明确保持现状**(spec 边界情况: "保持现状如实报告", FR-016)。

**Alternatives considered**:

| 备选 | 结论 | 理由 |
|---|---|---|
| A) 为"停机超时"改成非零退出码 | 否 | 超出本次范围(FR-016 明确保持现状), 会破坏既有验收基线, 属顺手扩大改动面(违反宪法行 40) |
| B) 新增 `CoreError::NotFound` 变体让它更像语义错误 | 否 | 新变体会产生新前缀(`错误: not found: …`), 与 `ricow status X` 的既有前缀不一致, 违反 FR-014 |
| C) 名字不存在时返回 `ok=true` 但带一个错误字段 | 否 | 与"非零退出码"要求冲突; 且会让脚本无法用退出码判失败 |

## 遗留

无。全部 `[NEEDS CLARIFICATION]` 已在 spec §八(D1/D2 用户拍板)与本文件(R-1~R-4 代码实证)消解。
