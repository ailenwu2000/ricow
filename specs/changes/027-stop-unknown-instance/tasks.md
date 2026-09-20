---
description: "027 停机动作对「不存在的策略名」误报成功 — 实施任务清单"
---

# 任务: 027 停机动作对"不存在的策略名"误报成功

**输入**: [plan.md](./plan.md) | [spec.md](./spec.md) | [research.md](./research.md) | [data-model.md](./data-model.md) | [contracts/](./contracts/) | [quickstart.md](./quickstart.md)

**前置条件**: plan.md(必填)、spec.md(用户故事必填)、research.md、data-model.md、contracts/

**测试**: 本变更含测试任务 —— 依据 [plan.md](./plan.md) D9/D10(修正缺陷期断言 + 新增 4 类用例)与宪法原则三(不触碰交易流程 → 纯逻辑用单元测试, 端到端用 demo 真实实例)。

**组织方式**: 任务按用户故事分组, 以便每个故事独立实施与测试。

## 格式: `[ID] [P?] [故事] 描述`

- **[P]**: 可并行(不同文件, 无依赖)
- **[故事]**: 任务所属用户故事(US1, US2, US3)
- 描述中必须包含精确文件路径

## 路径约定

- 单一项目: 仓库根 `d:\sunhuazhu\ricow`
- 生产代码全部在 bin crate `crates/ricow`(测试与被测代码同文件, 本仓库无独立 `tests/` 目录)

---

## 阶段 1: 搭建 (共享基础设施)

**目的**: 建立改动前基线, 作为验收动作"passed 不倒退"的比较基准

- [X] T001 在仓库根执行 `cargo test -p ricow --bin ricow`, 记录改动前的 passed / failed / ignored 三个计数, 作为验收动作 2 的基线
- [X] T002 在仓库根执行 `cargo build -p ricow` 并确认 `.\target\debug\ricow.exe --version` 输出 `ricow 0.7.0`, 保证后续手工验收可直接调用该可执行文件

---

## 阶段 2: 地基 (阻塞性前置)

**目的**: 收敛"名字存在性判定"与"未知名字文案"为**单点**, 并扩展停机回执协议 —— 三个用户故事全部依赖

**⚠️ 关键**: 本阶段完成前不得开始任何用户故事工作

- [X] T003 在 `crates/ricow/src/commands/instances.rs` 新增 `pub(crate) fn strategy_name_exists(root: &Path, name: &str) -> bool`(**唯一判定来源**, plan D1/D2): 空串或纯空白 → 前置短路返回 `false`(不查盘); `strategies/<name>.toml` 的 `is_file()` 为真 → `true`; `ledger::list_instances(root)` 结果含该名 → `true`(两来源取**并集**, 缺一不算不存在)
- [X] T004 在 `crates/ricow/src/commands/instances.rs` 新增 `pub(crate) fn unknown_name_message(name: &str) -> String`, 返回逐字 `未找到策略或实例 {name} (strategies/{name}.toml 不存在, 也无实例台账)` —— 与既有 `format_info` 的报错措辞同口径(plan D2/D4, FR-014)
- [X] T005 [P] 在 `crates/ricow/src/supervisor/proto.rs` 给 `StopReport` 新增 `#[serde(default)] pub already_stopped: bool`(doc 注释说明"名字存在但本来就没在跑", 不加 `skip_serializing_if`); 同步更新既有 `stop_report_roundtrip` 覆盖新字段, 并新增断言"缺 `already_stopped` 字段的旧格式 JSON 解析为 `false`"(plan D5/D10④, contracts/control-channel.md §三)

**检查点**: 地基就绪 —— 判定函数、文案函数、协议字段三者可用, 三个用户故事可开始实施

---

## 阶段 3: 用户故事 1 - 打错名字时立刻知道"这个策略不存在" (优先级: P1) 🎯 MVP

**目标**: 名字**完全不存在**时停机以非零退出码失败, 回执不含"已停止"、"未在运行"、"无需停止"任一字样, 并给出可执行的下一步

**独立测试**: 对从未存在过的名字执行 `ricow stop <name>` / `ricow stop <name> --close-all` / `ricow restart <name>`, 检查退出码与输出

- [X] T006 [US1] 在 `crates/ricow/src/supervisor/server.rs` 的 `stop()` 中, 把"句柄缺失"分支由"无条件返回成功 `StopReport`"改为三分支: (a) `!strategy_name_exists(&root, name)` → `Response::err(format!("{}; 可用 ricow list 查看现有实例", unknown_name_message(name)))`; (b) 判定为存在 → 返回既有形状的 `StopReport`(`exited=true, graceful=true, exit_code=None, waited_ms=0, note=Some("该策略未在运行 (无需停止)")`)但置 `already_stopped=true`; (c) 运行中路径(拿到句柄后的全部代码含 `request_stop` / `wait_exit` / `write_exit_record` / `cleanup_hint_for`)**一行不动**(plan D3/D4/D6, FR-002~FR-005)
- [X] T007 [US1] 在 `crates/ricow/src/commands/instances.rs` 新增 `strategy_name_exists` 单测, 覆盖 6 个用例: 空串 / 纯空白 / 仅 `strategies/<n>.toml` / 仅 `run/<n>.json` / 皆无 / 皆有(plan D10①, FR-008); 再加一条断言: `unknown_name_message("x")` 与 `format_info` 在未知名字下的报错措辞同口径
- [X] T008 [US1] 在 `crates/ricow/src/supervisor/server.rs` 修正既有测试 [L534-539](../../crates/ricow/src/supervisor/server.rs#L534-L539) 的**缺陷期期望**(原断言 `Request::Stop{name:"nope"}` → `ok=true` + `exited=true`, 正是本缺陷), 改为断言失败; 并在既有真实 TCP 回环测试(`serve_and_client_over_real_tcp`, 非 mock、不假 token)中断言: 不存在的名字 → `ok=false` 且 `error` 含 `未找到策略或实例`(plan D9/D10③, contracts/control-channel.md §二出口 A)
- [X] T009 [US1] 手工验收 [quickstart.md](./quickstart.md) 场景 A(不存在名字 × `stop` / `stop --close-all` / `restart`)、场景 E(daemon 未运行时的优先级, FR-009)、场景 F(空串 / 纯空白), 逐条比对 [contracts/cli-stop.md](./contracts/cli-stop.md) §一 §二 的逐字文案与退出码

**检查点**: 至此用户故事 1 应功能完整且可独立测试 —— 打错名字不再静默通过

---

## 阶段 4: 用户故事 2 - 已经停着的策略, 回执不再自相矛盾 (优先级: P2)

**目标**: 名字**存在但未在运行**时, 回执只陈述"未在运行 (无需停止)"这一件事, 退出码 0, 不出现"已停止"头衔; 同时真实停机路径逐字零回归

**独立测试**: 启动一个策略 → 停掉 → 再停一次, 检查第二次回执措辞与退出码

- [X] T010 [US2] 在 `crates/ricow/src/supervisor/server.rs` 的真实 TCP 回环测试中新增用例: 仅有 `strategies/<n>.toml`、无台账、未运行 → `ok=true` + `already_stopped=true` + `note` 逐字为 `该策略未在运行 (无需停止)`(plan D10③, contracts/control-channel.md §三 硬契约①: `already_stopped=true` 时 note 必填且固定)
- [X] T011 [US2] 在 `crates/ricow/src/commands/ctrl.rs` 的 `stop_daemon()` 中实现文案分派: 先判 `report.already_stopped` —— 为真则**跳过**"已停止 / 未观测到退出"头衔, 只走既有 `if let Some(note)` 输出; 为假则三条既有分支(`Some(0)` / `Some(code)` / `None`)逐字不变(plan D6, FR-006)
- [X] T012 [US2] 在 `crates/ricow/src/commands/ctrl.rs` 新增文案分派单测 4 组合: `already_stopped=true` → 输出**不含**"已停止"、含"未在运行 (无需停止)"; `already_stopped=false` 配 `exit_code = Some(0)` / `Some(非0)` / `None` 三情形 → 输出与改动前逐字一致(plan D10②, contracts/cli-stop.md §二 2.3)
- [X] T013 [US2] 手工验收 [quickstart.md](./quickstart.md) 场景 B(半存在: 仅 TOML、从未启动)与场景 C(启动 → 停止 → 再停一次), 断言退出码 0 且回执**不含**"已停止"
- [X] T014 [US2] 回归比对真实停机路径(SC-005): 对**运行中**的 demo 实例执行 `ricow stop <name>` 与 `ricow stop <name> --close-all`, 输出与退出码与改动前逐字一致(含模式感知清理提示), 结果记入验收记录

**检查点**: 用户故事 1 与 2 均独立可用 —— 假成功消失, 且"已停止"与"未在运行"不再互斥出现

---

## 阶段 5: 用户故事 3 - 终端与 AI 对同一个名字给同一句结论 (优先级: P3)

**目标**: 终端停机命令与 AI 侧停机工具共用同一份判定与文案基底, 对同一名字必然同结论

**独立测试**: 对同一组名字(不存在 / 仅 TOML / 存在未运行 / 运行中), 分别走终端与 AI 停机工具, 逐条比对结论

- [X] T015 [US3] 在 `crates/ricow/src/ai/tools.rs` 的 `prepare_stop()` 中改用同一判定(plan D8, FR-013/FR-015): `views()` 找不到 **且** `strategy_name_exists()` 判否 → 返回 `unknown_name_message()` + `可用 instance_status 确认现状`; 找不到**但**判是(有 TOML 无台账) → 归入既有"策略 X 当前未在运行…无需停机"分支; 找到但 `!running` → 既有分支不动。`ensure_daemon_running()` 的调用位置与**优先级**(先于名字判定, FR-009)**不动**
- [X] T016 [US3] 在 `crates/ricow/src/ai/tools.rs` 新增/更新单测: 不存在的名字 → 报错含 `未找到策略或实例` 且指向 `instance_status`; 仅有 TOML → 归入"未在运行 / 无需停机"; daemon 未运行 → 仍先如实报 daemon 未运行(**不得**被新的名字判定绕过, 且不影响既有 [tools.rs:2661-2697](../../crates/ricow/src/ai/tools.rs#L2661-L2697) 的 daemon 未运行路径断言)
- [X] T017 [US3] 手工验收 [quickstart.md](./quickstart.md) 场景 G(SC-004): 同一组 4 类名字分别走终端与 AI 会话内停机工具, 逐条比对结论与措辞口径, 冲突用例数必须为 0
  - **实测通道说明**: 本任务的 quickstart 方法(`ricow ai --plain` 交互 REPL)对**真实 TTY 用户**成立 —— `commands/ai.rs:40` 省略 prompt 时 `interactive=true`, L49 `has_input_channel = stdin().is_terminal()` = true。但**自动化执行环境 stdin 非 TTY**, 故 `ai/session.rs:181` 的 `interactive = opts.interactive && opts.has_input_channel` 为 false, `ai/tools.rs` L1011-1013 在 `prepare_*` 之前即"非交互短路"返回终端命令提示(这是 022/024 D5 的既有设计, 非缺陷)。因此本次改用**另一条交互通道** Web(`commands/web.rs:118` 无条件 `interactive:true, has_input_channel:true`)在同一隔离 root 实测四类名字: 不存在 → 与终端同基底 `未找到策略或实例 …` 且未登记待确认动作; 仅 TOML → `当前未在运行(从未启动, 无运行台账); 无需停机`; 存在未运行 → `当前未在运行(上次模式=demo); 无需停机`。**冲突用例数 = 0**(SC-004 满足)。若需在终端侧复现, 由用户在真实 TTY 运行 `ricow ai --plain` 并按 §7 逐条问即可。

**检查点**: 所有用户故事应独立可用 —— 终端与 AI 双口径分叉消除

---

## 阶段 6: 打磨与横切关注点

**目的**: 全量门禁、不回归面复核与文档一致性核对

- [X] T018 [P] 在仓库根执行 `cargo fmt --all -- --check`(零差异), 随后 `cargo clippy --workspace --all-targets --locked -- -D warnings`(零警告)
- [X] T019 在仓库根执行 `cargo test -p ricow --bin ricow`, 断言 `failed = 0` 且 passed 数**不倒退**于 T001 记录的基线
- [X] T020 [P] 不回归复核(FR-017, 验收动作 10): `ricow status` / `ricow status <name>` / `ricow info` / `ricow logs` / `ricow list` / `ricow fills` 的未知名字行为与输出**逐字不变**
- [X] T021 端到端验收改用 **demo 测试网真实实例**(不 mock、不假 token): 在 `RICOW_ROOT` 沙箱内 `start --demo` → `stop` → `stop --close-all`, 汇总 quickstart.md 场景 A~G 的实测输出
- [X] T022 [P] 文档体系一致性(plan D13): 按实测复核 `specs/architecture.md` 第 56 行停机章节与第 117 行的 `start|stop|restart` 描述、以及 `specs/roadmap.md` 记录; **无与本次冲突的现状描述则不改文档**, 结论记入 converge 阶段

---

## 依赖与执行顺序

### 阶段依赖

- **搭建 (阶段 1)**: 无依赖 —— 可立即开始
- **地基 (阶段 2)**: 依赖搭建完成 —— 阻塞所有用户故事
- **用户故事 (阶段 3+)**: 全部依赖地基完成
  - 之后用户故事可并行(若有资源)
  - 或按优先级顺序串行 (P1 → P2 → P3)
- **打磨 (最后阶段)**: 依赖所有目标用户故事完成

### 用户故事依赖

- **用户故事 1 (P1)**: 地基后即可开始 —— 不依赖其他故事(仅需 T003/T004/T005)
- **用户故事 2 (P2)**: 地基后即可开始 —— T011/T012 依赖 T005 的协议字段; T010 与 T006 落在**同一函数**`stop()`, 故实测顺序应在 T006 之后(编译面无依赖, 文件面有冲突)
- **用户故事 3 (P3)**: 地基后即可开始 —— T015 依赖 T003/T004(同一判定与文案), 与 US1/US2 无文件冲突

### 每个用户故事内部

- 先判定/文案地基后实施
- 先实现后补测试断言(本变更非 TDD, 测试与实现同文件同任务组)
- 先终端侧(US1/US2)后 AI 侧(US3)
- 完成当前故事后再进入下一优先级

### 并行机会

- 标记 [P] 的地基任务可并行(T005 与 T003/T004 不同文件)
- US1 与 US3 可由不同执行者并行实施(文件集不相交: `supervisor/server.rs` vs `ai/tools.rs`)
- 标记 [P] 的打磨任务可并行(T018 / T020 / T022 操作面互不冲突)
- **不可并行**: T003 与 T004(同文件 `commands/instances.rs`)、T006 与 T010(同函数 `server.rs#stop`)、T011 与 T012(同文件 `commands/ctrl.rs`)

---

## 并行示例: 地基与用户故事

```bash
# 阶段 2 并行启动(不同文件):
任务: "在 crates/ricow/src/commands/instances.rs 新增 strategy_name_exists() 与 unknown_name_message()"
任务: "在 crates/ricow/src/supervisor/proto.rs 给 StopReport 新增 already_stopped 字段并更新测试"

# 阶段 3 与阶段 5 并行启动(文件集不相交):
任务: "在 crates/ricow/src/supervisor/server.rs 实现 stop() 名字判定三分支"
任务: "在 crates/ricow/src/ai/tools.rs 让 prepare_stop() 改用同一判定"
```

---

## 实施策略

### MVP 优先 (仅用户故事 1)

1. 完成阶段 1: 搭建(拿到测试基线)
2. 完成阶段 2: 地基(关键 —— 阻塞所有故事)
3. 完成阶段 3: 用户故事 1
4. **停下验证**: 独立测试用户故事 1 —— `ricow stop no-such-strategy` 退出码非 0 且无"已停止"字样
5. 就绪则交付(此即消除本轮唯一安全性问题的增量)

### 增量交付

1. 完成搭建 + 地基 → 地基就绪
2. 添加用户故事 1 → 独立测试 → 交付(MVP: 假阴性回执消除)
3. 添加用户故事 2 → 独立测试 → 交付(文案自相矛盾消除)
4. 添加用户故事 3 → 独立测试 → 交付(双口径分叉消除)
5. 每个故事在不破坏既有故事的前提下增加价值

---

## 备注

- [P] 任务 = 不同文件, 无依赖
- [故事] 标签将任务映射到具体用户故事以保持可追溯
- 每个用户故事应可独立完成与测试
- **不新增**任何文件、模块、依赖、CLI 命令或参数(plan D12)
- **明确不改动**: `commands/mod.rs`、`supervisor/client.rs`、`supervisor/ledger.rs`、`supervisor/procs.rs`、`ai/session.rs`(5 处 `stop_daemon` 调用点)、`commands/daemon.rs`、`commands/run.rs`、`commands/instances.rs` 的既有函数体、以及 `restart` / `restart_flags` / `start`
- **不做** `git commit`: 宪法行 42 —— 用户明确说"提交"才可提交, 验收过程只改工作区
- 每个任务或逻辑组完成后可直接进入下一任务, 无需中途提交
