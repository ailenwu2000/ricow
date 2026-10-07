# 042 任务

- [x] **T001 通读定边界**
  - 通读 `strategies.js` 全文，列出 13 个可变状态 + 全部顶层函数 + 跨节引用。
  - 定出四份文件的行区间与"谁调用谁"的别名清单（跨文件符号）。
  - **产出**：脚本 `tmp/split_042.py` 里的区间常量与别名清单（review 对象）。

- [x] **T002 生成 4 份文件**
  - 脚本按区间切分 + 词法级状态改写（`gen` → `st.gen`），跳过注释/字符串/属性位置。
  - 外壳重写头部：`R.sg` 命名空间、工具、常量、状态袋、跨文件别名。
  - 三份子文件各写头部：职责注释 + `S` 取用 + 工具/常量别名 + 跨文件别名 + 末尾导出。
  - 结果：`strategies.js` / `strategy_form.js` / `strategy_editor.js` / `strategy_backtest.js`。

- [x] **T003 接线**
  - `assets.rs::ASSETS` 插 3 条（紧跟 `strategies.js`，排在 `runs.js` 之前）。
  - `index.html` 插 3 条 `<script>`，顺序同上。
  - 新增纪律断言：三份子文件在 `ASSETS` 里的下标必须大于 `strategies.js`（加载顺序即契约）。

- [x] **T004 静态核对**
  - `node --check` × 4 全过。
  - 逐符号脚本核对（双向）：
    - 每份文件引用的 `S.<name>` 都有定义（无未定义引用）；
    - 13 个状态名的裸出现 = 0（词法扫描，排除注释/字符串/属性名）；
    - `st.<name>` 的 `<name>` ⊆ 13 个状态名；
    - 每个跨文件偏名的右侧形如 `=> S.`（D2）；
    - 原文件的每个顶层函数名恰好出现在一份文件里（无遗漏/重复）。
  - `cargo test --bin ricow -- web::` 全绿（含 4 条纪律扫描 + 新增断言）。

- [x] **T005 门禁 + E2E**
  - `cargo fmt --check` / `clippy -D warnings` / `bash scripts/ci_grep_gates.sh` / 全量 `cargo test --workspace`。
  - 起内嵌服务跑 `e2e_web.py`（144 条）；起 `--web-assets-dir` 服务再跑一次。
  - **两者都必须 144 PASS / 0 FAIL**（042 是纯重构，任何红都是回归）。

- [x] **T006 真机走查 + 收敛**
  - 策略页四态走查：列表 → 新建（模板向导 / 空白）→ 详情（参数表单 / 清单编辑 / 编辑器高亮 /
    保存 / AI 改写入口 / 回测按钮）→ 回测结果（指标卡 / 权益曲线 / 平仓明细 / 策略日志 / 寻优 / 历史）；
    再切一次语言 + 切一次主题。
  - 文档：`architecture.md`（前端拆分段）、`roadmap.md`（档案行 + 基线）、`converge.md`、记忆。

> 全部任务于 2026-10-07 完成并收敛，见同目录 `converge.md`。
