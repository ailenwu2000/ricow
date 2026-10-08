# 032 任务分解: 合约双向配对网格策略

**依据**: [spec.md](./spec.md)(FR-001..FR-011) + [plan.md](./plan.md)(D1..D6) | **日期**: 2026-09-25

格式: `T编号 [阶段] 描述(含精确文件路径)` —— 每条尾部标注溯源的 D / FR。

**落地口径提醒**: 引擎/CLI/绑定层零策略参数名(架构铁律); 交易流程 testnet 真实调用(禁 mock 替身); 基线 538 passed 不倒退。

## 阶段 1 OrderFill.position_side 数据链(FR-004)

- [x] T001 `crates/ricow_core/src/types.rs`(L267-277): `OrderFill` 加 `pub position_side: Option<String>`(`#[serde(default)]`); 构造点全补: backtest execute_fill、强平 fill(L790)、DryRun execute_fill(context.rs L855)、实盘 futures_ws.rs `parse_order_trade_update`(解析 `o.ps` → "LONG"/"SHORT"/"BOTH"→long/short/None)、现货 ws.rs 填 None; `lua.rs fill_to_table`(L670-682) 加 `position_side`(None→nil); 编译器驱动补全存量测试字面量 → `cargo test --workspace` —— FR-004 / D-链

## 阶段 2 回测爆仓价与 mark(FR-006)

- [x] T002 `crates/ricow_strategy/src/backtest.rs`: 新增纯函数 `isolated_liq_price(entry, side, lev, mmr) -> Decimal`(long=`entry×(1−1/L+mmr)`, short=`entry×(1+1/L−mmr)`, OctoBot 公式); `open_position`(L1560) futures 时填 `liquidation_price`; `close_position`(L1543) 减仓重算、size→0 置 None; `settle_closed_bar`(L685) futures 对 size>0 仓刷 `mark_price=bar.close`; 注释声明显示口径 vs 触发口径差异 —— FR-006 / D4 / §五
- [x] T003 [P] backtest.rs 单测: `isolated_liq_price` 已知向量(lev=2/mmr=0.5%: long=0.505×entry, short=1.495×entry); 开/加仓重算; 减仓重算; 归零 None; mark 逐 bar 刷新 → `cargo test -p ricow_strategy` —— FR-006 / FR-011

## 阶段 3 Lua 绑定 pos_liq/pos_mark(FR-006/007)

- [x] T004 `crates/ricow_strategy/src/lua.rs`: `LuaCtxData` 加 `directional_liq: HashMap<String,Option<f64>>` / `directional_mark`; `fill_snapshot`(L464-473) 从 `position_directional` 填 liq/mark; `add_methods` 紧跟 `pos_entry`(L179) 注册 `pos_liq(pair,side)->Option<f64>`(无仓/无值 nil)、`pos_mark(pair,side)->f64`; unrealized 汇总(L462)改从 directionals 两侧累加(实盘修正 hedge 权益; 回测端为空操作, 审核 M3) —— FR-006 / FR-007 / D4
- [x] T005 [P] lua.rs 单测: 回测脚本经 `ctx:pos_liq` 读到公式值; 实盘快照透传 liq; 无仓返回 nil; hedge 两侧独立 → `cargo test -p ricow_strategy` —— FR-011

## 阶段 4 DryRun hedge + 直跑回填(FR-008/009)

- [x] T006 `crates/ricow_strategy/src/context.rs`(L666-1213): DryRunContext futures 分支 —— `virtual_positions` 键 `pair|long`/`pair|short`(现货/one-way 保持裸键); override `position_directional`; `position()` hedge 两侧 net 聚合; `apply_position_change` 按 `req.position_side` 定侧路由(同向加/反向减该侧, 不净仓翻转, 归零删键, reduce_only 按侧封顶); 新增 `wallets: HashMap<String,Decimal>` 保证金冻结/回笼(开仓 `free−=notional/lev`); `update_orderbook` 刷 mark/unrealized; liq 按 `isolated_liq_price` 同式填(mmr=`tier1_mmr_pct(pair)` 查表, 审核 M5/D6); 穿越 liq 只 WARN; `fee_model` futures 按 params(maker2/taker5bps); 资金费不模拟 —— FR-008 / D3
- [x] T007 `crates/ricow/src/commands/run.rs`(L66): exchange 构造下移到 config 加载后, futures 时 `BnFuturesExchange::new(FuturesClient::new()?)`(免凭据公共行情); 现货路径不变 —— FR-008
- [x] T008 `crates/ricow/src/strategies/catalog.rs`: `StrategyManifest` 加 `position_mode: Option<String>` / `default_leverage: Option<f64>`(serde default, 可选字段零破坏); `crates/ricow/src/commands/mod.rs` `resolve_builtin_script`(L473): 命中内置时回填 `config.market`/`position_mode`, 且 `config.backtest` 为空时注入 `leverage=default_leverage`(**时序**: 在 `apply_backtest_cli` L206 杠杆校验前, 审核 S1/M4) —— FR-009 / D5
- [x] T009 [P] context.rs 单测: DryRun 合约开双仓(LONG+SHORT 并存)/各侧独立平/reduce_only 按侧封顶/保证金占用与回笼/liq 填充/穿越只 WARN 不平仓; catalog 单测: 新可选字段解析 + 缺省兼容 —— FR-008 / FR-011
- [x] T010 `cargo test --workspace` 全绿(538+ 不倒退) —— SC

## 阶段 5 策略文件与注册(FR-001..005/010)

- [x] T011 `strategies/futures/paired_grid_futures.lua`: 双子网格状态 `S.long`/`S.short`(各 `enabled/built/ref_price/flag/lots/pending_entry/start_price/initial_qty`); `on_init` 启用门控(`start_price_*>0`, 都关 error); `on_tick` 新 bar 节流: 成本门槛校验/保证金预算日志/爆仓价 WARN(`ctx:pos_liq`, `liq_warn_pct`)/启用侧挂 grid 单带 `position_side`(平仓单不带 reduce_only)/追踪/`need_rehang` 全撤+启用侧重挂+未成交 entry 单原子返回; `on_fill` 按 `fill.position_side` 路由; `save_state` 按 `long_`/`short_` 前缀(关闭侧不序列化); 参数经 `num`/`cfg_str` 读取 —— FR-001..005 / FR-010 / D1 / D2
- [x] T012 `strategies/futures/paired_grid_futures.toml`: 清单(market="futures", position_mode="hedge", default_leverage=2, 参数表含 start_price_long/short 默认 0 + 启用说明, desc 注明"默认 2x、建议 ≤5x"); `catalog.rs` BUILTIN 加第三条目 `include_str!`; `.gitignore` 加 `!strategies/futures/paired_grid_futures.{toml,lua}` —— FR-009 / FR-002
- [x] T013 [P] `crates/ricow_strategy/src/builtin_tests.rs`: 合成 K 线先跌后涨 —— 双向建仓配对/lots 与 flag 两侧独立; **单侧用例**: 只传 start_price_long → 空头侧零挂单; 只传 start_price_short 对称; 都不传 → 启动报错 —— FR-002 / FR-011
- [x] T014 三门禁: `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` —— SC

## 阶段 6 三模式真实冒烟(FR-011)

- [x] T015 回测冒烟(真实 K 线): `ricow backtest --strategy paired_grid_futures --pair ETHUSDT --interval 1h --days 60 --cash 2000 --param start_price_long=<首根open> --param start_price_short=<首根open> --param initial_qty_long=0.1 --param initial_qty_short=0.1` → 两侧成交价==start_price(审核 S2: 取 open 保证首根必成交)、position_mode=hedge、hedge_sides 有值、默认杠杆==2、liq 告警路径 —— FR-003 / FR-006 / FR-009
- [x] T016 DryRun 冒烟(免凭据): `ricow run paired_grid_futures` → 双限价挂单、穿越成交、两侧独立配对、liq 显示 —— FR-008
- [x] T017 demo 真实双向下单(凭据见 specs/testnet.md): `ricow run paired_grid_futures --demo` → demo-fapi 同时出现 LONG/SHORT 两条 positionRisk、`pos_liq`==交易所 liquidationPrice、停机撤单干净 —— FR-007 / FR-011

## 阶段 7 文档与收敛

- [x] T018 文档同步: `specs/lua-api.md`(pos_liq/pos_mark、fill.position_side、修正 L112"实盘方向仓暂返回 0"过时句)、`specs/backtest.md §五`(爆仓价显示公式 vs 触发模型口径、双向保证金、DryRun MMR 同源)、`specs/roadmap.md`(新基线) —— spec §五
- [x] T019 收敛: `specs/changes/032-futures-paired-grid/converge.md`(范围核对 + 冒烟结果 + 基线) —— 宪法 SDD
