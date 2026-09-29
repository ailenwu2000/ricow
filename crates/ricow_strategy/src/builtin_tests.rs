//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/spot/`(paired_grid 现货动态非对称网格 +
//! shannon_grid 现货 香农网格)与 `strategies/futures/`(paired_grid_futures_long),
//! include_str! 编译期嵌入。
//! 断言每个内置脚本的关键行为 (建仓/激活/配对/挂单), 对齐 Rust 版已知向量。

use std::collections::HashMap;

use ricow_core::{Balance, Kline, OrderAction, OrderRequest, OrderSide, OrderType};
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

/// 测试共用: 跑一段序列, 并按装配层语义预装高周期序列; 返回 (每 tick 订单, context, 策略)。
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
        let mut bar_orders = orders.clone();
        // 039 事件模型对齐引擎: on_tick 与 on_fill 返回的订单都要 place(on_fill 返回经
        // settle_orders_test 递归结算, 链内限价单 defer 次 bar 生效, 与引擎 038 R1 同款);
        // on_fill 重挂批次并入本 bar 订单列表(旧断言按"全部可见订单"过滤)。
        let mut fill_batches = Vec::new();
        let mut placed = Vec::new();
        let mut fills = Vec::new();
        settle_orders_test(
            &mut ctx,
            &mut strategy,
            orders,
            0,
            0,
            &mut fill_batches,
            &mut placed,
            &mut fills,
        );
        for (_, follow) in fill_batches {
            bar_orders.extend(follow);
        }
        out.push(bar_orders);
    }
    (out, ctx, strategy)
}

fn flat_main(n: i64, px: i64) -> Vec<Kline> {
    (0..n).map(|h| bar_at_hour(h, px, px, px, px)).collect()
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

#[test]
fn test_paired_grid_on_fill_rehangs_immediately() {
    // 039 核心行为: 网格买成交后 on_fill **同 bar** 返回全撤重挂订单(不等下一根 bar);
    // 现货语义: 卖价锚 ref_price(新参考价)×(1+up) = 96.1538×1.04 = 100, 买价 = 新 ref÷(1+down)。
    // 链内限价单由 038 R1 次 bar 生效 -> 无乒乓链(恰 1 批)。
    let cfg = paired_cfg(&[]);
    let bars: Vec<Kline> = [100.0, 100.0, 96.0, 96.0]
        .iter()
        .enumerate()
        .map(|(h, px)| bar_at_f(h as i64, *px))
        .collect();
    let (batches, _placed, _fills) = run_event_full(cfg, &bars);

    // bar0: on_tick 激活(无建仓) ref=100, 挂买@96.1538 入簿
    // bar2: bar[96,96,96,96] open 跳空越过限价 -> 买@96.1538 按 open=96 成交(038 口径)
    //       -> on_fill 同 bar 返回 1 批重挂(ref := 成交价 96)
    let b2: Vec<&(usize, Vec<OrderRequest>)> = batches.iter().filter(|(i, _)| *i == 2).collect();
    assert_eq!(b2.len(), 1, "bar2 应恰 1 批 on_fill 重挂: {batches:?}");
    let re = &b2[0].1;
    let sell = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Sell)
        .expect("应含配对卖单");
    let sell_px = sell.price.unwrap().to_f64().unwrap();
    assert!((sell_px - 96.0 * 1.04).abs() < 1e-3, "现货卖价应锚新 ref(96)×1.04=99.84: {sell_px}");
    let buy = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Buy)
        .expect("应含网格买单");
    let buy_px = buy.price.unwrap().to_f64().unwrap();
    // flag=-1 -> 下方间距放大(0.04×(1+1×0.2)=0.048): 买 = 96 ÷ 1.048
    assert!((buy_px - 96.0 / 1.048).abs() < 1e-3, "买=新 ref÷(1+down=0.048): {buy_px}");
}

#[test]
fn test_paired_grid_on_fill_build_splits_immediately() {
    // 039: 建仓市价单成交后 on_fill **同 bar** 拆格重挂(ref=建仓成交价 100): 买@96.1538 + 卖@104。
    let cfg = paired_cfg(&[("initial_buy_amount", ConfigValue::Float(30.0))]);
    let bars: Vec<Kline> =
        [100.0, 100.0].iter().enumerate().map(|(h, px)| bar_at_f(h as i64, *px)).collect();
    let (batches, _placed, _fills) = run_event_full(cfg, &bars);

    let b0: Vec<&(usize, Vec<OrderRequest>)> = batches.iter().filter(|(i, _)| *i == 0).collect();
    assert_eq!(b0.len(), 1, "bar0 建仓成交应恰触发 1 批 on_fill 重挂: {batches:?}");
    let re = &b0[0].1;
    let sell = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Sell)
        .expect("建仓后应同 bar 挂出配对卖单");
    assert!((sell.price.unwrap().to_f64().unwrap() - 104.0).abs() < 1e-6, "卖=ref(100)×1.04=104");
    assert!(
        (sell.size.to_f64().unwrap() - 0.1).abs() < 1e-9,
        "卖量应=栈顶 lot(30/3 笔拆仓每笔 0.1 币)"
    );
    let buy = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Buy)
        .expect("应含网格买单");
    assert!((buy.price.unwrap().to_f64().unwrap() - 100.0 / 1.04).abs() < 1e-6, "买=100÷1.04");
}

// ============================================================================
// paired_grid_futures_long 合约配对做多网格 集成测试 (032 v2, 2026-09-26: 砍空头改纯配对做多;
// 2026-09-26 改名: id paired_grid_futures → paired_grid_futures_long, 为将来的做空版
// paired_grid_futures_short 让位)
// ============================================================================

const PAIRED_GRID_FUT: &str =
    include_str!("../../../strategies/futures/paired_grid_futures_long.lua");

/// 合约 hedge 配置: market/position_mode 走 config 顶层, 杠杆走 [backtest](引擎 resolve 口径)。
fn futures_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("spacing_pct", ConfigValue::Float(0.04)),
        ("direction_offset", ConfigValue::Float(0.0)), // 关偏移, 间距恒 4%, 可精确预测
        ("order_amount", ConfigValue::Float(10.0)),
        ("min_notional", ConfigValue::Float(1.0)), // 放宽守卫, 聚焦机制
    ];
    params.extend_from_slice(extra);
    let mut cfg = config(PAIRED_GRID_FUT, &params);
    cfg.market = "futures".into();
    cfg.position_mode = "hedge".into();
    cfg.backtest = Some(crate::config::BacktestToml { leverage: Some(2.0), ..Default::default() });
    cfg
}

/// 按 position_side 过滤挂单(hedge 路由断言用)。
fn ps_orders<'a>(orders: &'a [Vec<OrderRequest>], ps: &str) -> Vec<&'a OrderRequest> {
    orders.iter().flatten().filter(|o| o.position_side.as_deref() == Some(ps)).collect()
}

#[test]
fn test_paired_grid_futures_long_activate_market_build() {
    // 穿越激活(价 100 < start 110): 首 tick 市价建仓, 只下 LONG 侧单。
    let cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(10.0)),
    ]);
    let bars = flat_main(3, 100);
    let (orders, ctx, st) = run_accum_full(cfg, &bars, None);

    // 建仓单: 市价 buy + position_side=long; 全程不得出现 short 侧挂单。
    assert!(
        ps_orders(&orders, "long")
            .iter()
            .any(|o| o.side == OrderSide::Buy && o.order_type == OrderType::Market),
        "应穿越激活并市价建仓(position_side=long)"
    );
    assert!(
        ps_orders(&orders, "short").is_empty(),
        "只做多策略 -> 不得有任何 position_side=short 挂单"
    );

    // 建仓后多头仓 0.1 币(10 USDT @100), 建仓不计 flag。
    let long_sz = ctx.position_directional("ETHUSDT", OrderSide::Buy).map(|p| p.size);
    assert!(
        long_sz.map(|s| (s.to_f64().unwrap() - 0.1).abs() < 1e-9).unwrap_or(false),
        "建仓后应有 0.1 币多仓, 实际 {long_sz:?}"
    );
    assert_eq!(st.global_f64("flag"), Some(0.0), "建仓不计 flag");
    assert!(st.global_f64("fill_count").unwrap_or(0.0) >= 1.0, "建仓应成交");
}

/// 036 事件模型: 复刻引擎 settle_orders(034)——下单 → drain → on_fill(返回订单递归 place)。
/// 同时记录 on_fill 返回批次、全部下单批次与逐笔成交。
/// 038 R1: 链内 (depth≥1) 限价单次 bar 生效 (defer 开关, 与引擎同款)。
fn settle_orders_test(
    ctx: &mut BacktestContext,
    strategy: &mut LuaStrategy,
    orders: Vec<OrderRequest>,
    i: usize,
    depth: u32,
    fill_batches: &mut Vec<(usize, Vec<OrderRequest>)>,
    placed: &mut Vec<(usize, Vec<OrderRequest>)>,
    fills: &mut FillTrace,
) {
    if !orders.is_empty() {
        placed.push((i, orders.clone()));
    }
    for req in orders {
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
        fills.push((i, f.side, f.fill_size.to_f64().unwrap(), f.fill_price.to_f64().unwrap()));
        let follow = strategy.on_fill(ctx, f);
        fill_batches.push((i, follow.clone()));
        settle_orders_test(ctx, strategy, follow, i, depth + 1, fill_batches, placed, fills);
    }
}

/// 036 事件模型: 复刻引擎回测主循环 (step_bar → 成交先入账/on_fill 闭环 → on_tick → place),
/// 收集 on_fill 每次返回的订单批次 (bar 序号, 批次)。
fn run_event_loop(cfg: StrategyConfig, bars: &[Kline]) -> Vec<(usize, Vec<OrderRequest>)> {
    let (batches, _, _) = run_event_full(cfg, bars);
    batches
}

/// 同 [`run_event_loop`], 另返回全部下单批次与逐笔成交轨迹。
/// 036 事件模型跑批产物: (on_fill 返回批次, 全部下单批次, 逐笔成交), 每项带 bar 序号。
type EventRun = (Vec<(usize, Vec<OrderRequest>)>, Vec<(usize, Vec<OrderRequest>)>, FillTrace);

fn run_event_full(cfg: StrategyConfig, bars: &[Kline]) -> EventRun {
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    strategy.on_init(&mut ctx);
    let (mut batches, mut placed, mut fills) = (Vec::new(), Vec::new(), Vec::new());
    for (i, k) in bars.iter().enumerate() {
        ctx.step_bar(k.clone());
        settle_orders_test(
            &mut ctx,
            &mut strategy,
            Vec::new(),
            i,
            0,
            &mut batches,
            &mut placed,
            &mut fills,
        );
        let tick = strategy.on_tick(&mut ctx);
        settle_orders_test(
            &mut ctx,
            &mut strategy,
            tick,
            i,
            0,
            &mut batches,
            &mut placed,
            &mut fills,
        );
    }
    (batches, placed, fills)
}

#[test]
fn test_paired_grid_futures_long_on_fill_rehangs_immediately() {
    // 036 核心行为: 网格买成交后 on_fill **立即**返回全撤重挂订单(不等下一根 bar),
    // 卖价锚定栈顶成本×(1+up), 买价=新 ref÷(1+down), 重挂限价单不本 bar 再成交(深度安全)。
    // 路径: 100 激活(无建仓) → 100 重挂买@96.1538 → 96 买成交 → on_fill 同 bar 返回
    // cancel + 买@92.4556 + 卖@100(=96.1538×1.04)。
    let cfg = futures_cfg(&[("start_price", ConfigValue::Float(110.0))]);
    let bars: Vec<Kline> = [100.0, 100.0, 96.0, 96.0]
        .iter()
        .enumerate()
        .map(|(h, px)| bar_at_f(h as i64, *px))
        .collect();
    let (batches, _placed, _fills) = run_event_full(cfg, &bars);

    // 买成交发生在 bar2, on_fill 返回的批次只能有一个(重挂限价单不本 bar 再成交 → 无递归)。
    let b2: Vec<&Vec<OrderRequest>> =
        batches.iter().filter(|(i, _)| *i == 2).map(|(_, o)| o).collect();
    assert_eq!(b2.len(), 1, "bar2 应恰有 1 批 on_fill 返回(深度安全): {batches:?}");
    let re: Vec<&OrderRequest> = b2[0].iter().filter(|o| o.action == OrderAction::Place).collect();
    assert_eq!(re.len(), 2, "应返回买 + 卖 两条实单(另有 1 条全撤指令): {re:?}");
    let buy = re
        .iter()
        .copied()
        .find(|o| o.side == OrderSide::Buy && o.order_type == OrderType::Limit)
        .expect("应有网格买限价单");
    let sell = re
        .iter()
        .copied()
        .find(|o| o.side == OrderSide::Sell && o.order_type == OrderType::Limit)
        .expect("应有平多卖限价单");
    let buy_px = buy.price.unwrap().to_f64().unwrap();
    let sell_px = sell.price.unwrap().to_f64().unwrap();
    // 成交@96 -> ref=96, flag=-1: 下间距 = 0.04×(1+1×0.2)(num() 将 offset=0 回退默认 0.2)
    // = 4.8% -> 买 = 96÷1.048; 上间距 4% -> 卖 = 栈顶成本 96×1.04。
    assert!((buy_px - 96.0 / 1.048).abs() < 1e-3, "买价=新 ref÷1.048: {buy_px}");
    assert!((sell_px - 96.0 * 1.04).abs() < 1e-3, "卖价锚定栈顶成本×1.04: {sell_px}");
    assert!(sell_px > buy_px, "配对卖价必须 > 买价");
    assert_eq!(buy.position_side.as_deref(), Some("long"));
    assert_eq!(sell.position_side.as_deref(), Some("long"));
    // 卖量=栈顶 lot(fill 全量), 买量=order_amount/价。
    let fill_sz = 10.0 / (100.0 / 1.04);
    assert!(
        (sell.size.to_f64().unwrap() - fill_sz).abs() < 1e-9,
        "卖量应=栈顶买入量 {}",
        sell.size
    );
    assert!(
        (buy.size.to_f64().unwrap() * buy_px - 10.0).abs() < 1e-6,
        "买名义=order_amount: {}",
        buy.size
    );
}

#[test]
fn test_paired_grid_futures_long_on_fill_build_splits_immediately() {
    // 036: 建仓市价单成交后 on_fill **同 bar** 拆格重挂(ref=建仓成交价), 不等下一根 bar。
    let cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(10.0)),
    ]);
    let bars: Vec<Kline> =
        [100.0].iter().enumerate().map(|(h, px)| bar_at_f(h as i64, *px)).collect();
    let batches = run_event_loop(cfg, &bars);

    assert!(!batches.is_empty(), "建仓成交应触发 on_fill 重挂");
    let (i, re) = &batches[0];
    assert_eq!(*i, 0, "建仓成交发生在 bar0, 重挂应同 bar 返回");
    let sell = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Sell)
        .expect("应含配对卖单");
    let sell_px = sell.price.unwrap().to_f64().unwrap();
    assert!((sell_px - 104.0).abs() < 1e-6, "ref=建仓价100, 卖=100×1.04=104: {sell_px}");
    let buy = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Buy)
        .expect("应含网格买单");
    let buy_px = buy.price.unwrap().to_f64().unwrap();
    assert!((buy_px - 100.0 / 1.04).abs() < 1e-6, "买=100÷1.04: {buy_px}");
}

#[test]
fn test_paired_grid_futures_long_on_fill_no_order_keeps_rehang() {
    // 036: 资金/名义不足时 on_fill 返回空表, need_rehang 保持(下一 bar on_tick 继续重试)。
    // min_notional=10.3: 网格买名义恒=order_amount=10 < 10.3 跳过; 建仓 lot 卖名义
    // 10×1.04=10.4 ≥ 10.3 能挂出 -> 卖出后栈空, 两手皆空。
    let cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(20.0)),
        ("order_amount", ConfigValue::Float(10.0)),
        ("min_notional", ConfigValue::Float(10.3)),
    ]);
    let bars: Vec<Kline> =
        [100.0, 105.0, 105.0].iter().enumerate().map(|(h, px)| bar_at_f(h as i64, *px)).collect();
    let batches = run_event_loop(cfg, &bars);

    // bar0: 建仓成交 -> on_fill 重挂: 买 10<10.3 跳过, 仅卖出建仓 lot(@104)。
    let b0: Vec<&Vec<OrderRequest>> =
        batches.iter().filter(|(i, _)| *i == 0).map(|(_, o)| o).collect();
    assert_eq!(b0.len(), 1, "bar0 建仓成交应恰触发 1 批 on_fill: {batches:?}");
    assert!(
        b0[0].iter().any(|o| o.side == OrderSide::Sell),
        "bar0 应挂出建仓 lot 的配对卖单: {:?}",
        b0[0]
    );
    // bar1: 卖出建仓 lot(104)成交 -> on_fill 重挂下一 lot 卖@104 (038 R1: 链内限价单
    // 次 bar 生效, **不再同 bar 链式再成交**) -> 恰 1 批。
    let b1: Vec<&Vec<OrderRequest>> =
        batches.iter().filter(|(i, _)| *i == 1).map(|(_, o)| o).collect();
    assert_eq!(b1.len(), 1, "bar1 应恰 1 批 on_fill(链内限价次 bar 生效): {batches:?}");
    assert!(b1[0].iter().any(|o| o.side == OrderSide::Sell), "应重挂栈顶卖单: {:?}", b1[0]);
    // bar2: 次 bar 入簿的卖@104 成交 -> 第二次 on_fill 栈空, 两手皆空 -> 空表(保持 need_rehang)。
    let b2: Vec<&Vec<OrderRequest>> =
        batches.iter().filter(|(i, _)| *i == 2).map(|(_, o)| o).collect();
    assert_eq!(b2.len(), 1, "bar2 应有 1 批 on_fill: {batches:?}");
    assert!(b2[0].is_empty(), "栈空后无单可挂应返回空表(保持 need_rehang): {:?}", b2[0]);
}

#[test]
fn test_paired_grid_futures_long_on_fill_min_pair_profit_floor() {
    // 036: min_pair_profit 保底在 on_fill 重挂中仍生效 —— 卖价 = max(栈顶×(1+up), 栈顶×(1+mpp))。
    let cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("min_pair_profit", ConfigValue::Float(0.10)), // 10% > up 4% -> 保底接管
    ]);
    let bars: Vec<Kline> =
        [100.0, 100.0, 96.0].iter().enumerate().map(|(h, px)| bar_at_f(h as i64, *px)).collect();
    let batches = run_event_loop(cfg, &bars);
    let (i, re) = batches.last().expect("买成交应触发 on_fill 重挂");
    assert_eq!(*i, 2);
    let sell = re
        .iter()
        .find(|o| o.action == OrderAction::Place && o.side == OrderSide::Sell)
        .expect("应有卖单");
    let sell_px = sell.price.unwrap().to_f64().unwrap();
    // 成交@96 -> 栈顶成本 96: max(96×1.04, 96×1.10) = 96×1.10。
    assert!(
        (sell_px - 96.0 * 1.10).abs() < 1e-3,
        "卖价应被 min_pair_profit 抬到栈顶成本×1.10: {sell_px}"
    );
}

#[test]
fn test_paired_grid_futures_long_grid_pair_cycle() {
    // 网格配对闭环: 激活建仓 -> 下跌网格买成交(flag=-1) -> 反弹配对卖成交(flag=0)。
    // 036: 走事件模型主循环(on_fill 重挂订单同 bar place)。
    let cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(10.0)),
    ]);
    let bars = vec![
        bar_at_f(0, 100.0), // 穿越激活: 市价买入 0.1 @100; on_fill 同 bar 重挂买@96.15
        bar_at_f(1, 96.0), // 网格买@96.15 成交@96(flag=-1); on_fill 重挂: 买@91.60 + 配对卖@99.84(锚成本96×1.04)
        bar_at_f(2, 96.0), // 静默
        bar_at_f(3, 104.0), // 配对卖@99.84 成交(建仓 lot 平仓不计 flag)
        bar_at_f(4, 104.0),
    ];
    let (batches, placed, fills) = run_event_full(cfg, &bars);
    let all_placed: Vec<Vec<OrderRequest>> = placed.iter().map(|(_, o)| o.clone()).collect();
    assert!(
        ps_orders(&all_placed, "short").is_empty(),
        "只做多策略 -> 不得有任何 position_side=short 挂单"
    );
    assert!(
        ps_orders(&all_placed, "long")
            .iter()
            .any(|o| o.side == OrderSide::Buy && o.order_type == OrderType::Market),
        "应穿越激活并市价建仓"
    );
    // 逐笔轨迹: 建仓(买市价@100) + 网格买(@96) + 配对卖@99.84(bar3) + 链式卖@104(bar4) ——
    // 卖@99.84 成交 -> on_fill 同 bar 重挂栈顶(建仓 lot)卖@104, 但 038 R1 链内限价单
    // **次 bar 生效** -> 在 bar4 成交(栈清空; 建仓 lot 平仓不计 flag)。
    assert_eq!(fills.len(), 4, "建仓+网格买+配对卖+次 bar 链式卖应恰 4 笔成交: {fills:?}");
    assert_eq!(fills[0], (0, OrderSide::Buy, 0.1, 100.0));
    assert_eq!(fills[1].0, 1, "网格买应在 bar1 成交");
    assert_eq!(fills[1].1, OrderSide::Buy);
    assert_eq!(fills[2].0, 3, "配对卖应在 bar3 成交");
    assert_eq!(fills[2].1, OrderSide::Sell);
    assert_eq!(fills[3].0, 4, "链式卖应次 bar(bar4)成交(038 R1 保守化)");
    assert_eq!(fills[3].1, OrderSide::Sell);
    // 每笔成交都伴随 on_fill 重挂批次(事件模型核心行为)。
    for f in &fills {
        assert!(batches.iter().any(|(i, _)| i == &f.0), "bar{} 成交后应有 on_fill 重挂批次", f.0);
    }
}

#[test]
fn test_paired_grid_futures_long_above_start_waits() {
    // 价格高于 start_price: 不激活, 零挂单, 不报错(等待穿越)。
    let cfg = futures_cfg(&[("start_price", ConfigValue::Float(100.0))]);
    let bars = flat_main(4, 120);
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, None);
    assert!(orders.iter().all(|o| o.is_empty()), "未激活 -> 不得下任何单");
    assert_eq!(st.global_f64("fatal"), Some(0.0), "等待激活不是错误");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "无成交");
}

#[test]
fn test_paired_grid_futures_long_missing_start_price_halts() {
    // 缺 start_price(≤0) -> 启动停机(同现货语义), 零挂单。
    let cfg = futures_cfg(&[]);
    let bars = flat_main(6, 100);
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, None);
    assert!(orders.iter().all(|o| o.is_empty()), "缺必填参数 -> 不得下任何单");
    assert_eq!(st.global_f64("fatal"), Some(1.0), "应置停机标记");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "无成交");
}

#[test]
fn test_paired_grid_futures_long_cost_gate_halts() {
    // 成本门槛: spacing_pct(0.05%) ≤ 2×fee_side(0.1%) -> 首次校验停机。
    let cfg = futures_cfg(&[
        ("spacing_pct", ConfigValue::Float(0.0005)),
        ("start_price", ConfigValue::Float(100.0)),
    ]);
    let bars = flat_main(4, 100);
    let (orders, _ctx, st) = run_accum_full(cfg, &bars, None);
    assert!(orders.iter().all(|o| o.is_empty()), "成本门槛不满足必须停机且不下任何单");
    assert_eq!(st.global_f64("fatal"), Some(1.0), "应置停机标记");
}

// ============================================================================
// 032 v2 验收: 合约配对做多 vs 现货 paired_grid 等价性
// (用户核心诉求: 行为逻辑与现货完全一致 -> 同 K 线同参数逐笔成交轨迹一致)
// ============================================================================

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

/// 逐笔成交轨迹: (bar 序号, 方向, 数量, 价格)。
type FillTrace = Vec<(usize, OrderSide, f64, f64)>;

/// 跑完整序列并按 bar 收集逐笔成交, 供两条腿逐笔对照。
/// 036: 复刻引擎事件驱动主循环(step_bar → fill 先入账/on_fill 闭环 → on_tick → place)。
fn run_collect_fills(cfg: StrategyConfig, bars: &[Kline]) -> FillTrace {
    run_event_full(cfg, bars).2
}

#[test]
fn test_paired_grid_futures_long_matches_spot_fill_by_fill() {
    // 合约只多头 vs 现货 paired_grid, 同 K 线同参数: 网格机制逐笔一致 ——
    // 现货成交轨迹必须是合约轨迹的**前缀**(bar 序号/方向/数量/价格逐笔一致)。
    // 分叉点 = 现货第 10 条「无仓无单即结束」语义(v2 合约版不移植, 用户 2026-09-26 口径:
    // "任何一单成交 → ref=成交价 → 全撤重挂"= 网格持续运行): 现货跑完首个完整配对循环后
    // 不再交易, 合约版继续追踪+挂买。费用口径差异只允许落在净值, 不允许落在公共前缀轨迹。
    // 路径(initial_buy_amount=0, 无建仓): 120 横盘 2 根(>110 不激活) → 100 穿越激活
    // → 96 网格买成交(fill#1, ref=96.15) → 101 配对卖成交(fill#2, 栈清空+持仓=0
    //   → 现货第 10 条 finished, 残留买单 92.455 在尾段 95 不会成交)
    // → 95: 合约版(finished 不移植)重挂买 96.15 成交(fill#3, 网格持续运行)。
    let path: Vec<Kline> = [120.0, 120.0, 100.0, 96.0, 96.0, 101.0, 95.0, 95.0]
        .iter()
        .enumerate()
        .map(|(h, px)| bar_at_f(h as i64, *px))
        .collect();

    let spot_cfg = paired_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(0.0)),
        ("fee_side", ConfigValue::Float(0.0005)),
    ]);
    let fut_cfg = futures_cfg(&[
        ("start_price", ConfigValue::Float(110.0)),
        ("initial_buy_amount", ConfigValue::Float(0.0)),
        ("fee_side", ConfigValue::Float(0.0005)),
    ]);

    let spot_fills = run_collect_fills(spot_cfg, &path);
    let fut_fills = run_collect_fills(fut_cfg, &path);

    assert_eq!(spot_fills.len(), 2, "现货应 一买一卖后即结束: {:?}", spot_fills);
    assert!(
        fut_fills.len() > spot_fills.len(),
        "现货结束后合约版应继续成交(网格持续运行): spot={} fut={}",
        spot_fills.len(),
        fut_fills.len()
    );
    for (i, sf) in spot_fills.iter().enumerate() {
        let ff = &fut_fills[i];
        assert_eq!(sf.0, ff.0, "第 {i} 笔成交 bar 不一致: spot={sf:?} fut={ff:?}");
        assert_eq!(sf.1, ff.1, "第 {i} 笔成交方向不一致: spot={sf:?} fut={ff:?}");
        assert_eq!(sf.3, ff.3, "第 {i} 笔成交价不一致: spot={sf:?} fut={ff:?}");
        // 现货 cap_sell 有 1e-9 比例削裁(防超卖铁律), 合约版 cap_close 不自砍 → 数量差恰为相对 1e-9,
        // 容差放宽到 1e-8 容纳这一已知的唯一数量口径差异。
        let rel = (sf.2 - ff.2).abs() / sf.2.max(1e-12);
        assert!(rel < 1e-8, "第 {i} 笔成交数量不一致: spot={sf:?} fut={ff:?}");
    }
}

// ============================================================================
// 033 现货 香农网格 (shannon_grid) 集成测试
// ============================================================================

const SHANNON_GRID: &str = include_str!("../../../strategies/spot/shannon_grid.lua");

fn univ2_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("atr_mult", ConfigValue::Float(1.0)),
        ("min_notional", ConfigValue::Float(5.0)),
        ("fee_side", ConfigValue::Float(0.001)),
        ("invest_cash", ConfigValue::Float(10000.0)),
    ];
    params.extend_from_slice(extra);
    config(SHANNON_GRID, &params)
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

#[test]
fn test_shannon_grid_requires_tf_atr_channel() {
    // 高周期 ATR 通道未预装 → 一笔都不下(不猜 ATR 值, 不激活不建仓)。
    let (orders, _ctx, st) = run_univ2(
        univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]),
        &univ2_main(60, 200, 100, 10),
        None,
    );
    assert!(orders.iter().all(|o| o.is_empty()), "ATR 通道未就绪时必须完全不动");
    assert_eq!(st.global_f64("balance_price"), None, "不得激活");
}

#[test]
fn test_shannon_grid_requires_start_price() {
    let (orders, _ctx, st) =
        run_univ2(univ2_cfg(&[]), &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert!(orders.iter().all(|o| o.is_empty()), "缺必填 start_price 时不得下任何单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "缺参数 -> 无成交");
    assert!(
        st.state_snapshot().iter().any(|(k, v)| k == "halted" && v == "1"),
        "缺必填参数应 FATAL 停机"
    );
}

#[test]
fn test_shannon_grid_thin_spacing_halts() {
    // 成本门槛: 生效间距/价格 < 4×fee_side → [FATAL] 停机, 不建仓不挂单。
    // min_spacing_pct=-1 禁用下限(否则默认 0.4% 下限会托起间距绕过门槛)。
    let (orders, _ctx, st) = run_univ2(
        univ2_cfg(&[
            ("start_price", ConfigValue::Float(150.0)),
            ("atr_mult", ConfigValue::Float(0.0001)),
            ("min_spacing_pct", ConfigValue::Float(-1.0)),
        ]),
        &univ2_main(60, 200, 100, 10),
        Some(tf_bars(60)),
    );
    assert!(orders.iter().all(|o| o.is_empty()), "成本门槛不满足时必须停机且不下任何单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "不得建仓");
    assert!(st.state_snapshot().iter().any(|(k, v)| k == "halted" && v == "1"));
}

#[test]
fn test_shannon_grid_min_spacing_floor() {
    // 最小间距下限: atr_mult×ATR = 0.0002 远小于下限 0.01×价格 = 1.0
    // → 生效间距 = 1.0, 挂单 99/101(而非 99.9998/100.0002)。
    let cfg = univ2_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("atr_mult", ConfigValue::Float(0.0001)),
        ("min_spacing_pct", ConfigValue::Float(0.01)),
    ]);
    // h≥11 改纯平段(100,100,100,100): 间距 1.0 的挂单 99/101 不会被平段 high/low 触发,
    // 便于只断言挂单价格(ATR 来自 aux 序列, 主序列振幅此处无关)。
    let bars: Vec<Kline> = univ2_main(40, 200, 100, 10)
        .into_iter()
        .enumerate()
        .map(|(i, k)| if i >= 11 { bar_at_hour(i as i64, 100, 100, 100, 100) } else { k })
        .collect();
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓价 100 = 第一平衡价");
    // 038 R3: 建仓成交 on_fill 同 bar 立即重挂(batch 记录在 fill bar 15; 链内限价单由
    // 引擎 defer 次 bar 生效, 不参与本 bar 撮合)。
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(99)), "买价 = 平衡价 − 生效间距(下限 1.0)");
    assert_eq!(sell.price, Some(dec!(101)), "卖价 = 平衡价 + 生效间距(下限 1.0)");
}

#[test]
fn test_shannon_grid_activate_half_build_and_grid() {
    // 激活: 市价买 invest_cash/2 = 5000 名义; 成交价 100 = 第一平衡价;
    // 重挂: 平衡价 ± 1×ATR(=2) = 98 / 102, 量按"成交后 1:1 恢复"(含费修正)。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let market: Vec<_> =
        orders.iter().flatten().flatten().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(market.len(), 1, "激活时应恰有一笔市价建仓");
    assert_eq!(market[0].side, OrderSide::Buy);
    let notional = market[0].size * market[0].price.unwrap_or(dec!(100));
    assert!((notional - dec!(5000)).abs() < dec!(1), "建仓名义应为投入一半 5000: {notional}");
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓成交价应成为第一平衡价");
    assert_eq!(st.global_f64("invested0"), Some(10000.0));

    // 建仓成交后的重挂(038 R3: on_fill 同 bar 返回, 记录在 fill bar 15): 买 98 / 卖 102
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    assert_eq!(limits.len(), 2, "两侧都应挂单: {:?}", limits);
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(98)), "买价 = 平衡价 − 1×ATR");
    assert_eq!(sell.price, Some(dec!(102)), "卖价 = 平衡价 + 1×ATR");
    // 量: C=10000−5000−5(费)=4995, Q=50 → 买 q=(4995−50×98)/(98×2.001); 卖 q=(50×102−4995)/(102×1.999)
    let q_buy_exp = (4995.0 - 50.0 * 98.0) / (98.0 * 2.001);
    let q_sell_exp = (50.0 * 102.0 - 4995.0) / (102.0 * 1.999);
    let q_buy = buy.size.to_f64().expect("size 应可转 f64");
    let q_sell = sell.size.to_f64().expect("size 应可转 f64");
    assert!((q_buy - q_buy_exp).abs() < 1e-6, "买量应为 1:1 恢复量: {q_buy} vs {q_buy_exp}");
    assert!((q_sell - q_sell_exp).abs() < 1e-6, "卖量应为 1:1 恢复量: {q_sell} vs {q_sell_exp}");
}

#[test]
fn test_shannon_grid_buy_fill_restores_one_to_one() {
    // 买 98 成交后: 重挂 96/100(fill 先于 on_tick 派发 → 同 bar on_tick 重挂)。
    // bar18 收窄为 [97,99](high < 卖 100), 只触发买侧 —— 宽 bar 双侧场景由下方测试专测。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 99, 99, 97, 98);
    let (orders, ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 买 98 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(98.0), "平衡价 := 买成交价");
    let c = ctx.balance("USDT").expect("应有 USDT 余额");
    let q = ctx.position("ETHUSDT").expect("应有持仓").size;
    let diff = c - q * dec!(98);
    assert!(diff.abs() < dec!(0.01), "成交后应恢复 1:1: C={c} Q={q} |C−Q×98|={diff}");
    // 同 bar on_tick 重挂: 买 96 / 卖 100(不成交, bar high=99)
    let limits: Vec<_> = orders[18]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(96)));
    assert_eq!(sell.price, Some(dec!(100)));
}

#[test]
fn test_shannon_grid_sell_fill_restores_one_to_one() {
    // 卖 102 成交后: 重挂 100/104(fill 先于 on_tick 派发 → 同 bar on_tick 重挂)。
    // bar18 收窄为 [101,103](low > 买 100), 只触发卖侧。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 102, 103, 101, 102);
    let (orders, ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 卖 102 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(102.0), "平衡价 := 卖成交价");
    let c = ctx.balance("USDT").expect("应有 USDT 余额");
    let q = ctx.position("ETHUSDT").expect("应有持仓").size;
    let diff = c - q * dec!(102);
    assert!(diff.abs() < dec!(0.01), "成交后应恢复 1:1: C={c} Q={q} |C−Q×102|={diff}");
    // 同 bar on_tick 重挂: 买 100 / 卖 104(均不成交, bar 范围 [101,103])
    let limits: Vec<_> = orders[18]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(100)));
    assert_eq!(sell.price, Some(dec!(104)));
}

#[test]
fn test_shannon_grid_on_fill_chain_capped_at_engine_depth() {
    // 038 R1+R3: on_fill 立即重挂已恢复, 但链内限价单**次 bar 生效**(引擎 defer 开关) ——
    // 宽 bar [97,100] 触发买 98 成交后, 重挂的 买 96/卖 100 不再被同 bar 回溯匹配,
    // 乒乓链从机制上消失(037 病理修复的直接锁形测试)。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 100, 100, 97, 99);
    let (orders, ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(
        st.global_f64("fill_count"),
        Some(2.0),
        "建仓 1 + 宽 bar 买 98 各 1; 重挂单次 bar 生效, 本 bar 不再成交"
    );
    assert_eq!(st.global_f64("balance_price"), Some(98.0), "平衡价 := 买成交价");
    // 重挂批次记录在 fill bar 18(本 bar 未撮合)
    let limits: Vec<_> = orders[18]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(96)));
    assert_eq!(sell.price, Some(dec!(100)));
    // 1:1 恢复仍在成交价成立
    let c = ctx.balance("USDT").expect("应有 USDT 余额");
    let q = ctx.position("ETHUSDT").expect("应有持仓").size;
    let diff = c - q * dec!(98);
    assert!(diff.abs() < dec!(0.05), "成交后 1:1 仍近似成立: C={c} Q={q} diff={diff}");
}

#[test]
fn test_shannon_grid_no_position_activation_only_buy() {
    // invest_cash=4 → 建仓名义 2 < min_notional 5 → 按无建仓激活: 平衡价=现价, 只挂买单不挂卖单。
    let cfg = univ2_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("invest_cash", ConfigValue::Float(4.0)),
    ]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "无建仓 -> 无成交");
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "平衡价 := 激活时现价");
    let limits: Vec<_> = orders[16]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let sells: Vec<_> = limits.iter().filter(|o| o.side == OrderSide::Sell).collect();
    let buys: Vec<_> = limits.iter().filter(|o| o.side == OrderSide::Buy).collect();
    assert!(sells.is_empty(), "无持仓不得挂卖单");
    assert_eq!(buys.len(), 1, "应只挂买单");
    assert_eq!(buys[0].price, Some(dec!(98)));
}

#[test]
fn test_shannon_grid_state_snapshot() {
    // 状态快照(断点续接最小必需项): balance_price / built / pending_entry / invested0。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (_orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let snap = st.state_snapshot();
    let get = |k: &str| snap.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    assert_eq!(get("balance_price").as_deref(), Some("100.0000000000"));
    assert_eq!(get("built").as_deref(), Some("1"));
    assert_eq!(get("pending_entry").as_deref(), Some("0"));
    assert_eq!(get("invested0").as_deref(), Some("10000.00"));
}

#[test]
fn test_shannon_grid_ledger_cross_check() {
    // runner 时序(含预热跳过): 激活建仓 → 买 98 → 卖 100; 停机时策略模型账本 vs 引擎误差 < 0.01。
    let cfg = univ2_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let mut bars = univ2_main(60, 200, 100, 24);
    // low=97 触发买 98; 重挂卖 100 在下一根振幅 bar(h=101)成交 —— 事件模型下
    // 重挂单持久挂出, 与旧 bar 节流口径的成交集合一致(时点后移一根 bar)。
    bars[40] = bar_at_hour(40, 99, 99, 97, 98);
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(10000), locked: Decimal::ZERO },
    );
    ctx.set_tf_klines("ETHUSDT", "1h", tf_bars(60));
    strategy.on_init(&mut ctx);
    // 预热跳过 24 根(aux 1h×24 根 / 主时钟 1h), 与装配层一致; 037 事件模型递归。
    for k in &bars[24..] {
        ctx.step_bar(k.clone());
        let mut sink: Vec<Vec<Vec<OrderRequest>>> = vec![Vec::new()];
        for f in ctx.drain_fills() {
            let follow = strategy.on_fill(&mut ctx, f);
            settle_univ2(&mut ctx, &mut strategy, follow, &mut sink, 1);
        }
        let orders = strategy.on_tick(&mut ctx);
        settle_univ2(&mut ctx, &mut strategy, orders, &mut sink, 0);
    }
    strategy.on_stop(&mut ctx);
    let snap: HashMap<String, String> = strategy.state_snapshot().into_iter().collect();
    let stat = |k: &str| snap.get(k).cloned().unwrap_or_default();
    assert_eq!(
        stat("stat_fill_count"),
        "3",
        "应恰有 建仓+买98+卖100 3 笔成交: {}",
        stat("stat_fill_count")
    );
    let d_cash: f64 = stat("stat_ledger_diff_cash").parse().expect("stat_ledger_diff_cash");
    let d_pos: f64 = stat("stat_ledger_diff_pos").parse().expect("stat_ledger_diff_pos");
    assert!(d_cash.abs() < 0.01, "账本现金差应 < 0.01: {d_cash}");
    assert!(d_pos.abs() < 0.01, "账本持仓差应 < 0.01: {d_pos}");
    // 期末 1:1: 现金 ≈ 持仓 × 平衡价(100, 最后一笔 = 卖 100)
    let c = ctx.balance("USDT").expect("应有 USDT 余额");
    let q = ctx.position("ETHUSDT").expect("应有持仓").size;
    let diff = c - q * dec!(100);
    assert!(diff.abs() < dec!(0.01), "期末应恢复 1:1: C={c} Q={q} |C−Q×100|={diff}");
}

// ============================================================================
// 040 合约香农网格 (shannon_grid_futures) 集成测试
// 母本 = 现货 shannon_grid(复用 univ2_main/with_bar/settle_univ2/run_univ2 时序),
// 合约化 = futures_cfg 同款 hedge + [backtest].leverage。
// ============================================================================

const SHANNON_GRID_FUT: &str =
    include_str!("../../../strategies/futures/shannon_grid_futures.lua");

/// 合约 hedge 配置(默认: invest_cash=10000, 杠杆 2 → 总资金 20000, 建仓名义 10000)。
fn shannon_fut_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
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
fn test_shannon_grid_futures_leverage_bounds_halts() {
    // 杠杆越界(<1 或 >5)→ FATAL 停机, 不建仓不挂单。
    for lev in [0.5f64, 8.0f64] {
        let cfg = shannon_fut_cfg(&[
            ("start_price", ConfigValue::Float(150.0)),
            ("leverage", ConfigValue::Float(lev)),
        ]);
        let (orders, _ctx, st) =
            run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
        assert!(orders.iter().all(|o| o.is_empty()), "杠杆 {lev} 越界必须停机且不下任何单");
        assert_eq!(st.global_f64("fatal"), Some(1.0), "杠杆 {lev} 应置停机标记");
        assert_eq!(st.global_f64("fill_count"), Some(0.0), "杠杆 {lev} 不得建仓");
    }
}

#[test]
fn test_shannon_grid_futures_activate_builds_half_of_total() {
    // 激活: 市价开多 v_total/2 = 10000×2/2 = 10000 名义(position_side=long, 无 short 侧);
    // 虚拟账本初始化: v_pos=100, v_cash = 20000 − 10000 − 费(5) = 9995。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let all: Vec<_> = orders.iter().flatten().flatten().collect();
    assert!(
        all.iter().all(|o| o.position_side.as_deref() != Some("short")),
        "只做多策略 -> 不得有任何 position_side=short 挂单"
    );
    let market: Vec<_> = all.iter().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(market.len(), 1, "激活时应恰有一笔市价开多建仓");
    assert_eq!(market[0].side, OrderSide::Buy);
    assert_eq!(market[0].position_side.as_deref(), Some("long"), "建仓单必须带 position_side=long");
    let notional = market[0].size * market[0].price.unwrap_or(dec!(100));
    assert!((notional - dec!(10000)).abs() < dec!(1), "建仓名义应为总资金一半 10000: {notional}");
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓成交价应成为第一平衡价");
    assert_eq!(st.global_f64("invested0"), Some(10000.0));
    let v_pos = st.global_f64("v_pos").expect("应有虚拟仓位");
    let v_cash = st.global_f64("v_cash").expect("应有虚拟现金");
    assert!((v_pos - 100.0).abs() < 1e-6, "虚拟仓位应=建仓量 100: {v_pos}");
    assert!((v_cash - 9995.0).abs() < 0.01, "虚拟现金应=总资金−名义−费=9995: {v_cash}");
    // 引擎实际多头仓位与虚拟仓位一致
    let long_sz = ctx.position_directional("ETHUSDT", OrderSide::Buy).map(|p| p.size);
    assert!(long_sz.map(|s| (s.to_f64().unwrap() - v_pos).abs() < 1e-6).unwrap_or(false),
        "引擎多头仓位 {long_sz:?} 应与虚拟仓位一致");
}

#[test]
fn test_shannon_grid_futures_build_rehangs_atr_spacing() {
    // 建仓成交后立即重挂: 间距 = atr_mult(1.5)×ATR(2) = 3 → 买 97 / 卖 103;
    // 量按虚拟账本 1:1 恢复公式(v_cash=9995, v_pos=100)。
    // delev_cap_mult=-1 隔离降杠杆(默认开启会放大卖量), 本用例专测 1:1 公式本身。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("delev_cap_mult", ConfigValue::Float(-1.0)),
    ]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("balance_price"), Some(100.0));
    // 建仓成交 on_fill 同 bar 立即重挂(038 事件模型; 链内限价单引擎 defer 次 bar 生效)
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    assert_eq!(limits.len(), 2, "两侧都应挂单: {:?}", limits);
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有开多买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有平多卖单");
    assert_eq!(buy.price, Some(dec!(97)), "买价 = 平衡价 − 1.5×ATR");
    assert_eq!(sell.price, Some(dec!(103)), "卖价 = 平衡价 + 1.5×ATR");
    assert_eq!(buy.position_side.as_deref(), Some("long"));
    assert_eq!(sell.position_side.as_deref(), Some("long"));
    let q_buy_exp = (9995.0 - 100.0 * 97.0) / (97.0 * (2.0 + 0.0005));
    let q_sell_exp = (100.0 * 103.0 - 9995.0) / (103.0 * (2.0 - 0.0005));
    let q_buy = buy.size.to_f64().expect("size 应可转 f64");
    let q_sell = sell.size.to_f64().expect("size 应可转 f64");
    assert!((q_buy - q_buy_exp).abs() < 1e-6, "买量应为虚拟 1:1 恢复量: {q_buy} vs {q_buy_exp}");
    assert!((q_sell - q_sell_exp).abs() < 1e-6, "卖量应为虚拟 1:1 恢复量: {q_sell} vs {q_sell_exp}");
}

/// 5x 杠杆配置(041 降杠杆测试): invest 10000 × 5 = 总资金 50000, 建仓名义 25000(250@100),
/// V0 = 建仓价值 = 25000。
fn shannon_fut_cfg_lev5(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("start_price", ConfigValue::Float(150.0)),
        ("leverage", ConfigValue::Float(5.0)),
    ];
    params.extend_from_slice(extra);
    let mut cfg = shannon_fut_cfg(&params);
    if let Some(b) = cfg.backtest.as_mut() {
        b.leverage = Some(5.0);
    }
    cfg
}

#[test]
fn test_shannon_grid_futures_delev_sell_caps_to_initial_value() {
    // 041 v2 卖出侧降杠杆(锚定建仓价值): 5x 建仓 250@100, V0=25000; 挂卖 103 时仓位名义
    // 250×103=25750 > V0 → 超额 750 随卖单一并卖掉:
    // q_delev = (25750 − 25000)/(103×(1−0.0005)) = 7.2852 > 1:1 量 3.7024 → 取降杠杆量,
    // 仓位名义压回 V0; 计数与超额名义导出。
    let cfg = shannon_fut_cfg_lev5(&[]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price == Some(dec!(103)))
        .collect();
    assert_eq!(limits.len(), 1, "应有卖 103 挂单");
    let q_sell = limits[0].size.to_f64().expect("size 应可转 f64");
    // 建仓: 250@100, 费 0.05%×25000=12.5 -> v_cash = 50000 − 25000 − 12.5 = 24987.5
    let q_1to1 = (250.0 * 103.0 - 24987.5) / (103.0 * (2.0 - 0.0005));
    let q_delev = (250.0 * 103.0 - 25000.0) / (103.0 * (1.0 - 0.0005));
    assert!(q_delev > q_1to1, "用例前提: 降杠杆量应大于 1:1 量");
    assert!((q_sell - q_delev).abs() < 1e-3, "卖量应为降杠杆量: {q_sell} vs {q_delev}");
    assert!(
        st.global_f64("delev_count").unwrap_or(0.0) >= 1.0,
        "应记录降杠杆卖单次数"
    );
    let extra = st.global_f64("delev_extra_notional").unwrap_or(0.0);
    let extra_exp = (q_delev - q_1to1) * 103.0;
    assert!((extra - extra_exp).abs() < 0.01, "超额名义应=增量卖量×卖价: {extra} vs {extra_exp}");
}

#[test]
fn test_shannon_grid_futures_delev_fill_trims_cash_back_to_one_to_one() {
    // 041 v3 收口: 消减卖成交后账本必须回 1:1 —— 超额回笼的现金从 v_cash 削掉(退出虚拟网格),
    // 否则下一格买单会按 1:1 公式把超额买回, 降杠杆被抵消(v2 教训)。
    // 5x: 建仓 250@100(v_cash=24987.5), 首格消减卖 7.2852@103 成交 -> 卖后 v_pos×103 = 25000,
    // v_cash 临时 25737.5 -> 削 737.5 -> v_cash = 25000 = v_pos×103, 严格 1:1。
    let cfg = shannon_fut_cfg_lev5(&[]);
    let mut bars = univ2_main(19, 200, 100, 10);
    // bar18 穿越 103: 只触发卖侧成交(low 101 > 买 97)
    bars.push(bar_at_hour(19, 102, 103, 101, 102));
    let (_orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert!(st.global_f64("fill_count").unwrap_or(0.0) >= 2.0, "应有建仓+消减卖成交");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let balance = st.global_f64("balance_price").expect("平衡价");
    assert!((balance - 103.0).abs() < 1e-6, "平衡价应为成交价 103: {balance}");
    assert!(
        (v_cash - v_pos * 103.0).abs() < 0.01,
        "卖后账本必须严格 1:1: v_cash {v_cash} vs v_pos×103 = {}",
        v_pos * 103.0
    );
    let q_delev = (250.0 * 103.0 - 25000.0) / (103.0 * (1.0 - 0.0005));
    let trimmed_exp = 24987.5 + q_delev * 103.0 - 25000.0; // 超额回笼 − 成交费(费率口径差 <1)
    let trimmed = st.global_f64("delev_trimmed").unwrap_or(0.0);
    assert!((trimmed - trimmed_exp).abs() < 1.0, "削减现金应=超额回笼−费: {trimmed} vs {trimmed_exp}");
    assert!(trimmed > 0.0, "削减量必须 > 0");
}

#[test]
fn test_shannon_grid_futures_delev_drawdown_no_trim() {
    // 回撤不消减(用户口径核心): 深跌后(现价 < 建仓价, 闸门关闭)不再新增消减。
    // 唯一一次消减发生在建仓后首格挂单(卖 103 可实现名义 25750 > V0=25000, 现价=建仓价
    // 闸门临界开启); 此后下跌全程 delev 计数/超额名义定格不变。
    let cfg = shannon_fut_cfg_lev5(&[]);
    let mut bars = univ2_main(19, 200, 100, 10);
    for i in 0..8 {
        let p = 98 - i * 3;
        bars.push(bar_at_hour(19 + i, p, p + 1, p - 4, p - 3));
    }
    let (_orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert!(st.global_f64("fill_count").unwrap_or(0.0) >= 2.0, "应有建仓+网格买入成交");
    let balance = st.global_f64("balance_price").expect("平衡价");
    assert!(balance < 100.0, "应处于回撤(平衡价 < 建仓价): {balance}");
    // 首格消减量(建仓重挂时一次性计入): 超额卖量 × 卖价
    let q_1to1 = (250.0 * 103.0 - 24987.5) / (103.0 * (2.0 - 0.0005));
    let q_delev = (250.0 * 103.0 - 25000.0) / (103.0 * (1.0 - 0.0005));
    let extra_exp = (q_delev - q_1to1) * 103.0;
    assert_eq!(
        st.global_f64("delev_count"),
        Some(1.0),
        "回撤中不得新增消减, 仅剩建仓首格那一次"
    );
    let extra = st.global_f64("delev_extra_notional").unwrap_or(0.0);
    assert!((extra - extra_exp).abs() < 0.01, "超额名义应定格在首格消减量: {extra} vs {extra_exp}");
}

#[test]
fn test_shannon_grid_futures_delev_disabled_keeps_one_to_one() {
    // delev_cap_mult=-1 禁用(0/缺失 = 默认 1, 与 num() 哨兵约定一致, 负数才是禁用):
    // 5x 下卖量仍为正常 1:1 量, 不触发降杠杆。
    let cfg = shannon_fut_cfg_lev5(&[("delev_cap_mult", ConfigValue::Float(-1.0))]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price == Some(dec!(103)))
        .collect();
    assert_eq!(limits.len(), 1, "应有卖 103 挂单");
    let q_sell = limits[0].size.to_f64().expect("size 应可转 f64");
    let q_1to1 = (250.0 * 103.0 - 24987.5) / (103.0 * (2.0 - 0.0005));
    assert!((q_sell - q_1to1).abs() < 1e-3, "禁用时卖量应为 1:1 量: {q_sell} vs {q_1to1}");
    assert_eq!(
        st.global_f64("delev_count"),
        Some(0.0),
        "禁用时不记录降杠杆计数"
    );
}

#[test]
fn test_shannon_grid_futures_delev_default_caps_at_2x_too() {
    // 默认开启对 2x 同样生效: 建仓 100@100, V0=10000; 挂卖 103 时仓位名义 10300 > 10000 →
    // 超额 300 消减: q_delev = 300/(103×(1−0.0005)) = 2.9141 > 1:1 量 1.4808。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let limits: Vec<_> = orders[15]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price == Some(dec!(103)))
        .collect();
    assert_eq!(limits.len(), 1, "应有卖 103 挂单");
    let q_sell = limits[0].size.to_f64().expect("size 应可转 f64");
    let q_delev = (100.0 * 103.0 - 10000.0) / (103.0 * (1.0 - 0.0005));
    assert!((q_sell - q_delev).abs() < 1e-3, "2x 默认也应消减到建仓价值: {q_sell} vs {q_delev}");
    assert!(st.global_f64("delev_count").unwrap_or(0.0) >= 1.0);
}

#[test]
fn test_shannon_grid_futures_buy_fill_restores_one_to_one() {
    // 买 97 成交后: 虚拟账本恢复 1:1(v_cash ≈ v_pos×97), 平衡价 := 97, 重挂 94/100。
    // bar18 收窄为 [97,99](high < 卖 103), 只触发买侧。delev_cap_mult=-1 隔离降杠杆。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("delev_cap_mult", ConfigValue::Float(-1.0)),
    ]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 99, 99, 97, 98);
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 买 97 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(97.0), "平衡价 := 买成交价");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    let diff = v_cash - v_pos * 97.0;
    // 容差 0.1: 1:1 公式假设费 = fee_side×名义, 但引擎对限价单收 maker 费 0.02% (< 0.05%),
    // 虚拟账本按真实 fill.fee 记账 -> 少扣费产生小额合法富余(方向恒正)。
    assert!(diff.abs() < 0.1, "成交后虚拟账本应近似 1:1: v_cash={v_cash} v_pos={v_pos} diff={diff}");
    // 重挂: 买 94 / 卖 100(均不成交, bar 范围 [97,99])
    let limits: Vec<_> = orders[18]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(94)));
    assert_eq!(sell.price, Some(dec!(100)));
}

#[test]
fn test_shannon_grid_futures_sell_fill_restores_one_to_one() {
    // 卖 103 成交后: 虚拟账本恢复 1:1(v_cash ≈ v_pos×103), 平衡价 := 103, 重挂 100/106。
    // bar18 收窄为 [101,103](low > 买 97), 只触发卖侧。delev_cap_mult=-1 隔离降杠杆
    // (默认开启时卖量含消减超额, 成交后账本现金偏重, 非本用例语义)。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("delev_cap_mult", ConfigValue::Float(-1.0)),
    ]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 102, 103, 101, 102);
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 卖 103 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(103.0), "平衡价 := 卖成交价");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    let diff = v_cash - v_pos * 103.0;
    // 容差 0.1: 同上, 引擎限价 maker 费 0.02% < fee_side 假设 0.05%, 少扣费产生小额合法富余。
    assert!(diff.abs() < 0.1, "成交后虚拟账本应近似 1:1: v_cash={v_cash} v_pos={v_pos} diff={diff}");
    // 重挂: 买 100 / 卖 106(均不成交, bar 范围 [101,103])
    let limits: Vec<_> = orders[18]
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    let buy = limits.iter().find(|o| o.side == OrderSide::Buy).expect("应有买单");
    let sell = limits.iter().find(|o| o.side == OrderSide::Sell).expect("应有卖单");
    assert_eq!(buy.price, Some(dec!(100)));
    assert_eq!(sell.price, Some(dec!(106)));
}

#[test]
fn test_shannon_grid_futures_underfunded_buys_recorded() {
    // 虚拟资金不足可观测: invest_cash=90000 远超实际余额 10000 → 建仓被 cap_open 砍到真实可用,
    // 虚拟账本仍按 v_total=180000 记账 → 首次重挂买量巨大, 估算保证金超真实可用 →
    // underfunded_buys ≥ 1(挂单照常挂出, 不砍量)。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("invest_cash", ConfigValue::Float(90000.0)),
    ]);
    let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓应完成");
    let uf = st.global_f64("underfunded_buys").expect("underfunded_buys");
    assert!(uf >= 1.0, "应有虚拟资金不足记录: {uf}");
    let notional = st.global_f64("underfunded_notional").expect("underfunded_notional");
    assert!(notional > 0.0, "资金不足名义应 > 0: {notional}");
}

#[test]
fn test_shannon_grid_futures_state_snapshot() {
    // 状态快照(断点续接最小必需项): balance_price / built / pending_entry / invested0 / v_cash。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (_orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let snap = st.state_snapshot();
    let get = |k: &str| snap.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    assert_eq!(get("balance_price").as_deref(), Some("100.0000000000"));
    assert_eq!(get("built").as_deref(), Some("1"));
    assert_eq!(get("pending_entry").as_deref(), Some("0"));
    assert_eq!(get("invested0").as_deref(), Some("10000.00"));
    assert!(get("v_cash").is_some(), "虚拟现金应持久化(续接重建虚拟账本)");
}

#[test]
fn test_shannon_grid_futures_liquidation_halts_and_tracks_min_dist() {
    // 040-L: 深跌使钱包权益穿越维持保证金 → 引擎 LIQ- 强平 fill → 策略停机不再交易;
    // 运行期最近爆仓距离 stat_liq_dist_min 应被记录且 > 0。
    // 2x 半仓(名义 10000, 保证金 5000, 仓 100@100)线性穿越模型爆仓价 ≈ 50.5;
    // 价格 100 → 30 必然触发。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    // 前 10 根 base=100(激活建仓), 之后每根跌 5 直到 30, 再放 3 根 30 的 bar 让强平结算完成
    let mut bars = univ2_main(10, 100, 100, 10);
    let mut h = 10i64;
    let mut p = 95i64;
    while p >= 30 {
        bars.push(bar_at_hour(h, p, p + 1, p - 1, p));
        h += 1;
        p -= 5;
    }
    for i in 0..3 {
        bars.push(bar_at_hour(h + i, 30, 31, 29, 30));
    }
    let n = bars.len() as i64;
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));

    // 爆仓被识别: LIQ fill 计数 ≥ 1, 停机标记置位
    assert!(st.global_f64("liq_count").unwrap_or(0.0) >= 1.0, "应识别引擎 LIQ- 强平 fill");
    assert_eq!(st.global_f64("fatal"), Some(1.0), "爆仓应置停机标记");
    let dmin = st.global_f64("liq_dist_min").expect("最近爆仓距离应被记录");
    assert!(dmin <= 0.1, "应逼近甚至穿越估算爆仓价(负值=现价已低于估算爆仓价): {dmin}");
    // 爆仓后虚拟账本仍逐分同步引擎(爆仓同 bar 在途买单会回测工件性成交, 实盘撤单即时无此问题)
    let eng_pos = _ctx
        .position_directional("ETHUSDT", OrderSide::Buy)
        .map(|p| p.size.to_f64().unwrap())
        .unwrap_or(0.0);
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    assert!((v_pos - eng_pos).abs() < 1e-6, "爆仓后虚拟持仓应与引擎一致: v_pos={v_pos} eng={eng_pos}");
    // 爆仓 bar 之后不再产生任何订单(全撤指令除外)
    let after: usize = orders[(n - 2) as usize..].iter().flatten().flatten().count();
    assert_eq!(after, 0, "爆仓停机后不得再挂单");
    // state 快照持久化 halted(重启不复活); 显式触发 on_stop 校验 stat 导出
    let (mut st, mut ctx2) = (st, _ctx);
    st.on_stop(&mut ctx2);
    let snap = st.state_snapshot();
    let halted = snap.iter().find(|(k, _)| k == "halted").map(|(_, v)| v.clone());
    assert_eq!(halted.as_deref(), Some("1"), "halted 应持久化(重启不复活)");
    // stat 导出齐备
    let get = |k: &str| snap.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    assert!(get("stat_liq_dist_min").is_some(), "stat_liq_dist_min 应导出");
    assert!(get("stat_liq_dist_min_ts").is_some(), "stat_liq_dist_min_ts 应导出");
    assert!(get("stat_liq_count").is_some(), "stat_liq_count 应导出");
    assert!(get("stat_liq_pnl").is_some(), "stat_liq_pnl 应导出");
}
