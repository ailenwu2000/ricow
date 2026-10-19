# 056 实现计划

**输入**: [`spec.md`](spec.md) 的功能规格
**日期**: 2026-10-18

## 一、前置复核（改变了计划形状）

立项时打算做「最小 OMS + 门禁收口」。**开工前核对代码发现最小 OMS 已由 037 落地**
（`crates/ricow_engine/src/oms.rs` 530 行），组合敞口只读视图与遗留挂单接管同样已落地。
→ **计划缩小为只做「门禁收口 + 运行态可见性」**，Wave 1 其余部分（OMS 增强）另立变更。

详见评审文档 `requirements-optimization-plan-2026-10.md` 附录 B。

## 二、改动清单

| # | 文件 | 改动 |
|:--|:--|:--|
| 1 | `crates/ricow/src/commands/ctrl.rs` | 新增 `startup_gate(name)`：调 `catalog::run_block`，被拒转 `CoreError::InvalidArgument(why)` |
| 2 | `crates/ricow/src/commands/ctrl.rs` | `start_daemon` 首行调 `startup_gate`（建连**之前**判，避免先拉起 daemon 再失败） |
| 3 | `crates/ricow/src/commands/ctrl.rs` | `restart` 在 `stop` **之前**调 `startup_gate`（FR-3 先判后停） |
| 4 | `crates/ricow/src/web/runs/mod.rs` | `RunItem` / `StatusReply` 增 `declared: bool` + `duplicate_of: Option<String>` |
| 5 | `crates/ricow/src/web/runs/mod.rs` | 新增 `catalog_audit(name) -> (bool, Option<String>)` 纯查询，查不到回 `(true, None)` |
| 6 | `crates/ricow/src/web/runs/mod.rs` | `build_run_item` / `build_status` 填充新字段（各只查一次，不重复扫目录） |

## 三、关键决策

### 3.1 闸放 `start_daemon` 而非 `daemon` 服务端
`start_daemon` 是**CLI / Web / 对话 7 处**的公共汇聚点。放 daemon 服务端（`supervisor/server.rs`）
也能覆盖，但：① 需要 daemon 侧新增 catalog 依赖；② 拒绝会晚于建连，用户拿到的是协议错误而非可操作的中文原因。
**选 CLI 侧**。

### 3.2 Web 端点的既有门禁不删
`web/runs/mod.rs:296` 已有一份 `run_block`，那是**更早的 2026-10-05 修复**，在 mode 白名单之前、零副作用的位置。
保留它 → 闸后移了一道，结论一致、无副作用。**不删既有修复**（避免把"更早的一层防护"当成重复劳动删掉）。

### 3.3 `restart` 双判不优化掉
`restart` 判一次、`start_daemon` 又判一次 = 同进程内重复扫描目录。**刻意不去掉其中一处**：
省一次目录扫描 vs 让 `start_daemon` 的闸有被"优化"掉的风险，后者代价高得多（闸正是本变更的目的）。

### 3.4 `catalog_audit` 查不到回 `(true, None)`
内置策略的清单是编译期 `include_str!`，**不在用户 `strategies/` 目录里**，磁盘扫描查不到。
回 `(true, None)` 而非 `(false, ...)` —— 否则前端会给内置策略挂"未声明"假警告。
「查不到即已声明」在这里是事实陈述（内置恒已声明），不是猜测。

### 3.5 不做真机行为级断言
FR-3 的行为断言（被拒策略重启后实例仍在 running）需要真跑 daemon + 构造被拒副本，成本远高于收益。
改用**源码顺序静态护栏**（`restart_gates_before_stopping`：断言 `startup_gate` 的位置索引 < `stop`）。
顺序本身就是该需求的全部内容，且静态断言不受环境影响。

## 四、验证

| 环节 | 命令 |
|:--|:--|
| 编译 | `cargo build` |
| 单测 | `cargo test --workspace --no-fail-fast` |
| 门禁 | `bash scripts/ci_grep_gates.sh` |
| 格式 | `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` |

新增单测 3 条：`startup_gate_passes_builtin_and_unknown`、`startup_gate_reason_is_verbatim`、`restart_gates_before_stopping`。

## 五、遗留（本变更不做）

- 前端消费 `declared` / `duplicate_of` 两个字段（数据已到位，UI 展示另立）
- 真机端到端复跑（e2e / CDP）