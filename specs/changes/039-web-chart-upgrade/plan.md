# 039 计划

## 方案取向

1. **市场 K 线指标全在前端算**（D1）。`/api/markets/{symbol}/klines` 已返回 `volume`，
   MA 与量柱都是一次线性扫描；走后端要新增端点 + 单测，收益为零。指标画在**同一图表实例**的
   独立价格轴上（lightweight-charts v4 的 `priceScaleId` + `scaleMargins`），不引第二图表的
   时间轴同步代码。
2. **基准线与回撤线由后端算、随 `BacktestChart` 下发**（D2）。理由不是性能而是**口径一致**：
   - 基准线必须与指标卡 `benchmark_return_pct` 同源（首次成交价 + `entry_equity`），
     前端拿不到这两个标量之外的信息（`fills` 在超过 1000 笔时被裁掉最早的那批，前端从
     `fills[0]` 反推建仓时刻是**错的**）；
   - 回撤必须用**全分辨率**净值曲线算，再抽稀 —— 前端拿到的已经是抽稀后的序列，
     对抽稀序列重算回撤会系统性低估（最深处常落在被抽掉的点上）。
3. **平仓盈亏由引擎记账，不由前端配对**（D3）。`PnlTracker` 本就是"平仓事件"语义
   （`record_pnl` 一次 = 一次平仓，胜率/已实现盈亏都由它累加），给它加 `(时刻, 盈亏)` 记录即可
   与聚合指标天然自洽；前端按 fill 配对在合约对冲/组合模式下无法判定"这笔卖是开空还是平多"。
4. **时间戳口径**（D4）：回测用**虚拟 bar 时间**（`current_bar.close_time`）而非 `Utc::now()`
   —— 否则明细表与图表时间轴对不上；live/dry-run 路径用成交自身的 `fill.timestamp`。
5. **截断要报数**（D5）：明细保最近 1000 条 + 携带总数，界面明说截断（对应 `exposure` 视图
   的既有诚实性先例）。

## 影响面

| 文件 | 变化 |
|---|---|
| `crates/ricow_strategy/src/pnl.rs` | `ClosedTrade` 结构；`record_pnl` 增 `ts` 参数；`closed_trades()` / `closed_trades_total()`；上限 `MAX_CLOSED_TRADES=1000` |
| `crates/ricow_strategy/src/backtest.rs` | `close_position` 增 `ts` 参数（4 处调用点）；报告增 `closed_trades` / `closed_trades_total` / `benchmark_entry_time` |
| `crates/ricow_strategy/src/context.rs` | `apply_position_change` 增 `ts`（取 `fill.timestamp`） |
| `crates/ricow_engine/src/command.rs` | 测试调用点适配新签名 |
| `crates/ricow/src/commands/backtest.rs` | `BacktestChart` 增 `benchmark` / `drawdown` / `closed` / `closed_total`；纯函数 `benchmark_series` / `drawdown_series` / `downsample_opt` + 单测 |
| `crates/ricow/src/web/assets/markets.js` | MA7/25/99 三条线 + 成交量柱（同图叠加） |
| `crates/ricow/src/web/assets/strategies.js` | 基准线(虚线) + 回撤副图 + 平仓盈亏明细表 |
| `crates/ricow/src/web/assets/style.css` | 图例 chips / 明细表样式 |
| `specs/{backtest,architecture,roadmap}.md` | 收尾同步 |

## 测试策略

- **引擎单测**（纯逻辑，已知向量）:
  - `PnlTracker` 平仓记录：顺序、正负分类与聚合一致、超上限保留最近 + 总数不丢。
  - 回测报告带 `closed_trades`，且 `Σ closed_trades.pnl == realized_pnl`（>1000 笔场景例外，由上限用例覆盖）。
- **commands 单测**（纯函数）:
  - `benchmark_series`：建仓前为 `None`、建仓点起等于 `entry_equity × price/entry_price`、无成交全 `None`、末点与 `benchmark_return_pct` 一致。
  - `drawdown_series`：峰谷已知序列；峰值处为 0；最小值 = 序列层面的最大回撤。
  - `downsample_opt`：保首尾 + `None` 透传。
- **前端纪律**: `web/assets.rs` 既有扫描（禁 localStorage / 禁 `input.type=` / 只读）与
  `ALL_JS` ↔ `index.html` 对齐不变；`cargo test -p ricow --bin ricow` 全绿。
- **零回归**: `BacktestChart` 既有三条序列 `times/price/equity` 的长度与取值语义不变
  （只加字段），既有前端渲染路径不受影响。
- 无新增外部网络依赖路径 → **不新增 `#[ignore]` 用例**。

## 验证

- `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --locked -- -D warnings` /
  `scripts/ci_grep_gates.sh` / `cargo test --workspace --locked --no-fail-fast`。
- 真机取证: `RICOW_ROOT=<repo>/tmp/<名> target/debug/ricow.exe web --no-open --port <空闲端口>`，
  浏览器核对 K 线均线/量柱与回测图表（基准线/回撤/明细表）；纪律见 skill `ricow-webui-e2e-verify`。
