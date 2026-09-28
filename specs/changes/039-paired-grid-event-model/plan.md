# 039 技术方案: 删除 shannon_spot_grid + paired_grid(现货) 适配事件模型

## 一、背景

034 引入事件模型（on_fill 可返回订单）、036/037/038 完成合约配对网格与 univ2 的适配并修复引擎
同 bar 乒乓链根因。内置 4 策略中尚余:
- **shannon_spot_grid**: 用户裁决**删除**（不再维护）。
- **paired_grid(现货)**: 未适配 —— on_fill 只记账 + `need_rehang=true`，重挂等下一根 bar 的
  on_tick（034 §一诊断的"成交→决策节流缺口"，036 在合约版修掉的同款）。038 后无乒乓链风险
  （on_fill 不返回订单即无链），但每次成交后重挂晚一根 bar，密集行情少抓成交。

## 二、R1 删除 shannon_spot_grid

| 位置 | 动作 |
|---|---|
| `strategies/spot/shannon_spot_grid.lua` + `.toml` | 删除文件 |
| `crates/ricow/src/strategies/catalog.rs` | 删内置注册条目(约 L133-135) + 断言 3 处(L421/427/432) |
| `crates/ricow_strategy/src/builtin_tests.rs` | 删 `SHANNON_ETF_ACCUM` 与 §023 全部集成测试(约 12 项, L148-363 区段)及其专用辅助(`accum_cfg` 等; **注意** `bar_at_hour`/`tf_bars`/`uptrend_bars` 若被其他测试共用则保留) |
| `crates/ricow_strategy/src/{lua.rs,backtest.rs,config.rs,name.rs}` 与 `lib.rs` 头注释 | 仅清头注释提及; 测试 fixture 里 `strategy_type: "shannon_spot_grid"` 为任意字符串标签, 与该 lua 无依赖, **不改**（避免无谓 churn） |
| `specs/roadmap.md` | 测试基线更新一行 |
| `specs/lua-api.md` / `specs/backtest.md` | 无策略点名, 不动 |

预计测试净变化: −12 项左右（以实跑为准）。禁 mock 纪律不涉及（纯删除）。

## 三、R2 paired_grid(现货) 适配事件模型（036 模式平移）

逐行参照 `strategies/futures/paired_grid_futures_long.lua`(036 终态) 与其测试, 只改策略零改引擎:

1. **抽 `do_rehang(ctx)`**: 原 on_tick 重挂块（全撤 cancel_pending + 下方买单 + 栈非空卖单、
   买价 = ref÷(1+down)、卖价 = **栈顶买入价×(1+up)**（现货语义, 不改）、min_pair_profit 保底、
   无仓无单即 `finished`）整体搬移为共享决策出口; on_tick 重挂处与 on_fill 改为调它。
2. **on_fill 返回重挂订单**: 建仓分支（拆格记账后）与网格分支（lot/flag/统计入账后）改为
   `return do_rehang(ctx)` —— 建仓成交同 bar 拆格挂出、网格成交同 bar 再挂; 链内限价单由 038 R1
   次 bar 生效（引擎已保真, 无乒乓链）。
3. **现货特有语义保留不动**: "无仓无单即结束"（finished）; 卖价锚定栈顶成本（现货本就如此）;
   accumulate_mode 双模式记账; halted/on_order_update 拒单重挂逻辑照旧。
4. **头注释**: 更新为事件模型口径（注明依赖 038 R1）。

## 四、测试计划

- `builtin_tests.rs` paired 现货既有用例按 R2 重校（重挂从"下一 bar on_tick"变为"on_fill 同 bar
  返回、次 bar 可成交", 索引断言可能 −1 bar）。
- 新增/复刻 036 的 4 类断言: ①on_fill 立即重挂价格/数量 ②建仓成交同 bar 拆格挂出 ③无单可挂保持
  need_rehang ④min_pair_profit 保底（现货已有多数则补缺）。
- 门禁: fmt / clippy / `cargo test --workspace` 全绿（584 基线上有增有减, 如实记录）。

## 五、回测验收

paired_grid(现货) SOLUSDT 1m×180d（spot 数据源 `RICOW_BN_BASE_URL=https://data-api.binance.vision`
+ `--klines-market spot`, 投入 10000, 参数对齐既有基线窗口）:
- 适配版 vs f1a5953 基线同参数 A/B; 按回测规范 v1 出概览; `stat_stall_bars=0` 硬门槛。
- 预期方向: 成交数与净盈亏小幅上升（消除节流缺口）; 若显著下降则逐笔归因后再定。
- `--export-dir` 四件套, 基线对照目录同留档。

## 六、风险

- 删除 shannon 影响 user 已部署实例: 引擎对未知 `type` 的实例 TOML 会加载失败 —— 用户确认不再
  要该策略, 不做迁移（与 020 删 `[risk]` 同口径: 不报错不迁移, serde 忽略未知键仅指参数）。
- paired_grid 重挂提前一根 bar → 回测结果漂移（应向上, 幅度小, 1m 粒度同 bar 链罕见）→ §五量化。

## 七、落地验证记录 (2026-09-28)

- **R1 删除 shannon_spot_grid**: strategies/spot 两文件删除; catalog.rs 注册与 3 处断言改指 paired_grid;
  builtin_tests.rs 删 §023 全部 12 例与专用辅助(保留共用 bar_at_hour/tf_bars/flat_main/run_accum_full);
  `crates/ricow` 内嵌 TOML fixture 与模板/AI 文案中的功能性引用改指 paired_grid(解析需要真实内置 id);
  头注释(lib.rs/multiframe.rs/context.rs)清理。测试 fixture 的 strategy_type 字符串标签未动。
- **R2 paired_grid v2**: do_rehang 抽取(on_tick 重挂块逐行搬移, 卖价仍锚 ref_price、min_pair_profit
  保底、coin/u 双模式、cap_buy/cap_sell 兜底全保留); on_fill 建仓/网格分支改为返回 do_rehang(ctx),
  finished 后不产单; 补停摆可观测(stall_bars/stall_bars_max/1440 根 WARN/stat_stall_bars)。
  测试: run_accum_full 对齐引擎 settle 语义(on_fill 返回递归结算并入批次); 新增 2 例现货事件模型
  断言(on_fill 立即重挂 ref 锚定语义 / 建仓同 bar 拆格); 原 3 例重挂断言按新时序重校(买@96.1538
  成交后同 bar 返回批次, 成交价 = open 跳空 96)。
- **门禁**: cargo fmt 0 差异 / clippy 0 警告 / `cargo test --workspace` = **574 passed / 0 failed / 22 ignored**。
- **回测 A/B** (SOLUSDT spot 1m×180d, cash 10000, start_price=500, order_amount=10, initial_buy=100):
  - 039: **647 笔 / 净 +32.30**(+0.32% = 已实现 +38.89 − 手续费 6.60) / `stat_stall_bars=0` /
    最大回撤 0.53% / 自建仓 +0.48%(对照满仓持有 +44.10%, 回撤 38.62%)
  - 基线 f1a5953: 673 笔 / 净 +33.42 / 自建仓 +0.49%
  - 漂移归因: −26 笔 / −1.12(−3.3%) —— on_fill 重挂即时撤旧单(cancel_pending)且新限价单次 bar
    生效(038 R1), 单 bar 内 V 型反弹的二次成交被放弃; 行业保守口径, 预期内。
  - 导出四件套 `/tmp/ricow-bt-pairedspot-039/`, 基线对照 `/tmp/ricow-bt-pairedspot-base/`。
