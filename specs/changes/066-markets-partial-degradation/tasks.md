# 066 任务清单

## P0 后端：视野快照降级
- [x] T0.1 `Snapshot` 加 `spot_ok` / `futures_ok`
- [x] T0.2 `fetch()` 三份各自成败（`tokio::join!` + 各自吞错误）
- [x] T0.3 每半各自超时 `with_timeout`（25 秒）
- [x] T0.4 两半都失败才整体失败（附两半原因）
- [x] T0.5 半边失败记 `tracing::warn`（不是静默降级）
- [x] T0.6 `current_view` 用 `build_view_with` 带可用性
- [x] T0.7 `ensure_pair_in_scope`：要查的那半不可读 ⇒ 放行（不误判越界）

## P1 引擎：`PairsView`
- [x] T1.1 加 `spot_ok` / `futures_ok`（`Option<bool>`）
- [x] T1.2 `skip_serializing_if = "Option::is_none"`（老响应形状不变）
- [x] T1.3 `build_view_with`（带可用性的构造；默认视野下现货依赖合约白名单）
- [x] T1.4 `filter_view` 原样透传可用性
- [x] T1.5 从 `ricow_engine` 导出 `build_view_with`

## P2 前端
- [x] T2.1 `countOf(d, market)`（不可用 → 「—」）
- [x] T2.2 `renderScope` 走 `countOf`
- [x] T2.3 `renderGroup` 加 `readable` 参数；不可用 → 说明 + 指引（不给"看全部"空态）
- [x] T2.4 文案 `mkUnreadable` / `mkUnreadableHint`（中英）
- [x] T2.5 CSS `.mk-unreadable*`
- [x] T2.6 概览 bStock 卡：不可用侧「—」+ `dashBstockPartial` 说明
- [x] T2.7 `ScopeCounts` 加 `spot_ok` / `futures_ok` + 组装时传出

## P3 回归断言
- [x] T3.1 `test_build_view_all_keeps_spot_readable_when_futures_unavailable`
- [x] T3.2 `test_build_view_filtered_marks_spot_unreadable_when_futures_unavailable`
- [x] T3.3 `test_build_view_without_availability_keeps_legacy_behavior`
- [x] T3.4 `test_build_view_all_keeps_futures_readable_when_spot_unavailable`
- [x] T3.5 `test_build_view_filtered_spot_follows_futures_unavailability`
- [x] T3.6 `test_markets_view_shows_dash_not_zero_when_side_unreadable`
- [x] T3.7 `test_pairs_view_availability_is_optional_bool`
- [x] T3.8 `test_overview_scope_carries_availability_flags`

## P4 验证
- [x] T4.1 变异验证 **7/7** 全抓住
- [x] T4.2 真机降级实测（合约不可达 → 200 + 现货 1383 + 3.5 秒）
- [x] T4.3 真机默认视野实测（现货也标不可读）
- [x] T4.4 066 专项走查 `probe066.mjs` —— **8 PASS / 0 FAIL**
- [x] T4.5 门禁六件全绿（整仓 1088 passed / 0 failed / 22 ignored）
- [x] T4.6 e2e 460/0 + CDP 全绿 + 064/065 走查仍全绿
- [x] T4.7 converge.md
