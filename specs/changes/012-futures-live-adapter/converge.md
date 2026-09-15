# 012 收敛评估 (converge)

**日期**: 2026-09-13 | **范围**: 合约实盘适配层 (Binance USDT-M, demo 全链路真实验证)
**结论**: ✅ 已实施并真实验证, 缺口闭环; 遗留项见 §四。

---

## 一、评估覆盖

- **spec**: 3 用户故事 / 12 条 FR / 5 条 SC / 假设 A1-A6 —— 逐条对照实现
- **plan**: D1-D8 决策 + P1-P4 + 改动清单 7 项
- **宪法**: 原则一(务实) / 二(简单) / 三(测试纪律: 禁 mock, 真实 demo 调用) / 四(文档) / 五(禁止非必要复杂度)

## 二、验收标准逐项实测

| # | 判据 | 实测 | 结论 |
|:--|:--|:--|:--|
| SC-001 | 现货实盘路径不回归 | 011 探针复跑(挂单/平仓)与 `locus info` 均正常; 全量 282 全绿 | ✅ |
| SC-002 | 合约挂单 → 停机撤单兜底 → 交易所零残留 | `已撤挂单=1`; 交易所侧挂单无 / 持仓无 | ✅ |
| SC-003 | 合约 `stop --close-all` → 持仓归零 | 平仓单成交(单号 29 字符), `残留持仓=0`; `positionRisk` 该 symbol 归零 | ✅ |
| SC-004 | hedge 双向: 多空独立开仓且分别平掉 | dual=true 自动切换; 两笔平仓单(各带 positionSide); 两侧归零 | ✅ |
| SC-005 | 不引入主网/资金风险 | 全程 `demo-fapi.binance.com` + `demo-fstream.binance.com`; 杠杆 1x 逐仓; 单笔名义 ≈100 USDT; 收尾无残留仓位/挂单 | ✅ |

## 三、FR 对照(摘要)

| FR | 实现 | 证据 |
|:--|:--|:--|
| FR-001~003 合约 Exchange 适配 | `BnFuturesExchange` (markets/klines/depth/order/cancel/open_orders/balance/position/定向持仓) | 单测 + demo 真实下单/查询 |
| FR-004~005 WS 盘口与用户流 | `futures_ws.rs`: depth 增量合并; listenKey 用户流**就绪后返回** + 30min keepalive + 重连 | 单测 + 实测 `futures user data stream ready` |
| FR-006 事件解析 | `ORDER_TRADE_UPDATE` → `OrderFill`/`OrderUpdate`; `TRADE_LITE`/`ACCOUNT_UPDATE`/`ACCOUNT_CONFIG_UPDATE` 忽略(避免重复计成交) | 单测(含未知事件 → None) |
| FR-007 合约语义装配 | 杠杆/保证金/双向模式幂等设置(`No need to change ...` 视为成功) | 实测预配置输出 + dual 切换成功 |
| FR-008 定向持仓 | `LiveContext` 缓存键 `pair\|side`; `position()` 聚合净仓; `position_directional` 精确 | 单测(one-way/hedge/净仓/空仓) |
| FR-009 合约平仓参数 | one-way: `reduceOnly`; hedge: `positionSide` 且不带 `reduceOnly` | 单测 2 例 + 实测两类平仓成功 |
| FR-010 停机清理多笔 | `CleanupPlan.closes: Vec<OrderRequest>`; 单号 ≤36 字符 | 实测 hedge 两笔平仓 |
| FR-011 CLI 市场分派 | `run --live` 按 `market` 选交易所 + 合约预配置; 时钟预检按市场取数 | 实测(现货/合约服务器时间差 1.5~1.9s) |
| FR-012 info 合约快照 | 余额/定向持仓/挂单实时查询 | 实测输出 |

## 四、收敛遗留(不阻塞归档)

1. `StrategyScheduler` 去留(004 P8 遗留, 建议删除) —— 仍未处理。
2. 合约资金费率 / 强平建模: 不在本变更(记账与风险模型的下一层)。
3. hedge 模式下"只平一侧"(策略意图侧)未做: 现为两侧全平(`--close-all` 语义), 策略级细分留待真实需求。
4. `TRADE_LITE` 事件未消费(成交信息与 `ORDER_TRADE_UPDATE` 重复); 若未来交付低延迟需求可评估。

## 五、留档教训

1. **交易所硬限制要在生成端就约束**: `clientOrderId` ≤ 36 字符, 平仓单号用"策略名前缀 + 毫秒时间戳 + 序号"必超 —— 归属前缀越长越危险(策略名 20 字符时几乎必然失败)。生成单号的函数必须自带截断并保留可判归属的前缀头。
2. **SQLite 动态类型下 `SUM` 不可信**: 无行时 `COALESCE(..., 0)` 返回 INTEGER 会击穿 f64 解码(直接 panic), 且 REAL 往返污染账目小数(实测 `0.1595461800000000096577679187`)。账目类数字用 `Decimal` 侧聚合, 别交给 SQL。
3. **WS 盘口默认是增量**: 覆盖式更新会让盘口缺档, 进而让 `price()` 返回 None。新市场的盘口实现要先确认帧语义(diff vs snapshot)。
4. **市场维度天然不同**: 现货/合约的服务器时钟、过滤字段名(现货 `minNotional` vs 合约 `notional`)、平仓参数(`reduceOnly` vs `positionSide`)都不同, "一个市场通了"不等于另一个能直接复用分支。
