# 011 收敛评估 (converge)

**日期**: 2026-09-13 | **范围**: Phase 1(现货实盘运行器) | **Phase 2(合约实盘)**: 未开始
**结论**: ✅ Phase 1 已实施并**真实验证**(demo 现货全链路), 缺口闭环; 遗留项见 §四。

---

## 一、评估覆盖

- **spec**: 3 用户故事 / 16 条 FR / 5 条 SC / 假设 A1-A10 —— 逐条与实现对照(§二)。
- **plan**: 决策 D1-D11 + 实现决策 P1-P9 + 改动清单 10 项 —— 逐项核对(§三 含 3 处计划外修正)。
- **宪法 5 原则**: ①完全本地化(仅交易所 API, 无遥测/自动更新) ②策略层统一 Lua(实盘跑同一份脚本, 只换下单/账户事实源) ③测试纪律(交易流程 demo 真实调用, 禁 mock/假 token —— T013/T014/T015 均为真实调用) ④产物中文 ⑤少而精(零新增依赖、零新表/新通道)。

## 二、验收标准逐项实测

| # | 判据 | 实测证据 | 结论 |
|:--|:--|:--|:--|
| SC-001 | 基线不回归 + 新增实盘纯逻辑用例 | `cargo test --workspace` = **270 passed / 0 failed / 11 ignored**(基线 233 → +37); 新增覆盖: 参数对齐 / 时钟预检判定 / 归属匹配 / 清理编排与幂等 / 门禁 / proto 往返 / 现货 `executionReport` 解析 / 交易所过滤器解析 | ✅ |
| SC-002 | demo 真实调用: 下单 → 成交回写 → 落库 → 停机撤单兜底 → 交易所零残留 | T014: 限价挂单 0.004 ETH → 停机输出 `已撤挂单=1 撤单失败=0 残留挂单=0`, 交易所 `GET /api/v3/openOrders` = `[]`; T013: 用户流收到 `executionReport`(TRADE/FILLED)并转 `OrderFill`; T015: `locus fills` 可见 buy/sell 两笔成交落库 | ✅ |
| SC-003 | 门禁或时钟预检不达标时启动被拒 + 可执行提示 | 门禁双条件单测(单条件 → Dry Run + 原因文本); 真实拒单: 本机超前 **1142ms** → 拒绝启动并打印对齐步骤(自然漂移复现) | ✅(人为拨快复现未执行, 见 §四) |
| SC-004 | 平仓开关分支真实成交, 停机后持仓归零 | T015: 市价买入 0.004 → stdin 下发 `stop --close-all` → 平仓单真实成交 → 余额核对只剩手续费尘埃(0.000084 ETH, 低于 minNotional 无法再卖); 策略持仓按 `step_size` 全额平掉 | ✅ |
| SC-005 | 回测 / Dry Run 既有行为零变化 | 回测路径零改动(改动文件清单见 §三); `shannon_grid ETHUSDT 20d` 两次运行关键指标**逐位一致**(K线 480 / 成交 9 / 净盈亏 71.911606658838271030103339567 / 回撤 3.17% / 拒单 0) | ✅ |

## 三、FR / 决策对照要点

| 主题 | FR | 落地 | 证据 |
|:--|:--|:--|:--|
| 实盘启动门禁 | FR-013 | `live_gate(declared_live, cli_live)` 双条件; 缺一即 Dry Run 并打印原因; `run` 与 `start`(经 proto)同口径 | 单测 2 例 + CLI 实测(带 `--live` 未声明配置时如实打印原因) |
| 时钟预检 | FR-008 | CLI 门禁之后、构造引擎之前取现货 `/api/v3/time`, 超前 >1000ms 或滞后 >4000ms 拒绝 | 单测 3 态 + 实测拒单 |
| 账户事实源 | FR-002/003 | 启动装配: `get_markets`(过滤器) + `get_balance`(base/quote) + 现货持仓(base 可用余额包装) + `get_open_orders` 观测(启动即提示残留) | 实测日志 `实盘账户快照 base=ETH … quote=USDT …` |
| 下单参数对齐 | FR-010 | `prepare_live_order` = 归属前缀注入 + `align_order`(数量向下取整 / 价格"不劣于意图"侧 / 不足 min_qty·min_notional 拒单); 无过滤器缓存则不干预 | 单测 4+3 例; 实测 warn 记录 `0.0040000000000000000832667269 → 0.00400000`、`2245.230000000000018189894035 → 2245.23000000` |
| 成交回写与落库 | FR-004/D4 | 现货 **WS-API** 用户流 → `OrderFill` → `ctx.record_fill` + `db.insert_fill`; 非本实例成交忽略 | T013/T015 实测; `locus fills` 两笔 |
| 停机清理 | FR-005/006/007 | 停消费 → `on_stop` → 撤单兜底(前缀过滤, 单个失败不中断) → 可选平仓 → 残留复查 → 如实输出; `OnceGate` 幂等; 兜底平仓成交经 ≤5s 吸干窗口回写 | T014/T015 实测; 单测 6 例 |
| 可见性 | FR-011/012 | `info` 查交易所实时快照(失败如实标注"状态未知"); 台账 `mode=live` 由 supervisor 写, `list/status/info` 一致 | 实测 `locus info` 输出余额/持仓/挂单 + `mode` 显示 |
| 语境区分 | FR-014/D10 | 复用 `ctx:config_bool` + TOML params, 引擎不注入额外字段 | `specs/lua-api.md` §一 |
| 风控复用 | FR-009 | 实盘 `place_order` 走 004 同一 `RiskEngine::from_config`, 对齐后再校验(看到实际下单量) | 004 已有单测 + 012 接线位置 |
| 平仓开关 | FR-007/A9 | `start/run --close-all` 与停机指令 `stop --close-all` 二者取或; 经 008 stdin 管道下发 | 单测(指令文本/解析)+ T015 实测 |

### 计划外修正(均有实测依据, 已回填 spec/plan)

1. **D4 实现修正**: 现货成交来源改走现货 **WebSocket API** `userDataStream.subscribe.signature` —— legacy listenKey 已被币安 2026-02-20 永久下线(主网/demo 实测 410 Gone)。接口契约 `Exchange::subscribe_user_events` 不变, 语义(实时、非轮询)与 D4 一致。
2. **订阅就绪语义**(真实链路才暴露): 下单必须发生在订阅确认之后, 否则启动瞬间成交漏回写(实测"引擎 fills=0 但持仓已变") → `subscribe_user_events` 改为**就绪后返回**(15s 超时即报错)。
3. **新增代码位置**: `crates/locus_strategy/src/align.rs`(对齐 + 归属前缀 + 实盘下单前处理)、`crates/locus_engine/src/live.rs`(时钟预检判定 / 门禁 / 清理编排)、`crates/locus_binance/tests/demo_user_stream_live.rs`(用户流真实调用用例); `ws.rs` 现货用户流改 WS-API + demo host 映射; 删除已失效的 `create_listen_key`/`keepalive_listen_key` 及仅为它服务的 `http_client`/`api_key_opt`。

## 四、收敛遗留(不阻塞归档)

1. **T016 人为拨快时钟复现**: 需 `sudo date -s`(系统级操作由用户执行, agent 不代跑); 自然漂移已等效复现一次(1142ms 拒绝)。
2. **Phase 2 合约实盘(T019-T021)未开始**: 需新建 `BnFuturesExchange`(`Exchange` 适配 + fapi WS 盘口/用户流) + hedge 定向持仓 + one-way/hedge 参数装配; 合约 listenKey **仍可用**(实测 200), 成交来源不受下线影响; 建议拆为 012。
3. `StrategyScheduler` 去留(004 P8 遗留): 至今无消费者, 职责与 `Engine` 主循环重叠 → **建议删除**; 未在本变更内动(避免与实盘改动混在一起)。
4. 实盘告警对接 003-notifications(未上线): 先落日志 + `target=risk`/`target=align` 可筛。
5. 现货持仓的 `entry_price`/`mark_price` 在快照里记 0(现货无成本价概念): 策略若需成本价须自行记账; 合约实盘(Phase 2)可给真实值。

## 五、留档教训

1. **"外部依赖会变"要按实测而不是按文档**: legacy listenKey 在 plan 阶段是"已验证 TCP 连通", 实施当天已是"410 Gone" —— 第一验证目标的顺序(先验外部依赖)是对的, 且**验证结论必须落到实测**。
2. **就绪语义属于正确性, 不是优化**: 异步订阅"发出即返回"在测试里看不出问题(测试会 sleep), 真实链路里直接造成漏成交; 凡是"先建立通道再交易"的路径, 都应等通道**就绪确认**再放行。
3. **真实链路能暴露单测看不见的竞态**: T015 首跑暴露的正是"订阅 vs 下单"的时序窗口; 纯逻辑用例(270 个)一个都没覆盖到它。
4. **诚实边界**: 残留 0.000084 ETH 是交易所手续费与 minNotional 规则造成的尘埃, 不是未平仓; 文档与输出都必须说清, 否则会误导用户以为"清理没做干净"。

## 六、文档同步清单(T018)

| 文档 | 改动 |
|:--|:--|
| `specs/architecture.md` | §三 未实施项(实盘运行器改为已落地 + 合约实盘列为未实施)、§七 安全模型(双条件门禁/时钟预检/凭据/参数对齐/归属撤单)、§九 测试基线 270、§十一 缺口 B/C/D/E/F 关闭归档 |
| `specs/roadmap.md` | 测试基线 233 → 270(2026-09-13) + 变更档案表新增 011 行 |
| `specs/testnet.md` | 新增「2026-09-13 011 联调实测记录」: legacy listenKey 下线事实与 WS-API 替代路径、订阅就绪踩坑、T014/T015 闭环输出、参数对齐实况、尘埃说明 |
| `specs/lua-api.md` | on_stop 停机清理语义补实盘顺序(引擎兜底与脚本并存)、撤单归属、平仓开关、吸干窗口; FR-014 落地通道 |
| `specs/changes/011-live-runner/` | spec(起点事实 ⑨ + 拍板 2/3 实况 + A8 落地)、plan(D4 修正 / R1 结案)、tasks(T001-T018 勾选 + 实施记录)、本 converge.md |
