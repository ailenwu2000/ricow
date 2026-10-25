# 065 任务清单

## P0 判别
- [x] T0.1 `isNetworkError(err)`（只认 `network error` / `拉取失败` —— 判据要窄）
- [x] T0.2 `networkGuidance(err)`（非网络错误返回 null）
- [x] T0.3 `appendNetGuidance(box, err)`

## P1 文案
- [x] T1.1 中文：`sgBtNetTitle` / `sgBtNetProxy` / `sgBtNetBase` / `sgBtNetFapi`
- [x] T1.2 英文同上
- [x] T1.3 明说"不是策略写错了" + "只影响行情与回测，下单仍走主网"

## P2 渲染
- [x] T2.1 回测 error 态挂指引
- [x] T2.2 寻优 error 态挂指引
- [x] T2.3 多维优化 error 态挂指引
- [x] T2.4 验证 error 态挂指引
- [x] T2.5 CSS `.sg-net-guide*`

## P3 回归断言
- [x] T3.1 `test_backtest_network_error_shows_actionable_guidance`（含"四个渲染点都挂"计数断言）
- [x] T3.2 两份文案**分别**断言（"与"不是"或"）

## P4 验证
- [x] T4.1 变异验证 5/5 全抓住
- [x] T4.2 065 专项走查 `probe065.mjs` —— **12 PASS / 0 FAIL**
- [x] T4.3 门禁六件全绿
- [x] T4.4 e2e（用官方镜像）—— 460 PASS / 0 FAIL
- [x] T4.5 converge.md
