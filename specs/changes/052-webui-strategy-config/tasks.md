# 052 · 任务

## 方案 1 · 复制带清单

- [x] **T1** `SaveRequest` 加 `copy_from: Option<String>`(serde default)
- [x] **T2** 纯函数 `derive_manifest(src, new_id, market) -> StrategyManifest` + 单测
- [x] **T3** `save_strategy`: `copy_from` 与 `overwrite` 互斥 400; 未知源 400; 先写清单后落脚本; 失败回滚
- [x] **T4** 端到端: `copy_from=paired_grid` → 200、清单落盘、`declared=true`、`params` 与源一致
- [x] **T5** 前端 `openCopyModal` / 模板新建带 `copy_from`

## 方案 2 · 静态提取参数

- [x] **T6** 新模块 `strategies/params.rs`: `ParamHint` + `extract_param_hints`(遮罩/定位/解析/类型/默认)
- [x] **T7** 单测: 键集合 == `paired_grid` 清单键集合; 注释/字符串里的同名文本不误命中; 变量键不提取; 保留键跳过
- [x] **T8** 端点 `POST /api/strategies/extract-params` + 路由注册 + 无 token 401

## 方案 3 · AI 改配置

- [x] **T9** `AiEditRequest.mode`; `AiEditReply` 改可选 `code`/`params`
- [x] **T10** `ai/provider.rs`: `build_params_prompt` + `quick_params` + 单测
- [x] **T11** JSON 回包解析 + 键过滤(warning)/类型校验/全空 400 + 单测
- [x] **T12** 前端 AI 卡模式 chip + 参数回填(含 `pair` → `#sg-pair`)

## 方案 4 · 前端引导

- [x] **T13** 参数引导卡(两按钮)+ `onExtractParams` 预填清单行
- [x] **T14** 「其他已保存参数」补来源说明; `style.css` 补样式
- [x] **T15** `help.js` 策略一步补一句

## 收敛

- [x] **T16** 扩展 `e2e_web.py`(401 矩阵 + 静态 + copy_from 真机回读)
- [x] **T17** 扩展 `scripts/probe_web_ui.mjs`(复制后参数行数/中文名、引导卡、AI chip)
- [x] **T18** 门禁全跑: fmt / clippy / test / ci_grep_gates / web_frontend_check / e2e / CDP
- [x] **T19** `converge.md` + `roadmap.md` 基线 + `architecture.md` 端点清单与测试基线
- [x] **T20** 订正研究文档的过强验收表述

## 追加需求 N1 / N2 / N3（2026-10-12，用户指令）

- [x] **N1-1** `commands::strategy_file_candidates(root, name)`: 统一删除候选(实例 TOML + 市场子目录 `.lua`/`.toml` + 该 id 的 `.bak`)
- [x] **N1-2** `DELETE /api/strategies/{id}`: 内置 409 `reserved` / 运行中 409 / 回包如实列出 `removed`；**历史与台账不删**
- [x] **N1-3** 前端确认框（写明"回测历史与台账会保留"）+ 删除后刷新列表
- [x] **N2-1** `build_manifest` 与 `catalog::parse` **两处一起**放开"中文名/说明留空"
- [x] **N2-2** `manifest_summary` / `renderParams` / 清单编辑器三处**回退显示键名**
- [x] **N2-3** `strategies/params.rs::comment_docs` 从 Lua 注释搬说明原文（提不出来就留空，不编造）
- [x] **N2-4** `POST /api/strategies/ai-name-params` + `build_names_prompt`/`quick_names` + 前端「让 AI 起中文名」按钮
- [x] **N3-A1** `AiEditRequest.history` + `cap_history`（保留尾部，前缀标注截断）
- [x] **N3-A2** `build_params_prompt` 改"先讲怎么改/为什么/风险，最后给**整组** JSON"
- [x] **N3-A3** `split_advice_and_json`（逐行找最后一个可解析的对象行）+ `diff_params` → 回包 `advice`/`changed`
- [x] **N3-B1** `POST /api/strategies/{id}/ai-assess`（`ASSESS_METRICS` 白名单 17 项；跳过 `null`；`MAX_CHANGED_LINES` 超限如实计数）
- [x] **N3-B2** 前端 `runParamAssess`：busy 前置判断 → 跑改前 → 跑改后 → 问影响；窗口写死 `ASSESS_DAYS=90` / `"1h"`
- [x] **N3-B3** 对话线程 UI（用户/助手气泡、改变更列表、清空按钮；**按策略清空** —— 历史会拼进提示词）
- [x] **修复** 前端 `sg-mf-name` 撞 id（按钮点了没反应）→ `sg-mf-autoname`；`markets.js` 的 `mkRetry` 重复键
- [x] **护栏** `web_frontend_check.mjs` 加 **G 段同名碰撞**；`probe_web_ui.mjs` 补"按钮真的在/真的可点"
- [x] **文档** `spec.md` 目标 3 判据重写 · `new-tasks.md` 落地结果 · `roadmap.md` + `architecture.md` 双基线
- [x] **冒烟脚本** 修 3 条既有错断言（非法清单矩阵 / 邻居清单不存在 / 内置脚本不在 RICOW_ROOT）

## 收尾实跑（2026-10-11，第一批）

| 门禁 | 结果 |
|:--|:--|
| fmt | 0 差异 |
| clippy `-D warnings` | exit 0 |
| `cargo test --workspace --no-fail-fast` | **983 passed / 0 failed / 22 ignored**（966 → 983，+17） |
| `ci_grep_gates.sh` | 五条红线全绿 |
| `web_frontend_check.mjs` | 全绿（双语 429·429） |
| `e2e_web.py`（干净 root + `show_all_pairs=true`） | **329 PASS / 0 FAIL**（283 → 329，+46） |
| `probe_web_ui.mjs`（CDP 真机） | **65 PASS / 0 FAIL**（43 → 65，+22） |

## 收尾实跑（2026-10-12，含 N1/N2/N3）

| 门禁 | 结果 |
|:--|:--|
| fmt | 0 差异 |
| clippy `-D warnings` | exit 0（只剩已知 Windows 增量锁 `os error 5` 噪声） |
| `cargo test --workspace --no-fail-fast` | **1000 passed / 0 failed / 22 ignored**（983 → 1000，**+17**，增量全在 `ricow` bin 482 → 499） |
| `ci_grep_gates.sh` | 五条红线全绿 |
| `web_frontend_check.mjs` | 全绿（双语 **463·463** / C 416 键 / **G 段两段新增**） |
| `e2e_web.py`（干净 root `tmp/verify-053c`） | **372 PASS / 9 FAIL**（329 → 372，+43；**余 9 条全是同一条 `K 线第 0 片拉取失败`**，本机 `api.binance.com` 不可达，环境问题） |
| `probe_web_ui.mjs`（CDP 真机） | **75 PASS / 0 FAIL**（65 → 75，+10） |
