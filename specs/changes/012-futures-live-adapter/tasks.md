# 012 任务分解

> 状态: **全阶段实施并真实验证**(2026-09-13)。
> 基线: 270 → **282 passed / 0 failed / 11 ignored**(011 实施后 → 012 实施后)。
> 规则: 纯逻辑→单测; 交易流程→demo 真实调用(禁 mock/假 token), 每轮前对齐**合约** demo 时钟(`/fapi/v1/time`)。

## 阶段 1 — REST 与解析补齐(纯逻辑可测)

- [x] **T001 合约公开端点与账户读取**: `FuturesClient` 补 `get_klines` / `get_depth` / `balance_of(asset)`; 判据: 单测解析 + demo 真实调用
- [x] **T002 listenKey 生命周期**: `create_listen_key` / `keepalive_listen_key` / `close_listen_key`(PUT 续期 30min); 判据: demo 真实调用 200
- [x] **T003 合约事件解析纯逻辑**: `ORDER_TRADE_UPDATE` / `TRADE_LITE`(忽略) / `ACCOUNT_UPDATE` / `ACCOUNT_CONFIG_UPDATE`; 判据: 单测(成交字段/状态映射/未知事件返回 None)

## 阶段 2 — WS 与适配层

- [x] **T004 fapi WS 盘口**: `futures_ws.rs::subscribe_depth`(depthUpdate → `OrderBookUpdate`); 判据: 单测解析 + demo 订阅到帧
- [x] **T005 fapi WS 用户流**: listenKey + WS, **就绪后返回**(oneshot + 超时), 断线重连重新订阅; 判据: 单测(帧分派)+ demo 真实收到 ORDER_TRADE_UPDATE
- [x] **T006 `BnFuturesExchange`**: 实现 `Exchange` 全方法(位置/平仓参数透传); 判据: 单测(OrderRequest→fapi 参数装配)

## 阶段 3 — 引擎/上下文接线

- [x] **T007 LiveContext 定向持仓**: 缓存键 `pair|side` + `position()` 聚合净仓 + `position_directional` 精确; 判据: 单测(one-way 单侧/hedge 双仓/净仓聚合/空仓)
- [x] **T008 合约平仓参数装配**: `plan_cleanup` 支持 one-way(reduce_only)与 hedge(position_side)分支; 判据: 单测 3 例
- [x] **T009 `Engine::run_live` 市场分派**: 合约启动装配(杠杆/保证金/双向幂等)+ 合约账户快照 + 合约持仓查询; 判据: 单测(装配参数选择)+ T012 真实
- [x] **T010 CLI 按 market 选交易所 + info 合约快照**: 判据: 单测/实测输出

## 阶段 4 — 真实链路验证(demo 合约, 需时钟对齐)

- [x] **T011 合约用户流实测**: 订阅就绪 → 下单 → 收到 ORDER_TRADE_UPDATE 成交并转 OrderFill
- [x] **T012 合约实盘闭环(SC-002)**: 挂单 → 停机撤单兜底 → 交易所零残留
- [x] **T013 `stop --close-all` 合约平仓(SC-003)**: 开仓 → 停机平仓 → 持仓归零
- [x] **T014 hedge 双向(SC-004)**: dual=true, 多空独立开仓 → 停止后两侧归零(或只留策略意图侧)
- [x] **T015 基线不回归(SC-001/SC-005)**: 全量测试全绿 + 现货用例仍绿

## 阶段 5 — 文档与档案

- [x] **T016 文档同步**: architecture/roadmap/testnet/lua-api + 本档案(spec/plan/tasks/converge)

## 收尾待办(不阻塞归档)
1. `StrategyScheduler` 去留(004 P8 遗留, 建议删除)
2. 合约资金费率/强平建模(不在本变更)

---

## 实施记录 (2026-09-13, demo 合约真实链路)

**测试**: `cargo test --workspace` = **282 passed / 0 failed / 11 ignored**(011 基线 270, 新增 12 例)。

**真实链路(demo USDT-M, ETHUSDT, 杠杆 1x 逐仓, 每轮前对齐 `/fapi/v1/time`)**:

| 任务 | 实测 | 证据 |
|:--|:--|:--|
| T011 用户流 | ✅ | `bn.ws: futures user data stream ready (listenKey)`; 成交经 `ORDER_TRADE_UPDATE` 转 `OrderFill` 回写(`fills` 计数 + 落库) |
| T012 挂单兜底 | ✅ | 限价挂单 1 笔 → 停机 `已撤挂单=1`; `locus info` 复查: 挂单无 / 持仓无 |
| T013 兜底平仓 | ✅ | 市价开多 0.08(含上轮残留 0.04)→ `stop --close-all` → 平仓单 `probe-fut-hold-012-c2912370` 成交 → `残留持仓=0`; 交易所侧 `positionRisk=BOTH 0` |
| T014 hedge 双向 | ✅ | 预配置 `hedge 双向`(自动 dual=true) → 同 tick 开 LONG+SHORT 各 0.04 → 停机两笔平仓(`...c2913030`/`...c2913031`)→ 两侧归零; 之后 one-way 探针复核并把账户切回 one-way |
| T015 不回归 | ✅ | 全量 282 全绿; 现货 011 策略 `locus info`/实盘路径仍正常 |

**实施期发现并修复的真缺陷(计划外, 均由真实链路暴露)**:
1. **平仓单号超 36 字符被交易所拒**: `probe-fut-hold-012-close-<ms>-0` = 43 字符 → `BN 400 Client order id length should be less than 36 chars`, 平仓失败、残留 0.04 仓位(停机清理如实上报, 未静默)。修复: 单号前缀缩短(秒级 6 位)+ `close_client_order_id()` 截断到 36 且保留归属前缀, 单测覆盖。
2. **`locus info` 在无成交策略上 panic**: `COALESCE(SUM(CAST(fee AS REAL)), 0)` 在无成交时返回 INTEGER, 按 f64 解码直接崩溃; 同时 `SUM(CAST(... AS REAL))` 的 f64 往返把手续费变成 `0.1595461800000000096577679187`。修复: 改为 Rust 侧用 `Decimal` 精确累加(single query), 单测覆盖"无成交 / 零手续费 / 小数手续费"三类。
3. **盘口流覆盖式更新是错的**: 合约(及现货)`@depth` 帧是**增量 diff**, 覆盖式会让盘口缺档(实测帧 `b:[] a:[...]`)。修复: 改为按价格增量合并(size=0 删档)+ 深度上限, 现货/合约共用, 单测覆盖合并/删档/排序。

**偏离计划处**:
- `Exchange` trait 新增 `get_positions_directional`(带默认实现 = `get_position` 包装), 现货实现零改动; 这是 hedge 两侧独立可见的唯一必要接口放开。
- `CleanupPlan.close: Option<OrderRequest>` → `closes: Vec<OrderRequest>`(hedge 需两侧各一笔), CLI 输出随之调整为多笔。
- 时钟预检按**市场**取数: 实测现货/合约 demo 服务器时间相差 1.5~1.9s, 合约必须用 `/fapi/v1/time`。
