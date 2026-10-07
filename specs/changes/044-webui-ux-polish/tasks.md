# 044 任务

- [x] **T001** 写 `markdown.js`：受限子集 → DOM（无 `innerHTML`），导出 `R.markdown.render(text)`
- [x] **T002** `chat.js`：`renderRich(el, md)` 分段（报告卡片 / markdown / 纯文本）；`decorateTerms` 跳过 `pre`/`code`
- [x] **T003** `index.html`：`<script src="/markdown.js?token=...">`（在 `chat.js` 之前）+ 日志面板工具条
- [x] **T004** `style.css`：markdown 元素 / 日志工具条 / `.log-hidden`
- [x] **T005** `chat.js` 日志面板：搜索、级别（启发式）、跟随/暂停、诚实计数、双语
- [x] **T006** `assets.rs`：加资产（`markdown.js`）+ 顺序测试扩展 + 只读断言精确化（D4）
- [x] **T007** `e2e_web.py` + skill：补 `markdown.js` 断言，并同步回 skill 目录
- [x] **T008** 门禁：fmt / clippy / grep 红线 / `node --check` / `cargo test --workspace`
- [x] **T009** 真机（CDP）：渲染正确性 + 注入负例 + 日志搜索/过滤/暂停
- [x] **T010** 收敛：`converge.md`、`roadmap.md`、内存与日志

> 全部任务于 2026-10-07 完成并收敛，见同目录 `converge.md`。
