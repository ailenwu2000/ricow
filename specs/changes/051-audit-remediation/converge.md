# 051 收敛

> 基线 `HEAD = 7e11511`（审核对象）。本轮**只做整改**：不新增功能、不改用户可见文案、
> 不改回测数字口径、不动写确认面。

## 一、第一层：文档与治理

| 项 | 改动 | 证据 |
|:--|:--|:--|
| D1/自动重启矛盾 | `architecture.md:59` 拆成"OS 开机自启（结构性不做）"与"进程内崩溃自动重启（038，默认 none）" | `grep -n "无开机自启与自动重启" specs/architecture.md` 空；`:59` 现含"进程内" |
| D2/表数 | `architecture.md` 表数 **10 → 11**，补 `strategy_state`，注明以 `db.rs` 为准 | `db.rs` 的 `CREATE TABLE IF NOT EXISTS` 共 11 条 |
| D3/端点清单 | 补 `optimize`/`sweep`/`ai-generate`/`manifest`/`GET /api/strategies/{id}`/`/api/risk-ack`/`/api/logs`+`tail`/`/api/trades/*`；`status` 订正为 **GET** | `web/mod.rs:251-315` 的 `.route()/.merge()` 逐条对齐 |
| D4/命令集 | 补 `ricow new`、`ricow optimize` | `main.rs:53,69` |
| D5/假符号 | `BUILTIN_SCRIPTS` → `strategies::catalog::find` + `BUILTIN` 的 **8 个真实 id** | 全仓 `grep BUILTIN_SCRIPTS` 仅剩历史档案；`catalog.rs:157-198` |
| D6/product | `product.md:55` 单页布局 → **六视图工作台** | `index.html:35-41` |
| A8/A9 治理 | 新增本档案（spec/plan/tasks/converge）+ `specs/changes/README.md` 索引 + roadmap 记一行 | — |

**未做（有意）**：不物理重命名 032–044 的重号目录 —— 仓库多处以其路径引用、宪法「定稿不回改」，
重命名断链风险远大于收益。改为在 `specs/changes/README.md` 建索引并立"编号唯一、不复用"的**前向**约定。

## 二、第二层：代码缺陷 B1–B13

| 编号 | 改动 | 钉子 |
|:--|:--|:--|
| **B1** | 抽出 `guarded()` 保险丝，三端点的**整段**（内核 + 报告/指标/图表/日志）全在 `catch_unwind` 之内 | `test_guarded_turns_panic_into_error` |
| **B2** | `R.fmt.num` 去掉 `Number(...)` 往返，改纯字符串去尾零 | CDP：`fmt.num(1e-7) === "0.0000001"` |
| **B3** | `fmt.time` / `fmt.timeSec` 加 `Date` 合法性判定，不再吐 `"Invalid Date"` | CDP：`time(1e300)==="—"`、`timeSec("abc")===""` |
| **B4** | 把"命令面板是唯一例外"写进 `common.js` 的 toast 约定 + `palette.js` 用具名 `reportActionError` | `web_frontend_check.mjs` F 段：err toast 只准 palette.js |
| **B5** | `R.sse` 改"连上并**稳定存活 10s** 才清零计数" | 见下"未覆盖"说明 |
| **B6** | `load()` 跳过空 key + `decorate` 加零长匹配保护（双保险） | CDP：术语装饰能返回并包出词条 |
| **B7** | 抽屉收起判据从 `#nav` 放宽到**整个 `#sidebar`** + 监听 `hashchange` | CDP：点侧栏 / 点导航 / 程序化导航 三条路径 |
| **B8** | `R.chat.init()` 加 try/catch，失败不再吞掉 `startRouter()` | 六视图逐个激活（含未捕获异常检查） |
| **B9** | 抽 `ENGINE_INTERNAL_KEYS` + `reject_internal_param_keys()`，**三端点共用** | `test_reject_internal_param_keys` + 两处端到端断言 |
| **B10** | `redact_root()` 把数据根目录绝对路径换成 `<root>`（原文仍进服务端日志） | `test_redact_root_strips_data_dir` |
| **B11** | 引擎回放（同步 CPU 段）用 `block_in_place_if_multithread()` 让出 worker | 两条用例覆盖多线程与当前线程两种运行时 |
| **B12** | `R.currentView` 唯一真相移到 `router.js`；help.js 只读不写 | CDP：非法 hash 回退后 `currentView === "chat"` |
| **B13** | 实测为**冷启动/实时防护**伪慢（温跑 0.00s、整套 0.03s）；顺带把临时目录名加上纳秒戳，消除 pid 复用的残留目录 | 见下"复核结论" |

### B13 复核结论（如实）

审核当时记的是"该用例实跑 63.9 秒"。复核测量：

- 单跑该用例（温）：**0.00s**，整个 `web::assets` 22 用例串行 **0.03s**。
- 结论：那次 63.9s 是**首次链接后运行测试二进制**的冷启动成本（页入 + 实时防护扫描），
  与用例逻辑无关。**按"没有可复现的缺陷就不改行为"处理**，只做了一处无害的鲁棒性收紧
  （临时目录名加纳秒戳，避免 pid 复用命中上一轮残留目录而让开头的 `remove_dir_all` 变成真删除）。

## 三、第三层：伪通过治理 C1–C5

| 编号 | 改动 | 证据 |
|:--|:--|:--|
| **C1** | 真机走查**入库**：`scripts/probe_web_ui.mjs`（零依赖、CDP、参数化） | 本轮实跑 **43 PASS / 0 FAIL** |
| **C2** | `web_frontend_check.mjs` D 段：由"只比数量"改为**逐标签切片配对** | 26 个标签逐标签成对（含 3 组 placeholder） |
| **C3** | C 段扩到单引号/模板串；新增**反向死文案**检查 | 立刻抓出 **10 个真死键**并清除（`helpBtnTitle`/`helpClose`/`keysClearLive`/`keysClearDemo`/`keysSaving`/`sgFees`/`sgMfDescCol`/`sgMfOptions`/`sgOptKey`/`sgJumpErr`） |
| **C4** | `assets.rs` 测试模块头写明"这是防手抄漂移的门禁，不是行为验证"，并列出两处真行为验证的位置 | `assets.rs:255-270` |
| **C5** | 401 矩阵补 `/api/backtest/optimize`；optimize 补**维度上限(5 维→400)**与**重复键→400**；e2e 补 POST 端点 401 矩阵 | `test_backtest_endpoints_require_token_offline` / `test_start_optimize_validation_offline` / e2e 新断言 |

### 附带发现（本轮新抓，非原报告条目）

- **10 个死文案**（见 C3）—— 由新加的反向检查抓出，已清除。
- **`/favicon.ico` 必然 404**：页面未声明 `<link rel="icon">`，浏览器会自动请求，而全部端点都在
  token 门之后 → 每次加载一条控制台 404。**未修**（会给资产表加 `image/svg+xml` 条目并牵动
  首页对齐测试，超出本轮范围），已在走查脚本里**显式区分并打印 NOTE**，不让它污染"零异常"断言。

## 四、门禁实跑（2026-10-11）

| 门禁 | 结果 |
|:--|:--|
| `cargo fmt --all -- --check` | **0 差异** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0**，零代码告警 |
| `cargo test --workspace --no-fail-fast` | **966 passed / 0 failed / 22 ignored**（较基线 961 **+5**，全部是本轮新增用例） |
| `bash scripts/ci_grep_gates.sh` | **五条红线全绿** |
| `node scripts/web_frontend_check.mjs` | **全绿**（双语 412/412；逐标签配对；零死文案；err toast 只 palette） |
| `e2e_web.py`（干净 root + `show_all_pairs=true`） | **283 PASS / 0 FAIL**（较基线 282 **+1** = 新增的 POST 401 矩阵） |
| `scripts/probe_web_ui.mjs`（CDP 真机） | **43 PASS / 0 FAIL** |

**假绿复核（按纪律）**：e2e 的 283 与新增字样"POST 端点 3 条 无/错 token 均 401 空体且不回显"
在同一份日志里同时出现，已核对；第二次跑因 root 已被上一轮写入而出现 3 条 409，
改用干净 root 后归零 —— 说明那 3 条是**状态污染**而非产品缺陷。

### 未覆盖 / 遗留

- **B5 没有自动化钉子**：`R.sse` 的退避封顶需要"握手 200 后立即 EOF"的病态服务端，
  本轮没有搭这个装置。改动是局部的（`onopen` 不立即清零 + 稳定性定时器），逻辑经人工复核，
  但**没有断言钉住** —— 记为遗留项。
- `/favicon.ico` 404（见上）。
- 重号目录未改名（见上，有意）。

## 五、首轮结论

审核报告的三层条目（文档 9 条 / 代码 13 条 / 伪通过 5 条）**全部处理**：
10 处文档与治理修完、13 处代码缺陷修完（B13 判定为环境伪慢）、5 处伪通过治理完成并**新增一处
真机走查入库**。代码缺陷全部带断言（B5 除外，已如实标注）。四道门禁 + 真机两路全绿。

## 六、复核轮（同日的第二次完整审核）

按要求"补齐测试以后再次完整审核"，本轮**重新自查**了整改本身，产出如下。

### 6.1 查出并修掉的问题（都在整改自己身上）

| 编号 | 问题 | 证据 | 处置 |
|:--|:--|:--|:--|
| **R1（较重）** | **本档案自己踩了"编号唯一不复用"这条规则** —— 落成时取 **045**，而 045 早已被合约策略线占用 | `git log -S"045-R4"` → `6e94a48 feat(045-046): 合约全仓(cross)模式…`；`crates/ricow_strategy/src/backtest.rs` 有整片 `045-R3/R4` 注释；`specs/backtest.md:127,128,132` 写着「全仓(cross) margin 模式(045, 2026-09-30)」；`047-.../plan.md:39` 写着「045/046 改名让位」 | 档案**改号为 `051`**（045–050 均被占，见下）；同步改正 **`crates/` + `scripts/` 里 10 个文件的 43 处**「051 复核…」注释，以及本档案四件套、`specs/changes/README.md`、`roadmap.md` |
| **R2（索引失真）** | 索引表把 045 记成"最新"，却在其下又列了 047/049 —— **自相矛盾**；且 046/048/050 三个被引用过的编号在表里**凭空消失** | `specs/changes/README.md`（改前 `:52`） | 索引表补齐 045–050 并逐条标注"**被占用 / 无档案目录**"及取证；并写明**判据订正**：新编号应取「目录 ∪ 全仓引用」的最大值 + 1，**只数目录会踩到 050** |
| **R3（假引用）** | `assets.rs` 注释里写着「**046 记录**」，而**全仓没有任何 046 档案**（同 D5 那类"引用不存在的符号"） | `grep -rn 046 specs/changes/` → 无 `046-*` 目录 | 删除该半句，只留「051 C4 明确表述边界」 |
| **R4（手滑字）** | `backtest.rs` 注释「见**审反复核** B11」不成词 | `backtest.rs:1153` | 改为「051 复核 B11」 |

**为什么 R1 值得单记**：这条规则是**本变更自己写进 `README.md`** 的（"编号唯一、不复用…045 起遵守"），
而同一变更里**当场违反**。踩过之后判据才写准（R2 的"目录 ∪ 引用"）。

### 6.2 复核重跑（改号之后重新实跑，不是沿用改号前的数据）

| 门禁 | 结果 |
|:--|:--|
| `cargo fmt --all -- --check` | **0 差异**（exit 0） |
| `cargo clippy --workspace --all-targets -- -D warnings` | **exit 0**，零代码告警 |
| `cargo test --workspace --no-fail-fast` | **966 passed / 0 failed / 22 ignored**（分 target 加总：465+3+67+18+128+282+3 = 966） |
| `bash scripts/ci_grep_gates.sh` | **五条红线全绿** |
| `node scripts/web_frontend_check.mjs` | **全绿**（A 16 份语法 / B 双语 412·412 / C 静态引用 369 键零缺零死 / D 26 标签逐标签配对 / E parseHash 12 条 / F err toast 只 palette.js） |
| `e2e_web.py`（**干净 root** `tmp/verify-051c` + `show_all_pairs=true`） | **283 PASS / 0 FAIL**，日志第 5 行即本轮新增断言「POST 端点 3 条 无/错 token 均 401 空体且不回显」；`grep -c "^  PASS"` = 283、`FAIL` = 0 —— **计数与字样同时对上** |
| `scripts/probe_web_ui.mjs`（CDP 真机，Edg/155 headless + Node 22） | **43 PASS / 0 FAIL**，另有 1 条 NOTE（`/favicon.ico` 404，遗留项） |

### 6.3 复核"文档回填"本身有没有引入**新的**假陈述

逐条把文档里的可证伪断言拉出来对代码（这是 D 层修复自带的回归风险）：

| 文档断言 | 代码事实 | 结论 |
|:--|:--|:--|
| `architecture.md` "SQLite **共 11 张表**" | `db.rs` 的 `CREATE TABLE IF NOT EXISTS` **语句 11 条**（第 12 个 grep 命中是 `:221` 的**注释行**，不是建表） | ✅ |
| 同页"8 个内置 id"清单 | `catalog.rs::BUILTIN` 实际 8 个，`diff` 后**逐字一致** | ✅ |
| 同页 `commands/mod.rs::resolve_builtin_script` | 该函数在 `mod.rs:531` 存在 | ✅ |
| `architecture.md` 命令集补 `new` / `optimize` | `main.rs:53 Optimize(..)` / `:69 New(..)`，且 `:124` `:132` 有对应 `run` 分派 | ✅ |
| `architecture.md` 的端点清单 | 与 `web/mod.rs` 的 `.route()/.merge()` 抽样对齐（optimize/sweep/manifest/ai-generate/`{id}`/risk-ack/trades/logs+tail/metrics）；`status` 确为 **GET** | ✅ |

### 6.4 复核轮新增/变更的测试与文档

- `specs/roadmap.md`：新增 051 基线一行 + **补记 961 那笔未记档的欠账**（帮助计划 A–D 批次的
  +123 用例逐 target 拆开写清）+ 历史基线链补 `→ 961 → 966`。
- `specs/architecture.md` 第九节「测试策略」的基线**同步**（提交前 grep 旧基线数字时发现）：它是一份
  **会漂移的镜像**，自 044 起就没再维护（上两次变更也没碰），一直把 838 当作「当前基线」，与
  `roadmap.md` 的 966 自相矛盾。已补 051（966）/ 帮助计划（961）两条并把 838 降为「前基线」。
- `specs/changes/README.md`：编号占用表补齐 045–050、判据订正、**自体追认**段落。
- 本档案：全篇 045 → 051（`spec.md` / `plan.md` / `tasks.md` / `converge.md`）。
- 注释同改：`web/assets/{app,common,help,palette,router}.js`、`web/assets.rs`、`web/backtest_jobs.rs`、
  `commands/backtest.rs`、`scripts/{probe_web_ui.mjs,web_frontend_check.mjs}`。
- skill：`ricow-webui-e2e-verify/{SKILL.md,e2e_web.py}` 同步改号，计数沿革补 `051 = 283`。

### 6.5 本轮**没有**动的（如实）

- **不回头改 045–050 那批"有编号无档案"的历史欠账**：补写会与当时事实不符（宪法「定稿不回改」），
  且它们已被代码/文档引用为既成事实。只在索引里点明"被占用、无目录"。
- **B5 仍无自动化钉子**（同上，遗留项）。
- **`/favicon.ico` 仍 404**（同上，遗留项）。

## 七、结论

复查确认：三层条目的修复**都真实落在代码/文档里**（逐条读了 diff，不是只看勾选），
四道门禁 + 两路真机取证在**改号之后重新跑过**，且**修掉了 4 处由整改自己引入的问题**
（其中 R1 是编号复用、R2 是索引失真 —— 都属于"审核对象之外的自我审查"）。
未发现新引入的功能缺陷；未发现伪通过。

