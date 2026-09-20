# 契约: 控制通道 `Request::Stop` 与 `StopReport`

**规格**: [../spec.md](../spec.md) | **计划**: [../plan.md](../plan.md) | **数据模型**: [../data-model.md](../data-model.md) | **日期**: 2026-09-20

控制通道 = 仅绑定 `127.0.0.1` 的 TCP; 一行一个 JSON 对象, 每个请求携带 `token`(来自 `run/daemon.json`)。本文件锁定 `stop` 指令的**线上格式与语义**。

## 一、请求: `Request::Stop` (形状不变)

```json
{"token":"<run/daemon.json 中的 token>","cmd":"stop","name":"shannon-grid-eth","close_all":false}
```

| 字段 | 类型 | 缺省 | 语义 |
|---|---|---|---|
| `token` | string | 必填 | 常量时间比较; 不匹配 → `Response::err("令牌不匹配 (控制通道拒绝)")` |
| `cmd` | `"stop"` | 必填 | 标签枚举, 未知值解析失败 |
| `name` | string | 必填 | 策略名; **本次新增其存在性判定** |
| `close_all` | bool | `false`(`#[serde(default)]`) | 停机时平掉策略持仓; **不参与名字判定** |

**本次对 `Request` 零改动**: 不加字段、不加变体、不改形状(D12)。

## 二、响应: 两个出口

### 出口 A — 失败 (`ok=false`)

```json
{"ok":false,"error":"未找到策略或实例 no-such-strategy (strategies/no-such-strategy.toml 不存在, 也无实例台账); 可用 ricow list 查看现有实例"}
```

- 触发条件: `strategy_name_exists(root, name) == false`。
- `data` 缺省(`None`), **不产出 `StopReport`**。
- CLI 侧经 [Client::call_ok](../../../crates/ricow/src/supervisor/client.rs#L66-L73) 映射为 `CoreError::InvalidArgument` → 打印 `错误: invalid argument: <error>` + exit 1。
- 该分支**只可能由新 daemon 发出**; 旧 daemon 对任何名字都回 `ok=true`(本次要修的缺陷)。

### 出口 B — 成功 (`ok=true`)

```json
{"ok":true,"data":{"name":"shannon-grid-eth","exited":true,"graceful":true,"waited_ms":0,"note":"该策略未在运行 (无需停止)","already_stopped":true}}
```

`data` = `StopReport`。

## 三、`StopReport` 形状

| 字段 | 类型 | 缺省(旧 daemon) | 语义 | 本次 |
|---|---|---|---|---|
| `name` | string | 必填 | 目标策略名 | 不变 |
| `exited` | bool | 必填 | 是否观测到进程退出 | 不变 |
| `graceful` | bool | 必填 | 是否经停机指令优雅退出(`false` = 超时未退) | 不变 |
| `exit_code` | int? | `null` | 子进程退出码; 未知为 `null` | 不变 |
| `waited_ms` | u64 | 必填 | 等待退出耗时(毫秒) | 不变 |
| `note` | string? | `null` | 面向用户的附加说明(清理提示 / 超时说明 / "无需停止") | 不变 |
| **`already_stopped`** | **bool** | **`false`** | **`true` = 该名字本来就未在运行, 本次没有任何动作被执行** | **新增** |

### 新增字段的取值规则

| 场景 | `exited` | `graceful` | `exit_code` | `waited_ms` | `note` | `already_stopped` |
|---|---|---|---|---|---|---|
| 名字存在但未运行 | `true` | `true` | `null` | `0` | `"该策略未在运行 (无需停止)"` | **`true`** |
| 运行中, 优雅退出 | `true` | `true` | 实际退出码 | 实际 | 模式感知清理提示 | `false` |
| 运行中, 超时未观测到退出 | `false` | `false` | `null` | `≥ STOP_WAIT` | `"停机超时 (30s) 未观测到退出; 未强制终止, 请手工核对 (pid {pid})"` | `false` |

**硬契约**:

1. `already_stopped == true` ⇒ `note` **必须**为非空且固定为 `"该策略未在运行 (无需停止)"`。
   **理由**: 新 CLI 在该分支**只输出 `note`**([ctrl.rs#stop_daemon](../../../crates/ricow/src/commands/ctrl.rs#L220-L257)); 若 `note` 缺失就会退化成空输出(风险 R7)。该固定文案由 daemon 侧生成, 不由 CLI 拼接。
2. `already_stopped == true` ⇒ 未下发停机指令、未等待、未写台账、未触发清理。它是"什么都没做"的**显式声明**。
3. `already_stopped == false` ⇒ 三条既有语义**逐字不变**(FR-016 / SC-005)。

## 四、兼容规则

### 序列化

- `already_stopped` 使用 `#[serde(default)]`, **不加** `skip_serializing_if` → 新 daemon 总是显式输出该字段(便于人工用 `nc` 核对线上格式)。
- 缺该字段的旧格式 JSON 解析为 `false`。

### 混跑矩阵

| CLI | daemon | 名字不存在 | 名字存在未运行 | 运行中停机 |
|---|---|---|---|---|
| 旧 | 旧 | 假成功 `已停止` + exit 0(**缺陷**) | `已停止` + `无需停止` + exit 0(**文案矛盾**) | 现状 |
| 旧 | 新 | `错误: invalid argument: 未找到策略或实例 …` + exit 1 | 旧 CLI 不识别新字段 → 仍打印 `已停止` 头衔 + note(**可接受**: 与"旧+旧"同措辞, 不产生新的假成功) | 现状 |
| **新** | 旧 | 旧 daemon 回 `ok=true` → 新 CLI 见 `already_stopped` 缺省 `false` → 走既有三条分支(**与今日逐字一致, 无反向假阴性**) | 同左 | 现状 |
| **新** | **新** | **本次目标行为** | **仅输出 `该策略未在运行 (无需停止)` + exit 0** | 现状(逐字保留) |

> 交付与验收只覆盖"新 + 新"一行(同版本发布)。其余组合的约束是"**不倒退、不引入反向假阴性**"。

## 五、AI 侧同口径 (FR-013 / D2 / D8)

AI 停机工具(`stop_dry_run` / `stop_demo` / `stop_live` / `close_live`)与终端停机命令**共享同一判定与同一文案基底**:

| 环节 | 实现 | 位置 |
|---|---|---|
| 判定 | `commands::instances::strategy_name_exists(root, name)` | 唯一实现 |
| 文案基底 | `commands::instances::unknown_name_message(name)` | 唯一实现 |
| 停机执行 | `commands::ctrl::stop_daemon(root, name, close_all)` | 唯一实现(AI 不自己发 `Request::Stop`) |

`ai/tools.rs#prepare_stop` 的判定展开([tools.rs:1843-1855](../../../crates/ricow/src/ai/tools.rs#L1843-L1855)):

| 前置/条件 | 结论 | 文案 |
|---|---|---|
| `ensure_daemon_running()` 失败 | 先报 daemon 未运行(**优先级最高, 不变**) | 既有中文/英文文案 |
| `views()` 无该名 **且** `strategy_name_exists == false` | 拒绝(等价于出口 A) | `unknown_name_message(name)` + `; 可用 instance_status 确认现状` |
| `views()` 无该名 **但** `strategy_name_exists == true`(半存在) | 归入"未在运行, 无需停机" | `策略 {name} 当前未在运行(上次模式=?); 无需停机` |
| `views()` 有该名 且 `!running` | 同上(现有分支不动) | 同上 |
| `views()` 有该名 且 `running` | 既有模式匹配 + 确认块 | 逐字不变 |

**不改变**: `ensure_daemon_running()` 的位置与优先级; `prepare_restart_live()`; 四组确认块文案; `ai/session.rs` 的 5 处 `stop_daemon` 调用点。

## 六、验收断言 (单测/集成测)

| # | 断言 | 层级 |
|---|---|---|
| P1 | `StopReport` JSON 序列化含 `already_stopped` 字段 | proto 单测 |
| P2 | 缺 `already_stopped` 的旧格式 JSON → 解析为 `false` | proto 单测 |
| P3 | `Request::Stop{name:"nope"}`(无 TOML 无台账) → `ok == false` 且 `error` 含 `未找到策略或实例` | 真实 TCP 回环集成测 |
| P4 | `Request::Stop{name:"<仅 TOML>"}` → `ok == true` + `data["already_stopped"] == true` + `data["note"]` 非空 | 真实 TCP 回环集成测 |
| P5 | `Request::Stop{name:"<运行中>"}` → `data["already_stopped"] == false`(既有路径不回退) | 真实 TCP 回环集成测 |
