# 056 收敛记录

**变更**: `specs/changes/056-startup-gate-convergence/`
**收敛日期**: 2026-10-18

## 一、交付内容

| 文件 | 改动 |
|:--|:--|
| `crates/ricow/src/commands/ctrl.rs` | 新增 `startup_gate()`；`start_daemon` 首行调闸；`restart` 先判后停；3 条新单测 |
| `crates/ricow/src/web/runs/mod.rs` | `RunItem`/`StatusReply` 增 `declared`/`duplicate_of`；新增 `catalog_audit()` |
| `specs/roadmap.md` | 第 8 项（门禁收口）状态更新；第 9 项（P0-D）记 ⓑ 已拍板 + 三条边界 |
| `specs/research/requirements-optimization-plan-2026-10.md` | 新增附录 B 更正误判 |

## 二、验收结果

| 项 | 结果 |
|:--|:--|
| `cargo build` | ✅ 通过（首轮 E0631 已修：`duplicate_of` 已是 `Option<String>`，去掉多余 `map`） |
| `cargo test --workspace --no-fail-fast` | ✅ **1043 passed / 0 failed**（基线 1040，**+3** 为本变更新增） |
| `bash scripts/ci_grep_gates.sh` | ✅ 五条红线全绿 |
| `cargo fmt --all -- --check` | ✅ 0 差异 |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✅ |
| 新增 3 条单测实跑 | ✅ `startup_gate_reason_is_verbatim` / `startup_gate_passes_builtin_and_unknown` / `restart_gates_before_stopping` |

## 三、FR 逐条验收

| FR | 验收方式 | 结果 |
|:--|:--|:--|
| FR-1 闸落在唯一汇聚点 | `start_daemon` 首行；7 处调用点全部经由它 | ✅ |
| FR-2 拒绝原因原样透出 | 闸只做 `Err(InvalidArgument(why))`，不改写；`startup_gate_reason_is_verbatim` 断言结论 == `run_block` 结论 | ✅ |
| FR-3 `restart` 先判后停 | 闸位置索引 < stop 位置索引（源码顺序护栏） | ✅ |
| FR-4 运行态可见性 | 两个响应体新增两字段并填充 | ✅（前端消费留后续） |
| FR-5 内置/未知 id 放行 | `startup_gate_passes_builtin_and_unknown` | ✅ |

## 四、本轮最重要的一件事：**修正评审文档的误判**

评审初稿（`requirements-optimization-plan-2026-10.md`）把 `framework-vs-commercial-2026-10.md` 当现状引用，得出
「G1 无订单状态机未做」「G3 组合敞口不可见未做」「P0-A 未做」「P1 六项未做」。**开工前逐条核对代码发现全部已在 037 / 038 落地**：

- `crates/ricow_engine/src/oms.rs`（530 行）= 037 P0-B 最小 OMS
- `crates/ricow_engine/src/exposure.rs` + `commands/exposure.rs` = 037 P0-C 组合敞口只读视图
- P0-A 遗留挂单接管、P1 六项 = 037 / 038

`roadmap.md` 第 9 项末尾其实原文写着「同报告的其他项已收尾：P0-A / P0-B / P0-C 随 037 落地；P1-A~P1-F 六项随 038 全部落地」——**是我引用时没读到这一段**。

**因此本变更的形状被前置复核改小了**：原计划 Wave 1 含「最小 OMS」，现缩为「仅启动门禁收口」。
更正写入附录 B，剩余项收敛为 **G4 全局 kill switch / G2 下单阻塞事件循环 / G5 单交易对三层绑定 / G6 前端门禁不进 CI / G7 超限文件 + P0-D（已拍板 ⓑ，待落码）**。

**教训已写进附录 B**：`specs/research/` 的对比报告是**当时的快照，不是活文档**，引用当"现状"前必须先核 roadmap 与代码。

## 五、刻意不做（防下一个人当 bug 修）

1. **最小 OMS 未做** —— 037 已落地，别重复建
2. **未删 `web/runs/mod.rs:296` 既有门禁** —— 那是 2026-10-05 的更早一层防护，闸后移一道无害
3. **未消除 `restart` 与 `start_daemon` 的重复判定** —— 省一次目录扫描不值得让闸有被"优化"掉的风险
4. **`catalog_audit` 查不到回 `(true, None)`** —— 内置策略清单是编译期资产、不在用户目录里，回 `false` 会给内置策略挂假警告
5. **未做真机行为级断言**（FR-3）—— 改用源码顺序静态护栏，见 plan §3.5
6. **前端未消费新字段** —— 数据已到位，UI 另立
7. **P0-D 未落码** —— 已拍板 ⓑ，另立变更

## 六、后续

- **P0-D 落码**（固定比例单笔名义上限，不进配置面）—— 已拍板 ⓑ，待立变更
- **G4 全局 kill switch**（跨实例一键停机）—— 037 只有单实例 `stop --close-all`
- **G2 下单改异步** —— `context.rs:366-372` 的 `block_in_place + block_on`
- **G6 前端两道检查进 CI** —— `web_frontend_check.mjs` 进 gates，CDP 走查进 nightly
- 真机复跑（`e2e_web.py` / CDP）—— 本变更只动了启动路径的门禁与两个响应字段，未跑真机，列为下一步