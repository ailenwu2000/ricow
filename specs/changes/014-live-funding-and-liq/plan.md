# 014 实施计划: 实盘资金费记账 + 强平风险可见

## 一、决策

- **D1 数据源 = 交易所账单**: `GET /fapi/v1/income?incomeType=FUNDING_FEE`(签名)。不用"资金费率×名义"自算 —— 真实账单是唯一事实源(与"禁假数据"纪律一致)。
- **D2 落库表 `funding_fees`**: 与 `fills` 同层(账户事实)。主键用交易所 `tranId`(唯一)→ 幂等重放安全; 字段含 `strategy_id`(与 fills 同法, 由引擎写入)。
- **D3 水位 = `MAX(funding_time)`**: 增量拉取起点; 不引入状态字段/游标表(重启后从 db 自然续拉, 最简)。启动补拉窗口 7 天(资金费 8h 一条 → 上限 21 条), 足够覆盖停机时长。
- **D4 拉取时机**: 启动(补历史) + 运行期每 30 分钟 + 停机清理前各一次。资金费 8h 结算一次, 30 分钟粒度足够且不产生限频压力(权重 5/次)。
- **D5 展示而非合并**: 资金费单独统计与展示(`info` 的"累计资金费 N 笔 / 合计 X"), 不并入 PnlTracker 的成交口径(两者语义不同: 成交 = 策略行为, 资金费 = 持仓持有成本)。
- **D6 强平距离纯函数**: `liquidation_distance(mark, liq, side) -> Option<f64>`(缺失/异常 → None = "未知"); 阈值默认 15%, 从 params `liq_warn_pct` 覆盖(复用现有 config 通道, 不加配置项)。
- **D7 只提示不动作**: 低于阈值只 `warn`; 不自动减仓/平仓(风控护栏 004 的职责边界不加宽)。

## 二、改动清单

1. `crates/locus_binance/src/futures_client.rs`: `income(income_type, start_ms, limit)` + `FundingIncome` 结构 + 单测(解析/空数组/缺字段)
2. `crates/locus_strategy/src/db.rs`: `funding_fees` 表 + `insert_funding_fee`(幂等) + `funding_total(strategy_id)`(Rust 侧 Decimal 聚合) + `latest_funding_time` + 单测
3. `crates/locus_engine/src/live.rs`: `liquidation_distance` 纯函数 + 单测(多/空/缺失/异常)
4. `crates/locus_engine/src/command.rs`: 启动补拉 + 运行期 `select!` 分支(30 分钟)+ 停机前拉取 + 持仓距强平告警(快照与成交后刷新时)
5. `crates/locus_cli/src/commands/instances.rs`: `info` 显示累计资金费
6. 文档: `specs/testnet.md` / `architecture.md` / `roadmap.md` / 本档案

## 三、风险

- **R1 demo 账户可能没有资金费记录**(测试持仓多为分钟级, 未跨 8h 结算点) → 如实显示"0 笔", 不伪造; 端点可用性与解析由真实调用验证。
- **R2 `income` 端点限频/窗口**: 单次 limit 1000、权重 5, 30 分钟粒度无压力; 若失败只 warn 不影响交易循环。
- **R3 判定阈值拍脑袋**: 15% 只是提示阈值(不触发任何动作), 且可由 params 覆盖 → 风险低。
