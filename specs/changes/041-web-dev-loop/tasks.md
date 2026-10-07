# 041 任务分解（tasks）

> 依赖顺序：T001 → T002 → T003 → T004 → T005 → T006。

- [x] **T001** `web/assets.rs`：资产表 + 来源抽象 + 目录校验
  - `Asset` / `const ASSETS`（`style.css` + 10 份 JS）；`AssetSource{Embedded,Disk}`；`resolve_assets_dir`；
    `Loaded` + `text_response(.., no_store)`；`routes()` 由 `ASSETS` 循环生成；`index` 支持两种来源且仍做 token 替换。
  - 单测：三条纪律扫描改用派生清单（覆盖面不变）；`resolve_assets_dir` 三种拒绝路径 + 合法目录；
    资产表自检（名字唯一 / 平铺 / 文本类型）。
  - 结果：原 11 个独立 handler + test-only `ALL_JS` 全部并入 `ASSETS`，**路由也改为派生** —— 清单从"三处各抄一份"收敛到零处手抄。
- [x] **T002** `web/mod.rs`：装配
  - `WebState.assets`（默认 `Embedded`）+ `with_assets_dir()`；12 条静态路由改为 `.merge(assets::routes())`。
  - 端到端单测：磁盘模式内容 + `no-store`；内嵌模式回归不变（响应体逐字等于内嵌副本、且**无** `no-store`）；
    缺文件 500 且点出文件名；`/nope.js`、`/../Cargo.toml`、`/style.css.bak` → 404/400 且不泄漏 `Cargo.toml`；无 token 仍 401 空体。
- [x] **T003** `commands/web.rs`：CLI 开关
  - `--web-assets-dir DIR` + `with_assets_dir` 接线 + 双语开发模式警告。
  - 真机：目录不存在 / 不含 `index.html` 两种给错方式都在**启动时** exit 1 并给出可操作文案。
- [x] **T004** 门禁：`cargo fmt --check` / `clippy -D warnings` / `ci_grep_gates.sh` / `node --check` / 全量 `cargo test`。
  - 结果：fmt 0 差异；clippy **exit 0、零告警**（本轮连 Windows 增量锁噪音都是 0 条）；grep 红线 5/5；10 份 JS 过 `node --check`；
    全量 **834 passed / 0 failed / 22 ignored**（829 → +5）。
- [x] **T005** 真机验证。
  - 探针 `tmp/probe_041_devloop.py` → **19 PASS / 0 FAIL**（`tmp/probe_041_out.txt`）。
  - 官方冒烟 `e2e_web.py` **两种形态各一次**：内嵌模式 **144 PASS / 0 FAIL**、磁盘模式 **144 PASS / 0 FAIL**。
- [x] **T006** 文档收敛：`architecture.md` / `roadmap.md` / `converge.md` / 记忆。
