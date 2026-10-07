# 043 任务

- [x] **T001** `index.html`：`<head>` 加 `<meta name="ricow-token" content="__RICOW_TOKEN__" />`
- [x] **T002** `common.js`：`R.TOKEN` 改为"查询串 → meta 载体"取值（带注释说明为什么不能只靠 query）
- [x] **T003** `assets.rs`：加单测锁住"载体存在 + 读取方真的读它 + 替换后内容正确"
- [x] **T004** `e2e_web.py` 加"模拟浏览器导航"段（303 → Set-Cookie → 带 cookie 取 `/` → HTML 带 token），
      并同步回 skill 目录
- [x] **T005** `SKILL.md` 补记该坑与复跑方式（真浏览器专属路径）
- [x] **T006** 门禁：`cargo fmt --check` / `clippy -D warnings` / `ci_grep_gates.sh` / `node --check` / `cargo test --workspace`
- [x] **T007** 真机：headless Edge + CDP 复跑，确认 `R.TOKEN` 非空 + 会话 SSE `responseStatus == 200`
- [x] **T008** 收敛：`converge.md`；更新内存（这条坑值得长期留）

> 全部任务于 2026-10-07 完成并收敛，见同目录 `converge.md`。
