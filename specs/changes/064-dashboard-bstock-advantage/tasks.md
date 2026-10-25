# 064 任务清单

## P0 后端：overview 补字段
- [x] T0.1 `OverviewReply` 加 `scope`（视野规模，与 `/api/markets` 同源走 `pairs::current_view`）
- [x] T0.2 `scope` 取数加 **5 秒超时**（币安不通时不让整页 HTTP 000）
- [x] T0.3 `OverviewReply` 加 `recent_sessions`（最近 3 个会话，`updated_at` 倒序）
- [x] T0.4 `OverviewReply` 加 `alerts`（`catalog::all()` 里 `blocks_run()` 为真的条目）
- [x] T0.5 测试：`test_overview_always_carries_activity_and_alert_arrays`（三个新字段恒在，且数组不是 null）

## P1 前端：bStock 覆盖卡 + 统计格链接 + 最近操作 + 告警格
- [x] T1.1 文案（中英）：`dashBstock*` / `dashRecent*` / `dashAlert*` / `dashStats*Go` / `dashQuickAiChat`
- [x] T1.2 `S.defaultPair()` / `S.fillDefaultPair()`（默认交易对从视野端点取，不硬编码 BTCUSDT）
- [x] T1.3 `S.syncPairScope()`（新建实盘选中策略后校验视野，视野外就换 + 说明）
- [x] T1.4 文案占位符改 `replaceAll`（`String.replace` 只替换首个，第二次原样留在界面上）
- [x] T1.5 `renderBstock()`（两数字 + 口径徽章 + 优势说明 + 入口；取不到给「不可读」说明）
- [x] T1.6 统计格**每格包 button**（`dash-stat-link`，带 aria-label）
- [x] T1.7 `renderRecent()`（最近 3 会话；标题空 → 「未命名」；口径说明）
- [x] T1.8 `alertCount()` + `renderAlertDetails()`（0 条说"没有要处理的"；有事列 `id ← 复制的谁`）
- [x] T1.9 快捷入口补 AI 助手
- [x] T1.10 `S.fmtRelTime` 导出（概览复用策略库那套相对时间，不另写一份）
- [x] T1.11 `R.ui.icon` 加 `alert`（铃铛）
- [x] T1.12 CSS（`.dash-bstock*` / `.dash-recent*` / `.dash-stat-link` / `.dash-alert*`）

## P2 063 遗留：AI 解读入口
- [x] T2.1 `#sg-bt-assess-entry` 容器 + 文案
- [x] T2.2 `renderAssessEntry()`（有参数改动才显示；没改动给说明）
- [x] T2.3 点击 → 切回编辑 Tab + 滚到 AI 面板（**跳转**，不本地重复渲染）
- [x] T2.4 `S.refreshAssessEntry` 导出，`syncAssessPanel` 里调用（参数变动即刷新，防过期状态）

## P3 063 遗留：重置参数
- [x] T3.1 `paramEntry` 多收 `def`（清单默认值），存进 entry
- [x] T3.2 `S.writeParamEntries(entries, values)`（三种控件类型分别处理的通用写入）
- [x] T3.3 「重置参数」按钮（只回有默认值的键）
- [x] T3.4 重置反馈（「已把 N 个恢复」/「没有可恢复的」）

## P4 实盘列表市场列
- [x] T4.1 `RunItem` 加 `market`（透传 `InstanceView.market`）
- [x] T4.2 表头「市场」列 = **筛选控件**（`#rn-market-filter`）
- [x] T4.3 `buildRow` 加市场徽章（`data-market`，读不到显示 `--` 不猜）
- [x] T4.4 筛选逻辑（前端过滤，`tr.hidden`）
- [x] T4.5 **`data-zh` 改挂内层 `<span>`**（挂在 `<th>` 上会被 applyLang 清空子节点）

## P5 回归断言
- [x] T5.1 `test_default_pair_comes_from_market_scope_not_hardcoded`
- [x] T5.2 `test_new_live_checks_pair_scope_after_selecting_strategy`
- [x] T5.3 `test_new_live_reset_only_restores_manifest_defaults`
- [x] T5.4 `test_runs_table_has_market_column_and_filter`
- [x] T5.5 `test_backtest_tab_has_ai_assess_entry_that_jumps`
- [x] T5.6 `test_dashboard_shows_bstock_coverage_from_scope_endpoint`
- [x] T5.7 `test_dashboard_has_recent_activity_from_aggregate_endpoint`
- [x] T5.8 `test_dashboard_stat_cells_are_all_clickable`
- [x] T5.9 `test_dashboard_has_alert_cell_from_real_gate`
- [x] T5.10 `test_no_data_zh_on_elements_with_interactive_children`（通用扫描）
- [x] T5.11 `test_overview_always_carries_activity_and_alert_arrays`

## P6 验证
- [x] T6.1 门禁六件（fmt / clippy / 整仓 1079 / 红线 / 前端门 / 真机两路）
- [x] T6.2 064 专项走查 `probe064.mjs` —— **15 PASS / 0 FAIL**
- [x] T6.3 064 补充走查 `probe064b.mjs`（mock scope 验数字一致性）—— **5 PASS / 0 FAIL**
- [x] T6.4 CDP 走查 —— **156 PASS / 0 FAIL**
- [x] T6.5 e2e —— 451 PASS / 18 FAIL（18 条全是回测段，根因本机币安不可达，非产品缺陷）
- [x] T6.6 converge.md / roadmap / architecture 基线同步
