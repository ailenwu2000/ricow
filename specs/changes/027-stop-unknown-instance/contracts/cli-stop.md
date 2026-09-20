# 契约: `ricow stop` / `ricow restart` (终端)

**规格**: [../spec.md](../spec.md) | **计划**: [../plan.md](../plan.md) | **数据模型**: [../data-model.md](../data-model.md) | **日期**: 2026-09-20

本文档锁定停机命令的**对外可观测行为**: stdout / stderr 逐字文案与退出码。改动本文件中的任何一行文案或退出码 = 破坏契约。

## 一、命令面 (不变)

| 命令 | 参数 | 说明 |
|---|---|---|
| `ricow stop <name>` | `<name>` 位置参数(必填) | 停掉一个策略; 名字判定本次新增 |
| `ricow stop <name> --close-all` | `--close-all` 布尔标志 | 停机时平掉策略持仓; **不改变名字判定**(FR-005) |
| `ricow restart <name>` | `<name>` 位置参数(必填) | 停止(等清理完成) + 按原模式启动; 名字判定由 `stop` 继承 |

**本次不新增任何参数或子命令**(FR-018 / D12): 无 `--force`、无 `--yes`、无 `stop --all`。

## 二、输出与退出码契约 (名字判定四类)

### 2.1 名字不存在 → 分支 ①

```
$ ricow stop no-such-strategy
(无 stdout)

$ stderr:
错误: invalid argument: 未找到策略或实例 no-such-strategy (strategies/no-such-strategy.toml 不存在, 也无实例台账); 可用 ricow list 查看现有实例
```

| 项 | 契约 |
|---|---|
| stdout | **空** |
| stderr | `错误: invalid argument: ` + `unknown_name_message(name)` + `; 可用 ricow list 查看现有实例` |
| 退出码 | **非 0**(实测为 `1`) |
| 必须**不**包含 | `已停止`、`未在运行`、`无需停止` |
| 必须包含 | `未找到策略或实例` + 名字 + `ricow list`(FR-004 的可执行下一步) |

`ricow stop no-such-strategy --close-all` 与 `ricow restart no-such-strategy` 的输出与退出码**与上表完全一致**(`--close-all` 不改变判定; `restart` 在输出任何回执之前失败, FR-010)。

### 2.2 名字存在但未运行 → 分支 ②

```
$ ricow stop shannon-grid-eth         # 该策略已部署/曾启动, 当前不在跑
该策略未在运行 (无需停止)
$ echo $?
0
```

| 项 | 契约 |
|---|---|
| stdout | **仅一行** `该策略未在运行 (无需停止)` |
| stderr | 空 |
| 退出码 | **0** |
| 必须**不**包含 | `已停止` |
| 必须让用户明确知道 | 本次**没有动作被消耗**(无停机指令、无等待、无台账写入、无清理) |

适用于**半存在**情形(`strategies/<name>.toml` 存在但从未启动, 无 `run/<name>.json`)—— 同样是这一分支、这一退出码(spec §三 边界情况 + D1)。

### 2.3 运行中停机 (现状, 逐字保留)

`ricow stop <运行中的 name>` → 既有输出**一行不动**。四种子情形:

| 子情形 | stdout 第一行(逐字) |
|---|---|
| 优雅退出, 退出码 0 | `策略 {name} 已停止 (exit=0, 用时 {waited_ms}ms)` |
| 退出码非 0 | `策略 {name} 已停止, 但退出码 {code} (非正常退出; 详见 logs/{name}.log)` |
| 退出码未知 | `策略 {name} 已停止 (用时 {waited_ms}ms)` |
| 超时未观测到退出 | `策略 {name} 未观测到退出 (用时 {waited_ms}ms)` |

其后若 `note` 有值, 追加一行 `{note}` —— 内容包括**模式感知清理提示**:

| 场景 | note 内容(逐字) |
|---|---|
| 实盘停机 | `实盘停机(**真实资金**) …` |
| 测试网(demo)停机 | `测试网停机(模拟盘, 与真实资金无关) …` |
| `--close-all` | 含 `平掉策略持仓` 的说明 |
| Dry Run | 依脚本是否定义 `on_stop` 如实说明 |
| 无 TOML 可判定 | `无法判定清理实现` |
| 超时 | `停机超时 (30s) 未观测到退出; 未强制终止, 请手工核对 (pid {pid})` |

退出码一律 **0**(含超时 —— 现状即如此, 本次不改, FR-016)。

> 清理的**真实结果**由策略子进程写入 `logs/{name}.log`, CLI 不臆测(既有注释: "清理结果由策略进程如实写入日志; 这里不臆测结果")。

### 2.4 daemon 未运行 (现状, 优先于名字判定)

```
$ ricow stop any-name
错误: invalid argument: daemon 未运行 (无 run/daemon.json); 先执行 ricow daemon start
$ echo $?
1
```

| 项 | 契约 |
|---|---|
| 判定时机 | **早于**名字判定(`Client::connect` 失败即返回) |
| 文案 | 逐字不变(FR-009 / FR-017 对齐基准) |
| 退出码 | 非 0 |
| 必须**不**改口成 | `策略未在运行` / `无需停止` |

## 三、`restart` 的契约 (FR-010~FR-012)

`ricow restart <name>` = `stop(<name>, close_all=false)` + `start(<name>, 原模式)`。

| 情形 | 契约 |
|---|---|
| 名字不存在 | 在 `stop` 第一步失败 → 退出码非 0, **stdout 不出现任何**"策略 X 已停止"式回执, **不启动任何东西**(不得退化为"只启动不停止") |
| 名字存在但未运行 | `stop` 输出 `该策略未在运行 (无需停止)`(exit 0) → 继续 `start` 启动它(**既有行为, 本次不改**) |
| 名字存在且运行中 | 既有行为 + 既有模式提示(`注意: {name} 原本以**实盘**运行, …` / `原本以**测试网模拟盘(demo)**运行, …`), 按原模式重启, **不静默降级** |
| 模式提示的打印时机 | 仅在**停机前读到**该实例正在运行且模式为 live/demo 时打印; 名字不存在时 `prev_mode` 为 `None` → **无任何提示行** |

## 四、明确不变面 (FR-017 / D11)

以下命令的未知名字行为与输出**逐字不变**, 它们是本次的对齐基准而非改造对象:

| 命令 | 未知名字时的现状(逐字) |
|---|---|
| `ricow status <name>` | `错误: invalid argument: 未找到策略或实例 X (strategies/X.toml 不存在, 也无实例台账)` |
| `ricow info <name>` | 同上, 逐字一致 |
| `ricow logs <name>` | `错误: invalid argument: 日志不存在: …\logs\X.log (该策略尚未启动过?)` |
| `ricow list` | 无该名字(表格不含), 不作报错 |
| `ricow fills <name>` | `无成交记录 (strategy_id=X)` + exit 0(空集不是错误, 不纳入本次) |

## 五、验收对照 (人工逐条比对用)

| # | 命令 | 期望退出码 | 期望输出关键点 |
|---|---|---|---|
| C1 | `ricow stop no-such-strategy` | ≠0 | 含 `未找到策略或实例` + `ricow list`; 不含 `已停止`/`未在运行`/`无需停止` |
| C2 | `ricow stop no-such-strategy --close-all` | ≠0 | 同 C1(逐字一致) |
| C3 | `ricow restart no-such-strategy` | ≠0 | 同 C1; 且无任何"已停止"回执; 事后 `ricow list` 无该名 |
| C4 | `ricow stop <仅 TOML 未启动>` | 0 | 仅一行 `该策略未在运行 (无需停止)`; 不含 `已停止` |
| C5 | `ricow stop <曾经跑过已停>` | 0 | 同 C4 |
| C6 | `ricow stop <运行中>` | 0 | 与改动前逐字一致(含清理提示) |
| C7 | `ricow stop <运行中> --close-all` | 0 | 与改动前逐字一致(含 `平掉策略持仓`) |
| C8 | 停掉 daemon 后 `ricow stop <任意名>` | ≠0 | `daemon 未运行 (无 run/daemon.json)`; 不得改口 |
| C9 | `ricow stop ""` / `ricow stop "   "` | ≠0 | 落入"不存在"分支报错, 不得落入"未在运行" |
