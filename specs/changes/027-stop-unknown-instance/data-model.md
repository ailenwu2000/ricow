# 027 数据模型: 停机动作对"不存在的策略名"误报成功

**规格**: [spec.md](./spec.md) | **计划**: [plan.md](./plan.md) | **调研**: [research.md](./research.md) | **日期**: 2026-09-20

本变更**不引入新的持久化结构**。涉及的全部"实体"都是既有文件与既有协议对象; 数据模型要说明的是**判定口径**(名字存在性)与**回执形状**(停机三分支)这两件事的取值规则。

## 一、判定来源并集 (FR-001, D1)

`strategy_name_exists(root, name)` = 下列任一为真:

| # | 来源 | 路径 | 判定方式 | 命中含义 |
|---|---|---|---|---|
| S1 | 已部署清单 | `<root>/strategies/<name>.toml` | `Path::is_file()` | 该名字**曾被执行 `create`/`deploy` 部署过** |
| S2 | 实例台账 | `<root>/run/<name>.json` | `ledger::list_instances(root)` 的返回集合含该名 | 该名字**曾经由 `ricow start` 启动过** |

- **取并集, 缺一不算不存在** (D1)。判否 = S1 与 S2 **皆无**。
- 前置短路: `name.trim().is_empty()` → 直接 `false`(不查盘)。**理由**: 空名会拼出 `strategies/.toml` 与 `run/.json`, 结果会取决于磁盘上是否存在这两个特殊文件名, 必须显式前置判否 (FR-008)。
- `ledger::list_instances` 自身过滤掉 `run/daemon.json`([ledger.rs:170](../../../crates/ricow/src/supervisor/ledger.rs#L170) 的 `.filter(|n| n != "daemon")`), 故名字 `daemon` 不会因 daemon 自身的心跳文件而被判为"存在" —— 这是既有正确行为, 本次沿用。
- 与 `ricow status <name>` / `ricow info <name>` 的口径**逐字一致**: 它们在 `view.is_none() && cfg.is_none()` 时报"未找到策略或实例"([instances.rs:253-258](../../../crates/ricow/src/commands/instances.rs#L253-L258)), 即"socket 视图无 && TOML 无"→ 两者皆无。本次把同一口径搬到停机路径。

### 判定来源对照 (四类名字)

| 名字类别 | S1 TOML | S2 台账 | 运行中(`children`) | 判定 | 停机回执分支 |
|---|---|---|---|---|---|
| **不存在** | 无 | 无 | 无 | `false` | 分支 ① 报错 |
| **半存在**(已部署、从未启动) | 有 | 无 | 无 | `true` | 分支 ② 未在运行 |
| **存在但未运行**(跑过又停了) | 有或无 | 有 | 无 | `true` | 分支 ② 未在运行 |
| **运行中** | 有或无 | 通常有 | **有** | (不进判定) | 分支 ③ 既有停机 |

> 第 4 类之所以"不进判定": daemon 的 `stop` 先从 `state.children` 取句柄, **取到即直接走既有停机路径**, 名字判定只在句柄缺失时才发生 ([server.rs:213-225](../../../crates/ricow/src/supervisor/server.rs#L213-L225))。

## 二、停机回执三分支 (FR-002 / FR-006 / FR-007, D3/D4/D6)

三个分支的判定位置、协议响应、字段取值、CLI 输出与退出码:

### 分支 ① 名字不存在

| 项 | 取值 |
|---|---|
| 判定位置 | daemon `stop()` 句柄缺失分支内, `!strategy_name_exists(&root, name)` |
| 协议响应 | `Response { ok: false, error: Some("未找到策略或实例 {name} (strategies/{name}.toml 不存在, 也无实例台账); 可用 ricow list 查看现有实例"), data: None }` |
| `StopReport` | **不产出** |
| CLI stdout | (无) |
| CLI stderr | `错误: invalid argument: 未找到策略或实例 {name} (strategies/{name}.toml 不存在, 也无实例台账); 可用 ricow list 查看现有实例` |
| 退出码 | **1** |
| 禁止出现的字样 | `已停止` / `未在运行` / `无需停止` (FR-003) |

### 分支 ② 名字存在但未运行

| 项 | 取值 |
|---|---|
| 判定位置 | 同一分支内, `strategy_name_exists(&root, name)` 为真 |
| 协议响应 | `Response { ok: true, data: Some(StopReport) }` |
| `StopReport` 字段 | `name` = 入参; `exited = true`; `graceful = true`; `exit_code = None`; `waited_ms = 0`; `note = Some("该策略未在运行 (无需停止)")`; **`already_stopped = true`**(新增) |
| CLI stdout | **仅**一行: `该策略未在运行 (无需停止)` |
| 退出码 | **0** |
| 禁止出现的字样 | `已停止` (FR-006) |

> `exited`/`graceful`/`note` 的取值**逐字保持现状**(D6) —— 变的只有两点: daemon 侧新增 `already_stopped=true`; CLI 侧据此**跳过**"已停止 / 未观测到退出"头衔, 只印 note。

### 分支 ③ 名字存在且运行中 (本次一行不动)

| 项 | 取值 |
|---|---|
| 判定位置 | 不涉及名字判定; 从 `state.children` 取到句柄后直接进入 |
| `StopReport` | 既有形状不变, `already_stopped` 缺省 `false` |
| CLI stdout | 既有三条分支逐字不变, 见 [contracts/cli-stop.md](./contracts/cli-stop.md#三运行中停机现状逐字保留) |
| 退出码 | **0** |

## 三、停机回执的状态迁移 (StopReport)

```
                 ┌──────────────────────────────────────────┐
   stop(name) ──▶│ children 命中?                            │
                 └───────┬──────────────────────┬───────────┘
                         │ 是                    │ 否
                         ▼                       ▼
              ┌────────────────────┐   ┌──────────────────────────┐
              │ 既有停机流程        │   │ strategy_name_exists?    │
              │ (指令→等待→台账)   │   └────┬─────────────────┬───┘
              └─────────┬──────────┘        │ 否              │ 是
                        │                   ▼                 ▼
                        │            ┌────────────┐  ┌──────────────────────┐
                        │            │ Response:: │  │ StopReport{          │
                        │            │  err(...)  │  │  exited=true,        │
                        │            │ exit 1     │  │  already_stopped=true│
                        │            └────────────┘  │ }  exit 0            │
                        │                            └──────────────────────┘
                        ▼
        ┌────────────────────────────────────────────┐
        │ exited=true  → StopReport{already_stopped: │
        │                 false} + 模式感知清理提示  │
        │ exited=false → StopReport{exited:false,    │
        │                 note="停机超时 …"}         │
        │  (两者均 exit 0, 逐字保留现状)              │
        └────────────────────────────────────────────┘
```

## 四、`StopReport.already_stopped` 字段契约

| 属性 | 值 |
|---|---|
| 类型 | `bool` |
| serde 属性 | `#[serde(default)]`(不加 `skip_serializing_if` —— 显式输出 `false` 便于人工 `nc` 核对) |
| 默认值(旧 daemon 缺字段) | `false` |
| `true` 的含义 | 停机请求被接受, 但该名字**本来就未在运行**, 本次**没有任何动作被执行**(无指令下发、无等待、无台账写入、无清理) |
| `false` 的含义 | 既有语义 —— 要么真的执行了停机, 要么停机超时未观测到退出 |
| 与 `note` 的关系 | **`already_stopped=true` 时 `note` 必填且固定为 `"该策略未在运行 (无需停止)"`**(CLI 在该分支只输出 note, 见 R7) |

**兼容矩阵**(需同时读 [contracts/control-channel.md](./contracts/control-channel.md)):

| CLI | daemon | 行为 |
|---|---|---|
| 旧 | 旧 | 现状(含本缺陷) |
| 旧 | 新 | 名字不存在 → 旧 CLI 打印 `错误: invalid argument: …` + exit 1(旧 CLI 的 `call_ok` 同样把 `ok=false` 映射为错误); 存在未运行 → 旧 CLI 仍打印"已停止"+note(旧 CLI 不认识新字段) |
| **新** | 旧 | `already_stopped` 缺省 `false` → 新 CLI 走既有三条分支, **与今日行为逐字一致**(无反向假阴性) |
| 新 | 新 | 本次目标行为 |

> 本次交付的验证目标是"新 CLI + 新 daemon"一行(两者同版本发布)。其余组合只要求"不倒退、不假阴性", 即上表。

## 五、边界取值表

| 边界 (spec §三 边界情况) | 期望取值 | 依据 |
|---|---|---|
| daemon 未运行 | `Client::connect` 先失败 → `InvalidArgument("daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start")`, **优先于**名字判定 | FR-009; [client.rs:19-32](../../../crates/ricow/src/supervisor/client.rs#L19-L32) 实证 |
| `strategies/<n>.toml` 存在但从未启动 | `strategy_name_exists = true` → 分支 ② 未在运行, exit 0 | D1 / 用户拍板 |
| 名字存在且运行中 | 分支 ③ 既有行为 + 模式感知清理提示 | FR-007 |
| 名字存在, 停机超时未观测到退出 | `exited=false`, `graceful=false`, `note="停机超时 (30s) 未观测到退出; 未强制终止, 请手工核对 (pid …)"`, exit 0 | FR-016(现状不变) |
| 名字为空串 / 纯空白 | `strategy_name_exists = false` → 分支 ① 报错, **不得**落入分支 ② | FR-008 |
| 同时有同名 TOML 与同名台账 | `true`(并集), 不报错 | D1 |
| `restart` 第一步失败 | 不启动任何东西(`stop(...).await?` 的 `?` 立即返回) | FR-011 |
| 名字 = `daemon` | `false`(台账枚举排除 `run/daemon.json`) → 分支 ① 报错。**期望行为**: `daemon` 不是策略名, 停机 daemon 用 `ricow daemon stop` | 既有 `list_instances` 过滤 |
| 大小写 / 前后空白 / 特殊字符 | 沿用既有名字安全判定(`safe_strategy_name` / `validate_new_name`), **不放宽也不收紧** | FR-008 / spec §二"不做" |

## 六、不受影响的实体 (FR-017, D11)

| 实体 | 结论 |
|---|---|
| `InstanceView` | 零改动 |
| `Request` / `Envelope` / `Response` | 零改动 |
| `strategies/<name>.toml` 的解析与写入 | 零改动 |
| `run/<name>.json` 的读写 | 零改动(仅被判定函数**读取**枚举名) |
| SQLite 全部表(`fills` / `orders` / `positions` / …) | 不涉及 —— 本次不打库 |
