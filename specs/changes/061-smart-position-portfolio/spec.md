# 061 smart_position 多标的组合化 — 规格

> 依据: 用户 2026-10-21 需求(计划已批准, 蓝本 `.trae/documents/smart_position_portfolio_plan.md`)。
> 前序: 060-smart-position-earn(单币版 + Simple Earn 集成)。

## 一、目标(用户三项原话)

1. 每个币的占比可以调整，默认占比相同。
2. bStock 没有存储赚利息的功能。对应 bStock 或者其他没有存储赚息功能的，不进行存储赚息，也不要报错。
3. 回测要有 bStock 和币，各两个，共 4 个。

## 二、功能需求

- FR-01 策略组合化(唯一模式): `universe` 逗号串列多 pair(缺省回落单 `pair`, 与 060 等价); `weights` 逗号串与 universe 一一对应, 缺省等权; 校验(个数一致/每项∈[0,1]/合计≈1e-6 容差)违反即 fatal, 不自动归一。
- FR-02 `target_pct` 语义升级 = 全部币腿合计占权益比; 币 i 目标占比 = target_pct × w_i; 阈值闸逐币独立(只调越界的币); 时间闸/min_notional/防循环三重闸不变。
- FR-03 资金池共享: 卖出所得进 USDT 现金, 本周期先卖后买、买腿预算按卖出回笼滚动; 某币买入现金不足 → 赎回差额申请、仅跳过该币, 下周期再试。
- FR-04 理财腿只存 USDT(报价币); 回测引擎新增配置键 `earn_supported_assets`(逗号串, 默认 USDT): 不在列表的资产 earn_balance→None、申赎→false → 策略静默关闭理财腿, 不报错、不刷日志(bStock 无赚息在回测中可真实演练)。
- FR-05 CLI `ricow backtest` 组合接线: 生效 universe ≥2 pair → 逐 pair 取数(缓存/并发分页/缺口硬错/预热硬错同单标口径) → `build_interval_ticks` 统一时间轴 → `run_portfolio_backtest` → 组合概览 + 分标的明细 + 策略统计文本报告 + `--export-dir` 四件套(fills 带 pair 列)。单标的路径与 Web 路径零改动。
- FR-06 `run_portfolio_backtest` 收尾对齐 `run_backtest`: 调 `on_stop` + 透传 `strategy_stats`(smart_position 的 stat_* 全在 on_stop 导出)。
- FR-07 敏感性扫描组合路径不支持(universe≥2 时明确报错)。

## 三、边界与非目标

- 不做每币独立理财池(币腿即持仓, 闲置只以 USDT 形态存在)。
- 不做权重自动归一/动态再平衡权重。
- 不动合约路径与既有单币语义; 引擎零策略参数名(架构守卫通过)。

## 四、验收

- 单元测试: 等权建仓、自定义权重落位、weights 校验 fatal、逐币阈值独立触发、earn 不支持静默跳过、赎回延后补买、`earn_supported_assets` 白名单解析、runner 级组合端到端(真实 Lua)。
- `cargo test` 全绿 + 架构守卫 + clippy + fmt。
- 4 资产(BTC/ETH + TSLAB/NVDAB)132 天真实 K 线组合回测(2026-06-11→2026-10-21, bStock 现货历史硬约束), 报告按 specs/backtest.md §〇 v1 全量交付。
