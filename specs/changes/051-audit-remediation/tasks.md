# 051 任务

> 按审核报告一 / 二 / 三 的顺序推进；每条完成即勾。

## 一、文档与治理
- [x] **T101** `architecture.md`：自动重启矛盾（区分 OS 自启 vs 进程内重启）
- [x] **T102** `architecture.md`：表数 10 → 11 + 补 `strategy_state`
- [x] **T103** `architecture.md`：端点清单补全 + `status` 订正为 GET
- [x] **T104** `architecture.md`：命令集补 `new` / `optimize`
- [x] **T105** `architecture.md`：`BUILTIN_SCRIPTS` → `catalog::find` + 真实内置 id
- [x] **T106** `product.md`：单页布局 → 六视图工作台
- [x] **T107** 新增 `specs/changes/051-audit-remediation/`（spec/plan/tasks）
- [x] **T108** `specs/changes/README.md` 索引 + `roadmap.md` 记一行

## 二、代码缺陷
- [x] **T201** B1：回测后处理移进 `catch_unwind`
- [x] **T202** B2：`R.fmt.num` 去科学计数法
- [x] **T203** B3：`time`/`timeSec` 边界收口（不产 `"Invalid Date"`）
- [x] **T204** B4：toast 错误口径二选一（palette 改原地/统一约定）
- [x] **T205** B5：`R.sse` 退避封顶（区分"从未 open"与"open 后即断"）
- [x] **T206** B6：`decorate` 零长匹配保护 + `load()` 过滤空 key
- [x] **T207** B7：窄屏抽屉在 `#session-list` / 程序化导航 / `hashchange` 下收起
- [x] **T208** B8：`R.chat.init()` 异常保护（失败不阻断路由）
- [x] **T209** B9：回测端点拒绝引擎内部键（与 sweep/optimize 同 helper）
- [x] **T210** B10：内部绝对路径/panic 原文不回浏览器
- [x] **T211** B11：CPU 内核移出 axum 运行时线程
- [x] **T212** B12：`R.currentView` 单一来源
- [x] **T213** B13：慢用例根因（63.9s）

## 三、伪通过治理
- [x] **T301** C1：真机走查脚本入库 `scripts/probe_web_ui.mjs`
- [x] **T302** C2：`web_frontend_check.mjs` D 段逐标签配对
- [x] **T303** C3：文案检查扩形态 + 反向死文案检查
- [x] **T304** C4：门禁单测与行为验证的表述边界写清
- [x] **T305** C5：e2e 补 optimize 401 矩阵 + 维度上限/重复键

## 四、验证与收敛
- [x] **T401** 补单测 / 静态检查断言（每条 B、C 的修复都要有钉子）
- [x] **T402** 门禁：`cargo fmt --check` / `clippy -D warnings` / `cargo test --workspace` / `node scripts/web_frontend_check.mjs`
- [x] **T403** 真机：`e2e_web.py` + `probe_web_ui.mjs`，复核断言计数与字样
- [x] **T404** 收敛：`converge.md` + 更新 `roadmap.md` 测试基线（含补记 961 那笔未记档欠账）
- [x] **T405** 再次完整审核（确认无伪通过、无新缺陷）

## 五、复核轮（同日的第二次完整审核，T405 的展开）
- [x] **T501** 逐条读 diff 复核 B1–B13 / C1–C5 是否真落地（不是只看勾选）
- [x] **T502** R1：查出"本档案复用 045"并**改号为 051**（045–050 均被合约线占），同步 10 文件 36 处注释
- [x] **T503** R2：`specs/changes/README.md` 编号表补齐 045–050 + 判据订正为"目录 ∪ 引用取最大 + 1"
- [x] **T504** R3/R4：删掉假引用「046 记录」、改正「审反复核」手滑字
- [x] **T505** 复核文档回填**没有引入新假陈述**（11 表 / 8 内置 id / `resolve_builtin_script` / `new`+`optimize` 逐条对代码）
- [x] **T506** 改号后**重新实跑**四道门禁 + `e2e_web.py`（干净 root）+ `probe_web_ui.mjs`，并复核计数与新增字样
