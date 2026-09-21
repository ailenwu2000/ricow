//! 回测执行: 策略 + 历史 K 线 → 回测报告。
//!
//! - `run_backtest`: 单标的路径 (v0.2 起, 回归硬门槛 — 行为零改动)。
//! - `run_portfolio_backtest`: 组合路径 (M2, bs_momentum 轮动) — 同一撮合账本,
//!   多标的按统一时间轴逐 tick 驱动; 市价单由 BacktestContext 在 place_order 时
//!   按本 tick 各 pair bar open 即时撮合 (无前视: bar 于 tick 内已开盘)。

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use ricow_core::{Balance, Kline};
use ricow_strategy::{
    BacktestContext, BacktestReport, CancelIntent, Context, Strategy, StrategyConfig,
};

use crate::data::SeriesDriver;

/// 在历史 K 线上运行一次回测 (单标的)。
///
/// 流程: on_init → 逐 bar step_bar + on_tick + place_order + drain_fills + on_fill → report。
pub fn run_backtest(
    config: StrategyConfig,
    initial_balance: Balance,
    klines: &[Kline],
    strategy: &mut dyn Strategy,
) -> BacktestReport {
    // 028 审核修复: 回测**不产生盘口事件**(没有 ws 流), 只写 `on_quote` 的策略在回测里
    // 一次都不会触发 —— 与刚修的 data:timer 是同类"静默 0 成交"陷阱。
    // 告警实现统一在一处(声明路径与旧路径共用), 免得两条件文案漂移。
    crate::data::warn_if_quote_only(strategy);
    let mut ctx = BacktestContext::new(config, initial_balance);
    strategy.on_init(&mut ctx);

    for k in klines {
        ctx.step_bar(k.clone());
        let orders = strategy.on_tick(&mut ctx);
        for req in orders {
            let _ = ctx.place_order(req);
        }
        let fills = ctx.drain_fills();
        for fill in fills {
            strategy.on_fill(&mut ctx, fill);
        }
    }

    // 尾 bar 补结算 (013 FR-005): 最后一根 bar 的资金费/强平不由 push 路径触发
    ctx.finalize();
    ctx.report()
}

/// 把多标的日线对齐成统一日历 tick 序列 (组合回测驱动数据, M2-2)。
///
/// - 输入: pair → 日线 (每 pair 按 open_time 升序)。
/// - 输出: 按 UTC 日升序每交易日一个 tick, tick = 当日有 bar 的各 pair (pair, bar);
///   停牌/未上市/数据缺失的 pair 自然缺席 (其持仓估值沿用最近收盘, 见 step_portfolio)。
/// - 防御: 同 pair 同 UTC 日多根 bar 时取当日最后一根 (当日收盘口径; 输入应为 1d,
///   该分支仅为防错); 每 tick 内按 pair 名排序, 输出确定可测。
/// - 键语义: 成交轨 pair (币安完整 symbol, 如 TSLABUSDT); 前视边界 (信号日 d 名单
///   在 d+1 tick 执行) 由装配层负责, 本函数只做纯日历对齐。
pub fn build_daily_ticks(klines: &HashMap<String, Vec<Kline>>) -> Vec<Vec<(String, Kline)>> {
    let mut by_day: BTreeMap<NaiveDate, HashMap<String, Kline>> = BTreeMap::new();
    for (pair, bars) in klines {
        // 防御: 不依赖调用方升序 (乱序输入会破坏"同日取最后一根")。
        let mut sorted: Vec<&Kline> = bars.iter().collect();
        sorted.sort_by_key(|k| k.open_time);
        for bar in sorted {
            // 同 pair 同日多根 → 后者覆盖 (升序后取当日最后一根)。
            by_day.entry(bar.open_time.date_naive()).or_default().insert(pair.clone(), bar.clone());
        }
    }
    by_day
        .into_values()
        .map(|m| {
            let mut v: Vec<(String, Kline)> = m.into_iter().collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        })
        .collect()
}

/// 把多标的 K 线按任意 interval 分桶成统一 tick 序列 (盘中口径, 2026-09-11 新增)。
///
/// - 与 `build_daily_ticks` 同构, 唯一差别: 分桶键 = `open_time / interval_ms` 的整数除法
///   (⇒ 桶起点 = interval 边界对齐, 如 15m/30m/1h)。
/// - 用途: 盘中策略 (如"美股开盘 1 小时后按当日短期数据决策") 需要逐 15m/1h tick 推进,
///   而日线 tick 无法表达"当日盘中时刻"。
/// - 语义与防御同 `build_daily_ticks`: 同 pair 同桶多根取后者; 桶内按 pair 名排序; 缺档自然缺席。
pub fn build_interval_ticks(
    klines: &HashMap<String, Vec<Kline>>,
    interval_ms: i64,
) -> Vec<Vec<(String, Kline)>> {
    assert!(interval_ms > 0, "interval_ms 必须为正");
    let mut by_bucket: BTreeMap<i64, HashMap<String, Kline>> = BTreeMap::new();
    for (pair, bars) in klines {
        let mut sorted: Vec<&Kline> = bars.iter().collect();
        sorted.sort_by_key(|k| k.open_time);
        for bar in sorted {
            let key = bar.open_time.timestamp_millis().div_euclid(interval_ms);
            by_bucket.entry(key).or_default().insert(pair.clone(), bar.clone());
        }
    }
    by_bucket
        .into_values()
        .map(|m| {
            let mut v: Vec<(String, Kline)> = m.into_iter().collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        })
        .collect()
}

/// 把策略声明驱动的序列接进回测主循环 (028 T024, 新路径)。
///
/// **时序约定(无前视的关键, 逐条对齐既有回测模型)**:
/// 1. `ctx.step_bar(k)` 先推进账本 —— 此时 bar `k` 是"正在形成的 bar", 成交按它的 `open` 撮合
///    (与既有路径一致: 市价单在 `place_order` 时按本 tick 的 `bar.open` 成);
/// 2. 然后才把**已收盘**的 bar 推给策略: 可见时刻用 `k.open_time`, 于是"刚刚收盘"的是 `k-1`
///    (它的 `close_time = k.open_time - 1`) —— 策略拿到的是**上一根**的完整信息, 却只能在
///    `k` 的开盘价上成交。反过来(先看 `k` 的收盘、再在 `k` 的开盘成交)就是作弊, 本实现不做。
/// 3. 最后一根 bar 的 `on_bar` **不派发**: 它之后没有可成交的刻度, 派发只会产生永不成交的挂单。
///
/// 与 [`run_backtest`] 的其余部分完全相同: `on_tick` / 订单返回值 / `on_fill` 都照旧,
/// 主动订单出口(`ctx:place_order` / `ctx:cancel_order`)与返回值**同批落地**, 无第二条通道。
pub fn run_backtest_with_series(
    config: StrategyConfig,
    initial_balance: Balance,
    klines: &[Kline],
    driver: &mut SeriesDriver,
    strategy: &mut dyn Strategy,
) -> BacktestReport {
    let mut ctx = BacktestContext::new(config, initial_balance);
    strategy.on_init(&mut ctx);
    drain_intents(&mut ctx, strategy);

    // 028: 声明定时器走**同一份调度器**(虚拟钟刻度) —— 之前只接了 bar, `data:timer` 声明的
    // 策略在回测里 on_timer 一次都不响(审核实测: 同一声明在实盘会响 10 次), 回测静默给出
    // "策略没在跑"的假结论。这里与 Dry Run/实盘同一份 `TimerScheduler`, 只是时钟换成刻度。
    let mut timers = timers_for(&mut *strategy, klines.first());
    // 主时钟标的: 它的撮合参考 bar 由本 tick 的 k 直接给定(不依赖 forming_bars 的推理)。
    let primary_symbol = driver.infos().first().map(|i| i.symbol.clone());

    for k in klines {
        let tick_ms = k.open_time.timestamp_millis();
        // 1) **先**设本 tick 的撮合参考 bar, 再推进账本 —— `step_bar` 内部会立刻 `match_pending`
        //    (挂单撮合), 若参考 bar 晚一刻设置, 声明路径的限价单会比旧路径晚一根成交(审核实测)。
        let mut forming = driver.forming_bars(tick_ms);
        if let Some(sym) = &primary_symbol {
            // 主时钟兜底: 只有该标的**没有**从自己序列拿到参考 bar 时才用本 tick 的 k。
            // (不能无条件覆盖 —— 同标的声明了更细周期时, 覆盖会把"取最细周期"的规则打掉,
            //  于是成交价随主时钟粒度变化; 这是第三轮审核用例抓到的。)
            if !forming.iter().any(|(s, _)| s == sym) {
                forming.push((sym.clone(), k.clone()));
            }
        }
        ctx.set_declared_bars(&forming);
        // 2) 账本推进到 bar k(本 tick 的成交价 = bar k 的 open)
        ctx.step_bar(k.clone());
        // 3) 推"已收盘"的 bar: 可见时刻 = bar k 的开盘时刻 ⇒ 新收盘的是 k-1
        for (info, bar) in driver.advance(tick_ms) {
            let orders = strategy.on_bar(&mut ctx, &info, &bar);
            place_all(&mut ctx, orders);
            drain_intents(&mut ctx, strategy);
            let fills = ctx.drain_fills();
            for fill in fills {
                strategy.on_fill(&mut ctx, fill);
            }
        }
        // 4) 声明定时器: 与 bar 同一条下单管线(订单出口只有一个)
        for label in timers.due(tick_ms) {
            let orders = strategy.on_timer(&mut ctx, &label);
            place_all(&mut ctx, orders);
            drain_intents(&mut ctx, strategy);
            let fills = ctx.drain_fills();
            for fill in fills {
                strategy.on_fill(&mut ctx, fill);
            }
        }
        // 3) 旧回调路径: 与本 tick 的撮合同批
        let orders = strategy.on_tick(&mut ctx);
        place_all(&mut ctx, orders);
        drain_intents(&mut ctx, strategy);
        let fills = ctx.drain_fills();
        for fill in fills {
            strategy.on_fill(&mut ctx, fill);
        }
    }

    ctx.finalize();
    ctx.report()
}

/// 声明定时器 → 调度器(回测语义: **虚拟钟**, 起点 = 首根刻度; 与实盘的墙钟装配同一份
/// [`TimerScheduler`], 只是时钟来源不同 —— 见 `data/driven.rs` 的对应构造)。
///
/// 审核发现的问题就在这: 028 首版只把 bar 接进回测, `data:timer` 声明的策略在回测里
/// `on_timer` 一次都不响(同一声明在实盘会响 10 次), 回测会静默给出"策略没在跑"的假结论。
fn timers_for(
    strategy: &mut dyn Strategy,
    first: Option<&Kline>,
) -> ricow_strategy::TimerScheduler {
    let decls = strategy.timer_declarations();
    let start_ms = first.map(|k| k.open_time.timestamp_millis()).unwrap_or(0);
    ricow_strategy::TimerScheduler::new(decls, start_ms)
}

/// 下单一律忽略单笔失败(与既有回测路径同语义: 拒单不中断回测)。
fn place_all(ctx: &mut BacktestContext, orders: Vec<ricow_core::OrderRequest>) {
    for req in orders {
        let _ = ctx.place_order(req);
    }
}

/// 落地主动订单意图: `ctx:place_order` 入队的订单 + `ctx:cancel_order` 的撤单意图 (FR-002 ②③)。
fn drain_intents(ctx: &mut BacktestContext, strategy: &mut dyn Strategy) {
    let intents = strategy.take_intents();
    if intents.is_empty() {
        return;
    }
    for req in intents.orders {
        let _ = ctx.place_order(req);
    }
    for intent in intents.cancels {
        let res = match intent {
            CancelIntent::Owned => ctx.cancel_owned_orders(None),
            CancelIntent::OwnedIn { pair } => ctx.cancel_owned_orders(Some(&pair)),
            CancelIntent::ByOrderId { pair, order_id } => {
                ctx.cancel_order(&pair, &order_id).map(|_| 1)
            }
            CancelIntent::ByClientId { pair, client_order_id } => {
                ctx.cancel_order(&pair, &client_order_id).map(|_| 1)
            }
        };
        if let Err(e) = res {
            tracing::warn!(target: "backtest", "撤单意图未生效: {e}");
        }
    }
}

/// 组合回测 (M2): 多标的统一时间轴驱动。
///
/// 流程与 `run_backtest` 对齐: on_init → 逐 tick [step_portfolio(推进 + 撮合前 tick
/// 残留限价单) → on_tick 产单 → place_order(市价即时按本 tick 各 pair bar open 成交;
/// 资金不足拒单计数) → drain_fills → on_fill] → report_portfolio。
///
/// 028 T038: 原 `signal_klines` 参数(信号线预装)已退役 —— 多标的只是"同一账本按统一时间轴
/// 撮合"(保留), 历史数据由策略自己声明 `data:series{...}` / `market:subscribe`.
pub fn run_portfolio_backtest(
    config: StrategyConfig,
    initial_balance: Balance,
    ticks: &[Vec<(String, Kline)>],
    strategy: &mut dyn Strategy,
) -> BacktestReport {
    let mut ctx = BacktestContext::new(config, initial_balance);
    strategy.on_init(&mut ctx);

    for bars in ticks {
        ctx.step_portfolio(bars);
        let orders = strategy.on_tick(&mut ctx);
        for req in orders {
            let _ = ctx.place_order(req);
        }
        let fills = ctx.drain_fills();
        for fill in fills {
            strategy.on_fill(&mut ctx, fill);
        }
    }

    ctx.report_portfolio()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ricow_strategy::{ConfigValue, LuaStrategy};
    use rust_decimal::prelude::ToPrimitive;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn kline(day: i64, open: i64, close: i64) -> Kline {
        Kline {
            open_time: chrono::DateTime::from_timestamp_millis(day * 86_400_000).unwrap(),
            open: Decimal::from(open),
            high: Decimal::from(open.max(close) + 1),
            low: Decimal::from(open.min(close) - 1),
            close: Decimal::from(close),
            volume: dec!(1000),
            close_time: chrono::DateTime::from_timestamp_millis(day * 86_400_000 + 1).unwrap(),
        }
    }

    fn strategy_config() -> StrategyConfig {
        StrategyConfig {
            name: "bsm-runner".into(),
            strategy_type: "lua".into(),
            enabled: true,
            exchange: "binance".into(),
            params: Default::default(),
            dry_run_started_at: None,
            live_enabled: false,
            market: "spot".into(),
            position_mode: "one-way".into(),
            backtest: None,
        }
    }

    fn kline_at(ms: i64, open: i64) -> Kline {
        Kline {
            open_time: chrono::DateTime::from_timestamp_millis(ms).unwrap(),
            open: Decimal::from(open),
            high: Decimal::from(open),
            low: Decimal::from(open),
            close: Decimal::from(open),
            volume: dec!(1),
            close_time: chrono::DateTime::from_timestamp_millis(ms + 1).unwrap(),
        }
    }

    /// 盘中 tick 生成: 按 interval 边界分桶, 桶内确定性排序; 日线路径不受影响。
    #[test]
    fn build_interval_ticks_buckets_by_interval_and_orders_pairs() {
        let ms15 = 900_000_i64;
        let mut m: HashMap<String, Vec<Kline>> = HashMap::new();
        m.insert("ABUSDT".into(), (0..8).map(|i| kline_at(i * ms15, 100 + i)).collect());
        m.insert("BBUSDT".into(), (2..8).map(|i| kline_at(i * ms15, 200 + i)).collect());
        let t = build_interval_ticks(&m, ms15);
        assert_eq!(t.len(), 8, "每个 15m 桶一个 tick");
        assert_eq!(t[0].len(), 1, "首桶只有 ABUSDT");
        assert_eq!(t[2].len(), 2, "第 2 桶起两只都有");
        assert_eq!(t[2][0].0, "ABUSDT", "桶内按 pair 名排序 (确定性)");
        assert_eq!(t[2][1].0, "BBUSDT");
        // 同一 UTC 日内的 8 根 15m bar → 日线口径仍折成 1 个 tick (回归: 旧路径零改动)
        assert_eq!(build_daily_ticks(&m).len(), 1);
    }

    fn balance(initial: i64) -> Balance {
        Balance { asset: "USDT".into(), free: Decimal::from(initial), locked: Decimal::ZERO }
    }

    /// Lua 名单型轮动脚本 (T5.3: 组合冒烟改 LuaStrategy 驱动 — 替代将删的 BsMomentum):
    /// plan[tick] = 目标名单, truth-based diff (现查持仓) 先卖后买, 预算 = min(E'/5, cash/m)
    /// 净回笼口径 (fee=10bps 默认, 与撮合同源经 ctx:config_f64), 语义对齐 bs_momentum.lua。
    const ROTATION_SCRIPT: &str = r#"
        plan = { {}, { "TSLABUSDT" }, { "TSLABUSDT", "NVDABUSDT" } }
        tick = 0
        -- 028: 组合快照靠"声明"驱动(退役装配层 universe 配置注入) —— 要盯的标的自己声明。
        market:subscribe({ pair = "TSLABUSDT" })
        market:subscribe({ pair = "NVDABUSDT" })
        pool = { "TSLABUSDT", "NVDABUSDT" }
        function on_tick(ctx)
            tick = tick + 1
            local targets = {}
            for _, p in ipairs(plan[tick] or {}) do targets[p] = true end
            local orders = {}
            for _, p in ipairs(pool) do
                local sz = ctx:pos_size(p, "long")
                if sz > 0 and not targets[p] then
                    orders[#orders + 1] = { pair = p, side = "sell", size = sz,
                                            order_type = "market", reduce_only = true }
                end
            end
            local newcomers = {}
            for _, p in ipairs(pool) do
                if targets[p] and ctx:pos_size(p, "long") <= 0 then
                    newcomers[#newcomers + 1] = p
                end
            end
            local fee = ctx:config_f64("fee_taker_bps") / 10000
            local cash = ctx:balance("USDT") or 0
            local m = #newcomers
            if m > 0 then
                local per = math.min(cash / 5, cash / m) * 0.9999
                for _, p in ipairs(newcomers) do
                    local px = ctx:price(p) or 0
                    if px > 0 then
                        orders[#orders + 1] = { pair = p, side = "buy",
                                                size = per / px / (1 + fee),
                                                order_type = "market" }
                    end
                end
            end
            return orders
        end
    "#;

    fn lua_strategy() -> (LuaStrategy, StrategyConfig) {
        let mut cfg = strategy_config();
        cfg.params.insert("script".into(), ConfigValue::String(ROTATION_SCRIPT.into()));
        let s = LuaStrategy::from_source(ROTATION_SCRIPT, cfg.clone()).expect("脚本应编译通过");
        (s, cfg)
    }

    #[test]
    fn test_build_daily_ticks_gap_alignment() {
        // 三标的起始/停牌不同: A 全程; B 晚上市 (day2 起); C day2 停牌。
        // day1 = 2026-09-01, day2 = 09-02, day3 = 09-03。
        let mut by_pair: HashMap<String, Vec<Kline>> = HashMap::new();
        by_pair.insert(
            "AAPLUSDT".into(),
            vec![kline(1, 100, 101), kline(2, 101, 102), kline(3, 102, 103)],
        );
        by_pair.insert("TSLABUSDT".into(), vec![kline(2, 200, 201), kline(3, 201, 202)]);
        by_pair.insert("NVDABUSDT".into(), vec![kline(1, 50, 51), kline(3, 52, 53)]);

        let ticks = build_daily_ticks(&by_pair);
        assert_eq!(ticks.len(), 3, "三个 UTC 日各一个 tick");
        let days: Vec<Vec<String>> = ticks
            .iter()
            .map(|t| {
                let mut ps: Vec<String> = t.iter().map(|(p, _)| p.clone()).collect();
                ps.sort();
                ps
            })
            .collect();
        assert_eq!(days[0], vec!["AAPLUSDT", "NVDABUSDT"], "day1: A + C");
        assert_eq!(days[1], vec!["AAPLUSDT", "TSLABUSDT"], "day2: A + B (C 停牌缺席)");
        assert_eq!(days[2], vec!["AAPLUSDT", "NVDABUSDT", "TSLABUSDT"], "day3 全在");
        // 同日多根防御: 追加一根同日 bar → 取最后一根。
        by_pair.get_mut("AAPLUSDT").unwrap().push(kline(3, 103, 110));
        let ticks = build_daily_ticks(&by_pair);
        let last_day = &ticks[2];
        let aapl = last_day.iter().find(|(p, _)| p == "AAPLUSDT").unwrap();
        assert_eq!(aapl.1.close, dec!(110), "同日多根取当日最后一根");
    }

    #[test]
    fn test_run_portfolio_two_pairs_match_and_route() {
        // 端到端 (LuaStrategy 驱动, T5.3): 名单 [TSLA] tick2 → 建 TSLA; [TSLA, NVDA]
        // tick3 → 留存 TSLA + 买入 NVDA。断言撮合按 pair 路由、成交价 = 本 tick open
        // (非前视)、预算净回笼口径零拒单。
        let initial = balance(100_000);
        let (mut s, cfg) = lua_strategy();

        // 3 tick 日历; TSLA tick2 open=100 close=200 (大幅拉升, 防"按 close 成交"静默前视)。
        let ticks = vec![
            vec![
                ("TSLABUSDT".to_string(), kline(1, 90, 91)),
                ("NVDABUSDT".to_string(), kline(1, 200, 201)),
            ],
            vec![
                ("TSLABUSDT".to_string(), kline(2, 100, 200)),
                ("NVDABUSDT".to_string(), kline(2, 201, 202)),
            ],
            vec![
                ("TSLABUSDT".to_string(), kline(3, 200, 205)),
                ("NVDABUSDT".to_string(), kline(3, 205, 210)),
            ],
        ];

        let report = run_portfolio_backtest(
            cfg, // 与策略同一声明配置 — ctx 装载后快照按声明(市场订阅)遍历
            initial, &ticks, &mut s,
        );

        assert_eq!(report.total_bars, 3);
        assert_eq!(report.rejected_count, 0, "无资金不足拒单");
        assert_eq!(report.fills.len(), 2, "TSLA tick2 建仓 + NVDA tick3 建仓");
        let tsla_fill =
            report.fills.iter().find(|f| f.pair == "TSLABUSDT").expect("TSLA 成交必须存在");
        // 成交价 = tick2 (建仓日) open=100 附近 (滑点内), 而非 close=200 — 前视防护断言。
        let tsla_price = tsla_fill.fill_price.to_f64().unwrap();
        assert!(
            (tsla_price - 100.0).abs() < 1.0,
            "TSLA 成交价应≈tick2 open 100, 实得 {tsla_price} (若≈200 即静默前视)"
        );
        let nvda_fill =
            report.fills.iter().find(|f| f.pair == "NVDABUSDT").expect("NVDA 成交必须存在");
        let nvda_price = nvda_fill.fill_price.to_f64().unwrap();
        assert!(
            (nvda_price - 205.0).abs() < 1.0,
            "NVDA 成交价应≈tick3 open 205, 实得 {nvda_price}"
        );
        // 组合估值: 预算 = min(E/5, cash/m) ≈ 每新进 20k (E/5 主导)。
        // TSLA 建仓 ~20k @100 → 期末 @205 ≈ 41k; NVDA 建仓 ~24k?→ 实际 20k @205 → 期末
        // @210 ≈ 20.5k; 现金 ≈ 60k ⇒ 权益 ≈ 121k (价格故意大涨以验前视, 非泡沫)。
        let fe = report.final_equity.to_f64().unwrap();
        assert!(fe > 110_000.0 && fe < 130_000.0, "期末权益异常 (期望≈121k): {fe}");
        assert_eq!(report.fills[0].pair, "TSLABUSDT", "先建 TSLA 后建 NVDA (名单序)");
    }
}
