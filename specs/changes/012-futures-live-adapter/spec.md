# 012 功能规格: 合约实盘适配层 (BnFuturesExchange)

**状态**: 已实施 (2026-09-13) | **编号**: 012 | **关联**: 011 Phase 2(011 明确"合约实盘后置, 可拆 012")

## 起点事实 (实测, 2026-09-13)

> ① `Exchange` trait 的唯一实现是现货 `BnSpotExchange` (`crates/locus_binance/src/spot.rs:99`); 合约只有 `FuturesClient` 裸 REST(签名下单/撤单/账户/持仓/杠杆/保证金/双向模式齐备, **无 WS**);
> ② 011 已把实盘链路做成通用形状(`Engine::run_live` / 停机清理 / 参数对齐 / 归属前缀 / 门禁 / 时钟预检), 合约缺的是 **`Exchange` 适配层 + fapi WS**: 不需要新数据通道;
> ③ **实测 (2026-09-13)**: 合约 demo WS 端点
>    - 盘口流 `wss://demo-fstream.binance.com/ws/<symbol>@depth@100ms` → 推送 `depthUpdate`(`b`/`a` 档位, `E` 事件时间);
>    - 用户流 `POST /fapi/v1/listenKey` → 200 `{listenKey}`(未随现货一起下线), WS `wss://demo-fstream.binance.com/ws/<listenKey>` 直推事件(无 `subscriptionId` 外壳);
>    - 事件类型: `ORDER_TRADE_UPDATE`(订单/成交, `o.x`=NEW/TRADE, `o.ap` 均价, `o.L`/`o.l` 最新成交价量, `o.n` 手续费, `o.t` tradeId)、`TRADE_LITE`(轻量成交)、`ACCOUNT_UPDATE`(余额/持仓 `a.P[].pa/ep/up/ps/mt`)、`ACCOUNT_CONFIG_UPDATE`(杠杆);
>    - 幂等: 已是目标值时 `marginType` 返 400 "No need to change..."、`positionSide/dual` 返 -4059 (均按成功处理); 主网 `fstream.binance.com` 对应同名端点;
> ④ 合约过滤器已可解析(`MIN_NOTIONAL` 字段名 `notional`, 实测 ETHUSDT ≈ 50 USDT), 011 的参数对齐/拒单逻辑与市场无关, 可直接复用;
> ⑤ one-way 模式 `positionRisk.positionSide = "BOTH"`(非 LONG); hedge 模式才有 LONG/SHORT 两条(**实测, specs/testnet.md §实测语义要点**)。

## 用户场景与测试

### 用户故事 1 - 同一个实盘运行器跑合约 (P1)
用户把已部署策略的 `market` 配为 `futures`(可选 `position_mode = "hedge"`), 用与现货完全相同的方式启动: `locus run <name> --live` / `locus start <name> --live`。引擎以**合约账户**事实源(可用余额/持仓/挂单)初始化, 行情驱动同一份 Lua 脚本下真实合约单; 成交经用户流回写并落库; 停机清理撤单兜底 + (可选)平仓, 合约语义正确(one-way `reduce_only`, hedge `positionSide`)。

### 用户故事 2 - hedge 双向持仓可见 (P2)
hedge 模式下策略需同时看到多空两侧独立仓位(`ctx.pos_size(pair, "long"/"short")`), 而不是被合并成净仓; 停机平仓需按持仓方向分别下平仓单。

### 用户故事 3 - 风险与不一致可见 (P3)
杠杆/保证金类型/双向模式在启动时按配置幂等设置(交易所"无需变更"不当作错误); 账户事实源读取失败、用户流断线、参数对齐拒单等一律如实上报(不静默)。

## 功能需求

- **FR-001**: `BnFuturesExchange` 必须实现 `Exchange` 全部方法(markets/klines/orderbook/place_order/cancel_order/get_open_orders/get_balance/get_position/subscribe_orderbook/subscribe_user_events)
- **FR-002**: 盘口与用户流必须走 **fapi WS**(demo/mainnet 域名按 REST base 推导), 用户流用 listenKey(不存在则新建; 断线重连后重新取用)
- **FR-003**: 用户流订阅必须**就绪后**才返回 Stream(011 的实测教训: 未就绪即下单会漏成交)
- **FR-004**: 合约持仓查询必须区分 one-way(BOTH)与 hedge(LONG/SHORT); `position_directional(pair, side)` 在 hedge 下返回对应方向仓
- **FR-005**: 启动装配按配置幂等设置杠杆(默认 1x)/保证金类型/双向模式; "无需变更"类错误按成功处理, 其他错误如实抛出
- **FR-006**: 停机清理复用 011 编排: 撤单兜底(归属前缀过滤)+ 可选平仓; 合约平仓 one-way 用 `reduce_only`, hedge 用 `positionSide`(两者互斥, fapi 会拒同时带)
- **FR-007**: 合约下单必须携带调用方 `clientOrderId`(011 已修 F1)并支持 `position_side`/`reduce_only` 透传
- **FR-008**: CLI 按策略 `market` 选择交易所实现; `info` 的账户快照对合约显示合约余额/持仓/挂单
- **FR-009**: 复用 011 的门禁(双条件)/时钟预检/参数对齐/风控/台账 `mode=live`, **不新增并行机制**
- **FR-010**: 合约账户余额按 `asset` 查询(`/fapi/v2/balance`), 缺失资产返回 0(不报错)

## 成功标准

- **SC-001**: `cargo test --workspace` 全过, 基线 270 不回归; 新增纯逻辑用例(合约事件解析/持仓语义/平仓参数装配)
- **SC-002**: demo 合约真实调用闭环: 下单 → 用户流成交回写 → 落库 → 停机撤单兜底 → 交易所零残留
- **SC-003**: demo 合约 `stop --close-all` 兜底平仓: 平仓后该 pair 持仓归零
- **SC-004**: hedge 模式: 多空两侧独立可见且可分别平掉(持仓归零)
- **SC-005**: 现货路径零变化(011 的验证结论不回归; 现货用例仍全绿)

## 口径拍板 (自主决策, 依据=实测/最简; 可推翻)

- **拍板 1 用户流机制 = listenKey + fapi WS**(实测 200 可用; 合约无 WS-API 用户流, 与现货不同), 订阅就绪语义照 011。
- **拍板 2 hedge 持仓表示 = 定向缓存**(LiveContext 缓存按 `pair|side` 存; `position(pair)` 聚合为净仓)。依据: 现有单条缓存装不下 hedge 两侧, 而 Lua API 已承诺 `pos_size(pair, side)` 双仓可见 → 放开缓存键即可, 不新造接口。
- **拍板 3 平仓参数 = one-way `reduceOnly` / hedge `positionSide`**(specs/testnet.md 实测: hedge 带 reduceOnly 会被 fapi 拒)。
- **拍板 4 合约 K 线/盘口 = 直接复用 `/fapi/v1/klines` `/fapi/v1/depth`**(公开端点, 与现货同构解析), 不引入新的数据源。
- **拍板 5 市场选择 = 按策略 TOML `market` 字段**(`spot`/`futures` 已有字段, 008 前就存在), 不新增开关。

## 假设
- A1: 主网合约端点与 demo 同构(`fapi` / `fstream` / `fapi/v1/listenKey`), 默认不跑主网(用户主导)
- A2: 合约实盘只在 demo 真实验证(011 A6 同口径); 强平/资金费等交易所行为不在本变更范围
- A3: 依赖 011 已交付的通用能力(不做重复实现)
