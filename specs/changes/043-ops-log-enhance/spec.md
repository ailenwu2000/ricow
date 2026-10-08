# 043 运维日志增强: JSON 日志 + logs 级别过滤 + status 一览列

## 背景

竞品对标 (`.trae/documents/competitor-gap-analysis.md`) P1 项。实施前核实 (2026-09-29):

- `ricow status` / `ricow logs` 命令**已存在** (008), 密钥环境变量覆盖 (`RICOW_DEMO_KEY/SECRET` > ricow.toml) **已存在** —— 原计划第 6/7 项大部分已完成。
- 剩余真实差距: ① 日志仅纯文本 (无 JSON 选项); ② `logs` 不能按级别过滤; ③ `status` 表无最新成交/盈亏列, 一览需逐个 `info`。

用户口径不变: Web UI 不做, 风控不做。

## 需求

### FR-1 JSON 结构化日志
- 环境变量 `RICOW_LOG_FORMAT=json` 时, tracing_subscriber 以 JSON 行输出 (含时间戳/级别/target/字段); 缺省/其它值 = 现纯文本不变。
- daemon 子进程日志重定向 (logs/<name>.log) 自动跟随子进程格式 —— 无需 supervisor 改动。
- 依赖: `tracing-subscriber` serde/json feature (仅特性开关, 不换库)。

### FR-2 `ricow logs --level`
- `--level <LEVEL>`: 按级别过滤输出 (DEBUG/INFO/WARN/ERROR, 不区分大小写)。
- 兼容两种日志形态: 纯文本 (`ERROR ricow_engine: ...` 依 tracing fmt 的级别词) 与 JSON (`"level":"ERROR"`)。
- 未知级别报错; 与 `--follow` 组合可用。

### FR-3 `status` 一览增列
- 表格新增 `最新成交` (最后 fill 的 ISO 时间 + 价格×数量, 来自 db.rs FillRecord 最新一条) 与 `当日盈亏` 列。
- 查询失败/无数据显示 `-`, 不因单个实例 db 异常中断整表。
- 仅对有 TOML 的已部署实例查询; 无 db 文件静默跳过。

## 非目标

- `ricow optimize` (P2, 044)。
- 日志索引/集中采集 (Loki 等)。
- Web 展示。

## 验收

- 单测: logs 级别过滤对纯文本与 JSON 行均正确; status 列格式化对无数据/有数据两态。
- 手验: `RICOW_LOG_FORMAT=json` 启动 daemon 子进程后 logs/<name>.log 为 JSON 行; `ricow logs <name> --level ERROR --lines 200` 只出错误; `ricow status` 显示新列。
