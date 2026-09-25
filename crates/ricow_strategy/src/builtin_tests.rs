//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/spot/`(shannon_spot_grid 香农现货网格 + paired_grid 现货动态非对称网格),
//! include_str! 编译期嵌入。
//! 断言每个内置脚本的关键行为 (建仓/激活/配对/挂单), 对齐 Rust 版已知向量。

use std::collections::HashMap;

use ricow_core::{Balance, Kline, OrderRequest, OrderSide, OrderType};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::backtest::BacktestContext;
use crate::config::{ConfigValue, StrategyConfig};
use crate::context::Context;
use crate::lua::LuaStrategy;
use crate::strategy::Strategy;
use rust_decimal::prelude::ToPrimitive;

const SHANNON_ETF_ACCUM: &str = include_str!("../../../strategies/spot/shannon_spot_grid.lua");

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
// 023 香农 ETF 指数增加策略 (shannon_spot_grid) 集成测试
// ============================================================================

/// 023 测试用: 第 `hour` 小时的 bar(1h 间隔; 引擎不校验周期, 策略只看时间戳)。
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

/// 023 测试用: 每根 TR 恒为 2 (high−low=2 且前收盘落在区间内) → ATR(14) 精确等于 2。
fn tf_bars(n: i64) -> Vec<Kline> {
    (0..n).map(|h| bar_at_hour(h, 100, 101, 99, 100)).collect()
}

/// 023 测试用: 先 30 根横盘(EMA10 ≈ EMA20), 再 40 根上行 → 中途出现金叉。
fn uptrend_bars() -> Vec<Kline> {
    let mut v: Vec<Kline> = (0..30).map(|h| bar_at_hour(h, 100, 100, 100, 100)).collect();
    for (i, h) in (30..70).enumerate() {
        let px = 100 + (i as i64 + 1) * 5;
        v.push(bar_at_hour(h, px, px, px, px));
    }
    v
}

fn accum_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("atr_mult", ConfigValue::Float(2.0)),
        ("min_notional", ConfigValue::Float(5.0)),
        // 030: 日线判据默认开 —— 测试桩默认关(未装日线序列时判据未就绪会整体不下单);
        // 判据相关的用例在 extra 里显式打开。
        ("regime_filter", ConfigValue::String("off".into())),
        ("cash", ConfigValue::Float(10000.0)),
        // 虚拟账本口径(023 v3): 默认 1× 使 v_cap == 测试本金(单一变量); 测"卖不出去"时改成 10×。
        ("leverage_mult", ConfigValue::Float(1.0)),
    ];
    params.extend_from_slice(extra);
    config(SHANNON_ETF_ACCUM, &params)
}

/// 023: 跑一段序列, 并按装配层语义预装高周期序列; 返回 (每 tick 订单, context)。
fn run_accum(
    cfg: StrategyConfig,
    bars: &[Kline],
    tf: Option<Vec<Kline>>,
) -> (Vec<Vec<OrderRequest>>, BacktestContext) {
    let (orders, ctx, _st) = run_accum_full(cfg, bars, tf);
    (orders, ctx)
}

/// 023 v3: 同 `run_accum`, 但把策略句柄也返回(读 Lua 全局计数与不变量 I1 用)。
fn run_accum_full(
    cfg: StrategyConfig,
    bars: &[Kline],
    tf: Option<Vec<Kline>>,
) -> (Vec<Vec<OrderRequest>>, BacktestContext, LuaStrategy) {
    run_accum_multi(cfg, bars, &[("1h", tf)])
}

/// 023 v4(趋势判据): 预装**多套**高周期序列(键 = `pair|tf`), 用于同时供 ATR 与日线趋势判据。
fn run_accum_multi(
    cfg: StrategyConfig,
    bars: &[Kline],
    tfs: &[(&str, Option<Vec<Kline>>)],
) -> (Vec<Vec<OrderRequest>>, BacktestContext, LuaStrategy) {
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    for (tf, series) in tfs {
        if let Some(bars) = series {
            ctx.set_tf_klines("ETHUSDT", tf, bars.clone());
        }
    }
    strategy.on_init(&mut ctx);
    let mut out = Vec::new();
    for k in bars {
        ctx.step_bar(k.clone());
        let orders = strategy.on_tick(&mut ctx);
        for req in &orders {
            let _ = ctx.place_order(req.clone());
        }
        let fills = ctx.drain_fills();
        for f in fills {
            strategy.on_fill(&mut ctx, f);
        }
        out.push(orders);
    }
    (out, ctx, strategy)
}

#[test]
fn test_shannon_spot_grid_requires_tf_atr_channel() {
    // 高周期通道未预装 → 一笔都不下(不猜 ATR 值, 不建仓)。
    let (orders, _ctx) = run_accum(accum_cfg(&[]), &uptrend_bars(), None);
    assert!(orders.iter().all(|o| o.is_empty()), "ATR 通道未就绪时必须完全不动");
}

/// 030 测试用: 主序列 —— 横盘 `n` 根 @ `px`(与高周期序列同粒度, 引擎不校验周期)。
fn flat_main(n: i64, px: i64) -> Vec<Kline> {
    (0..n).map(|h| bar_at_hour(h, px, px, px, px)).collect()
}

/// 030 测试用: 高周期(1h)序列 —— 前 `flat` 根横盘, 之后每根 +`step` → EMA3 上穿 EMA5。
/// high−low = 2 且前收落在区间内 → ATR 恒为 2; 平盘段让 EMA3/EMA5 收敛到相等。
fn tf_bars_up(flat: i64, n: i64, step: i64) -> Vec<Kline> {
    (0..n)
        .map(|h| {
            let px = if h < flat { 100 } else { 100 + (h - flat + 1) * step };
            bar_at_hour(h, px, px + 1, px - 1, px)
        })
        .collect()
}

#[test]
fn test_shannon_spot_grid_requires_signal_ema_channel() {
    // 信号 EMA 序列未预装(ema_interval 指向未装的 "4h")→ 一笔都不下(不猜 EMA 值)。
    let bars = flat_main(60, 100);
    let (orders, _ctx) = run_accum(
        accum_cfg(&[("ema_interval", ConfigValue::String("4h".into()))]),
        &bars,
        Some(tf_bars_up(20, 40, 5)),
    );
    assert!(orders.iter().all(|o| o.is_empty()), "信号 EMA 通道未就绪时必须完全不动");
}

#[test]
fn test_shannon_spot_grid_thin_spacing_halts() {
    // R7 成本门槛(硬校验): atr_mult×ATR ≤ 4×fee_side×价格 → 停机(FATAL), 不再下单。
    let bars = flat_main(60, 100);
    let (orders, _ctx, st) = run_accum_full(
        accum_cfg(&[("atr_mult", ConfigValue::Float(0.0001))]),
        &bars,
        Some(tf_bars_up(20, 60, 1)),
    );
    assert!(orders.iter().all(|o| o.is_empty()), "成本门槛不满足时必须停机且不下任何单");
    assert_eq!(st.global_f64("buy_count"), None.or(Some(0.0)), "不得建仓");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "不得有成交");
}

#[test]
fn test_shannon_spot_grid_leverage_clamped_to_5() {
    // R6: 杠杆钳制到 1~5 —— 配 10 倍时虚拟资金按 5 倍算(投入资金 10000 → 50000)。
    let bars = flat_main(40, 100);
    let (_orders, _ctx, st) = run_accum_full(
        accum_cfg(&[("leverage_mult", ConfigValue::Float(10.0))]),
        &bars,
        Some(tf_bars_up(20, 60, 1)),
    );
    assert_eq!(st.global_f64("v_cap"), Some(50000.0), "杠杆应钳制到 5");
}

// ─────────────────── 030-A 网格挂单语义(2026-09-23 用户口径) ───────────────────

/// 030-A 测试用: 主序列价格从 base 走到 then(之后保持), 用于触发/验证 `start_price` 激活门槛。
fn drop_main(n: i64, base: i64, then: i64, at: i64) -> Vec<Kline> {
    (0..n)
        .map(|h| {
            let p = if h < at { base } else { then };
            bar_at_hour(h, p, p, p, p)
        })
        .collect()
}

#[test]
fn test_shannon_spot_grid_requires_start_price() {
    let bars = drop_main(40, 200, 100, 10);
    let (orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &bars, Some(tf_bars(60)));
    assert!(orders.iter().all(|o| o.is_empty()), "缺必填 start_price 时不得下任何单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "缺参数 -> 无成交");
}

#[test]
fn test_shannon_spot_grid_activate_with_initial_buy_then_ladder() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("initial_buy_amount", ConfigValue::Float(1000.0)),
    ]);
    let bars = drop_main(60, 200, 100, 10);
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, Some(tf_bars(60)));
    let market: Vec<_> =
        orders.iter().flatten().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(market.len(), 1, "激活时应恰有一笔市价初始建仓");
    assert_eq!(market[0].side, OrderSide::Buy);
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "初始建仓成交价应成为第一次平衡价");
    // 挂单: 平衡价 ± atr_mult×ATR = 100 ± 2×2 = 96 / 104; 无仓可卖(真实持仓很少) -> 只有买单
    let limits: Vec<_> = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    assert!(!limits.is_empty(), "激活后应挂出网格单");
    let buys: Vec<_> = limits.iter().filter(|o| o.side == OrderSide::Buy).collect();
    let sells: Vec<_> = limits.iter().filter(|o| o.side == OrderSide::Sell).collect();
    assert!(
        !buys.is_empty() && !sells.is_empty(),
        "两侧都应有挂单(挂单量由账本 50:50 决定): buys={} sells={}",
        buys.len(),
        sells.len()
    );
    let buy_px = limits[0].price.expect("限价单必须带价格");
    assert_eq!(buy_px, dec!(96), "买单价位应为 平衡价 − 2×ATR = 96");
}

#[test]
fn test_shannon_spot_grid_activate_without_initial_buy() {
    let cfg = accum_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = drop_main(60, 200, 100, 10);
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "未设初始仓位 -> 无成交");
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "未设初始仓位 -> 平衡价 := 激活时现价");
    let limits: Vec<_> = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    assert!(!limits.is_empty(), "未设初始仓位也应挂出买单");
    assert_eq!(limits[0].price.expect("限价单必须带价格"), dec!(96), "买单价位 = 平衡价 − 2×ATR");
}

#[test]
fn test_shannon_spot_grid_state_snapshot_has_anchor_and_cap() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("initial_buy_amount", ConfigValue::Float(1000.0)),
    ]);
    let bars = drop_main(60, 200, 100, 10);
    let (_orders, _ctx, st) = run_accum_full(cfg, &bars, Some(tf_bars(60)));
    let snap = st.state_snapshot();
    assert!(
        snap.iter().any(|(k, _)| k == "balance_price"),
        "状态快照应含平衡价(断点续接的最小必需项)"
    );
    assert!(snap.iter().any(|(k, _)| k == "v_cap"), "状态快照应含账本规模");
}

/// 030 门控: trend_gate=off(默认, 纯网格)在 BULL 状态下仍挂卖单。
#[test]
fn test_shannon_spot_grid_gate_off_still_sells_in_bull() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(1000.0)),
        // 主时钟 = 1h 且主序列与判据序列同一条: 判据需要 200+ 根 1h 可见 bar 才就绪
        ("interval", ConfigValue::String("1h".into())),
        ("regime_filter", ConfigValue::String("ema200".into())),
        ("regime_interval", ConfigValue::String("1h".into())),
        ("regime_ema_period", ConfigValue::Integer(200)),
        ("trend_gate", ConfigValue::String("off".into())),
        ("leverage_mult", ConfigValue::Float(2.0)),
        // 必须有初始仓, 否则卖量被"持仓不足"限制为 0, 无法区分门控是否生效
        ("initial_buy_amount", ConfigValue::Float(5000.0)),
    ]);
    // 1h: 横盘 10 根 -> 强势上行; 长度 910 > 判据 EMA200 预热(603 根), 末段 close 远高于 EMA200 = BULL
    let bull = tf_bars_up(10, 900, 1);
    let (orders, _ctx, _st) = run_accum_full(cfg, &bull.clone(), Some(bull));
    let buys = orders.iter().flatten().filter(|o| o.side == OrderSide::Buy).count();
    let sells = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.side == OrderSide::Sell)
        .count();
    assert!(buys > 0, "桩应产生订单(否则门控效果无法验证): buys={buys} ticks={}", orders.len());
    assert!(sells > 0, "trend_gate=off 应为纯网格: BULL 下仍应挂卖单 (实际 {sells})");
}

/// 030 门控: trend_gate=on 在 BULL 状态暂停卖出(不得挂出卖单)。
#[test]
fn test_shannon_spot_grid_gate_on_blocks_sell_in_bull() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(1000.0)),
        ("interval", ConfigValue::String("1h".into())),
        ("regime_filter", ConfigValue::String("ema200".into())),
        ("regime_interval", ConfigValue::String("1h".into())),
        ("regime_ema_period", ConfigValue::Integer(200)),
        ("trend_gate", ConfigValue::String("on".into())),
        ("leverage_mult", ConfigValue::Float(2.0)),
        // 必须有初始仓, 否则卖量被"持仓不足"限制为 0, 无法区分门控是否生效
        ("initial_buy_amount", ConfigValue::Float(5000.0)),
    ]);
    let bull = tf_bars_up(10, 900, 1);
    let (orders, _ctx, _st) = run_accum_full(cfg, &bull.clone(), Some(bull));
    let sells = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.side == OrderSide::Sell)
        .count();
    assert_eq!(sells, 0, "trend_gate=on 时 BULL 下不应挂卖单 (实际 {sells})");
}

/// 030: `real_cash` 缺省时必须按**账户权益**(= --cash 本金)建账本, 而不是硬编码 1 万 ——
/// 否则用户设 `--cash 20000` 却得到按 1 万算的虚拟账本(实战踩过的陷阱)。
#[test]
fn test_shannon_spot_grid_real_cash_defaults_to_equity() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(1000.0)),
        ("leverage_mult", ConfigValue::Float(2.0)),
    ]);
    // 测试本金 = 10000(见 run_accum_multi 的 Balance) -> 未设 real_cash 时 v_cap 应为 10000 × 2
    let (_orders, _ctx, st) = run_accum_full(cfg, &uptrend_bars(), None);
    assert_eq!(
        st.global_f64("v_cap"),
        Some(20000.0),
        "未设 real_cash 时虚拟资金应为 账户权益 10000 × 杠杆 2 = 20000"
    );
}

/// 030 收益分解: 建仓成交价/建仓量/投入本金必须被记录(否则停机分解会静默缺失)。
#[test]
fn test_shannon_spot_grid_records_entry_for_pnl_split() {
    let cfg = accum_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("initial_buy_amount", ConfigValue::Float(1000.0)),
    ]);
    let (_orders, _ctx, st) = run_accum_full(cfg, &drop_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(
        st.global_f64("entry_price"),
        Some(100.0),
        "初始建仓成交价应被记录(收益分解的持仓成本基准)"
    );
    assert!(st.global_f64("entry_size").unwrap_or(0.0) > 0.0, "初始建仓量应被记录");
    assert!(st.global_f64("invested0").unwrap_or(0.0) > 0.0, "投入本金应被记录");
}

// ============================================================================
// paired_grid 现货动态非对称网格 集成测试 (2026-09-24)
// ============================================================================

const PAIRED_GRID: &str = include_str!("../../../strategies/spot/paired_grid.lua");

fn paired_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("start_price", ConfigValue::Float(110.0)),
        ("spacing_pct", ConfigValue::Float(0.04)), // 4% 固定间距
        ("direction_offset", ConfigValue::Float(0.0)), // 关方向偏移, 间距恒 4%, 可精确预测买卖价
        ("order_amount", ConfigValue::Float(10.0)),
        // 放宽名义守卫, 聚焦机制(价格 100 × 0.1 ≈ 10 USDT 本已 > 默认 5, 仍显式设小防边界误伤)
        ("min_notional", ConfigValue::Float(1.0)),
    ];
    params.extend_from_slice(extra);
    config(PAIRED_GRID, &params)
}

#[test]
fn test_paired_grid_activate_build_and_grid_structure() {
    // start_price=110, 价格 100 激活; 建仓 30 USDT(不计 flag); 建仓后上下各挂一单(等比 4%)。
    let cfg = paired_cfg(&[("initial_buy_amount", ConfigValue::Float(30.0))]);
    let bars = flat_main(2, 100); // 2 根 @100: 首 tick 即激活建仓
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, None);

    // 建仓市价单
    let market: Vec<_> =
        orders.iter().flatten().filter(|o| o.order_type == OrderType::Market).collect();
    assert!(!market.is_empty(), "激活后应建仓市价买入, 实际无市价单");
    assert_eq!(market[0].side, OrderSide::Buy);

    // 等比 4%: 下方买单 @ 100/(1+0.04)=96.154; 上方卖单 @ 100×1.04=104
    let buy_px: Vec<f64> = orders
        .iter()
        .flatten()
        .filter(|o| o.side == OrderSide::Buy && o.order_type == OrderType::Limit)
        .filter_map(|o| o.price.map(|p| p.to_f64().unwrap()))
        .collect();
    let sell_px: Vec<f64> = orders
        .iter()
        .flatten()
        .filter(|o| o.side == OrderSide::Sell && o.order_type == OrderType::Limit)
        .filter_map(|o| o.price.map(|p| p.to_f64().unwrap()))
        .collect();
    assert!(
        buy_px.iter().any(|p| (p - 96.1538).abs() < 1e-3),
        "应挂下方买单 @96.154, 实际买单: {buy_px:?}"
    );
    assert!(
        sell_px.iter().any(|p| (p - 104.0).abs() < 1e-6),
        "应挂上方配对卖单 @104, 实际卖单: {sell_px:?}"
    );

    // 建仓不计 flag; 本序列无网格成交 -> flag 恒 0
    assert_eq!(st.global_f64("flag"), Some(0.0), "建仓不计 flag, 无网格成交时 flag 应恒为 0");
}

#[test]
fn test_paired_grid_pair_sell_above_buy() {
    // 买单@96.154 成交(flag−1) → 反弹卖单@100 成交(flag+1): 配对卖价必须高于买价, flag 归 0。
    let cfg = paired_cfg(&[]);
    let mut bars = flat_main(1, 100); // 首 tick 激活(不建仓), ref=100
    bars.push(bar_at_hour(1, 96, 96, 96, 96)); // 买单@96.154 成交(bar low 96 ≤ 96.154)
    bars.push(bar_at_hour(2, 96, 96, 96, 96));
    bars.push(bar_at_hour(3, 100, 100, 100, 100)); // 卖单@100 成交
    bars.push(bar_at_hour(4, 100, 100, 100, 100));
    bars.push(bar_at_hour(5, 100, 100, 100, 100));
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, None);

    let sell_px: Vec<f64> = orders
        .iter()
        .flatten()
        .filter(|o| o.side == OrderSide::Sell && o.order_type == OrderType::Limit)
        .filter_map(|o| o.price.map(|p| p.to_f64().unwrap()))
        .collect();
    assert!(!sell_px.is_empty(), "买单成交后应挂配对卖单, 实际无卖单");
    assert!(
        sell_px.iter().all(|p| *p > 96.0),
        "配对卖价必须高于买入价 96.154, 实际卖单: {sell_px:?}"
    );

    // 买单成交(flag−1) + 卖单成交(flag+1) -> 归 0
    assert_eq!(st.global_f64("flag"), Some(0.0), "一买一卖配对完成后 flag 应归 0");
}

#[test]
fn test_paired_grid_flag_negative_on_downtrend() {
    // 连续下跌只成交买单 -> flag 持续为负。
    let cfg = paired_cfg(&[]);
    let mut bars = flat_main(1, 100);
    bars.push(bar_at_hour(1, 96, 96, 96, 96)); // 买单成交(flag=-1)
    bars.push(bar_at_hour(2, 96, 96, 96, 96));
    bars.push(bar_at_hour(3, 92, 92, 92, 92)); // 买单成交(flag=-2)
    bars.push(bar_at_hour(4, 92, 92, 92, 92));
    bars.push(bar_at_hour(5, 88, 88, 88, 88)); // 买单成交(flag=-3)
    let (_orders, _ctx, st) = run_accum_full(cfg, &bars, None);

    let flag = st.global_f64("flag").expect("flag 应可读");
    assert!(flag < 0.0, "连续下跌只买不卖 → flag 应为负, 实际 {flag}");
}
