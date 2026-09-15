# 功能规格: bs_momentum 策略 Lua 化合规整改(006-bs-momentum-lua)

**功能目录**: `specs/changes/006-bs-momentum-lua`

**创建日期**: 2026-09-09

**状态**: 已实施(整改计划 `.hermes/plans/2026-09-09_121926-bs-momentum-lua-compliance.md`
plan-review 一轮通过; 2026-09-09 定稿执行完毕, 见 §验收)

> **⚠ 已下线 (2026-09-11)**: 本变更交付的策略 `strategies/builtin/bs_momentum.lua` 已删除。
> 依据: 真实 bStock 成交轨期望 ≈0 (spot 91 天 每 bar −0.0198%, 期末 −5.38%; futures 220 天
> 每 bar +0.0367%, 期末 −1.08%), 十年 R1 的 +15,088% 含幸存者偏误不作证据 → 用户判定
> "期望值为负, 不合格"。通用能力 (组合信号模式 / 组合回测入口 / `us_klines` 缓存) 保留,
> 暂无内置消费者。证据与判定细节: `specs/research/bs-momentum-attribution-2026-09.md` §七。


**输入**: 宪法原则二「策略层统一 Lua」——M2 期将 bs_momentum 写成 Rust(bs_momentum.rs +
bs_signals 预计算 + TargetBook 名单注入)是计划期绕路产物, 用户 2026-09-09 否掉并拍板
整改方案乙: ①策略必须 Lua; ②策略可获得更长美股 K 线(Nasdaq 源/us_klines 已有)仅供打分;
③交易与回测撮合仍用币安 bStock; ④架构保持简洁。

## 背景与范围

- **根因**: 长窗指标(IBD RS 需 ROC126/189/252 + EMA200, 253+ 根)在币安 bStock 侧无数据
  (现货日线最长 91 根, fapi EQUITY ~220 天) → 当初把信号预计算做成 Rust 层 + 名单注入,
  违背宪法原则二(内置策略必须 Lua 源码, 用户可参考/修改/测试)。
- **方案乙**: 信号轨 = 美股 Nasdaq 日线(组合信号模式注入 ctx, 引擎按执行日截断无前视),
  Lua 脚本内自算打分/确认(公式源码可见可改); 成交轨 = 币安 bStock(不变)。

**范围(本次整改)**:
1. 引擎最小支撑: `BacktestContext.signal_klines` 容器 + 全局 tick 时间截断;
   `run_portfolio_backtest` 签名携带 signal_klines(空 = 现行为, 单标的零改动);
2. Lua ctx 多标的化: universe 键触发组合信号模式, ctx:klines 放开 100 根 cap(单标的维持);
3. 撮合参数回写: resolve 后 fee/slippage/leverage/mmr/funding 写回 config.params,
   策略读到的费率与撮合层同源(净回笼预算闭合前提);
4. 新内置策略 `strategies/builtin/bs_momentum.lua`(打分/确认/调仓全在脚本);
5. CLI 组合回测入口 `locus backtest --strategy bs_momentum --market spot|futures`(免 --pair;
   spot → bStock 池 / futures → fapi EQUITY 池; Nasdaq 信号线窗口截取 ~N+260 根)。

**范围外(不在此档案/已撤销)**: Rust bs_signals 预计算层 / bs_momentum.rs / TargetBook 名单
注入设计 / loader bs_momentum 注册(M2 撤销项); R1 十年美股成交轨回测(成交轨仅币安,
样本窗口短, 研究回测 = R2-spot 91 天 / R2-futures 220 天并注明局限); 实盘组合执行(M3,
LiveContext 无美股信号线通道 — YAGNI, ctx:klines 截断语义留扩展不预建实盘代码)。

## 架构(极简形态)

```
Nasdaq(美股日线) ──拉取/缓存──> us_klines ──装配层按窗口截取 ~N+260 根/标的──>
BacktestContext.signal_klines(pair → 美股日线段)  (引擎按全局 tick 时间截断, 无前视)
币安 bStock 日线 ──> build_daily_ticks ──> 组合 runner ticks ──> 撮合/估值(成交轨)
策略 on_tick(Lua): ctx:klines(pair) = 美股信号线截断层 → 脚本内算 IBD RS/EMA200/确认
  → 池内选前 min(top_k,5) → truth-based diff(ctx:position 现查) → 先卖后买(bStock 市价)
```

- 打分指标(EMA/ROC/52w 高)为 Lua 函数写在脚本内(纯递推, 用户可改窗口/权重);
- 池(70 只 bStock symbol)静态注入 config 参数 `universe`(逗号串, 装配层生成);
- 截断无前视由引擎保证: 信号 bar.open_time < 组合全局 tick 时间(= 当 tick 各 pair bar
  open_time 最大值; 数据缺口日仍按全局时间, 不悬空)。
- Nasdaq bar 时间契约: 交易日 → 当日 UTC 00:00 锚(单测固化, nasdaq.rs 头注释)。

## 门禁(本档案验收标准)

- **原则二符合性(硬门禁)**: bs_momentum 策略本体 100% Lua;
  grep 无 Rust 策略载体残留(`BsMomentum`/`TargetBook`/`load_bs_momentum`/bs_signals
  mod 声明 = 0 命中, 注释性历史说明除外); 策略脚本可直接阅读/修改/复制自定义。
- **测试纪律**: 交易流程用合成数据 e2e 真撮合(禁 mock 替身); 真数据冒烟走币安公共 +
  Nasdaq 官方 API 免 key 真实调用。
- **无前视**: signal 截断单测(改未来价断言返回不变)+ R2 冒烟首笔成交 > 信号日收盘。

## 用户场景与测试 *(必填)*

### 用户故事 1 - Lua 源码可改可参考 (P1)

用户打开 `strategies/builtin/bs_momentum.lua` 即可看到打分公式(IBD RS 权重)、牛市确认
条件、池门槛 253、调仓预算公式(净回笼口径), 修改后复制到 `strategies/scripts/` 即自定义。

**独立测试**: 沙箱编译门禁单测(内置脚本 from_source 通过); e2e 换血零拒单断言预算与
撮合费用同源闭合。

### 用户故事 2 - 组合回测可跑真实数据 (P1)

`locus backtest --strategy bs_momentum --market spot --days 91` 拉真实 bStock 日线 +
Nasdaq 信号线(缓存), 输出组合报告(成交/手续费/换手率/拒单/期末持仓)。

**独立测试**: R2-spot 冒烟(rejected 0, 期末持仓 5 只, 曲线/名单一致); R2-futures 冒烟
(合约模式含资金费, funding_net 披露)。

**验收场景**:

1. **给定** bs_momentum 组合入口, **当** 以 spot 运行, **则** 91 天回测完成且
   rejected_count = 0, 期末持仓 ≤ 5 只, 有真实换手成交;
2. **给定** Lua 化整改完成, **当** grep Rust 残留符号, **则** 零命中(注释性说明除外);
3. **给定** 组合信号模式, **当** 修改未来信号价后断言已返回段, **则** 内容不变(无前视)。

## 涉及文件

- 改: `crates/locus_strategy/src/{backtest,lua,lib}.rs`(signal 容器/截断/universe 快照/
  费率回写; bs_momentum 导出删除)
- 改: `crates/locus_engine/src/{backtest_runner,loader,lib}.rs`(签名 + Lua 化; bs_signals
  引用删除)
- 改: `crates/locus_cli/src/commands/{mod,backtest}.rs`(BUILTIN 注册 + 组合入口)
- 删(执行中引用清理后文件删除): `crates/locus_strategy/src/bs_momentum.rs`、
  `crates/locus_engine/src/bs_signals.rs`
- 新: `strategies/builtin/bs_momentum.lua`、`crates/locus_engine/tests/bs_momentum_lua_e2e.rs`
- 改: `specs/{lua-api,architecture}.md`、005-market-filter spec(表述核对)、
  技能 hyper-local-development refs/bstock-momentum-rotation-strategy.md(执行状态重写)
