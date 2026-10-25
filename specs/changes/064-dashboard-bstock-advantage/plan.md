# 064 实施计划

| 阶段 | 内容 | 依赖 |
| -- | -- | -- |
| P0 | 后端 `/api/overview` 补 `scope` 字段 | 无 |
| P1 | 概览页「bStock 覆盖」卡 + 一键放宽入口 | P0 |
| P2 | 「模拟回测」Tab 接「AI 解读」按钮 | 无（端点已存在） |
| P3 | 新建实盘表单「重置参数」 | 无 |
| P4 | 实盘列表市场列 + 筛选 | 无 |
| P5 | 验证（门禁六件 + 真机两路 + 064 专项走查） | P1–P4 |

## P0：后端 scope 字段

**文件**：`crates/ricow/src/web/overview.rs`

- `OverviewReply` 加 `scope: Option<ScopeCounts>`：
  ```rust
  #[derive(Debug, Serialize)]
  pub(super) struct ScopeCounts {
      pub spot: u64,
      pub futures: u64,
      pub filtered: bool,
  }
  ```
- 取数走 `crate::commands::pairs::current_view(&state.root, false).await` —— **与 `/api/markets` 同源**，不新造一套白名单。
- `current_view` 失败（交易所快照拉不到）→ `scope = None`，前端显示"—"（与其它项同口径：**取不到 ≠ 0**）。
- **不阻塞其它项**：`tokio::join!` 里加第五份，任一份失败不影响其它。

## P1：bStock 覆盖卡

**文件**：`crates/ricow/src/web/assets/dashboard.js` + `style.css`

- 新卡「bStock 覆盖」放**左列顶部**（比统计卡更靠前 —— 这是差异化卖点，不该藏在第三屏）。
- 内容：`现货 N 个 · 合约 M 个` + 一行说明（币安官方机器人只开放少数 stock，ricow 覆盖全部美股代币）+ 视野状态徽章（"股票类视野" / "全部交易对"）。
- 入口按钮「查看全部」→ `R.navigate("#markets?all=1")`。
  - **需先确认市场视图认不认 hash 里的 query**：不认就改成 `R.navigate("#markets")` 后由市场视图自己的"全部"开关切（本轮先查 `markets.js` 的 activate 是否解析 query）。
- 数字取不到时显示"—"，**不显示 0**（0 = "真的一个都没有"，与"没数到"是两件事）。

## P2：AI 解读按钮

**文件**：`crates/ricow/src/web/assets/strategy_editor.js`（已有三步逻辑）+ `strategies.js`（Tab 挂接）

- 在「模拟回测」Tab 的回测结果区下方加按钮「AI 解读」。
- 显示条件：`paramDiffs().length > 0`（沿用 054 T36 的判据 —— 没改动就没必要评估）。
- 点击 → 调既有 `runAssess()`（跑改前 → 跑改后 → `ai-assess`），结果渲染进 `#sg-ai-assess` 面板。
- 未配密钥：`need_keys` → 面板里给"去配置密钥"入口（沿用 `showNeedKeys`）。

## P3：重置参数

**文件**：`crates/ricow/src/web/assets/runs.js`

- 新建实盘表单的参数区头部加「重置参数」按钮（`R.ui.linkBtn`，小字号）。
- 行为：把每个参数输入框恢复成**清单声明的默认值**（`spec.default`）；无默认值的**清空**（清空 = 让用户填，不是填 0 —— 填 0 是编造）。
- 与 `#sg-undo`（代码撤销）**不是一回事**，文案要区分：「重置参数」不是「撤销」。

## P4：实盘列表市场列

**文件**：`crates/ricow/src/web/assets/runs.js` + 后端 `runs/mod.rs`

- 表头加「市场」列（现货/合约徽章，复用策略库那套 `sg-badge-*`）。
- 表头加筛选下拉（全部/现货/合约）—— **前端过滤**（实例数本就不多，不必为筛选加后端参数）。
- 后端确认 `RunRow` 已带 market：若已带只改前端；没带则补字段。

## P5：验证

- 门禁六件：fmt / clippy / 整仓 test / 红线门 / 前端门 / 真机两路。
- **064 专项走查**（新脚本 `probe064.mjs`）：
  1. 概览页有 bStock 覆盖卡，数字**与 `/api/markets` 实拉一致**（探针自己拉一次 API 比对，不是看页面写了啥）；
  2. 「查看全部」点了真跳到市场视图且带全部口径；
  3. 「模拟回测」Tab 有 AI 解读按钮（有改动才显示）；
  4. 新建实盘有「重置参数」，点了值真回到默认；
  5. 实盘列表有市场列，筛选真能过滤。
- 回归断言：scope 字段的结构、AI 解读按钮的显示条件、重置参数"清空而非填 0"。
