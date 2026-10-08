# 032 技术方案: 合约双向配对网格策略

**依据**: spec.md | **完整设计**: `.trae/documents/feat-futures-paired-grid.md`(已按审核修订)

## 决策

1. **单侧独立运行**(用户补充): `start_price_* > 0` 即该侧启用, ≤0 关闭; 两侧都关启动报错; 零新增参数。
2. **激活/建仓语义**: 两侧各挂一张限价建仓单@start_price, 挂单即激活; 回测穿越按请求价成交==开始价; 实盘/DryRun 穿越成交。
3. **DryRun 一并扩展 hedge**: quote 保证金模型 + 按侧持仓键 + 公式爆仓价(不模拟强平/资金费)。
4. **pos_liq 双口径**: 回测公式 `liq=entry×(1∓1/lev±mmr)`; 实盘透传币安真值。
5. **默认杠杆回填**(审核 S1): manifest `default_leverage=2` → `resolve_builtin_script` 注入 `config.backtest.leverage`, 时序在 `apply_backtest_cli` 校验前。
6. **DryRun MMR 同源**(审核 M5): `tier1_mmr_pct(pair)` 查表, 未知回落 1.0%。

## 技术要点

- **OrderFill.position_side 全链**: types.rs 加字段(`#[serde(default)]`) → backtest execute_fill/强平 fill/DryRun execute_fill/实盘 futures_ws 解析 `ps`/现货 ws 填 None; lua fill_to_table 透传。
- **cancel_pending 按 pair 全撤**: 策略侧原子化"全撤 + 启用侧重挂"规避, 不改引擎。
- **回测 liq/mark**: `isolated_liq_price` 纯函数 + open/close_position 填充 + settle_closed_bar 刷 mark。
- **Lua 绑定**: directionals 快照扩 liq/mark; `pos_liq`/`pos_mark` 方法; unrealized 两侧累加仅实盘有意义(审核 M3)。
- **DryRunContext**: 按侧键 `pair|long|short` + `wallets` map + 保证金冻结/回笼 + liq 填充 + 穿越 WARN。
- **run.rs**: exchange 构造下移到 config 加载后, 按 market 分支。

## 风险

- 显示爆仓价 vs 引擎触发口径差异(spec §五已声明)。
- hedge 回测低估强平风险(bar 内路径不建模, backtest.md §七.6 既有声明)。
- DryRun 不模拟资金费 → 空头侧收益偏乐观(日志标注)。
- 双向保证金开仓瞬间冻结 ≈ Σ(启用侧 N)/lev×price(启动日志提示)。
- 挂单数翻倍(两侧最多 6 张/pair), 护栏无压力。

## 验证

三门禁(fmt/clippy/test 538+ 不倒退) + 回测/DryRun/demo 三模式真实冒烟(demo-fapi 双向 positionRisk、pos_liq==交易所真值)。
