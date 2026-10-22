//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/spot/`(linear_position_grid 现货线性仓位网格)与
//! `strategies/futures/`(shannon_*_grid_futures), include_str! 编译期嵌入。
//! 断言每个内置脚本的关键行为 (建仓/激活/配对/挂单), 对齐 Rust 版已知向量。

use std::collections::HashMap;

use ricow_core::{Balance, Kline, OrderAction, OrderFill, OrderRequest, OrderSide, OrderType};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::backtest::BacktestContext;
use crate::config::{ConfigValue, StrategyConfig};
use crate::context::Context;
use crate::lua::LuaStrategy;
use crate::strategy::Strategy;
use rust_decimal::prelude::ToPrimitive;

fn config(script: &str, params: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut map = HashMap::new();
    map.insert("script".into(), ConfigValue::String(script.into()));
    for (k, v) in params {
        map.insert(k.to_string(), v.clone());
    }
    StrategyConfig {
        name: "t".into(),
        strategy_type: "lua".into(),
        enabled: true,
        exchange: "binance".into(),
        params: map,
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    }
}

// ============================================================================
// 测试共用辅助
// ============================================================================

/// 测试用: 第 `hour` 小时的 bar(1h 间隔; 引擎不校验周期, 策略只看时间戳)。
fn bar_at_hour(hour: i64, open: i64, high: i64, low: i64, close: i64) -> Kline {
    let ms = hour * 3_600_000;
    Kline {
        open_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap(),
        open: Decimal::from(open),
        high: Decimal::from(high),
        low: Decimal::from(low),
        close: Decimal::from(close),
        volume: Decimal::ONE,
        close_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms + 3_599_999).unwrap(),
    }
}

/// 测试共用: 每根 TR 恒为 2 (high−low=2 且前收盘落在区间内) → ATR(14) 精确等于 2。
fn tf_bars(n: i64) -> Vec<Kline> {
    (0..n).map(|h| bar_at_hour(h, 100, 101, 99, 100)).collect()
}

/// f64 价格的平 bar(o=h=l=c=px; 引擎不校验周期)。
fn bar_at_f(hour: i64, px: f64) -> Kline {
    let ms = hour * 3_600_000;
    let d = Decimal::from_f64_retain(px).unwrap();
    Kline {
        open_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap(),
        open: d,
        high: d,
        low: d,
        close: d,
        volume: Decimal::ONE,
        close_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms + 3_599_999).unwrap(),
    }
}

/// 033: 主序列 —— h<at 为 base, 之后为 then(触发 start_price 激活门槛), 每根带 ±1 振幅
/// (振幅只为让重采样后的 1h ATR 非零, 平段 ATR=0 会被"ATR 未就绪"拦下)。
fn univ2_main(n: i64, base: i64, then: i64, at: i64) -> Vec<Kline> {
    (0..n)
        .map(|h| {
            let p = if h < at { base } else { then };
            bar_at_hour(h, p, p + 1, p - 1, p)
        })
        .collect()
}

/// 033: 把第 `idx` 根替换为自定义 OHLC(制造一次穿越成交)。
fn with_bar(mut bars: Vec<Kline>, idx: i64, o: i64, h: i64, l: i64, c: i64) -> Vec<Kline> {
    bars[idx as usize] = bar_at_hour(idx, o, h, l, c);
    bars
}

/// 037 事件模型: on_fill 返回的订单即下单 → drain → 递归(复刻引擎 settle_orders, 034,
/// 含 [`MAX_FILL_DECISION_DEPTH`] 同款深度上限 8); 下单批次按 bar 收集进 `out`。
const MAX_FILL_DECISION_DEPTH: u32 = 8;

fn settle_univ2(
    ctx: &mut BacktestContext,
    strategy: &mut LuaStrategy,
    orders: Vec<OrderRequest>,
    out: &mut Vec<Vec<Vec<OrderRequest>>>,
    depth: u32,
) {
    if depth > MAX_FILL_DECISION_DEPTH {
        return;
    }
    if !orders.is_empty() {
        out.last_mut().expect("out 应已按 bar 预留").push(orders.clone());
    }
    for req in orders {
        // 038 R1: 链内 (depth≥1) 限价单次 bar 生效 —— 与引擎 settle_orders 同款 defer 开关;
        // 市价单/撤单指令即时处理。
        let defer = depth > 0 && req.order_type == OrderType::Limit;
        if defer {
            ctx.set_defer_new_limits(true);
        }
        let _ = ctx.place_order(req);
        if defer {
            ctx.set_defer_new_limits(false);
        }
    }
    for f in ctx.drain_fills() {
        let follow = strategy.on_fill(ctx, f);
        settle_univ2(ctx, strategy, follow, out, depth + 1);
    }
}

/// 033/037: runner 同款时序(step_bar → 撮合成交先入账 → on_tick → 下单 → 即时成交入账),
/// 与 `backtest_runner::run_backtest` 逐 bar 循环一致(成交先于决策, 同 bar 重挂生效);
/// 037 起含事件模型递归(on_fill 返回订单即下单)。返回逐 bar 全部下单批次
/// (on_fill 返回 + on_tick, 每项为该 bar 的批次列表)。
fn run_univ2(
    cfg: StrategyConfig,
    bars: &[Kline],
    tf: Option<Vec<Kline>>,
) -> (Vec<Vec<Vec<OrderRequest>>>, BacktestContext, LuaStrategy) {
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    if let Some(bars) = tf {
        ctx.set_tf_klines("ETHUSDT", "1h", bars);
    }
    strategy.on_init(&mut ctx);
    let mut out: Vec<Vec<Vec<OrderRequest>>> = Vec::new();
    for k in bars {
        ctx.step_bar(k.clone());
        out.push(Vec::new());
        for f in ctx.drain_fills() {
            // 引擎口径: step_bar 撮合出的成交在 settle_orders(d=0) 首轮派发,
            // on_fill 的 follow 从 depth+1 起(深度上限 8)。
            let follow = strategy.on_fill(&mut ctx, f);
            settle_univ2(&mut ctx, &mut strategy, follow, &mut out, 1);
        }
        let orders = strategy.on_tick(&mut ctx);
        settle_univ2(&mut ctx, &mut strategy, orders, &mut out, 0);
    }
    (out, ctx, strategy)
}

// ============================================================================
// 合约香农网格 (shannon_grid_futures) 集成测试
// (flag = 卖笔数 − 买笔数, 建仓与强平不计; |flag|≥2 趋势侧间距 × mult^(|flag|−1))。
// ============================================================================

const SHANNON_GRID_FUT: &str = include_str!("../../../strategies/futures/shannon_grid_futures.lua");

/// 合约 hedge 配置(默认: invest_cash=10000, 杠杆 2 → 总资金 20000, 建仓名义 10000)。
fn shannon_grid_fut_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("min_notional", ConfigValue::Float(50.0)),
        ("fee_side", ConfigValue::Float(0.0005)),
        ("invest_cash", ConfigValue::Float(10000.0)),
    ];
    params.extend_from_slice(extra);
    let mut cfg = config(SHANNON_GRID_FUT, &params);
    cfg.market = "futures".into();
    cfg.position_mode = "hedge".into();
    cfg.backtest = Some(crate::config::BacktestToml { leverage: Some(2.0), ..Default::default() });
    cfg
}

#[test]
fn test_shannon_grid_futures_no_short_orders() {
    // 对冲已移除: 默认跑激活序列, 全程不得出现任何 position_side=short 订单。
    let cfg = shannon_grid_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert!(
        orders.iter().flatten().flatten().all(|o| o.position_side.as_deref() != Some("short")),
        "无对冲腿策略 -> 不得有任何空头订单"
    );
    assert_eq!(st.global_f64("fill_count").unwrap_or(0.0), 1.0, "应恰有建仓 1 笔成交");
    assert_eq!(st.global_f64("flag"), Some(0.0), "建仓成交不计 flag");
}

#[test]
fn test_shannon_grid_futures_default_symmetric_spacing() {
    // flag=0(建仓不计)且默认 mult=1.2: |flag|≤1 不放大 —— 建仓/首挂按纯 1:1 公式落位:
    // 买 97 @1.5202 / 卖 103 @1.4809。
    let cfg = shannon_grid_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("balance_price"), Some(100.0));
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(97)));
    assert_eq!(sell.price, Some(dec!(103)));
    let q_buy_exp = (9995.0 - 100.0 * 97.0) / (97.0 * (2.0 + 0.0005));
    let q_sell_exp = (100.0 * 103.0 - 9995.0) / (103.0 * (2.0 - 0.0005));
    assert!(
        (buy.size.to_f64().unwrap() - q_buy_exp).abs() < 1e-6,
        "flag=0 买量应=1:1 恢复量 {q_buy_exp}: {}",
        buy.size
    );
    assert!(
        (sell.size.to_f64().unwrap() - q_sell_exp).abs() < 1e-6,
        "flag=0 卖量应=1:1 恢复量 {q_sell_exp}: {}",
        sell.size
    );
    assert_eq!(st.global_f64("flag"), Some(0.0), "仅建仓 -> flag=0(建仓不计)");
}

/// 扫描全部下单批次, 返回**最后一个同时含买/卖限价单**批次的 (买价, 卖价)。
fn last_two_sided(orders: &[Vec<Vec<OrderRequest>>]) -> (f64, f64) {
    let mut res = None;
    for batches in orders {
        for b in batches {
            let buy = b.iter().find(|o| {
                o.order_type == OrderType::Limit && o.side == OrderSide::Buy && o.price.is_some()
            });
            let sell = b.iter().find(|o| {
                o.order_type == OrderType::Limit && o.side == OrderSide::Sell && o.price.is_some()
            });
            if let (Some(bu), Some(se)) = (buy, sell) {
                res = Some((
                    bu.price.unwrap().to_f64().unwrap(),
                    se.price.unwrap().to_f64().unwrap(),
                ));
            }
        }
    }
    res.expect("应存在同时含买卖限价单的重挂批次")
}

/// 两笔网格买入成交的序列: 建仓(bar15)@100 → 重挂买97/卖103; bar16 穿买97(flag−1)→买94/卖100;
/// bar17 穿买94(flag−2)→重挂; bar18 守卫(不穿越)。返回该 bars 与 (orders, ctx, st)。
fn shannon_two_buys(
    extra: &[(&str, ConfigValue)],
) -> (Vec<Vec<Vec<OrderRequest>>>, BacktestContext, LuaStrategy) {
    let mut params = vec![("start_price", ConfigValue::Float(150.0))];
    params.extend_from_slice(extra);
    let cfg = shannon_grid_fut_cfg(&params);
    let bars = with_bar(
        with_bar(with_bar(univ2_main(19, 200, 100, 10), 16, 98, 99, 96, 97), 17, 95, 96, 93, 94),
        18,
        95,
        96,
        94,
        95,
    );
    run_univ2(cfg, &bars, Some(tf_bars(60)))
}

#[test]
fn test_shannon_grid_futures_flag_counts_fills() {
    // 建仓(买)不计 flag; 两笔网格买入成交 → flag=−2(若建仓也计则会是 −3)。买 −1 口径。
    let (_orders, _ctx, st) = shannon_two_buys(&[]);
    assert_eq!(st.global_f64("fill_count"), Some(3.0), "建仓 + 两笔网格买");
    assert_eq!(st.global_f64("flag"), Some(-2.0), "建仓不计, 每笔买 −1 -> flag=−2");
    assert_eq!(st.global_f64("flag_min"), Some(-2.0), "flag 历史最小值=−2");
    assert_eq!(st.global_f64("flag_max"), Some(0.0), "全程无卖 -> flag_max=0");
}

#[test]
fn test_shannon_grid_futures_flag_buy_spacing_amplified() {
    // flag=−2(两笔买)→ 买间距 ×1.2^(2−1)=×1.2, 卖间距不变。末次重挂(平衡 94):
    // 买 94−3.6=90.4 / 卖 94+3=97。量随放大后买价自动跟随(1:1 由重建恒等式保证)。
    let (orders, _ctx, st) = shannon_two_buys(&[]);
    assert_eq!(st.global_f64("flag"), Some(-2.0));
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - 90.4).abs() < 0.01, "买间距应放大到 3.6 -> 买价 90.4: {buy_px}");
    assert!((sell_px - 97.0).abs() < 0.01, "卖间距不变 -> 卖价 97: {sell_px}");
}

#[test]
fn test_shannon_grid_futures_flag_sell_spacing_amplified() {
    // flag=+2(两笔卖)→ 卖间距 ×1.2, 买间距不变。建仓@100→卖103(fill,+1)→卖106(fill,+2)
    // → 末次重挂(平衡 106): 买 106−3=103 / 卖 106+3.6=109.6。
    let cfg = shannon_grid_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(
        with_bar(
            with_bar(univ2_main(19, 200, 100, 10), 16, 102, 104, 101, 103),
            17,
            105,
            107,
            104,
            106,
        ),
        18,
        105,
        106,
        104,
        105,
    );
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("flag"), Some(2.0), "每笔卖 +1 -> flag=+2");
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - 103.0).abs() < 0.01, "买间距不变 -> 买价 103: {buy_px}");
    assert!((sell_px - 109.6).abs() < 0.01, "卖间距应放大到 3.6 -> 卖价 109.6: {sell_px}");
}

#[test]
fn test_shannon_grid_futures_flag_mult_one_disables() {
    // flag_spacing_mult=1.0: 即使 flag=−2 也不放大 —— 末次重挂买 94−3=91 / 卖 94+3=97(对称)。
    let (orders, _ctx, st) = shannon_two_buys(&[("flag_spacing_mult", ConfigValue::Float(1.0))]);
    assert_eq!(st.global_f64("flag"), Some(-2.0));
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - 91.0).abs() < 0.01, "禁用放大 -> 买价 91: {buy_px}");
    assert!((sell_px - 97.0).abs() < 0.01, "禁用放大 -> 卖价 97: {sell_px}");
}

#[test]
fn test_shannon_grid_futures_flag_state_persist_and_recon() {
    // flag 持久化 + stat_flag_* 导出 + 重建恒等式(去 delev 项后)逐分闭合。
    let (_orders, mut ctx, st) = shannon_two_buys(&[]);
    let mut st = st;
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    assert_eq!(snap.get("flag").map(|s| s.as_str()), Some("-2"), "flag 应持久化");
    assert_eq!(snap.get("stat_flag").map(|s| s.as_str()), Some("-2"));
    assert_eq!(snap.get("stat_flag_min").map(|s| s.as_str()), Some("-2"));
    assert_eq!(snap.get("stat_flag_max").map(|s| s.as_str()), Some("0"));
    assert!(snap.contains_key("stat_eff_flag_spacing_mult"), "stat_eff_flag_spacing_mult 应导出");
    let diff: f64 =
        snap.get("stat_v_recon_diff").expect("stat_v_recon_diff").parse().expect("数值");
    assert!(diff.abs() < 0.01, "去 delev 项的重建核对应通过: {diff}");
    assert!(!snap.contains_key("stat_delev_count"), "delev 统计应已移除");
}

// ============================================================================
// 合约中性香农网格 (shannon_neutral_grid_futures) 集成测试 —— one-way 单向持仓、
// 启动不建仓(虚拟账本自 start_price 起 1:1)、空仓卖单开空、双侧漂移纠偏、
// 方向 flag 间距放大(同母本口径: 卖 +1 / 买 −1, 建仓/强平/期末清仓不计)。
// (复用 univ2_main/with_bar/tf_bars/last_two_sided/run_univ2 时序; ATR 恒 2 → 间距 3)
// ============================================================================

const SHANNON_NEUTRAL_GRID_FUT: &str =
    include_str!("../../../strategies/futures/shannon_neutral_grid_futures.lua");

/// one-way 中性配置(invest_cash=10000, 杠杆 3 → 总资金 30000, Q0=150@100, v_cash=15000)。
fn shannon_neutral_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("start_price", ConfigValue::Float(100.0)),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("min_notional", ConfigValue::Float(50.0)),
        ("fee_side", ConfigValue::Float(0.0005)),
        ("invest_cash", ConfigValue::Float(10000.0)),
    ];
    params.extend_from_slice(extra);
    let mut cfg = config(SHANNON_NEUTRAL_GRID_FUT, &params);
    cfg.market = "futures".into();
    cfg.position_mode = "one-way".into();
    cfg.backtest = Some(crate::config::BacktestToml {
        leverage: Some(3.0),
        margin_mode: Some("cross".into()),
        ..Default::default()
    });
    cfg
}

/// 启动序列(bar15 ATR 就绪): 全程无市价单(不建仓), 首挂买 97 / 卖 103, 量按 1:1 恢复公式。
#[test]
fn test_shannon_neutral_startup_no_market_orders() {
    let (orders, ctx, st) =
        run_univ2(shannon_neutral_cfg(&[]), &univ2_main(20, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("fatal"), Some(0.0), "正常启动不得停机");
    assert!(
        orders.iter().flatten().flatten().all(|o| o.order_type != OrderType::Market),
        "启动不建仓 -> 全程不得有市价单"
    );
    assert!(
        orders.iter().flatten().flatten().all(|o| o.position_side.is_none() && !o.reduce_only),
        "one-way 挂单不得带 position_side / reduce_only"
    );
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "平衡价 := start_price");
    assert_eq!(st.global_f64("q0"), Some(150.0), "Q0 = 30000/(2×100)");
    assert_eq!(st.global_f64("v_cash"), Some(15000.0), "初始虚拟现金 = 总资金一半");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "无成交(价 100 在带内)");
    assert!(ctx.position_directional("ETHUSDT", OrderSide::Buy).is_none(), "真实仓位应为 0");
    assert!(ctx.position_directional("ETHUSDT", OrderSide::Sell).is_none(), "真实仓位应为 0");
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(97)));
    assert_eq!(sell.price, Some(dec!(103)));
    let q_buy_exp = (15000.0 - 150.0 * 97.0) / (97.0 * (2.0 + 0.0005));
    let q_sell_exp = (150.0 * 103.0 - 15000.0) / (103.0 * (2.0 - 0.0005));
    assert!(
        (buy.size.to_f64().unwrap() - q_buy_exp).abs() < 1e-6,
        "买量应=1:1 恢复量 {q_buy_exp}: {}",
        buy.size
    );
    assert!(
        (sell.size.to_f64().unwrap() - q_sell_exp).abs() < 1e-6,
        "卖量应=1:1 恢复量 {q_sell_exp}: {}",
        sell.size
    );
}

#[test]
fn test_shannon_neutral_requires_start_price() {
    let (orders, _ctx, st) = run_univ2(
        shannon_neutral_cfg(&[("start_price", ConfigValue::Float(0.0))]),
        &univ2_main(60, 200, 100, 10),
        Some(tf_bars(60)),
    );
    assert!(orders.iter().all(|o| o.is_empty()), "缺必填 start_price 时不得下任何单");
    assert_eq!(st.global_f64("fatal"), Some(1.0), "缺参数应 FATAL 停机");
}

/// 两笔网格卖成交序列(bar16 穿卖 103, bar17 穿卖 106, bar18/19 守卫): flag=+2, 卖间距放大。
fn neutral_two_sells() -> (Vec<Vec<Vec<OrderRequest>>>, BacktestContext, LuaStrategy) {
    let bars = with_bar(
        with_bar(
            with_bar(univ2_main(20, 200, 100, 10), 16, 103, 104, 102, 103),
            17,
            106,
            107,
            105,
            106,
        ),
        18,
        106,
        107,
        105,
        106,
    );
    // bar19 守卫(不穿越): 防末根 univ2_main 的 101 穿越放大后买 103 污染末次重挂断言。
    let bars = with_bar(bars, 19, 106, 107, 105, 106);
    run_univ2(shannon_neutral_cfg(&[]), &bars, Some(tf_bars(60)))
}

#[test]
fn test_shannon_neutral_empty_sell_opens_short() {
    // 中性核心: 空仓卖单成交开空(one-way 先平后开), 真实净仓 = v_pos − Q0 < 0。
    let (_orders, ctx, st) = neutral_two_sells();
    let short = ctx.position_directional("ETHUSDT", OrderSide::Sell).expect("应持有空头仓");
    assert!(ctx.position_directional("ETHUSDT", OrderSide::Buy).is_none(), "不得有多头仓");
    assert!(short.size.to_f64().unwrap() > 4.0, "两笔卖成交后空头应有实质规模: {}", short.size);
    assert_eq!(st.global_f64("flag"), Some(2.0), "每笔卖 +1 -> flag=+2");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let net = st.global_f64("r_long").unwrap_or(0.0) - st.global_f64("r_short").unwrap_or(0.0);
    assert!(net < 0.0, "净仓应为负(空头): {net}");
    assert!(
        (v_pos - (150.0 + net)).abs() < 1e-6,
        "净仓恒等式 v_pos = Q0 + 真实净仓: {v_pos} vs {}",
        150.0 + net
    );
    assert!(
        (short.size.to_f64().unwrap() - (150.0 - v_pos)).abs() < 1e-6,
        "引擎空头 = Q0 − v_pos: {} vs {}",
        short.size,
        150.0 - v_pos
    );
}

#[test]
fn test_shannon_neutral_flag_sell_spacing_amplified() {
    // flag=+2(两笔卖)→ 卖间距 ×1.2^(2−1)=×1.2, 买间距不变。末次重挂(平衡 106):
    // 买 103 / 卖 109.6。
    let (orders, _ctx, st) = neutral_two_sells();
    assert_eq!(st.global_f64("flag"), Some(2.0));
    assert_eq!(st.global_f64("flag_max"), Some(2.0));
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - 103.0).abs() < 0.01, "买间距不变 -> 买价 103: {buy_px}");
    assert!((sell_px - 109.6).abs() < 0.01, "卖间距应放大到 3.6 -> 卖价 109.6: {sell_px}");
}

/// 两笔网格买成交序列(bar16 穿买 97, bar17 穿买 94, bar18/19 守卫): flag=−2, 买间距放大。
fn neutral_two_buys() -> (Vec<Vec<Vec<OrderRequest>>>, BacktestContext, LuaStrategy) {
    // 末根守卫(不穿越): 防 univ2_main 的 101 穿越卖单污染末次重挂断言。
    let bars = with_bar(univ2_main(20, 200, 100, 10), 19, 95, 96, 94, 95);
    let bars = with_bar(bars, 18, 95, 96, 94, 95);
    let bars = with_bar(bars, 17, 94, 95, 93, 94);
    let bars = with_bar(bars, 16, 97, 98, 96, 97);
    run_univ2(shannon_neutral_cfg(&[]), &bars, Some(tf_bars(60)))
}

#[test]
fn test_shannon_neutral_flag_buy_spacing_amplified() {
    // flag=−2(两笔买)→ 买间距 ×1.2, 卖间距不变。末次重挂(平衡 94):
    // 买 94−3.6=90.4 / 卖 94+3=97。
    let (orders, _ctx, st) = neutral_two_buys();
    assert_eq!(st.global_f64("flag"), Some(-2.0));
    assert_eq!(st.global_f64("flag_min"), Some(-2.0));
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - 90.4).abs() < 0.01, "买间距应放大到 3.6 -> 买价 90.4: {buy_px}");
    assert!((sell_px - 97.0).abs() < 0.01, "卖间距不变 -> 卖价 97: {sell_px}");
}

#[test]
fn test_shannon_neutral_gap_drift_reanchors_sell_side() {
    // 双侧漂移纠偏(卖侧镜像): bar16 跳空低开, 买 97 按开盘价 90 成交(引擎贴齐开盘价) →
    // v_cash 相对新平衡价 90 超配(C ≥ Q×卖价 93)→ 卖量出负 → 仅剩下方买单, 上涨行情
    // 永不成交 → 死锁。触发即重锚平衡价到 C/Q ≈ 97.106, 两侧恢复挂单。
    // 末根守卫(不穿越): 防 univ2_main 末根 101 穿越卖单污染末次重挂断言。
    let bars = with_bar(univ2_main(20, 200, 100, 10), 19, 98, 99, 97, 98);
    let bars = with_bar(bars, 16, 90, 91, 89, 90);
    let bars = with_bar(bars, 17, 98, 99, 97, 98);
    let bars = with_bar(bars, 18, 98, 99, 97, 98);
    let (orders, _ctx, st) = run_univ2(shannon_neutral_cfg(&[]), &bars, Some(tf_bars(60)));
    let balance = st.global_f64("balance_price").expect("balance_price");
    assert!(
        (balance - 97.106).abs() < 0.01,
        "卖侧漂移应触发重锚 C/Q ≈ 97.106(而非停在成交价 90): {balance}"
    );
    let (buy_px, sell_px) = last_two_sided(&orders);
    assert!((buy_px - (balance - 3.0)).abs() < 0.01, "重锚后买价 = 平衡价 − 3: {buy_px}");
    assert!((sell_px - (balance + 3.0)).abs() < 0.01, "重锚后卖价 = 平衡价 + 3: {sell_px}");
}

#[test]
fn test_shannon_neutral_liq_halts() {
    // one-way 组合强平(LIQ-{pair}, position_side=None): 记损失(自维护多头均价口径) + 停机。
    let (_orders, mut ctx, mut st) = neutral_two_buys();
    let r_long = st.global_f64("r_long").expect("前提: 持有净多头");
    assert!(r_long > 0.0);
    let avg_l = st.global_f64("avg_l").expect("多头均价");
    let liq_fill = OrderFill {
        trade_id: Some("t1".into()),
        exchange_order_id: "ex1".into(),
        client_order_id: "LIQ-ETHUSDT".into(),
        pair: "ETHUSDT".into(),
        side: OrderSide::Buy, // 多头被平(引擎 side = 被平方向)
        fill_price: dec!(50),
        fill_size: Decimal::from_f64_retain(r_long).unwrap(),
        fee: dec!(0.1),
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(19 * 3_600_000).unwrap(),
        position_side: None, // one-way 组合强平无侧标
    };
    let follow = st.on_fill(&mut ctx, liq_fill);
    assert!(follow.iter().any(|o| o.action == OrderAction::CancelPending), "爆仓应回传全撤");
    assert_eq!(st.global_f64("liq_count"), Some(1.0));
    assert_eq!(st.global_f64("fatal"), Some(1.0), "真爆仓应停机");
    assert!(st.halted());
    let liq_pnl = st.global_f64("liq_pnl").expect("liq_pnl");
    let expect = r_long * (50.0 - avg_l);
    assert!((liq_pnl - expect).abs() < 0.01, "爆仓损失按多头均价口径 {expect}: {liq_pnl}");
    // 停机后不再产单
    assert!(st.on_tick(&mut ctx).is_empty(), "停机后不得再产单");
}

#[test]
fn test_shannon_neutral_close_fill_no_flag_and_recon() {
    // CLOSE- 期末强平: 按普通成交推进账本, 盈亏按被平侧自维护均价单列, 不计 flag;
    // 重建恒等式 v_cash = v_init − Σ费 − Σ买 + Σ卖 逐分闭合。
    let (_orders, mut ctx, mut st) = neutral_two_buys();
    let r_long = st.global_f64("r_long").expect("前提: 持有净多头");
    let avg_l = st.global_f64("avg_l").unwrap();
    let flag_before = st.global_f64("flag").unwrap();
    let close_fill = OrderFill {
        trade_id: Some("t2".into()),
        exchange_order_id: "ex2".into(),
        client_order_id: "CLOSE-123".into(),
        pair: "ETHUSDT".into(),
        side: OrderSide::Sell, // 平多
        fill_price: dec!(95),
        fill_size: Decimal::from_f64_retain(r_long).unwrap(),
        fee: dec!(0.1),
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(19 * 3_600_000).unwrap(),
        position_side: Some("long".into()),
    };
    st.on_fill(&mut ctx, close_fill);
    assert_eq!(st.global_f64("flag"), Some(flag_before), "CLOSE- 不计 flag");
    let close_pnl = st.global_f64("close_pnl").expect("close_pnl");
    let expect = r_long * (95.0 - avg_l);
    assert!((close_pnl - expect).abs() < 0.01, "期末清仓盈亏按多头均价口径 {expect}: {close_pnl}");
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let diff: f64 =
        snap.get("stat_v_recon_diff").expect("stat_v_recon_diff").parse().expect("数值");
    assert!(diff.abs() < 0.01, "重建恒等式应逐分闭合: {diff}");
    assert!(snap.contains_key("stat_net_pos_final"), "stat_net_pos_final 应导出");
    assert!(snap.contains_key("stat_max_net_notional"), "stat_max_net_notional 应导出");
    assert!(snap.contains_key("stat_q0"), "stat_q0 应导出");
    assert_eq!(snap.get("stat_flag_min").map(|s| s.as_str()), Some("-2"));
}

#[test]
fn test_notify_halt_and_stall_flags() {
    // 042: 引擎 halted/stall 通知的取值口径 —— Lua 全局 fatal 与 _RICOW_STATE 键
    let src = "fatal = 0\n";
    let mut st = LuaStrategy::from_source(src, config(src, &[])).expect("编译");
    assert!(!st.halted(), "未置标记 → false");
    assert_eq!(st.stall_bars(), None, "未暴露停摆计数 → None");

    // state 表路径 (ctx:state_set 持久化键; 重启后经 state_restore 注回也应识别)
    st.state_restore(vec![("halted".into(), "1".into()), ("stat_stall_bars".into(), "7".into())]);
    assert!(st.halted(), "_RICOW_STATE['halted']='1' → true");
    assert_eq!(st.stall_bars(), Some(7));

    // 全局路径 (策略运行期置 fatal = 1)
    let src2 = "fatal = 1\n";
    let st2 = LuaStrategy::from_source(src2, config(src2, &[])).expect("编译");
    assert!(st2.halted(), "全局 fatal=1 → true");
}

// ============================================================================
// 合约做空香农网格 (shannon_short_grid_futures) 集成测试
// 用虚拟香农网格做多的方式管理实际空头: 启动即市价卖出开空一半资金(不等信号),
// 虚拟账本恒 1:1 计算挂单量, 平衡价上下挂平空/开空限价单, 任一成交后立即更新
// 平衡价并全撤重挂两侧(事件模型, 复用 settle_univ2/run_univ2 时序)。
// 杠杆 1x。母本 = shannon_grid_futures 测试时序(univ2 tf 风格: 平段 bar TR=2 → ATR=2)。
// ============================================================================

const SHANNON_SHORT_GRID_FUT: &str =
    include_str!("../../../strategies/futures/shannon_short_grid_futures.lua");

/// f64 OHLC 小时 bar。
fn sg_bar(hour: i64, o: f64, h: f64, l: f64, c: f64) -> Kline {
    let ms = hour * 3_600_000;
    let d = |v: f64| Decimal::from_f64_retain(v).unwrap();
    Kline {
        open_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap(),
        open: d(o),
        high: d(h),
        low: d(l),
        close: d(c),
        volume: Decimal::ONE,
        close_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms + 3_599_999).unwrap(),
    }
}

/// 平段 bar(open=100, high=101, low=99, close=100): TR 恒 2 → ATR=2 → 间距=2。
fn sg_flat(hour: i64) -> Kline {
    sg_bar(hour, 100.0, 101.0, 99.0, 100.0)
}

/// 阶梯 bar: open=close=c, high=c+0.5, low=c−0.5 (TR=1)。
fn sg_step(hour: i64, c: f64) -> Kline {
    sg_bar(hour, c, c + 0.5, c - 0.5, c)
}

/// 主序列: n 根平段(建仓在此完成) + 自定义阶梯段。
fn sg_main(n_flat: usize, steps: &[f64]) -> Vec<Kline> {
    let mut bars: Vec<Kline> = (0..n_flat as i64).map(sg_flat).collect();
    for (i, c) in steps.iter().enumerate() {
        bars.push(sg_step(n_flat as i64 + i as i64, *c));
    }
    bars
}

/// 做空网格配置(1x isolated 默认口径, 主时钟/ATR 全 1h 同周期走 primary)。
fn sg_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("interval", ConfigValue::String("1h".into())),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("min_notional", ConfigValue::Float(5.0)),
        ("fee_side", ConfigValue::Float(0.0005)),
        ("invest_cash", ConfigValue::Float(10000.0)),
    ];
    params.extend_from_slice(extra);
    let mut cfg = config(SHANNON_SHORT_GRID_FUT, &params);
    cfg.market = "futures".into();
    cfg.position_mode = "hedge".into();
    cfg.backtest = Some(crate::config::BacktestToml { leverage: Some(2.0), ..Default::default() });
    cfg
}

/// 收集全部订单(展平)。
fn sg_all_orders(orders: &[Vec<Vec<OrderRequest>>]) -> Vec<OrderRequest> {
    orders.iter().flatten().flatten().cloned().collect()
}

#[test]
fn test_short_grid_builds_immediately_and_hangs_two_sided() {
    // 启动即建仓(2x): 总资金 = 10000×2 = 20000, 首个 ATR 就绪 bar 市价卖出开空一半 = 100@100
    // (名义 10000, 不超现金), 成交价 = 第一平衡价; 建仓后立即重挂: 平衡价±间距(2)
    // → 平空限价 98 / 开空限价 102, 量按 1:1 恢复公式。
    let (orders, mut ctx, mut st) = run_univ2(sg_cfg(&[]), &sg_main(20, &[]), None);
    assert_eq!(st.global_f64("fatal"), Some(0.0), "正常启动不得停机");
    assert_eq!(st.global_f64("fill_count"), Some(1.0), "恰有建仓 1 笔成交");
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓成交价 = 第一平衡价");
    assert_eq!(st.global_f64("flag"), Some(0.0), "建仓不计 flag");
    let all = sg_all_orders(&orders);
    let mkt: Vec<_> = all.iter().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(mkt.len(), 1, "建仓 = 一笔市价卖出");
    assert_eq!(mkt[0].side, OrderSide::Sell);
    assert_eq!(mkt[0].position_side.as_deref(), Some("short"), "建仓单必须带 short 方向仓");
    // 真实空头 = 100(总资金一半 1 万 / 价格 100), 与引擎逐分一致。
    let eng = ctx
        .position_directional("ETHUSDT", OrderSide::Sell)
        .map(|p| p.size.to_f64().unwrap())
        .unwrap_or(0.0);
    assert!((st.global_f64("m_pos").unwrap() - eng).abs() < 1e-6, "m_pos 应与引擎空头一致");
    assert!((eng - 100.0).abs() < 1e-6, "建仓空头 = 总资金一半/价格 = 100: {eng}");
    // 建仓重挂: 两侧限价单 @98(平空) / @102(开空)。
    let limits: Vec<_> =
        all.iter().filter(|o| o.order_type == OrderType::Limit && o.price.is_some()).collect();
    assert!(limits.len() >= 2, "建仓后应挂两侧限价单: {}", limits.len());
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有平空限价单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有开空限价单");
    assert_eq!(buy.price, Some(dec!(98)), "平空挂价 = 平衡价 − 间距(2)");
    assert_eq!(sell.price, Some(dec!(102)), "开空挂价 = 平衡价 + 间距(2)");
    // 1:1 恢复量: C=9995(总资金一半扣建仓 taker 费 5), Q=100 →
    //   平空 q = (C − Q×98)/(98×(2+f)) ≈ 0.9946; 开空 q = (Q×102 − C)/(102×(2−f)) ≈ 1.0052。
    let c0 = 10000.0 - 5.0;
    let q_buy_exp = (c0 - 100.0 * 98.0) / (98.0 * (2.0 + 0.0005));
    let q_sell_exp = (100.0 * 102.0 - c0) / (102.0 * (2.0 - 0.0005));
    assert!(
        (buy.size.to_f64().unwrap() - q_buy_exp).abs() < 1e-4,
        "平空量应=1:1 恢复量 {q_buy_exp}: {}",
        buy.size
    );
    assert!(
        (sell.size.to_f64().unwrap() - q_sell_exp).abs() < 1e-4,
        "开空量应=1:1 恢复量 {q_sell_exp}: {}",
        sell.size
    );
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let dp: f64 = snap["stat_ledger_diff_pos"].parse().unwrap();
    let dr: f64 = snap["stat_v_recon_diff"].parse().unwrap();
    assert!(dp.abs() < 0.01, "账本核对误差 < 0.01: {dp}");
    assert!(dr.abs() < 0.01, "虚拟账本重建核对误差 < 0.01: {dr}");
    assert_eq!(snap["stat_eff_leverage"], "2.0", "生效杠杆应导出 2.0");
}

#[test]
fn test_short_grid_never_touches_long_side() {
    // 全程只下 SHORT 侧: 振荡行情跑完, 不得出现任何 position_side=long 订单。
    let steps = [97.0, 101.0, 97.0, 101.0, 103.0, 99.0];
    let (orders, _ctx, _st) = run_univ2(sg_cfg(&[]), &sg_main(20, &steps), None);
    assert!(
        sg_all_orders(&orders).iter().all(|o| o.position_side.as_deref() != Some("long")),
        "纯做空策略不得有任何 long 侧订单"
    );
}

#[test]
fn test_short_grid_fill_updates_balance_and_rehangs() {
    // 建仓@100 → 振荡段: 平空/开空限价单交替成交(跳空 bar 引擎按开盘价成交, 合法漂移),
    // 每笔成交后平衡价 := 成交价 + 立即全撤重挂两侧。flag 口径闭合 + 引擎逐分对账。
    let steps = [97.0, 101.0, 97.0, 101.0];
    let (orders, mut ctx, mut st) = run_univ2(sg_cfg(&[]), &sg_main(20, &steps), None);
    assert!(st.global_f64("fill_count").unwrap() >= 3.0, "建仓 + 至少一笔平空一笔开空");
    assert!(st.global_f64("buy_count").unwrap() >= 1.0, "跌穿应有平空成交");
    assert!(st.global_f64("sell_count").unwrap() >= 2.0, "建仓 + 涨穿应有开空成交");
    assert_eq!(st.global_f64("flag"), Some(0.0), "一平一空一开一空 flag 归零");
    assert!(st.global_f64("rehang_count").unwrap() >= 3.0, "每笔成交后都应重挂");
    // 平衡价 := 末笔成交价(跳空 bar 按开盘 101 成交)。
    let bal = st.global_f64("balance_price").unwrap();
    assert!((bal - 97.0).abs() < 1e-6 || (bal - 101.0).abs() < 1e-6, "平衡价 := 成交价: {bal}");
    // 真实空头与引擎逐分一致(1:1 恒等受跳空合法漂移, 由重建恒等式核对兜底)。
    let eng = ctx
        .position_directional("ETHUSDT", OrderSide::Sell)
        .map(|p| p.size.to_f64().unwrap())
        .unwrap_or(0.0);
    assert!((st.global_f64("m_pos").unwrap() - eng).abs() < 1e-6, "m_pos 与引擎空头一致");
    // 全部订单 = 市价(建仓) 或 short 侧限价(撤单指令除外)。
    for b in sg_all_orders(&orders) {
        if b.action == OrderAction::CancelPending {
            continue;
        }
        assert_eq!(b.position_side.as_deref(), Some("short"));
        assert!(matches!(b.order_type, OrderType::Market | OrderType::Limit));
    }
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let dp: f64 = snap["stat_ledger_diff_pos"].parse().unwrap();
    let dr: f64 = snap["stat_v_recon_diff"].parse().unwrap();
    assert!(dp.abs() < 0.01, "账本核对: {dp}");
    assert!(dr.abs() < 0.01, "虚拟账本重建恒等式: {dr}");
}

#[test]
fn test_short_grid_flag_spacing_amplified() {
    // 连续上涨: 建仓@100 → 涨穿 102 开空#1(flag+1, 平衡=102) → 涨穿 104 开空#2(flag+2)。
    // |flag|=2 → 趋势侧(开空/卖)间距 ×1.2^(2−1), 平空(买)侧不变。
    let steps = [102.5, 104.5];
    let (_orders, _ctx, st) = run_univ2(sg_cfg(&[]), &sg_main(20, &steps), None);
    assert_eq!(st.global_f64("flag"), Some(2.0), "两笔开空 → flag=+2");
    assert_eq!(st.global_f64("flag_max"), Some(2.0));
    assert_eq!(
        st.global_f64("buy_count").unwrap_or(0.0),
        0.0,
        "上涨段不得有平空成交(low 均高于买价)"
    );
    let bs = st.global_f64("last_buy_spacing").unwrap();
    let ss = st.global_f64("last_sell_spacing").unwrap();
    assert!(bs > 0.0, "平空间距应已刷新: {bs}");
    assert!((ss / bs - 1.2).abs() < 1e-6, "flag=+2 → 开空间距 ×1.2, 平空不变: buy={bs} sell={ss}");
}

#[test]
fn test_short_grid_liquidation_halts() {
    // 1x isolated 半仓做空: 均值成本≈100, 钱包≈4985 → 线性穿越爆仓价 ≈ 198
    // → 涨到 250 必触发 LIQ-…-short → 停机不再交易。
    let steps = [150.0, 200.0, 250.0, 250.0, 250.0];
    let (orders, mut ctx, mut st) = run_univ2(sg_cfg(&[]), &sg_main(20, &steps), None);
    assert!(st.global_f64("liq_count").unwrap_or(0.0) >= 1.0, "应识别引擎 LIQ- 强平 fill");
    assert_eq!(st.global_f64("fatal"), Some(1.0), "爆仓应置停机标记");
    assert_eq!(st.global_f64("m_pos").unwrap(), 0.0, "强平后引擎空头清零, m_pos 同步");
    let eng = ctx
        .position_directional("ETHUSDT", OrderSide::Sell)
        .map(|p| p.size.to_f64().unwrap())
        .unwrap_or(0.0);
    assert!(eng.abs() < 1e-6, "引擎侧空头应被清空: {eng}");
    let liq_pnl = st.global_f64("liq_pnl").expect("liq_pnl");
    assert!(liq_pnl < 0.0, "爆仓损失为负: {liq_pnl}");
    // 停机后不再产生任何订单(全撤指令除外)。
    let tail: Vec<_> = orders[orders.len() - 2..]
        .iter()
        .flatten()
        .flatten()
        .filter(|o| o.action != OrderAction::CancelPending)
        .collect();
    assert!(tail.is_empty(), "爆仓停机后不得再下单");
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    assert_eq!(snap["halted"], "1", "halted 应持久化(重启不复活)");
    assert!(snap.contains_key("stat_liq_dist_min"), "stat_liq_dist_min 应导出");
    assert!(snap.contains_key("stat_liq_count"), "stat_liq_count 应导出");
}

#[test]
fn test_short_grid_thin_spacing_halts() {
    // atr_mult≈0 + 禁用下限 → 生效间距 < 4×费率 → 成本门槛 FATAL, 不建仓不交易。
    let (orders, _ctx, st) = run_univ2(
        sg_cfg(&[
            ("atr_mult", ConfigValue::Float(0.0001)),
            ("min_spacing_pct", ConfigValue::Float(-1.0)),
        ]),
        &sg_main(20, &[]),
        None,
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "间距低于成本门槛 → 停机");
    assert!(orders.iter().all(|b| b.is_empty()), "停机不得下单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0));
}

#[test]
fn test_short_grid_state_snapshot() {
    // 状态快照(断点续接最小必需项): balance_price / built / pending_entry / invested0。
    let (_orders, _ctx, st) = run_univ2(sg_cfg(&[]), &sg_main(20, &[]), None);
    let snap = st.state_snapshot();
    let get = |k: &str| snap.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    assert_eq!(get("balance_price").as_deref(), Some("100.0000000000"));
    assert_eq!(get("built").as_deref(), Some("1"));
    assert_eq!(get("pending_entry").as_deref(), Some("0"));
    assert_eq!(get("invested0").as_deref(), Some("10000.00"));
    assert!(get("v_cash").is_some() && get("v_pos").is_some(), "虚拟账本应持久化");
    assert!(
        get("buy_notional").is_some() && get("sell_notional").is_some(),
        "重建核对累计项应持久化"
    );
}

// ============================================================================
// 现货线性仓位网格 (linear_position_grid) 集成测试
// 三阶段: 逐步建仓(低于 start_price 分批市价买) → 信号网格(主时钟 EMA3/6 金叉/死叉 +
// 现价距上次成交价 > 门控间距 → 市价落位线性仓位, 不挂限价单) → 可选动态止盈。
// tf 用 tf_bars(TR 恒 2 → ATR=2, 1h 序列在主 bar15 起满 15 根可见 → ATR 就绪),
// 门控间距 = atr_mult×ATR = 1×2 = 2。主 bar 每小时一根(bar_at_f, ts 差 3600s →
// build_interval_hours=1 每根一批; 建仓不依赖 ATR/EMA)。
// ============================================================================

const LINEAR_POSITION_GRID: &str =
    include_str!("../../../strategies/spot/linear_position_grid.lua");

fn lgrid_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("start_price", ConfigValue::Float(100.0)),
        ("p_low", ConfigValue::Float(80.0)),
        ("p_high", ConfigValue::Float(120.0)),
        ("pos_low_pct", ConfigValue::Float(0.7)),
        ("pos_high_pct", ConfigValue::Float(0.3)),
        ("invest_cash", ConfigValue::Float(10000.0)),
        ("build_steps", ConfigValue::Integer(10)),
        ("build_interval_hours", ConfigValue::Float(1.0)),
        ("interval", ConfigValue::String("1m".into())),
        ("ema_fast", ConfigValue::Integer(3)),
        ("ema_slow", ConfigValue::Integer(6)),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("atr_mult", ConfigValue::Float(1.0)),
        ("min_spacing_pct", ConfigValue::Float(0.004)),
        ("min_notional", ConfigValue::Float(5.0)),
        ("fee_side", ConfigValue::Float(0.001)),
        ("out_of_range", ConfigValue::String("exit".into())),
        ("tp_min_profit_pct", ConfigValue::Float(0.1)),
        ("tp_dd_atr_mult", ConfigValue::Float(3.0)),
    ];
    params.extend_from_slice(extra);
    config(LINEAR_POSITION_GRID, &params)
}

/// 价格序列 → 每小时一根平 bar。
fn lgrid_main(prices: &[f64]) -> Vec<Kline> {
    prices.iter().enumerate().map(|(h, p)| bar_at_f(h as i64, *p)).collect()
}

/// 取某 bar 全部批次里首个满足条件的下单(限价/市价、方向)。
fn lgrid_order(
    out: &[Vec<Vec<OrderRequest>>],
    bar: usize,
    side: OrderSide,
    ty: OrderType,
) -> Option<OrderRequest> {
    out[bar]
        .iter()
        .flatten()
        .find(|o| o.action == OrderAction::Place && o.side == side && o.order_type == ty)
        .cloned()
}

/// 布尔状态经 _RICOW_STATE 快照读取(save_state 持久化 "1"/"0")。
fn lgrid_state(st: &LuaStrategy, key: &str) -> Option<String> {
    st.state_snapshot().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

#[test]
fn test_linear_grid_param_fatal() {
    // 区间非法 p_low≥p_high → FATAL 零单。
    let (orders, _ctx, st) = run_univ2(
        lgrid_cfg(&[("p_low", ConfigValue::Float(130.0))]),
        &lgrid_main(&[99.0; 20]),
        Some(tf_bars(64)),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "p_low≥p_high 应 FATAL");
    assert!(orders.iter().all(|b| b.iter().all(|x| x.is_empty())), "FATAL 后不得下单");
}

#[test]
fn test_linear_grid_start_price_out_of_range_fatal() {
    // start_price 不在区间内 → FATAL。
    let (orders, _ctx, st) = run_univ2(
        lgrid_cfg(&[("start_price", ConfigValue::Float(150.0))]),
        &lgrid_main(&[99.0; 20]),
        Some(tf_bars(64)),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "start_price 越界应 FATAL");
    assert!(orders.iter().all(|b| b.iter().all(|x| x.is_empty())));
}

#[test]
fn test_linear_grid_pct_inverted_fatal() {
    // pos_high_pct ≥ pos_low_pct → FATAL。
    let (orders, _ctx, st) = run_univ2(
        lgrid_cfg(&[("pos_high_pct", ConfigValue::Float(0.8))]),
        &lgrid_main(&[99.0; 20]),
        Some(tf_bars(64)),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "仓位占比倒置应 FATAL");
    assert!(orders.iter().all(|b| b.iter().all(|x| x.is_empty())));
}

#[test]
fn test_linear_grid_thin_spacing_halts() {
    // atr_mult≈0 + 禁用下限 → 生效间距 < 4×费率 → 成本门槛 FATAL(建满后首次重挂时判)。
    let (_orders, _ctx, st) = run_univ2(
        lgrid_cfg(&[
            ("atr_mult", ConfigValue::Float(0.0001)),
            ("min_spacing_pct", ConfigValue::Float(-1.0)),
        ]),
        &lgrid_main(&[99.0; 16]),
        Some(tf_bars(64)),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "间距低于成本门槛 → 停机");
    assert_eq!(lgrid_state(&st, "built").as_deref(), Some("1"), "停机前已建满");
    assert_eq!(st.global_f64("sell_count"), Some(0.0), "网格从未挂出");
}

#[test]
fn test_linear_grid_no_build_when_price_above_start() {
    // 价格 ≥ start_price 不建仓(独立建仓门槛)。
    let (orders, _ctx, st) =
        run_univ2(lgrid_cfg(&[]), &lgrid_main(&[105.0; 20]), Some(tf_bars(64)));
    assert!(orders.iter().all(|b| b.iter().all(|x| x.is_empty())), "价高于 start 不建仓");
    assert_eq!(st.global_f64("fill_count"), Some(0.0));
    assert_eq!(st.global_f64("build_done_steps"), Some(0.0));
}

#[test]
fn test_linear_grid_build_one_batch_per_bar_and_no_grid_orders_before_atr() {
    // 99<100: 每小时一批; 建仓期(bar0~8)只发市价买、不挂任何限价网格单; 10 批建满。
    let (out, _ctx, st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&[99.0; 15]), Some(tf_bars(64)));
    assert_eq!(st.global_f64("build_done_steps"), Some(10.0), "应建满 10 批");
    assert_eq!(st.global_f64("buy_count"), Some(10.0), "建仓 10 笔市价买");
    assert_eq!(st.global_f64("sell_count"), Some(0.0), "建仓期不卖");
    assert_eq!(lgrid_state(&st, "built").as_deref(), Some("1"));
    // 建仓期(bar0~8)不得出现限价单。
    for (b, batches) in out.iter().enumerate().take(9) {
        for batch in batches {
            for o in batch {
                assert_ne!(o.order_type, OrderType::Limit, "建仓期(bar{b})不得挂限价网格单");
            }
        }
    }
    // 建仓目标: w(100)=0.5 → T=0.5×10000/100=50 币, 每批 5 币。
    let q = st.global_f64("build_filled_qty").unwrap();
    assert!((q - 50.0).abs() < 1e-6, "建仓总量应=50 币: {q}");
}

#[test]
fn test_linear_grid_build_interval_defer() {
    // build_interval_hours=2: 相邻 bar(间隔 1h)不足 2h → 该批顺延: bar0,2,...,14 → 8 批。
    let (_out, _ctx, st) = run_univ2(
        lgrid_cfg(&[("build_interval_hours", ConfigValue::Float(2.0))]),
        &lgrid_main(&[99.0; 15]),
        Some(tf_bars(64)),
    );
    assert_eq!(st.global_f64("build_done_steps"), Some(8.0), "间隔 2h 应顺延至 8 批");
    assert_eq!(lgrid_state(&st, "built").as_deref(), Some("0"), "未满 10 批不进网格");
}

#[test]
fn test_linear_grid_rehang_prices_and_sizes() {
    // 建满(bar9, 10批×5币@99 → Q=50, 现金=10000−10×(495+0.495)=5045.05)。ATR 就绪(bar15)重挂:
    // ref=99, Δ=1×2=2 → 买@97 / 卖@101。落位公式(腿价估值, w(97)=0.7−0.4×17/40=0.53, w(101)=0.49):
    //   买 q=(w·E−Q·p)/(p(1+wf)): E=50×97+5045.05=9895.05 → q=(5244.38−4850)/(97×1.00053)=4.0636;
    //   卖 q=(Q·p−w·E)/(p(1−wf)): E=50×101+5045.05=10095.05 → q=(5050−4946.57)/(101×0.99951)=1.0245。
    let (out, _ctx, _st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&[99.0; 16]), Some(tf_bars(64)));
    let buy = lgrid_order(&out, 15, OrderSide::Buy, OrderType::Limit).expect("bar15 应挂限价买单");
    let sell =
        lgrid_order(&out, 15, OrderSide::Sell, OrderType::Limit).expect("bar15 应挂限价卖单");
    assert!((buy.price.unwrap().to_f64().unwrap() - 97.0).abs() < 1e-6, "买=ref99−Δ2=97");
    assert!((sell.price.unwrap().to_f64().unwrap() - 101.0).abs() < 1e-6, "卖=ref99+Δ2=101");
    assert!((buy.size.to_f64().unwrap() - 4.0636).abs() < 5e-3, "买量=落位公式: {}", buy.size);
    assert!((sell.size.to_f64().unwrap() - 1.0245).abs() < 5e-3, "卖量=落位公式: {}", sell.size);
}

#[test]
fn test_linear_grid_buy_fill_updates_ref_and_rehangs() {
    // 重挂后(bar15 买@97)价格下探 97 触及买腿 → 成交, ref:=97, 全撤重挂(新买@95/卖@99)。
    let mut prices = vec![99.0; 17];
    prices.push(97.0); // bar17 触及买腿
    prices.push(97.0);
    let (out, _ctx, st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&prices), Some(tf_bars(64)));
    let buy = lgrid_order(&out, 17, OrderSide::Buy, OrderType::Limit).expect("bar17 应重挂买单");
    assert!((buy.price.unwrap().to_f64().unwrap() - 95.0).abs() < 1e-6, "新买=97−2=95");
    assert!((st.global_f64("ref_price").unwrap() - 97.0).abs() < 1e-6, "ref := 成交价 97");
}

#[test]
fn test_linear_grid_uptrend_no_deadlock() {
    // 单边上涨死锁回归(用户报"成交量少了很多"真因): 建满@99 后价格持续上行(步长 2 > Δ=ATR×1),
    // 纯成交驱动重挂会死锁(卖腿被穿越守卫抑制、再无成交事件) → 整段零成交。
    // 修复后偏离触发重挂 + 重锚: 上行段必须持续成交(网格跟随价格上移 + 线性减仓)。
    let mut prices = vec![99.0; 17]; // bar0~9 建满, bar10~16 稳住(ATR bar15 就绪)
    for p in (101..140).step_by(2) {
        prices.push(p as f64); // bar17~ 单边上行 101→139
    }
    let (_out, _ctx, st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&prices), Some(tf_bars(64)));
    let sells = st.global_f64("sell_count").unwrap();
    assert!(sells > 5.0, "单边上行段网格必须持续成交减仓(旧版死锁=0 卖): sells={sells}");
    assert!(st.global_f64("halted").is_none_or(|h| h == 0.0), "上行段不得停机");
}

#[test]
fn test_linear_grid_position_invariant_every_fill() {
    // 用户核心口径回归: 建满@99 稳住(bar15 ATR 就绪挂腿)后渐变 98→96(触发买@97 成交)回升 97→…→101
    // (触发卖成交), 断言落位最大误差 < 1e-6 + 现金未到 p_low 不耗尽。
    let mut prices = vec![99.0; 17];
    for p in [
        98.0, 97.0, 96.0, 95.0, 96.0, 97.0, 98.0, 99.0, 100.0, 101.0, 102.0, 101.0, 100.0, 99.0,
        98.0, 97.0, 96.0,
    ] {
        prices.push(p);
    }
    let (_out, mut ctx, _st) = run_univ2(
        lgrid_cfg(&[
            ("p_low", ConfigValue::Float(10.0)),
            ("p_high", ConfigValue::Float(400.0)),
            ("pos_low_pct", ConfigValue::Float(1.0)),
            ("pos_high_pct", ConfigValue::Float(0.1)),
        ]),
        &lgrid_main(&prices),
        Some(tf_bars(64)),
    );
    let mut st = _st;
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let err: f64 = snap.get("stat_pos_err_max").unwrap().parse().unwrap();
    let cash: f64 = snap.get("stat_cash_final").unwrap().parse().unwrap();
    let fills: f64 = snap.get("stat_fill_count").unwrap().parse().unwrap();
    assert!(fills > 14.0, "渐变路径应产生多笔网格成交(>10 建仓笔): {fills}");
    assert!(err < 1e-6, "每次成交后落位误差须≈0(用户口径), 实际 {err}");
    assert!(cash > 0.0, "价格未到 10 现金绝不耗尽: cash={cash}");
}

#[test]
fn test_linear_grid_exit_out_of_range_halts_keeps_position() {
    // 建仓期(bar3)价格 79 < p_low 80 → exit 停机, 撤单不清仓, 保留已建 3 批持仓。
    let mut prices = vec![99.0; 3];
    prices.push(79.0);
    prices.extend(vec![79.0; 5]);
    let (out, _ctx, st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&prices), Some(tf_bars(64)));
    assert_eq!(lgrid_state(&st, "halted").as_deref(), Some("1"), "出界 exit 应停机");
    assert_eq!(st.global_f64("build_done_steps"), Some(3.0), "停机前建 3 批");
    assert!(
        out[3].iter().flatten().any(|o| o.action == OrderAction::CancelPending),
        "exit 应撤光在途挂单"
    );
    for (b, batches) in out.iter().enumerate().skip(4) {
        assert!(
            batches.iter().flatten().all(|o| o.action != OrderAction::Place),
            "停机后不再下单(bar{b})"
        );
    }
    // 不清仓: 已建 3 批持仓保留(每批 5 币)。
    assert!((st.global_f64("m_pos").unwrap() - 15.0).abs() < 1e-6, "exit 不得清仓");
}

#[test]
fn test_linear_grid_wait_out_of_range_resumes_and_reanchors() {
    // wait: 出界暂停, 回界内恢复 + ref 重锚现价。建满(bar0~9)后 bar10/11 出界, bar12 回界内。
    let mut prices = vec![99.0; 10];
    prices.push(79.0); // bar10 出界 → wait 暂停
    prices.push(79.0); // bar11 仍界外
    prices.push(95.0); // bar12 回界内 → 恢复
    prices.push(95.0);
    let (_out, _ctx, st) = run_univ2(
        lgrid_cfg(&[("out_of_range", ConfigValue::String("wait".into()))]),
        &lgrid_main(&prices),
        Some(tf_bars(64)),
    );
    assert_eq!(lgrid_state(&st, "halted").as_deref(), Some("0"), "wait 不得停机");
    assert_eq!(lgrid_state(&st, "paused").as_deref(), Some("0"), "回界内应解除暂停");
    assert!((st.global_f64("ref_price").unwrap() - 95.0).abs() < 1e-6, "ref 重锚现价 95");
}

#[test]
fn test_linear_grid_dynamic_take_profit_clears() {
    // 宽区间 + 高仓位(几乎不减仓) → 冲高 380 积累巨额浮盈; 回撤到 370(dd=10 ≥ 3×ATR=6)
    // 且 ATR 就绪(bar15) → 止盈: 撤光 + 市价清仓 + 停机。
    let mut prices = vec![99.0; 10];
    prices.extend(vec![380.0; 5]); // bar10~14 冲高(区间内), peak=380
    prices.push(370.0); // bar15 ATR 就绪 + 回撤达标 + 盈利达标 → 止盈
    prices.push(370.0);
    let (out, _ctx, st) = run_univ2(
        lgrid_cfg(&[
            ("p_high", ConfigValue::Float(400.0)),
            ("pos_low_pct", ConfigValue::Float(0.95)),
            ("pos_high_pct", ConfigValue::Float(0.9)),
        ]),
        &lgrid_main(&prices),
        Some(tf_bars(64)),
    );
    assert_eq!(lgrid_state(&st, "tp_closed").as_deref(), Some("1"), "动态止盈应触发清仓");
    assert_eq!(lgrid_state(&st, "halted").as_deref(), Some("1"), "止盈后停机");
    assert!((st.global_f64("peak_price").unwrap() - 380.0).abs() < 1e-6, "peak 记录最高价");
    assert!(
        out[15].iter().flatten().any(|o| o.action == OrderAction::Place
            && o.side == OrderSide::Sell
            && o.order_type == OrderType::Market),
        "止盈应市价清仓"
    );
    // 清仓成交照常入账(halted 后只记账不产单): 模型持仓应归零。
    assert!(st.global_f64("m_pos").unwrap() < 1e-6, "止盈清仓后持仓应归零");
}

#[test]
fn test_linear_grid_take_profit_profit_gate_blocks() {
    // 回撤达标但盈利未达 10%(建仓期手续费磨损 ≈ −0.5%) → 不止盈。
    let mut prices = vec![99.0; 10];
    prices.extend(vec![100.0; 6]); // 微涨: 盈利远小于 10%
    prices.push(93.0); // bar16: dd=7 ≥ 6 但盈利不足 → 不触发
    let (_out, _ctx, st) = run_univ2(lgrid_cfg(&[]), &lgrid_main(&prices), Some(tf_bars(64)));
    assert_eq!(lgrid_state(&st, "tp_closed").as_deref(), Some("0"), "盈利不达标不得止盈");
}

#[test]
fn test_linear_grid_take_profit_disabled() {
    // tp_min_profit_pct=0 → 禁用止盈, 同样冲高回撤也不清仓。
    let mut prices = vec![99.0; 10];
    prices.extend(vec![380.0; 5]);
    prices.push(370.0);
    prices.push(370.0);
    let (_out, _ctx, st) = run_univ2(
        lgrid_cfg(&[
            ("p_high", ConfigValue::Float(400.0)),
            ("pos_low_pct", ConfigValue::Float(0.95)),
            ("pos_high_pct", ConfigValue::Float(0.9)),
            ("tp_min_profit_pct", ConfigValue::Float(0.0)),
        ]),
        &lgrid_main(&prices),
        Some(tf_bars(64)),
    );
    assert_eq!(lgrid_state(&st, "tp_closed").as_deref(), Some("0"), "止盈禁用不得清仓");
}

#[test]
fn test_linear_grid_ledger_and_snapshot() {
    // 账本交叉核对(误差<0.01) + 断点续接键持久化。
    let (_out, mut ctx, st) =
        run_univ2(lgrid_cfg(&[]), &lgrid_main(&[99.0; 16]), Some(tf_bars(64)));
    let mut st = st;
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let dc: f64 = snap.get("stat_ledger_diff_cash").unwrap().parse().unwrap();
    let dp: f64 = snap.get("stat_ledger_diff_pos").unwrap().parse().unwrap();
    assert!(dc.abs() < 0.01 && dp.abs() < 0.01, "账本与引擎逐分对齐: cash差{dc} 仓差{dp}");
    assert!(snap.contains_key("ref_price"), "ref_price 应持久化");
    assert!(snap.contains_key("peak_price"), "peak_price 应持久化");
    assert_eq!(snap.get("built").map(|s| s.as_str()), Some("1"), "built 应持久化");
    assert!(snap.contains_key("build_done_steps"), "build_done_steps 应持久化");
    assert!(snap.contains_key("last_step_ts"), "last_step_ts 应持久化");
}

// ============================================================================
// 现货智能持仓 (smart_position) 集成测试 (060)
// 周期调仓(时间闸+阈值闸) + Simple Earn 活期理财: 闲置现金腿申购、现金不足先赎回
// 下周期再调。市价单即时成交(bar.open), 默认 taker 10bps = fee_side 默认 0.001 同源。
// ============================================================================

const SMART_POSITION: &str = include_str!("../../../strategies/spot/smart_position.lua");

fn sp_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("target_pct", ConfigValue::Float(0.5)),
        ("cycle_days", ConfigValue::Float(1.0)),
        ("rebalance_threshold_pct", ConfigValue::Float(2.0)),
        ("earn_enabled", ConfigValue::Float(1.0)),
        ("earn_buffer_pct", ConfigValue::Float(0.5)),
        ("earn_apr", ConfigValue::Float(0.02)),
        ("min_notional", ConfigValue::Float(5.0)),
        ("fee_side", ConfigValue::Float(0.001)),
        ("interval", ConfigValue::String("1h".into())),
    ];
    params.extend_from_slice(extra);
    config(SMART_POSITION, &params)
}

/// 逐 bar 下单批次 → 全序列中某方向的市价单列表。
fn sp_market_orders(out: &[Vec<Vec<OrderRequest>>], side: OrderSide) -> Vec<(usize, OrderRequest)> {
    let mut v = Vec::new();
    for (i, bar) in out.iter().enumerate() {
        for batch in bar {
            for o in batch {
                if o.action == OrderAction::Place
                    && o.side == side
                    && o.order_type == OrderType::Market
                {
                    v.push((i, o.clone()));
                }
            }
        }
    }
    v
}

fn sp_snap(st: &mut LuaStrategy, ctx: &mut BacktestContext) -> HashMap<String, String> {
    st.on_stop(ctx);
    st.state_snapshot().into_iter().collect()
}

fn sp_f64(snap: &HashMap<String, String>, key: &str) -> f64 {
    snap.get(key).unwrap_or_else(|| panic!("缺失 {key}")).parse().unwrap()
}

#[test]
fn test_smart_position_build_then_earn_subscribe() {
    // bar0 首周期: 空仓偏离 -50pp → 市价买至 50% 目标; bar24 二周期: 阈内不调仓,
    // 闲置现金(留 0.5% 权益缓冲)入理财。48 根 1h 平 bar @100。
    let bars: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    let (out, mut ctx, mut st) = run_univ2(sp_cfg(&[]), &bars, None);

    let buys = sp_market_orders(&out, OrderSide::Buy);
    let sells = sp_market_orders(&out, OrderSide::Sell);
    assert_eq!(buys.len(), 1, "全程应恰 1 笔市价买(bar0 建仓到位)");
    assert_eq!(buys[0].0, 0);
    assert!(sells.is_empty(), "不应有卖单");
    let size = buys[0].1.size.to_f64().unwrap();
    assert!((size - 5000.0 / (100.0 * 1.001)).abs() < 1e-3, "买量=Δ/(p(1+f)): {size}");

    let earn = ctx.earn_balance("USDT").unwrap().to_f64().unwrap();
    let cash = ctx.balance("USDT").unwrap().to_f64().unwrap();
    // 权益 ≈ 9995(扣建仓费 5.0): 缓冲 0.5% ≈ 50, 其余现金全进理财。
    assert!(earn > 4900.0 && earn < 5000.0, "理财应吸收闲置现金: {earn}");
    assert!(cash > 40.0 && cash < 60.0, "现货现金应≈权益×0.5%: {cash}");
    let eq = ctx.mark_to_market(&dec!(100)).to_f64().unwrap();
    assert!((eq - 9995.005).abs() < 0.5, "权益 = 现金+市值+理财(仅费损耗): {eq}");

    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_rebalance_count"), 1.0);
    assert_eq!(sp_f64(&snap, "stat_buy_count"), 1.0);
    assert_eq!(sp_f64(&snap, "stat_sell_count"), 0.0);
    assert_eq!(sp_f64(&snap, "stat_pending_redeem_count"), 0.0);
    assert_eq!(sp_f64(&snap, "stat_earn_supported"), 1.0);
    assert!(sp_f64(&snap, "stat_earn_usdt_days") > 0.0, "理财计时应有累计");
    // 账本交叉核对(成交+划转意图 vs 引擎): 逐分对齐。
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_pos").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_earn").abs() < 0.01);
}

#[test]
fn test_smart_position_over_threshold_sell() {
    // 24 根 @100(建仓) → 24 根 @100(理财申购) → 涨至 110: cur≈52.4% 偏离 +2.36pp ≥ 2
    // → 市价卖回目标。
    let mut bars: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    bars.extend((48..96).map(|h| bar_at_f(h as i64, 110.0)));
    let (out, mut ctx, mut st) = run_univ2(sp_cfg(&[]), &bars, None);

    let sells = sp_market_orders(&out, OrderSide::Sell);
    assert_eq!(sells.len(), 1, "涨破阈值应恰 1 笔市价卖(bar48 周期)");
    assert_eq!(sells[0].0, 48);
    let notional = sells[0].1.size.to_f64().unwrap() * 110.0;
    assert!((notional - 248.0).abs() < 5.0, "卖名义≈Δ≈248: {notional}");
    assert_eq!(sp_market_orders(&out, OrderSide::Buy).len(), 1, "买入仍仅建仓一笔");

    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_sell_count"), 1.0);
    // 账本交叉核对: 卖单周期不申购理财(划转 drain 先于成交, 按滚动现金申购会被拒),
    // 下周期现金落账后再申购 → 无被拒划转, 三项逐分对齐。
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_earn").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_pos").abs() < 0.01);
}

#[test]
fn test_smart_position_cash_shortfall_redeems_then_rebalances() {
    // 建仓(bar0)+申购(bar24)后暴跌至 80: 需买入但现货现金不足 → bar48 只提交赎回申请
    // (零订单), bar72 资金就绪才补买。防"要求即时到账"。
    let mut bars: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    bars.extend((48..120).map(|h| bar_at_f(h as i64, 80.0)));
    let (out, mut ctx, mut st) = run_univ2(sp_cfg(&[]), &bars, None);

    // bar48 周期: 无任何订单(赎回走划转队列, 不产订单)。
    assert!(out[48].iter().all(|b| b.is_empty()), "现金不足周期不得下单");
    let buys = sp_market_orders(&out, OrderSide::Buy);
    let bars_seen: Vec<usize> = buys.iter().map(|(i, _)| *i).collect();
    assert_eq!(bars_seen, vec![0, 72], "bar0 建仓 + bar72 赎回后就绪补买");

    let earn = ctx.earn_balance("USDT").unwrap().to_f64().unwrap();
    assert!(earn > 4400.0 && earn < 4600.0, "应赎回差额≈452: {earn}");

    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_pending_redeem_count"), 1.0, "恰一次赎回申请");
    assert_eq!(sp_f64(&snap, "stat_buy_count"), 2.0);
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_earn").abs() < 0.01);
}

#[test]
fn test_smart_position_earn_disabled_no_loop() {
    // 理财关闭 + 价格恒定: 建仓一笔后永不再动(时间闸+阈值闸防循环调仓)。
    let bars: Vec<Kline> = (0..120).map(|h| bar_at_f(h, 100.0)).collect();
    let (out, mut ctx, mut st) =
        run_univ2(sp_cfg(&[("earn_enabled", ConfigValue::Float(0.0))]), &bars, None);
    assert_eq!(sp_market_orders(&out, OrderSide::Buy).len(), 1);
    assert_eq!(sp_market_orders(&out, OrderSide::Sell).len(), 0);
    assert!(out[1..].iter().all(|bar| bar.iter().all(|b| b.is_empty())), "建仓后零订单");
    assert_eq!(ctx.earn_balance("USDT").unwrap(), Decimal::ZERO, "关闭理财不得划转");
    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_earn_enabled"), 0.0);
}

#[test]
fn test_smart_position_param_fatal() {
    // target_pct 越界 → FATAL 停机零订单。
    let bars: Vec<Kline> = (0..24).map(|h| bar_at_f(h, 100.0)).collect();
    let (out, _ctx, st) =
        run_univ2(sp_cfg(&[("target_pct", ConfigValue::Float(1.5))]), &bars, None);
    assert_eq!(st.global_f64("fatal"), Some(1.0), "target_pct>1 应 FATAL");
    assert!(out.iter().all(|bar| bar.iter().all(|b| b.is_empty())), "FATAL 后不得下单");
}

#[test]
fn test_backtest_earn_transfer_preserves_equity() {
    // D4 锚点: 理财划转是资产位置变化, 不是盈亏 —— 申购/赎回前后 mark_to_market 逐分不变;
    // balance() 不混入理财; 超额划转拒绝。
    let cfg = sp_cfg(&[]);
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    let eq0 = ctx.mark_to_market(&dec!(100));
    assert!(ctx.earn_subscribe("USDT", dec!(3000)));
    assert_eq!(ctx.mark_to_market(&dec!(100)), eq0, "申购不得改变权益");
    assert_eq!(ctx.balance("USDT").unwrap(), dec!(7000), "balance 只含现货现金");
    assert_eq!(ctx.earn_balance("USDT"), Some(dec!(3000)));
    assert!(ctx.earn_redeem("USDT", dec!(1500)));
    assert_eq!(ctx.mark_to_market(&dec!(100)), eq0, "赎回不得改变权益");
    assert!(!ctx.earn_redeem("USDT", dec!(8000)), "在理财不足 → 拒绝");
    assert!(!ctx.earn_subscribe("USDT", dec!(99999)), "现货现金不足 → 拒绝");
    assert!(!ctx.earn_subscribe("USDT", Decimal::ZERO), "非正金额 → 拒绝");
}

#[test]
fn test_backtest_earn_unsupported_for_futures() {
    // 合约端无理财概念: earn_balance=None, 划转一律拒绝(策略据此降级由 Lua 侧覆盖)。
    let mut cfg = sp_cfg(&[]);
    cfg.market = "futures".into();
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    assert_eq!(ctx.earn_balance("USDT"), None);
    assert!(!ctx.earn_subscribe("USDT", dec!(100)));
    assert!(!ctx.earn_redeem("USDT", dec!(100)));
}

#[test]
fn test_earn_ops_queue_drains_once() {
    // `_RICOW_EARN_OPS` 入队 → 回调返回后 drain 执行 → **清表**: 同一笔划转不得跨周期重复执行。
    let script = r#"
        done = false
        function on_tick(ctx)
            if not done then
                done = true
                ctx:earn_subscribe("USDT", 100)
            end
            return {}
        end
    "#;
    let cfg = config(script, &[("pair", ConfigValue::String("ETHUSDT".into()))]);
    let mut st = LuaStrategy::from_source(script, cfg.clone()).expect("脚本应编译");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    ctx.step_bar(bar_at_f(0, 100.0));
    st.on_init(&mut ctx);
    let _ = st.on_tick(&mut ctx);
    assert_eq!(ctx.earn_balance("USDT"), Some(dec!(100)), "drain 应执行划转");
    let _ = st.on_tick(&mut ctx);
    assert_eq!(ctx.earn_balance("USDT"), Some(dec!(100)), "队列已清, 不得重复执行");
    assert_eq!(ctx.balance("USDT").unwrap(), dec!(9900));
}

#[test]
fn test_smart_position_report_final_equity_includes_earn_leg() {
    // 回归锚点 (060): 报告 final_equity / equity_change_pct 必须含理财腿。
    // 修复前现货分支 = cash + pos×price, 静默丢理财面值 → 报告"总价值"头条失真
    // (实测 SOL 一年: 7934 → 3958, -20.7% 被误报成 -60.4%)。
    let bars: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    let (_out, ctx, _st) = run_univ2(sp_cfg(&[]), &bars, None);
    let rep = ctx.report();
    let bare = rep.final_cash + rep.final_pos_size * rep.final_price;
    assert!(
        rep.final_equity > bare,
        "final_equity 应含理财腿: {} vs 裸权益 {}",
        rep.final_equity,
        bare
    );
    // 划转不改权益: 平 bar 无市场波动, 期末权益 ≈ 初始 − 交易费。
    let drift = (rep.final_equity - dec!(10000)).abs();
    assert!(drift < dec!(50), "权益应≈初始(仅手续费磨损): drift {drift}");
    // equity_change_pct 与 final_equity 同源。
    let pct = ((rep.final_equity - dec!(10000)) / dec!(10000)).to_f64().unwrap() * 100.0;
    assert!(
        (rep.equity_change_pct - pct).abs() < 1e-9,
        "口径分叉: {} vs {}",
        rep.equity_change_pct,
        pct
    );
}

// ── 061 组合语义 (universe ≥ 2 币) ──────────────────────────────────────────

/// 061 组合配置: 单币默认参数 + universe 覆盖 (pair 保持 ETHUSDT = universe 首项)。
fn sp_cfg2(universe: &str, extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut p = vec![("universe", ConfigValue::String(universe.into()))];
    p.extend_from_slice(extra);
    sp_cfg(&p)
}

/// 061 组合 tick 序列: ETH/SOL 两币同小时 bar 合成统一时间轴
/// (与 build_interval_ticks 产出同型: 每 tick 按 pair 名升序)。
fn sp_ticks2(eth_bars: &[Kline], sol_bars: &[Kline]) -> Vec<Vec<(String, Kline)>> {
    assert_eq!(eth_bars.len(), sol_bars.len(), "两币序列等长");
    eth_bars
        .iter()
        .zip(sol_bars)
        .map(|(e, s)| vec![("ETHUSDT".to_string(), e.clone()), ("SOLUSDT".to_string(), s.clone())])
        .collect()
}

/// 061 组合 runner: 复刻 `run_portfolio_backtest` 逐 tick 时序
/// (step_portfolio → 撮合成交先入账 → on_tick 产单 → settle 事件闭环)。
fn run_sp_portfolio(
    cfg: StrategyConfig,
    ticks: &[Vec<(String, Kline)>],
) -> (Vec<Vec<Vec<OrderRequest>>>, BacktestContext, LuaStrategy) {
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    strategy.on_init(&mut ctx);
    let mut out: Vec<Vec<Vec<OrderRequest>>> = Vec::new();
    for bars in ticks {
        ctx.step_portfolio(bars);
        out.push(Vec::new());
        // 引擎口径: 前 tick 残留成交先派发 (市价即时成交下通常为空)。
        settle_univ2(&mut ctx, &mut strategy, Vec::new(), &mut out, 0);
        let orders = strategy.on_tick(&mut ctx);
        settle_univ2(&mut ctx, &mut strategy, orders, &mut out, 0);
    }
    (out, ctx, strategy)
}

#[test]
fn test_smart_position_portfolio_equal_weight_build_and_earn() {
    // 等权 2 币: tick0 各币按 target_pct×w_i=25% 市价建腿 (Δ=2500),
    // tick24 阈内不调、闲置现金入理财。48 根 1h 平 bar @100。
    let eth: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &eth.clone());
    let (out, mut ctx, mut st) = run_sp_portfolio(sp_cfg2("ETHUSDT,SOLUSDT", &[]), &ticks);

    let buys = sp_market_orders(&out, OrderSide::Buy);
    assert_eq!(buys.len(), 2, "tick0 每币恰一笔建仓买");
    assert!(buys.iter().all(|(i, _)| *i == 0), "两腿同在 tick0 发出");
    let size_of = |pair: &str| {
        buys.iter()
            .find(|(_, o)| o.pair == pair)
            .unwrap_or_else(|| panic!("缺 {pair} 买腿"))
            .1
            .size
            .to_f64()
            .unwrap()
    };
    let expect = 2500.0 / (100.0 * 1.001);
    assert!((size_of("ETHUSDT") - expect).abs() < 1e-3, "ETH 买量=Δ/(p(1+f))");
    assert!((size_of("SOLUSDT") - expect).abs() < 1e-3, "SOL 买量=Δ/(p(1+f))");
    assert!(sp_market_orders(&out, OrderSide::Sell).is_empty(), "不应有卖单");

    let earn = ctx.earn_balance("USDT").unwrap().to_f64().unwrap();
    let cash = ctx.balance("USDT").unwrap().to_f64().unwrap();
    assert!(earn > 4900.0 && earn < 5000.0, "tick24 闲置现金入理财: {earn}");
    assert!(cash > 40.0 && cash < 60.0, "现货现金≈权益×0.5%: {cash}");

    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_buy_count"), 2.0);
    assert_eq!(sp_f64(&snap, "stat_earn_supported"), 1.0);
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_pos").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_earn").abs() < 0.01);
}

#[test]
fn test_smart_position_portfolio_weights_allocation() {
    // weights=0.7,0.3: tick0 各币按 target_pct×w_i 落位 (Δ=3500/1500)。
    let eth: Vec<Kline> = (0..24).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &eth.clone());
    let (out, mut ctx, mut st) = run_sp_portfolio(
        sp_cfg2("ETHUSDT,SOLUSDT", &[("weights", ConfigValue::String("0.7,0.3".into()))]),
        &ticks,
    );
    let buys = sp_market_orders(&out, OrderSide::Buy);
    assert_eq!(buys.len(), 2);
    let size_of = |pair: &str| {
        buys.iter()
            .find(|(_, o)| o.pair == pair)
            .unwrap_or_else(|| panic!("缺 {pair} 买腿"))
            .1
            .size
            .to_f64()
            .unwrap()
    };
    assert!((size_of("ETHUSDT") - 3500.0 / (100.0 * 1.001)).abs() < 1e-3, "ETH 权重 0.7 → Δ=3500");
    assert!((size_of("SOLUSDT") - 1500.0 / (100.0 * 1.001)).abs() < 1e-3, "SOL 权重 0.3 → Δ=1500");
    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(snap.get("stat_weights").unwrap(), "0.7000,0.3000");
}

#[test]
fn test_smart_position_portfolio_weights_invalid_halts() {
    // weights 个数不匹配 / 合计≠1 → FATAL 停机 (stat_halted=1, 零订单)。
    let eth: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &eth.clone());
    for bad in ["0.7", "0.6,0.6"] {
        let (out, mut ctx, mut st) = run_sp_portfolio(
            sp_cfg2("ETHUSDT,SOLUSDT", &[("weights", ConfigValue::String(bad.into()))]),
            &ticks,
        );
        assert!(out.iter().all(|b| b.iter().all(|x| x.is_empty())), "weights={bad} 停机后不得下单");
        let snap = sp_snap(&mut st, &mut ctx);
        assert_eq!(sp_f64(&snap, "stat_halted"), 1.0, "weights={bad} 应 FATAL");
    }
}

#[test]
fn test_smart_position_portfolio_mixed_quote_halts() {
    // universe 混报价 (ETHUSDT + BTCUSDC): 全池共用单一报价币 → on_init 即 FATAL 停机。
    let cfg = sp_cfg2("ETHUSDT,BTCUSDC", &[]);
    let mut st = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    st.on_init(&mut ctx);
    assert_eq!(st.global_f64("fatal"), Some(1.0), "混报价 universe 应 FATAL");
    let orders = st.on_tick(&mut ctx);
    assert!(orders.is_empty(), "停机后不得产单");
}

#[test]
fn test_smart_position_portfolio_per_coin_threshold_only_outlier() {
    // 逐币阈值闸: ETH 100→120 (dev≈+3.6pp ≥2 → 卖回目标), SOL 恒 100
    // (dev≈−1.2pp <2 → 不动)。理财关闭: 卖+申购同周期是 Lua 已知账本漂移
    // 场景 (见 over_threshold_sell 注释), 本测试聚焦阈值闸语义。
    let mut eth: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    eth.extend((48..96).map(|h| bar_at_f(h as i64, 120.0)));
    let sol: Vec<Kline> = (0..96).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &sol);
    let (out, mut ctx, mut st) = run_sp_portfolio(
        sp_cfg2("ETHUSDT,SOLUSDT", &[("earn_enabled", ConfigValue::Float(0.0))]),
        &ticks,
    );
    let sells = sp_market_orders(&out, OrderSide::Sell);
    assert_eq!(sells.len(), 1, "只调越界的 ETH 一腿");
    assert_eq!(sells[0].0, 48);
    assert_eq!(sells[0].1.pair, "ETHUSDT");
    let notional = sells[0].1.size.to_f64().unwrap() * 120.0;
    assert!((notional - 373.5).abs() < 6.0, "卖名义≈Δ≈373: {notional}");
    assert_eq!(sp_market_orders(&out, OrderSide::Buy).len(), 2, "买入仅 tick0 两腿建仓");
    assert!(out[72..].iter().all(|b| b.iter().all(|x| x.is_empty())), "调回后阈内零订单");
    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_sell_count"), 1.0);
    assert_eq!(sp_f64(&snap, "stat_buy_count"), 2.0);
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
}

#[test]
fn test_smart_position_portfolio_earn_unsupported_silent() {
    // earn_supported_assets="" → earn_balance 恒 None: 理财静默关闭、零划转、
    // 不报错, 调仓腿照常跑 (061 T2 降级路径)。
    let eth: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &eth.clone());
    let (out, mut ctx, mut st) = run_sp_portfolio(
        sp_cfg2("ETHUSDT,SOLUSDT", &[("earn_supported_assets", ConfigValue::String("".into()))]),
        &ticks,
    );
    assert_eq!(sp_market_orders(&out, OrderSide::Buy).len(), 2, "调仓照常: tick0 两腿建仓");
    assert_eq!(ctx.earn_balance("USDT"), None, "白名单空 → 不支持");
    let cash = ctx.balance("USDT").unwrap().to_f64().unwrap();
    assert!(cash > 4990.0 && cash < 5010.0, "现金零划转: {cash}");
    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_earn_supported"), 0.0);
    assert_eq!(sp_f64(&snap, "stat_earn_final"), 0.0);
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
}

#[test]
fn test_smart_position_portfolio_redeem_skip_then_next_cycle() {
    // ETH 100→80 越界需买、现货现金不足 → tick48 只提交赎回申请零订单;
    // tick72 资金就绪补买。SOL 恒 100 全程阈内不动 (dev≈+1.3pp <2)。
    let mut eth: Vec<Kline> = (0..48).map(|h| bar_at_f(h, 100.0)).collect();
    eth.extend((48..120).map(|h| bar_at_f(h as i64, 80.0)));
    let sol: Vec<Kline> = (0..120).map(|h| bar_at_f(h, 100.0)).collect();
    let ticks = sp_ticks2(&eth, &sol);
    let (out, mut ctx, mut st) = run_sp_portfolio(sp_cfg2("ETHUSDT,SOLUSDT", &[]), &ticks);

    assert!(out[48].iter().all(|b| b.is_empty()), "现金不足周期不得下单");
    let buys = sp_market_orders(&out, OrderSide::Buy);
    let bars_of = |pair: &str| -> Vec<usize> {
        buys.iter().filter(|(_, o)| o.pair == pair).map(|(i, _)| *i).collect()
    };
    assert_eq!(bars_of("ETHUSDT"), vec![0, 72], "ETH: tick0 建仓 + tick72 赎回后就绪补买");
    assert_eq!(bars_of("SOLUSDT"), vec![0], "SOL 全程仅建仓一笔");
    assert!(sp_market_orders(&out, OrderSide::Sell).is_empty(), "SOL 阈内不卖");

    let earn = ctx.earn_balance("USDT").unwrap().to_f64().unwrap();
    assert!(earn > 4600.0 && earn < 4650.0, "赎回差额≈327: {earn}");

    let snap = sp_snap(&mut st, &mut ctx);
    assert_eq!(sp_f64(&snap, "stat_pending_redeem_count"), 1.0, "恰一次赎回申请");
    assert_eq!(sp_f64(&snap, "stat_buy_count"), 3.0);
    assert!(sp_f64(&snap, "stat_ledger_diff_cash").abs() < 0.01);
    assert!(sp_f64(&snap, "stat_ledger_diff_earn").abs() < 0.01);
}

#[test]
fn test_backtest_earn_supported_assets_whitelist() {
    // 061 T2: 白名单缺省 {USDT}; 不支持资产 earn_balance=None、划转 false;
    // 显式 ""= 全不支持; 逗号串大小写/空白不敏感。
    let mut ctx = BacktestContext::new(
        sp_cfg(&[]),
        Balance { asset: "USDT".into(), free: dec!(1000), locked: Decimal::ZERO },
    );
    assert_eq!(ctx.earn_balance("USDT"), Some(Decimal::ZERO));
    assert_eq!(ctx.earn_balance("TSLAB"), None);
    assert!(!ctx.earn_subscribe("TSLAB", dec!(100)));
    assert!(!ctx.earn_redeem("TSLAB", dec!(100)));

    let mut ctx2 = BacktestContext::new(
        sp_cfg(&[("earn_supported_assets", ConfigValue::String("".into()))]),
        Balance { asset: "USDT".into(), free: dec!(1000), locked: Decimal::ZERO },
    );
    assert_eq!(ctx2.earn_balance("USDT"), None, "显式空串 = 全不支持");
    assert!(!ctx2.earn_subscribe("USDT", dec!(100)));

    let mut ctx3 = BacktestContext::new(
        sp_cfg(&[("earn_supported_assets", ConfigValue::String("usdt, TSLAB".into()))]),
        Balance { asset: "USDT".into(), free: dec!(1000), locked: Decimal::ZERO },
    );
    assert!(ctx3.earn_subscribe("usdt", dec!(100)), "大小写/空白不敏感");
    assert_eq!(ctx3.earn_balance("TSLAB"), Some(Decimal::ZERO));
}
