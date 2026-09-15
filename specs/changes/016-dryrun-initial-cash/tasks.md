# 016 任务分解

> 状态: **全部完成** (2026-09-13) | 基线: 304 → **306 passed / 0 failed / 11 ignored**(零警告)

- [x] **T001** `dry_run_initial_cash(Option<f64>) -> Result<Decimal, String>` + 默认常量
- [x] **T002** 单测: 缺省/自定义/非法(0、-1、NaN、inf 均报错且含参数名)
- [x] **T003** `run.rs` Dry Run 分支接线 + 打印虚拟本金
- [x] **T004** 真实预演: `initial_cash = 300.0` + `[risk] max_position_notional = 300` → `Dry Run 虚拟本金: 300 USDT`,
      建仓 `50%×300 = 150 USDT`(`size=0.060554314192115 @ ~2477.115`), 拒单 0
      —— 对比修改前同一份配置: 65 单全被拒(`current=50000, limit=400`)
- [x] **T005** 非法值真实复现: `initial_cash = 0.0` → `params.initial_cash 非法 (需 > 0 的有限数, 收到 0)`
- [x] **T006** 文档同步(`lua-api.md`/`architecture.md`/`roadmap.md`)+ P4 runbook 改为"一份配置" + `converge.md`

## 实施记录 (2026-09-13)

- 插入单测时重复引入 `use rust_decimal_macros::dec;`(模块里已有一处)导致 E0252, 已去重 —— 教训: 往测试模块追加代码前先看该模块已有的 `use`。
- P4 runbook 里原先写的"Dry Run 用 100k 口径 / 切实盘前改成小资金口径"两阶段绕行做法**已删除**, 改为一份配置(本变更的价值所在)。
