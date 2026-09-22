//! 回测执行: 策略 + 历史 K 线 → 回测报告。
//!
//! - `run_backtest`: 单标的路径 (v0.2 起, 回归硬门槛 — 行为零改动)。
//! - `run_portfolio_backtest`: 组合路径 (M2, bs_momentum 轮动) — 同一撮合账本,
//!   多标的按统一时间轴逐 tick 驱动; 市价单由 BacktestContext 在 place_order 时
//!   按本 tick 各 pair bar open 即时撮合 (无前视: bar 于 tick 内已开盘)。

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use ricow_core::{Balance, Kline};
use ricow_strategy::{BacktestContext, BacktestReport, Context, Strategy, StrategyConfig};

/// 在历史 K 线上运行一次回测 (单标的)。
///
/// 流程: on_init → 逐 bar step_bar + on_tick + place_order + drain_fills + on_fill → report。
pub fn run_backtest(
    config: StrategyConfig,
    initial_balance: Balance,
    klines: &[Kline],
    strategy: &mut dyn Strategy,
) -> BacktestReport {
    // 023 高周期序列预装: 策略声明 `atr_interval`(网格间距) 与/或 `regime_interval`(趋势判据) 时,
    // 用**全段** klines(含预热段)重采样一次装入 ctx —— 键 = `pair|tf`, 同一 pair 可同时装多套;
    // 重采样在装配层只做一次, 引擎不逐 tick 重算。预热段(前 `warmup_bars` 根)只喂高周期指标,
    // 不进 tick 循环与报告。
    let tf_pair = config.get_str("pair").map(str::to_string);
    let mut tf_labels: Vec<String> = Vec::new();
    for key in ["atr_interval", "regime_interval"] {
        if let Some(tf) = config.get_str(key) {
            if !tf_labels.iter().any(|t| t == tf) {
                tf_labels.push(tf.to_string());
            }
        }
    }
    let warmup = config.get_i64("warmup_bars").unwrap_or(0).max(0) as usize;
    let mut ctx = BacktestContext::new(config, initial_balance);
    if let Some(pair) = tf_pair.as_deref() {
        for tf in &tf_labels {
            match ricow_strategy::tf_ms_of(tf) {
                Some(tf_ms) => {
                    let bars = ricow_strategy::resample_complete(klines, tf_ms);
                    tracing::info!(
                        target: "multiframe",
                        pair = pair,
                        tf = tf.as_str(),
                        buckets = bars.len(),
                        "高周期序列预装"
                    );
                    ctx.set_tf_klines(pair, tf, bars);
                }
                None => tracing::warn!(
                    target: "multiframe",
                    tf = tf.as_str(),
                    "高周期标签不是受支持周期, 该通道关闭"
                ),
            }
        }
    }
    strategy.on_init(&mut ctx);

    // 预热段跳过(仅当预热段短于全段时; 数据不足时退化为整段回测, 不静默丢数据)。
    let skip = if warmup > 0 && warmup < klines.len() { warmup } else { 0 };
    for k in &klines[skip..] {
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
    // 023: 回测收尾也跑一次 `on_stop` —— 策略的统计输出(跳过计数/重挂次数/末次方向)必须能在
    // 回测里看到, 否则"跳过占比"这类验收数字无处可取。语义是"收尾回调", 与实盘停机清理
    // 无关(回测没有交易所资源可清); 当前无内置策略在 on_stop 里下单。
    strategy.on_stop(&mut ctx);
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

/// 组合回测 (M2): 多标的统一时间轴驱动。
///
/// 流程与 `run_backtest` 对齐: on_init → 逐 tick [step_portfolio(推进 + 撮合前 tick
/// 残留限价单) → on_tick 产单 → place_order(市价即时按本 tick 各 pair bar open 成交;
/// 资金不足拒单计数) → drain_fills → on_fill] → report_portfolio。
///
/// `signal_klines` (bs_momentum Lua 化, 2026-09-09): 美股信号日线 (键 = 原始 pair 名,
/// 装配层按窗口截取), 构造 ctx 后装载 — 组合信号模式下 ctx:klines(pair) 返回按全局
/// tick 时间截断的信号段 (无前视); 空 map = 信号通道关闭 (纯成交轨, 现行为)。
pub fn run_portfolio_backtest(
    config: StrategyConfig,
    initial_balance: Balance,
    ticks: &[Vec<(String, Kline)>],
    strategy: &mut dyn Strategy,
    signal_klines: HashMap<String, Vec<Kline>>,
) -> BacktestReport {
    let mut ctx = BacktestContext::new(config, initial_balance);
    ctx.set_signal_klines(signal_klines);
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
        // universe 键 → Lua 快照组合模式: 逐只填池内 price/position (T3.2 判据)。
        cfg.params.insert("universe".into(), ConfigValue::String("TSLABUSDT,NVDABUSDT".into()));
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
            cfg, // 与策略同一 config (含 universe) — ctx 装载后快照按池遍历
            initial,
            &ticks,
            &mut s,
            HashMap::new(), // 空信号 map = 纯成交轨 (现行为)
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
