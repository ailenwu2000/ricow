# 056 任务清单

**输入**: [`plan.md`](plan.md) 的实现设计
**日期**: 2026-10-18

## 门禁收口（T01–T05）

| # | 任务 | 文件 | 状态 |
|:--|:--|:--|:--|
| T01 | 新增 `startup_gate(name)`：`catalog::run_block` → `CoreError::InvalidArgument` | `commands/ctrl.rs` | ✅ |
| T02 | `start_daemon` 首行调闸（**建连之前**） | `commands/ctrl.rs` | ✅ |
| T03 | `restart` **先判后停**：闸在 `stop` 之前 | `commands/ctrl.rs` | ✅ |
| T04 | 确认 7 处调用点全部经过闸 | `ai/session.rs`×5 / `web/runs/mod.rs` / `ctrl.rs` | ✅（均为 `start_daemon` 的调用方） |
| T05 | **不改** `web/runs/mod.rs:296` 既有门禁（2026-10-05 的更早一层防护） | — | ✅ 有意保留 |

## 运行态可见性（T06–T08）

| # | 任务 | 文件 | 状态 |
|:--|:--|:--|:--|
| T06 | `RunItem` / `StatusReply` 增 `declared` + `duplicate_of` | `web/runs/mod.rs` | ✅ |
| T07 | `catalog_audit()` 纯查询，查不到回 `(true, None)`（内置不在用户目录） | `web/runs/mod.rs` | ✅ |
| T08 | 两个构造函数各只查一次（不重复扫目录） | `web/runs/mod.rs` | ✅ |

## 测试（T09–T11）

| # | 任务 | 状态 |
|:--|:--|:--|
| T09 | `startup_gate_passes_builtin_and_unknown`：内置 id / 未知 id 放行 | ✅ |
| T10 | `startup_gate_reason_is_verbatim`：闸结论 == `run_block` 结论（不改写） | ✅ |
| T11 | `restart_gates_before_stopping`：源码顺序护栏，闸位置 < stop 位置 | ✅ |

## 验证（T12–T14）

| # | 任务 | 状态 |
|:--|:--|:--|
| T12 | `cargo build` 编译通过 | ✅（首轮 E0631：`duplicate_of` 已是 `Option<String>`，去掉多余 `map`） |
| T13 | `cargo test --workspace` 全绿 | ✅ |
| T14 | `ci_grep_gates.sh` 五条红线 + `fmt` + `clippy -D warnings` | ✅ |

## 文档（T15–T16）

| # | 任务 | 文件 | 状态 |
|:--|:--|:--|:--|
| T15 | roadmap 第 8 项（门禁收口）状态改为已拍板并开工 | `specs/roadmap.md` | ✅ |
| T16 | roadmap 第 9 项（P0-D）记 **ⓑ 已拍板** + 三条边界（不做最大持仓/日亏/回撤熔断） | `specs/roadmap.md` | ✅ |

## 修正文档（T17）

| # | 任务 | 文件 | 状态 |
|:--|:--|:--|:--|
| T17 | **评审文档加附录 B 更正误判**：G1/G3/P0-A/P1 六项其实已由 037/038 落地，原结论作废；剩余项收敛为 G4/G2/G5/G6/G7 + P0-D | `specs/research/requirements-optimization-plan-2026-10.md` | ✅ |

## 明确不做（写下来免得当 bug 修）

1. **不做**最小 OMS —— 037 已落地（`ricow_engine/src/oms.rs`），评审初稿误判，附录 B 已更正
2. **不做**删 `web/runs/mod.rs:296` 的既有门禁 —— 那是 2026-10-05 的更早一层防护，保留
3. **不做**优化掉 `restart` 与 `start_daemon` 之间的重复判定 —— 省一次目录扫描不值得让闸有被"优化"掉的风险
4. **不做**前端消费新字段 —— 数据已到位，UI 另立
5. **不做**真机行为级断言（FR-3）—— 改用源码顺序静态护栏，见 plan §3.5
6. **本变更不含 P0-D 落码** —— 已拍板取 ⓑ，另立变更