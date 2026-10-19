# 056 — 启动门禁收口（Wave 1 起手）

**功能目录**: `specs/changes/056-startup-gate-convergence`
**立项日期**: 2026-10-18
**来源**: `specs/research/requirements-optimization-plan-2026-10.md`（技术总监 + 项目总监评审）Wave 1 第 2 项 + roadmap「下一步」第 8 项
**编号依据**: 目录最大 054，但全仓已引用到 `055`（见 `specs/changes/README.md` 编号约定：目录 ∪ 全仓引用取最大 +1）→ 取 **056**

---

## 一、需求来源与背景

`specs/roadmap.md`「下一步」清单第 8 项（2026-10-05 审计）状态为**未落码，待用户拍板**：

> **第 8 项 · 策略门禁覆盖面收口 + 运行态可见性**
> - 可见性缺口：`GET /api/runs` / `status` 不带 `declared` / `duplicate_of`，用户看不到运行中实例是不是未声明副本。
> - 覆盖面缺口：`catalog::run_block` 只 4 个调用点；公共汇聚点 `ctrl::start_daemon` 自身**无闸**却被 7 处调用 —— **对话里让 AI「启动 X」照样能起未声明副本**。
> - 收口顺序陷阱：`ctrl::restart` 是**先 stop 后 start**，闸只加 `start_daemon` 会出现「实例被停掉、没起来」（可能带未平仓敞口）。

2026-10-18 用户指示「按建议执行」，本变更取评审建议：**第 8 项立即实施，且门禁收口必须与 `restart` 先判后停绑定落地，不允许只做一半。**

---

## 二、目标（FR）

| 编号 | 需求 | 验收 |
|:--|:--|:--|
| FR-1 | `ctrl::start_daemon` 成为**唯一启动汇聚闸**，7 处调用点（CLI start / Web runs / 对话 5 条 L1 动作）**全部**受 `catalog::run_block` 约束 | 每条启动路径都能被同一门禁拒 |
| FR-2 | 门禁拒绝时**如实回报策略名 + 原文原因**，不静默跳过、不换措辞 | 拒绝文本 = `run_block` 原文 |
| FR-3 | `restart` **先判后停**：门禁判定在 `stop` **之前**完成，拒则**实例保持运行**、不做任何停机动作 | 构造被拒策略 → 断言实例仍在 running |
| FR-4 | `GET /api/runs` / `/api/strategies/{id}/status` 返回 `declared` / `duplicate_of`，用户能看出运行中实例是不是未声明副本 | 响应体含两字段 |
| FR-5 | 门禁对**未知 id 放行**（交由 daemon 按 404 处理），对**内置 id 放行** | 与 `run_block` 既有单测口径一致 |

## 三、非目标（明确不做）

- **不做**运行态可见性以外的任何 UI 改造（本轮只补数据字段，前端消费留给后续轮）。
- **不做**「修掉未声明副本」——门禁只负责**拦住并说清原因**，不替用户改配置、不自动补清单。
- **不做** `start_daemon` 之外的第二道闸（避免多处判定口径漂移）。
- **不做**本轮 Wave 1 的 OMS 部分（最小 OMS 另立变更，见 §六）。

## 四、实现要点

### 4.1 闸落在 `start_daemon` 内
把 `catalog::run_block(&name)` 检查放进 `ctrl::start_daemon` 函数体首行（`crates/ricow/src/commands/ctrl.rs:224`），返回 `CoreError::InvalidArgument(why)`。

理由：这是**唯一的公共汇聚点**，7 处调用点全部经过它（`ctrl.rs:88`、`web/runs/mod.rs:375`、`ai/session.rs:1234/1295/1348/1403/1436`）。闸加在调用方 = 下一条新路径必然漏。

### 4.2 `restart` 先判后停
`ctrl::restart`（`ctrl.rs:300`）改为：
1. 先做门禁判定（复用同一份 `run_block`）；
2. 被拒 → 直接 `Err`，**不执行 `stop`**；
3. 放行 → 原有 stop + start 逻辑不变。

**这是本变更最关键的一条**：若只加 4.1 而不改 `restart`，被拒策略会先被停掉再拒绝启动，可能带着未平仓敞口静默下线 —— 比不做门禁更糟。

### 4.3 拒绝文案
原样透出 `run_block` 的中文原因（该文案已含三个自助改法），**不加前缀、不改写**。前缀只在必要时补策略名（`run_block` 原文已含 id）。

## 五、测试要求

| 类型 | 断言 |
|:--|:--|
| 单测（纯函数） | 门禁判定对内置 id / 未知 id / 已声明副本 / 未声明副本四态的结果 |
| 单测（`restart` 顺序） | 被拒策略 → `restart` 返回 `Err` 且**实例仍在 running**（不得出现「停掉没起来」） |
| 单测（汇聚性） | `start_daemon` 体内含门禁调用（防止后续有人把闸挪走） |
| 回归 | 全量 `cargo test --workspace` 通过；`ci_grep_gates.sh` 五条红线全绿 |

## 六、后续（不在本变更内）

评审文档 Wave 1 的其余部分另立变更：
- **最小 OMS**：在途订单表 + `client_order_id` 幂等 + 5 态状态机（风险：DB schema 迁移，采用**新增表**而非改 `orders`）
- Wave 2：组合敞口只读视图 + kill switch + P0-D 固定常量名义上限（ⓑ 方案）
- Wave 3：下单改异步 + 前端门禁进 CI
- Wave 4：超限文件拆分