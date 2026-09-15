# 012 实施计划: 合约实盘适配层 (BnFuturesExchange)

**日期**: 2026-09-13 | **前置**: 011 已实施(现货实盘闭环) | **规格**: spec.md

## 摘要

把 011 的实盘运行器接到合约: ① 新建 `BnFuturesExchange`(实现 `Exchange`, REST 用现成 `FuturesClient`, WS 新建 fapi 盘口/用户流);
② `FuturesClient` 补公开端点(klines/depth)与账户余额(listenKey 管理); ③ `LiveContext` 持仓缓存支持**定向**(hedge 多空独立可见);
④ `Engine::run_live` 装配按市场分派(合约: 杠杆/保证金/双向模式幂等设置 + 合约账户快照 + 合约持仓);
⑤ 停机兜底平仓按 one-way/hedge 语义装配参数; ⑥ CLI 按 `market` 选交易所; ⑦ demo 真实闭环验证。

## 技术上下文

- **零新增依赖**; 复用 tokio-tungstenite(已有)、rust_decimal、chrono。
- **复用(不重写)**: 门禁/时钟预检/参数对齐/归属前缀/停机清理编排/logs/台账/风控 全部来自 011; `FuturesClient` 的签名端点已实测通过(demo)。
- **新增 WS**: `crates/locus_binance/src/futures_ws.rs` —— 盘口(`fstream` depth)+ 用户流(listenKey, 就绪后再返回)。
- **端点(实测 2026-09-13)**: demo `wss://demo-fstream.binance.com/ws/...` / 主网 `wss://fstream.binance.com/ws/...`; listenKey `POST /fapi/v1/listenKey`(PUT 续期 / DELETE 关闭)。

## 决策

| # | 决策 | 结论 |
|:--|:--|:--|
| D1 | 适配层形态 | 新建 `BnFuturesExchange`(与 `BnSpotExchange` 平行), 不把合约塞进现货实现; `Exchange` trait 不改 |
| D2 | 用户流 | listenKey + fapi WS(实测可用); 订阅**就绪后**返回; keepalive 30min(PUT)与 011 现货旧实现同款思路(此处保留, 因 listenKey 仍有效) |
| D3 | 持仓语义 | 缓存键 `pair|side`; `position(pair)` 聚合净仓(hedge: size 差 + side 取大者); `position_directional` 精确 |
| D4 | 平仓参数 | one-way: `reduce_only=true`; hedge: `position_side=Some(持仓方向)` 且不带 reduceOnly(实测 fapi 拒绝两者同带) |
| D5 | 装配 | 启动幂等: set_leverage(参数 `leverage`, 默认 1)/ set_margin_type(isolated 默认) / set_position_side_dual(market/position_mode) |
| D6 | 市场分派 | CLI 与引擎按 `StrategyConfig.market` 选交易所; 现货路径代码路径不变(SC-005) |
| D7 | 合约账户快照 | `/fapi/v2/balance`(按 asset) + `positionRisk`(按 symbol); 启动即观测残留挂单 |
| D8 | 与现货的差异面 | 手续费按 USDT 计、持仓有 `entry_price/mark_price` 真实值(现货为 0)、`position_side` 语义 |

## 改动清单

1. `crates/locus_binance/src/futures_client.rs`: 补 `get_klines` / `get_depth` / `balance_of(asset)` / `create_listen_key` / `keepalive_listen_key` / `close_listen_key`
2. `crates/locus_binance/src/futures_ws.rs` (新): 盘口订阅 + 用户流(listenKey, 就绪语义) + 事件解析(ORDER_TRADE_UPDATE / TRADE_LITE / ACCOUNT_UPDATE / ACCOUNT_CONFIG_UPDATE)
3. `crates/locus_binance/src/futures.rs` (新): `BnFuturesExchange: Exchange`
4. `crates/locus_binance/src/lib.rs`: 导出
5. `crates/locus_strategy/src/context.rs`: `LiveContext` 定向持仓缓存(`pair|side`)+ `position()` 聚合 + `position_directional`
6. `crates/locus_engine/src/command.rs`: 装配按市场分派(合约设置杠杆/保证金/双向 + 合约账户快照 + 合约持仓 + 平仓参数按 one-way/hedge)
7. `crates/locus_engine/src/live.rs`: `plan_cleanup` 支持合约平仓参数(position_side/reduce_only)
8. `crates/locus_cli/src/commands/mod.rs` + `run.rs` + `instances.rs`: 按 market 构造交易所; info 合约快照
9. 测试: 纯逻辑单测(合约事件解析/持仓聚合/平仓参数) + demo 真实闭环(`demo_futures_live_runner.rs`)
10. 文档: architecture §三(合约实盘落地) / roadmap(012 行 + 基线) / testnet(合约 WS 实测记录) / lua-api(合约持仓语义) + 本档案

## 验证

- 单测: 基线 270 不回归 + 新增(事件解析/持仓聚合/平仓参数/余额提取)
- demo 真实链路: a) 用户流就绪并发成交 b) 开仓→停机撤单兜底零残留 c) `stop --close-all` 平仓归零 d) hedge 双向开平
- 不回归: 现货用例(demo_user_stream_live / demo_open_orders_live)仍全绿

## 风险

| # | 风险 | 处置 |
|:--|:--|:--|
| R1 | 合约服务器时间与现货不同步(实测差 1.5-1.9s) | 时钟预检按**合约**市场取 `/fapi/v1/time`(不能复用现货取数) |
| R2 | hedge 模式下 fapi 拒绝 reduceOnly | 平仓参数按 D4 分支; 单测覆盖 |
| R3 | 合约 WS 断线(lifespan 限制) | 用户流断线=异常停机(011 P1 口径); 盘口流指数退避重连 |
| R4 | TRADE_LITE 新事件与 ORDER_TRADE_UPDATE 重复 | 只用 ORDER_TRADE_UPDATE 做成交回写(含 tradeId/手续费), TRADE_LITE 忽略并注明 |
