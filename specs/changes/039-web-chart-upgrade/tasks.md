# 039 任务与完成记录

## T001 引擎: 平仓事件记账（FR-7 / FR-8）

- [x] `pnl.rs`: `ClosedTrade { time, pnl }`；`record_pnl(amount, ts)`；`closed_trades()` / `closed_trades_total()`
- [x] 上限 `MAX_CLOSED_TRADES = 1000`（保留最近，总数单列计数）
- [x] `backtest.rs`: `close_position(..., ts)` 四处调用点（强平 2 + 现货/合约平仓 2）
- [x] `context.rs`: `apply_position_change(req, fill_price, ts)`（`fill.timestamp`）
- [x] `ricow_engine/src/command.rs` 测试调用点适配
- [x] 单测: 顺序 / 分类与聚合一致 / 超限保留最近 + 总数

## T002 引擎: 报告透出（FR-5 / FR-7）

- [x] `BacktestReport` 增 `closed_trades` / `closed_trades_total` / `benchmark_entry_time`
- [x] 单标的 `report()` 与 `report_portfolio()` 两处都填
- [x] 单测: `Σ closed_trades == realized_pnl`；`benchmark_entry_time` 与 `benchmark_entry_price` 同时为 Some/None

## T003 后端: 图表载荷补强（FR-5 / FR-6 / FR-8）

- [x] `BacktestChart` 增 `benchmark: Vec<Option<f64>>` / `drawdown: Vec<f64>` / `closed` / `closed_total`
- [x] 纯函数 `benchmark_series` / `drawdown_series` / `downsample_opt`（保首尾、None 透传）
- [x] 抽稀同步: 四条序列同一 `stride`，长度与 `times` 恒等
- [x] 单测: 基准口径（建仓前 None / 建仓点起等比 / 无成交全 None / 末点与 `benchmark_return_pct` 一致） /
      回撤（0 在峰、最小值 = max_drawdown） / 抽稀保首尾

## T004 前端: 市场 K 线指标（FR-1 ~ FR-4）

- [x] `markets.js`: MA7/25/99 三条线（数据不足断线，不补 0）
- [x] `markets.js`: 成交量柱（独立价格轴 + `scaleMargins`，涨红跌绿）
- [x] 主题切换（`ricow:theme`）后配色跟着重绘（复用既有兜底路径）
- [x] 周期切换重算（沿用既有 `loadChart` 路径）

## T005 前端: 回测图表与明细（FR-5 ~ FR-9）

- [x] `strategies.js`: 基准线（虚线、muted 色、建仓前空白）
- [x] `strategies.js`: 回撤副图（area、独立轴、向下为负）
- [x] `strategies.js`: 平仓盈亏明细表（涨红跌绿 + 一致性副标 + 截断说明）
- [x] 无平仓事件 → 文案提示（不渲染空表）
- [x] `style.css`: 图例 / 明细表样式

## T006 验证与同步

- [x] fmt / clippy / ci_grep_gates / `cargo test --workspace --no-fail-fast`
- [x] 真机 web 取证（K 线均线+量柱 / 回测基准+回撤+明细）
- [x] 同步 `specs/backtest.md`（图表口径）/ `specs/architecture.md`（如有布局变化）/ `specs/roadmap.md`（基线）
- [x] converge.md
