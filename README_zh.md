# ricow

完全本地运行的多平台量化策略引擎(回测 / Dry Run / 实盘), 纯本地 CLI 客户端工具。

## 当前内容

- `specs/` — 唯一文档体系: constitution(项目宪法)/ product(产品方案)/ architecture(架构)/ lua-api(Lua API 规范)/ roadmap(里程碑进度)/ research(调研资料)/ changes(变更档案, SDD 流程产物)
- `crates/` — 5 crate workspace: core / binance / strategy / engine / cli
- `strategies/builtin/` — 内置参考实现: shannon_grid.lua(策略样板)+ executors/{dca,twap,vwap,pullback,ladder}(执行模式示例, 非策略); exec 执行组件为引擎内置(Rust 实现, Lua 策略直接调用 exec.*)
- `examples/` — 用户策略模板与示例(strategy_template.toml / ema_cross.lua)
- `crates/ricow/src/supervisor/` — 策略进程管理器(常驻 daemon + 本机控制通道 + 实例台账, 见 [specs/architecture.md §三](specs/architecture.md))

## 竞品调研结论摘要

- 商业端 MCP 全是云端账号侧 — "本地私钥 + 本地回测门禁 + Dry Run 默认" 是差异化位置
- 2026 年无 star>200 新竞品, 格局稳定
- 详见 [specs/research/competitors.md](specs/research/competitors.md)

## 免责声明

本软件仅供学习与研究, 不构成投资建议。加密货币交易风险极高。

### 香农动态网格(shannon_grid)风险提示

- **选品约束(强制)**: 本策略只适合长期上涨的主流资产(如 BTC/ETH/BNB 等大盘);**拒绝 meme 币、新币、高退市风险资产**——标的长期下跌/归零时, 再平衡差价收益无法覆盖标的跌幅, 将持续亏损; 策略无止损功能, 风险由选品约束承担。
- **建仓 = 用户主观判断**: 策略启动时一次市价买入 `target_ratio`(默认 0.5 = 50%)权益, 之后按目标比例中轴再平衡(价格涨 → 卖出回平衡, 价格跌 → 买入回平衡), 不判断趋势——只有用户自己认为标的长线看涨时才应启动。
- **band 自动随波动率缩放(ATR, 默认开)**: band_eff = max(rebalance_band, atr_mult × ATR(atr_period)/price), 高波动交易对(日波动大的币)无需手动放宽 band, 自动防手续费吃光; `atr_mult=0` 可禁用回退固定 band。
- **target_ratio 可调(默认 0.5)**: 看涨调高(0.6~0.7 涨市多吃, 但跌市回撤更大)、防守调低(0.3~0.4); 比例越高越接近满仓持有。参数语义与调参方向详见 [specs/product.md §十二](specs/product.md)。
- **band 下限是保本防线**: rebalance_band(默认 0.5%)的价格当量须 > 2×双边手续费(现货 10bps×2 = 0.2%);band 过小时每次再平衡的价差收益会被手续费吃光。
- 单边行情存在回撤风险; 如需对冲, 用户可在交易所自行购买期权/永续空单(ricow 不内置对冲功能)。
- **建议使用交易所单独子账号运行**: 策略会使用账号内大部分现金(目标比例再平衡), 子账号隔离风险、独立核算。

---

[English](README.md) · [项目宪法](specs/constitution.md)
