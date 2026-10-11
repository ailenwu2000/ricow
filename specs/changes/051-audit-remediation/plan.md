# 051 实施计划

## 关键决策

- **D1 自动重启表述**：不改 038 的行为，只改文档措辞。区分 `OS 开机自启（结构性不做）` 与
  `进程内崩溃自动重启（038，默认 none）`，并把两处交叉引用对上。
- **B1 panic 覆盖面**：把 `metrics()`/`chart()`/`format_backtest_report` 全部移进 `catch_unwind`，
  与 sweep/optimize 的"唯一出口"对齐 —— 是"向既有的正确形态看齐"，不是新机制。
- **B5 退避封顶**：保留"连上就清计数"的健康语义，但**只在心跳确认后**清（引入"已 open 过的次数"区分
  "从未 open"与"open 后立即断"）；不改重连回调语义。
- **B6 零长匹配**：在 `decorate` 的扫描循环内加 `if (matched[0].length === 0) { re.lastIndex++; continue; }`，
  同时在 `load()` 过滤空 key（双保险）。
- **B9/B10 一致性**：把 sweep/optimize 已有的"拒绝内部键"抽成一个共享 helper，回测端点复用；
  内部路径改经"文件名而非全路径"回给客户端（保留可操作性、去掉绝对路径）。
- **B11**：CPU 内核 + 后处理整体放进 `tokio::task::spawn_blocking`，不阻塞 axum worker。
- **C1 脚本入库**：新增 `scripts/probe_web_ui.mjs`（零依赖，Node 22 内建 `WebSocket` + headless Edge CDP），
  以 `RICOW_WEB_URL` + `RICOW_CDP_PORT` 参数化；文档写明复跑方式。
- **C2/C3**：`web_frontend_check.mjs` 的 D 段改为"按标签切片逐标签配对"；C 段扩引号形态 + 反向死文案检查。

## 影响面

- 后端：`crates/ricow/src/web/backtest_jobs.rs`、`crates/ricow/src/commands/mod.rs`
- 前端：`common.js`、`app.js`、`palette.js`、`router.js`、`help.js`
- 门禁：`scripts/web_frontend_check.mjs`、`crates/ricow/src/web/assets.rs`（单测）
- 新增：`scripts/probe_web_ui.mjs`、`specs/changes/README.md`、本档案
- 文档：`specs/architecture.md`、`specs/product.md`、`specs/roadmap.md`

## 风险与回退

- 前端全部零构建链、`include_str!` 嵌入 → 改完必须 `cargo build` + 真机走查（历史教训：042 的漏导出
  四道静态门全绿）。
- `web_frontend_check.mjs` 的 C 段扩形态时，必须同时认 `Object.assign(R.TEXT.zh, …)` 与
  `R.TEXT = { zh:{…}, en:{…} }` 两种写法，否则会把基础字典整批漏掉造一堆假红。
- 所有改动保持"既有用户可见文案零变化 / 回测数字零变化 / 写确认面未动"。
