# 051 架构与代码审核整改（文档回填 + 缺陷收口 + 伪通过治理）

## 一、动机

2026-10-11 以软件架构师视角对本仓库做了一次**只读**审核（需求 ↔ 实现 ↔ 测试 三层），
产出一份带 `文件:行` 证据的报告。审核确认：**没有"功能完全没实现"的条目**，但存在
三类系统性偏差，需要一次性收口：

1. **文档已明显滞后于代码**：`architecture.md` 内部自相矛盾（既写"无自动重启"又完整描述 038 崩溃自动重启）、
   SQLite 表数写少一张、端点/命令清单漏记 7 个端点与 2 个子命令、引用了**全仓不存在**的符号 `BUILTIN_SCRIPTS`；
   `product.md` 仍描述 032 之前的单页布局。
2. **代码层"同一件事两套口径"或"边界未收口"**：共 10 处真实缺陷（中 6 / 低 4），
   其中 `backtest_jobs` 单次回测的 panic 保护只包住内核（后处理裸奔 → 作业可卡死 60 分钟）影响面最大。
3. **伪通过风险**：被引为权威的 "e2e 282 passed" 绝大多数是对 HTTP 取回的 JS/HTML 做**子串匹配**，
   **不驱动浏览器**；真正的行为走查脚本放在**未入库**的 `tmp/`（不在 CI、不可复现）。

本轮**不是**新功能，而是"把审核发现的偏差补齐 + 给薄弱断言补真断言"。

## 二、范围（按审核报告的一 / 二 / 三 三节）

### 第 1 层：文档与治理
- `architecture.md:59` 自动重启矛盾 → 改为明确区分"OS 开机自启（无）"与"进程内崩溃自动重启（038 有，默认关）"
- `architecture.md:207` 表数 10 → 11，补 `strategy_state`，并注明以 `db.rs` 为准
- `architecture.md:115` 端点清单补全（optimize/sweep/ai-generate/manifest/risk-ack/trades/logs 清单与 tail），`status` 订正为 GET
- `architecture.md:95` 命令集补 `new` / `optimize`
- `architecture.md:198` `BUILTIN_SCRIPTS` → `strategies::catalog::find` + `BUILTIN` 的 8 个真实 id
- `product.md:55` 单页布局 → 六视图工作台
- 补 `specs/changes/051-audit-remediation/`（本档案）+ `roadmap.md` 记一行

### 第 2 层：代码缺陷（B1–B13）
| 编号 | 位置 | 问题 |
|:--|:--|:--|
| B1 | `backtest_jobs.rs` 单次回测 | 后处理（`format_backtest_report`/`metrics()`/`chart()`）在 `catch_unwind` 之外 → panic 逃逸、作业卡 running |
| B2 | `common.js` `R.fmt.num` | `Number(toFixed())` 往返 → 低价币现价泄漏科学计数法 `1e-7` |
| B3 | `common.js` `time`/`timeSec` | 超 `Date` 范围或非数字 → 输出字面量 `"Invalid Date"` |
| B4 | `palette.js` | 用 `R.toast(..., "err")` 报错，与 `common.js` 自述"toast 只承载成功/中性"冲突 |
| B5 | `common.js` `R.sse` | `onopen` 无条件清零重试计数 → "连上即断"永续 ~1s 重连、`onfail` 不回调 |
| B6 | `common.js` `decorate` | 无零长匹配保护 → 空 key 时死循环 |
| B7 | `app.js` | 窄屏抽屉只绑 `#nav` → 点会话/程序化导航/hashchange 不收起 |
| B8 | `app.js` | `R.chat.init()` 无异常保护 → 抛错则整站路由不启动且无提示 |
| B9 | `backtest_jobs.rs:471` | 回测端点未拒绝引擎内部键 `pair/script/script_path`（sweep/optimize 都拒绝） |
| B10 | `commands/mod.rs` / `backtest_jobs.rs` | 内部绝对路径与 panic 原文随 `job.error` 回浏览器 |
| B11 | `backtest_jobs.rs` | 同步 CPU 内核直接在 axum 运行时线程跑（未 `spawn_blocking`） |
| B12 | `router.js` / `help.js` | `R.currentView` 两份真相 → 非法 hash 首屏时帮助抽屉"当前视图"错 |
| B13 | `assets.rs` 单测 | `test_resolve_assets_dir_rejects_bad_dirs` 实跑 63.9s |

### 第 3 层：伪通过治理（C1–C5）
- C1 真机走查脚本从 `tmp/` 提进仓库 `scripts/`（伪通过根因）
- C2 `web_frontend_check.mjs` D 段由"只比数量"改为"逐标签配对"
- C3 文案检查扩到单引号/模板串，并加"定义了但没人用"的反向检查
- C4 把 `assets.rs` 门禁单测与"行为验证"的表述边界写清
- C5 e2e 补 optimize 的 401 矩阵 + 维度上限/重复键用例

## 三、验收

1. 文档：对每条 D 用 grep 给出"文档已改 + 代码事实"的双向证据。
2. 代码：每条 B 有对应的单测或静态检查断言钉住（"修了但没测"不算完成）。
3. 伪通过：真机走查脚本入库、可被 CI 或文档指引复跑；薄弱断言改真断言。
4. 全门禁：`cargo fmt --check` / `clippy -D warnings` / `cargo test --workspace` /
   `node scripts/web_frontend_check.mjs` / `e2e_web.py` 全绿，且**断言计数与新增字样对得上**。
5. 复核：修复后再做一次完整审核，确认未引入新缺陷。

## 四、明确不做

- **不物理重命名已有的重号变更目录**（033/034/…/044 各两个）。理由：仓库多处引用
  `specs/changes/XXX/` 路径、git 历史与宪法「定稿不回改」；重命名的收益远小于断链风险。
  改为在 `specs/changes/README.md` 建立索引 + 记明"编号唯一、不复用"的**前向**约定。
- **不补写 045–050 那批"有编号无档案"的历史欠账**（合约策略线占用了 045/046/048/050 与已建目录的
  047/049，其中前四个无档案）。补写会与当时事实不符；只在本索引里点明"被占用、无目录"。
- 不引入前端构建链、不新增运行时依赖（宪法"少而精"）。
- 不改任何已有 CLI 用户可见文案、不改回测数字口径。

## 五、编号（本档案的自体修正）

本档案最初落成时取 **045** —— 而 045 早已被合约策略线的「全仓 cross」占用
（`6e94a48 feat(045-046)…`、`ricow_strategy/src/backtest.rs` 的 `045-R3/R4` 注释、`specs/backtest.md:127`）。
**已改号为 051**（045–050 全部被占：045/046/048/050 无档案、047/049 有档案）。
判据随之订正为「**目录 ∪ 全仓引用** 取最大编号 + 1」—— 只数目录会正好踩到 050。
详见 `converge.md` §六。
