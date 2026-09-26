# 032 功能规格: 合约双向配对网格策略(032-futures-paired-grid)

**状态**: 草稿 (2026-09-25) | **分支**: `feat/futures-paired-grid` | **依据**: 用户原始需求 6 点 + 补充需求(单侧可独立运行) + 已审核修订计划 `.trae/documents/feat-futures-paired-grid.md` + `specs/constitution.md`

## 一、为什么

现有 `paired_grid` 是现货配对网格。用户要把它搬到合约 USDT-M 并升级为双向持仓(hedge): 相当于两个子网格同时跑(开多平多 / 开空平空), 各自独立开始价与建仓数量, 且两侧可独立启停。

回测引擎已具备期货 hedge 能力(013 按侧独立钱包/强平), 实盘适配器已支持 positionSide/杠杆/真实爆仓价(012/014)。缺口在策略层 + 三处基础设施: 成交事件不带方向(OrderFill 无 position_side)、DryRun 只有现货净仓模型、内置策略直跑路径不回填 market/杠杆。

## 二、范围

**做**:

- **策略** `strategies/futures/paired_grid_futures.{lua,toml}`: 双向配对网格, 多空两侧独立子网格(各自 start_price / initial_qty / 间距 / lots 栈 / flag), 共享 `need_rehang`(触发时启用侧一起重挂, 规避 `cancel_pending` 按 pair 全撤)。
- **单侧独立运行**: `start_price_long > 0` 启用多头侧、`start_price_short > 0` 启用空头侧; 支持只做多 / 只做空 / 双向同跑; 两侧都 ≤0 启动报错。
- **OrderFill.position_side 数据链**: 全链补 `position_side: Option<String>`, 让策略 `on_fill` 能路由回对应子网格。
- **回测爆仓价填充**: 新增纯函数 `isolated_liq_price(entry, side, lev, mmr)`(OctoBot 公式), `open_position`/`close_position` 填充 `Position.liquidation_price`; `settle_closed_bar` 逐 bar 刷新 `mark_price`。
- **Lua 数据 API**: `ctx:pos_liq(pair, side)`(回测公式 / 实盘透传币安真值)、`ctx:pos_mark(pair, side)`; `fill_to_table` 加 `position_side`。
- **DryRunContext hedge 扩展**: quote 保证金模型 + 按侧持仓键 + 公式估算爆仓价(不模拟强平动作/资金费)。
- **内置直跑回填**: `StrategyManifest` 加 `position_mode` / `default_leverage`; `resolve_builtin_script` 从清单回填 market/position_mode/`[backtest].leverage`。
- **run.rs market 分支**: DryRun 路径 exchange 按 market 构造(合约用免凭据 `FuturesClient::new()`)。

**不做**:

- **不模拟 DryRun 强平动作与资金费**(只 WARN 穿越爆仓价; 资金费不结算)。
- **不做档位 MMR 阶梯**(维持单一 `tier1_mmr_pct` 首档表, 与回测 CLI 同源)。
- **不新增引擎策略参数名**(架构铁律: 引擎/CLI/绑定层零策略参数名; 参数名只出现在 Lua/manifest 层)。
- **不改现货 paired_grid 行为**(回归约束)。
- **不强制实例 TOML 回填**(见"已知限制")。
- **"建议 ≤5x"不做引擎硬约束**(020: 风控政策属策略, manifest 文案 + 启动日志提示)。

## 三、功能需求

- **FR-001**: 策略在合约 USDT-M hedge 模式下运行, 多头侧与空头侧各自独立: 独立开始价、建仓数量、网格间距、lots 栈、方向 flag。
- **FR-002**: 两侧可独立启停: `start_price_long>0` 启用多头侧, `start_price_short>0` 启用空头侧, 支持只做多/只做空/双向; 两侧都 ≤0 启动报错。
- **FR-003**: 建仓语义统一: 两侧各在 `start_price` 挂一张限价建仓单(多头 buy / 空头 sell), 挂单即激活; 回测首根 bar 穿越即按请求价成交(成交价==开始价), 实盘/DryRun 价格穿越才成交。
- **FR-004**: `OrderFill` 携带 `position_side`, 策略 `on_fill` 按方向路由到对应子网格(多头开仓 buy+long 与空头平仓 buy+short 可区分)。
- **FR-005**: 平仓单只带 `position_side`、不带 `reduce_only`(fapi 拒绝同带); 挂单/撤单按"全撤 + 启用侧一起重挂"原子化。
- **FR-006**: 回测端 `Position.liquidation_price` 按逐仓公式 `liq=entry×(1∓1/lev±mmr)` 填充, `mark_price` 逐 bar 刷新; `ctx:pos_liq` 可读。
- **FR-007**: 实盘端 `ctx:pos_liq` 透传币安 positionRisk 真实 `liquidationPrice`(012 已解析)。
- **FR-008**: DryRun 支持合约双向虚拟仓: quote 保证金模型、按侧持仓键、公式估算爆仓价; 穿越爆仓价只 WARN 不平仓; 资金费不模拟。
- **FR-009**: 默认 2x 杠杆(manifest `default_leverage=2`, 回测直跑经回填生效), 建议 ≤5x 仅文案。
- **FR-010**: 距爆仓价 < `liq_warn_pct` 打 WARN(只告警不动作)。
- **FR-011**: 交易流程 testnet 真实调用(币安 demo-fapi 双向下单验证), 纯逻辑用单元测试。

## 四、已知限制(审核 a2)

用户经 `create_strategy` 部署**实例 TOML** 跑合约时, `position_mode="hedge"` 需实例顶层 `[strategy]` 自己写对(加载链读顶层字段, 不走 manifest 回填); 直跑路径(`ricow run <内置id>`)由 `resolve_builtin_script` 自动回填。

## 五、口径声明(审核 S1/M3/M5)

- **回测 pos_liq 为"开仓口径估算"**: 公式是显示/告警口径; 引擎实际强平用钱包线性穿越模型(`check_liquidation_side`), 手续费/资金费侵蚀后实际爆仓价更近。
- **unrealized 两侧累加只修正实盘权益**: 回测端 `BacktestContext.unrealized_pnl` 从不维护, 该修复对回测是空操作(权益曲线由 `mark_to_market` 独立计算)。
- **DryRun 与回测 MMR 同源**: 都按 `tier1_mmr_pct(pair)` 查首档表, 未知币回落 1.0%。
- **杠杆分层**: 实例 TOML 杠杆只写 `[strategy.params]` 会被 resolve 写回覆盖, 统一写 `[backtest] leverage`。
