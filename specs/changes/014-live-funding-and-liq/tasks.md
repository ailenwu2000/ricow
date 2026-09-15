# 014 任务分解

> 状态: **全部完成**(2026-09-13)。
> 基线: 286 → **289 passed / 0 failed / 11 ignored**。
> 规则: 纯逻辑单测; 交易所调用在 demo 真实执行(禁 mock/假 token)。

## 阶段 1 — 数据源与存储(纯逻辑 + 单测)

- [x] **T001 `FuturesClient::income`**: 签名 `GET /fapi/v1/income`(incomeType/startTime/limit)+ `FundingIncome` 解析; 判据: 单测(正常/空数组/字段缺失)
- [x] **T002 `funding_fees` 落库**: 建表 + `insert_funding_fee`(tran_id 幂等)+ `funding_total`(Decimal 聚合)+ `latest_funding_time`; 判据: 单测(幂等/空表/多笔精确合计)
- [x] **T003 距强平距离纯函数**: `liquidation_distance(mark, liq, side)`; 判据: 单测(多/空/零价/缺失)

## 阶段 2 — 引擎接线

- [x] **T004 启动补拉 + 运行期增量**: 启动拉 7 天窗口落库; `select!` 增加 30 分钟定期拉取分支(失败只 warn)
- [x] **T005 停机前拉取 + 摘要**: 停机清理前再拉一次, 输出"累计资金费 N 笔/合计 X"
- [x] **T006 强平距离告警**: 快照与成交后刷新持仓时计算, 低于阈值(默认 15%, params `liq_warn_pct` 覆盖)warn; liq 缺失标"未知"

## 阶段 3 — 可见性与验证

- [x] **T007 CLI `info` 显示累计资金费**(Rust 侧 Decimal 聚合; 查询失败如实标注)
- [x] **T008 demo 真实调用**: `income` 端点真实拉取(空结果如实显示)+ 真实持仓 liq 距离与告警(高杠杆小仓现场验证)
- [x] **T009 全量回归** + 文档同步(testnet/architecture/roadmap)+ converge

## 收尾待办(不阻塞)
1. L4(hedge 空方向单成交但零动作仍计费)—— 撮合层口径待拍板
2. `StrategyScheduler` 去留(004 P8 遗留)

---

## 实施记录 (2026-09-13)

**实现**:
- `locus_core`: `FundingIncome` 类型(账户级事实, 与交易所实现解耦)+ `Exchange::funding_income` 默认方法(空 = 现货不支持)
- `locus_binance`: `FuturesClient::income(income_type, start_ms, limit)` + `parse_income` 纯函数(异常行整行跳过, 不伪造 0); `BnFuturesExchange` 覆盖 `funding_income` 走 `FUNDING_FEE`
- `locus_strategy::db`: `funding_fees` 表(`tran_id` 主键)+ `insert_funding_fee`(INSERT OR IGNORE 幂等)+ `funding_total`(Rust 侧 Decimal 精确聚合)+ `latest_funding_time`(增量水位)
- `locus_engine::live`: `liquidation_distance(mark, liq, side)` 纯函数(多/空双向; 缺失 → None; 穿越 → 负数如实报出)
- `locus_engine::command`: 启动补拉 7 天 + `select!` 增 30 分钟 `LiveEvent::Funding` 分支 + 停机清理前补拉; 快照与成交后刷新持仓时 `warn_near_liquidation`(阈值 params `liq_warn_pct`, 默认 15%)
- `locus_cli`: `info` 显示累计资金费(账户口径)与持仓"距强平 X%"(缺 liq → "未知")

**单测(286 → 289)**:
- `test_parse_income_rows_and_skips_bad`: 正常两行 + 非数字 income + 缺 tranId 的行整行跳过; 空数组/非数组安全; 精确小数不丢
- `test_funding_fees_idempotent_and_exact_sum`: 重复 tran_id(含跨策略)→ 只保留一行; 精确合计(`-0.12345678 + 0.000004`); 水位; 空表; per-strategy 过滤
- `test_liquidation_distance_directions_and_missing`: 多头/空头各 15%; 穿越 → 负数; 零价 → None

**demo 真实调用**:
- `GET /fapi/v1/income` 签名调用 → 200 `[]`(账户无跨 8h 持仓, 如实显示 0 笔; 入库路径未用真实流水验证, 已如实标注)
- 50x 逐仓小仓 → `WARN risk: 接近强平: 距离 1.48% (阈值 15.0%) liq=2443.31 mark=2479.89`, 与公式 1.475% 一致; 停机 `--close-all` 平仓归零
- `locus info`: "资金费: 0 笔(账户无资金费流水)" + 挂单/持仓实时查询

**未做**: 真实资金费流水的入库验证(需持仓跨越 8h 结算点)。
