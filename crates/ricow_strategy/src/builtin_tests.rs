//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/spot/`(paired_grid 现货动态非对称网格 +
//! shannon_grid 现货 香农网格)与 `strategies/futures/`(paired_grid_futures_long),
//! include_str! 编译期嵌入。
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
#[allow(clippy::too_many_arguments)]
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
// 040 合约香农对冲网格 (shannon_hedge_grid_futures) 集成测试
// 母本 = 现货 shannon_grid(复用 univ2_main/with_bar/settle_univ2/run_univ2 时序),
// 合约化 = futures_cfg 同款 hedge + [backtest].leverage。
// ============================================================================

const SHANNON_HEDGE_GRID_FUT: &str =
    include_str!("../../../strategies/futures/shannon_hedge_grid_futures.lua");

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
    let mut cfg = config(SHANNON_HEDGE_GRID_FUT, &params);
    cfg.market = "futures".into();
    cfg.position_mode = "hedge".into();
    cfg.backtest = Some(crate::config::BacktestToml { leverage: Some(2.0), ..Default::default() });
    cfg
}

#[test]
fn test_shannon_hedge_grid_futures_leverage_bounds_halts() {
    // 杠杆越界(<1 或 >5)→ FATAL 停机, 不建仓不挂单。
    for lev in [0.5f64, 8.0f64] {
        let cfg = shannon_fut_cfg(&[
            ("start_price", ConfigValue::Float(150.0)),
            ("leverage", ConfigValue::Float(lev)),
        ]);
        let (orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
        assert!(orders.iter().all(|o| o.is_empty()), "杠杆 {lev} 越界必须停机且不下任何单");
        assert_eq!(st.global_f64("fatal"), Some(1.0), "杠杆 {lev} 应置停机标记");
        assert_eq!(st.global_f64("fill_count"), Some(0.0), "杠杆 {lev} 不得建仓");
    }
}

#[test]
fn test_shannon_hedge_grid_futures_activate_builds_half_of_total() {
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
    assert!(
        long_sz.map(|s| (s.to_f64().unwrap() - v_pos).abs() < 1e-6).unwrap_or(false),
        "引擎多头仓位 {long_sz:?} 应与虚拟仓位一致"
    );
}

#[test]
fn test_shannon_hedge_grid_futures_build_rehangs_atr_spacing() {
    // 建仓成交后立即重挂: 间距 = atr_mult(1.5)×ATR(2) = 3 → 买 97 / 卖 103;
    // 量按虚拟账本 1:1 恢复公式(v_cash=9995, v_pos=100)。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
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
    assert!(
        (q_sell - q_sell_exp).abs() < 1e-6,
        "卖量应为虚拟 1:1 恢复量: {q_sell} vs {q_sell_exp}"
    );
}

/// 5x 杠杆配置(测试辅助): invest 10000 × 5 = 总资金 50000, 建仓名义 25000(250@100)。
fn shannon_fut_cfg_lev5(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params =
        vec![("start_price", ConfigValue::Float(150.0)), ("leverage", ConfigValue::Float(5.0))];
    params.extend_from_slice(extra);
    let mut cfg = shannon_fut_cfg(&params);
    if let Some(b) = cfg.backtest.as_mut() {
        b.leverage = Some(5.0);
    }
    cfg
}

#[test]
fn test_shannon_hedge_grid_futures_buy_fill_restores_one_to_one() {
    // 买 97 成交后: 虚拟账本恢复 1:1(v_cash ≈ v_pos×97), 平衡价 := 97, 重挂 94/100。
    // bar18 收窄为 [97,99](high < 卖 103), 只触发买侧。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 99, 99, 97, 98);
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 买 97 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(97.0), "平衡价 := 买成交价");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    let diff = v_cash - v_pos * 97.0;
    // 容差 0.1: 1:1 公式假设费 = fee_side×名义, 但引擎对限价单收 maker 费 0.02% (< 0.05%),
    // 虚拟账本按真实 fill.fee 记账 -> 少扣费产生小额合法富余(方向恒正)。
    assert!(
        diff.abs() < 0.1,
        "成交后虚拟账本应近似 1:1: v_cash={v_cash} v_pos={v_pos} diff={diff}"
    );
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
fn test_shannon_hedge_grid_futures_sell_fill_restores_one_to_one() {
    // 卖 103 成交后: 虚拟账本恢复 1:1(v_cash ≈ v_pos×103), 平衡价 := 103, 重挂 100/106。
    // bar18 收窄为 [101,103](low > 买 97), 只触发卖侧。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let bars = with_bar(univ2_main(19, 200, 100, 10), 18, 102, 103, 101, 102);
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(st.global_f64("fill_count"), Some(2.0), "建仓 + 卖 103 各成交一次");
    assert_eq!(st.global_f64("balance_price"), Some(103.0), "平衡价 := 卖成交价");
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    let diff = v_cash - v_pos * 103.0;
    // 容差 0.1: 同上, 引擎限价 maker 费 0.02% < fee_side 假设 0.05%, 少扣费产生小额合法富余。
    assert!(
        diff.abs() < 0.1,
        "成交后虚拟账本应近似 1:1: v_cash={v_cash} v_pos={v_pos} diff={diff}"
    );
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
fn test_shannon_hedge_grid_futures_underfunded_buys_recorded() {
    // 虚拟资金不足可观测: invest_cash=90000 远超实际余额 10000 → 建仓被 cap_open 砍到真实可用,
    // 虚拟账本仍按 v_total=180000 记账 → 首次重挂买量巨大, 估算保证金超真实可用 →
    // underfunded_buys ≥ 1(挂单照常挂出, 不砍量)。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("invest_cash", ConfigValue::Float(90000.0)),
    ]);
    let (_orders, _ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("balance_price"), Some(100.0), "建仓应完成");
    let uf = st.global_f64("underfunded_buys").expect("underfunded_buys");
    assert!(uf >= 1.0, "应有虚拟资金不足记录: {uf}");
    let notional = st.global_f64("underfunded_notional").expect("underfunded_notional");
    assert!(notional > 0.0, "资金不足名义应 > 0: {notional}");
}

#[test]
fn test_shannon_hedge_grid_futures_state_snapshot() {
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
fn test_shannon_hedge_grid_futures_liquidation_halts_and_tracks_min_dist() {
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
    assert!(
        (v_pos - eng_pos).abs() < 1e-6,
        "爆仓后虚拟持仓应与引擎一致: v_pos={v_pos} eng={eng_pos}"
    );
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

#[test]
fn test_shannon_hedge_grid_futures_hedge_entry_opposes_80pct() {
    // 042 对冲腿(差异 #8): hedge_enabled=1 → 建仓成交后市价开 0.8×多头(100) = 80 空头
    // (position_side=short, market); 对冲腿与 1:1 虚拟账本隔离(v_cash/v_pos 不受空头影响)。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("hedge_enabled", ConfigValue::Float(1.0)),
    ]);
    let (orders, ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    let all: Vec<_> = orders.iter().flatten().flatten().collect();
    let shorts: Vec<_> =
        all.iter().filter(|o| o.position_side.as_deref() == Some("short")).collect();
    assert_eq!(shorts.len(), 1, "应恰有一笔对冲调整(建仓 80% 空头): {shorts:?}");
    let h = shorts[0];
    assert_eq!(h.order_type, OrderType::Market, "对冲调整必须市价");
    assert_eq!(h.side, OrderSide::Sell, "欠对冲 → 市价开空");
    let q = h.size.to_f64().unwrap();
    assert!((q - 80.0).abs() < 0.1, "空头量应 = 0.8×100: {q}");
    // 引擎空头仓建立且均值成本 ≈ 100
    let short = ctx.position_directional("ETHUSDT", OrderSide::Sell).expect("空头仓应存在");
    assert!((short.size.to_f64().unwrap() - 80.0).abs() < 0.1, "引擎空头仓应为 80");
    assert_eq!(st.global_f64("hedge_adjust_count"), Some(1.0));
    assert_eq!(st.global_f64("hedge_count"), Some(1.0));
    let havg = st.global_f64("hedge_avg_entry").expect("空头均值成本应记录");
    assert!((havg - 100.0).abs() < 0.1, "空头均值成本应≈成交价 100: {havg}");
    // 账本隔离: v_cash/v_pos 仍只描述多头网格
    let v_pos = st.global_f64("v_pos").expect("v_pos");
    let v_cash = st.global_f64("v_cash").expect("v_cash");
    assert!((v_pos - 100.0).abs() < 1e-6, "虚拟仓位应只含多头: {v_pos}");
    assert!((v_cash - 9995.0).abs() < 0.01, "虚拟现金不应被空头成交扰动: {v_cash}");
}

#[test]
fn test_shannon_hedge_grid_futures_hedge_disabled_by_default() {
    // 默认关闭: 行为与无对冲版本一致 —— 任何 position_side=short 订单/成交都不得出现。
    let cfg = shannon_fut_cfg(&[("start_price", ConfigValue::Float(150.0))]);
    let (orders, ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert!(
        orders.iter().flatten().flatten().all(|o| o.position_side.as_deref() != Some("short")),
        "默认关闭 → 不得有任何空头订单"
    );
    assert!(ctx.position_directional("ETHUSDT", OrderSide::Sell).is_none(), "不得持有空头仓");
    assert_eq!(st.global_f64("hedge_adjust_count"), Some(0.0));
}

#[test]
fn test_shannon_hedge_grid_futures_hedge_deadband_skips_small_deviation() {
    // 死区: 网格买入 97 成交后多头 100→101.52(+1.52%), 偏差 −1.5% < 5% → 不调整(仅建仓调整 1 次);
    // 同时验证对冲腿启用时网格交易照常进行。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("hedge_enabled", ConfigValue::Float(1.0)),
    ]);
    let mut bars = univ2_main(60, 200, 100, 10);
    bars[30] = bar_at_hour(30, 97, 98, 96, 97); // 穿越买 97
    let (orders, _ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert!(st.global_f64("fill_count").unwrap_or(0.0) >= 2.0, "网格买 97 应成交(建仓+买)");
    assert_eq!(st.global_f64("hedge_adjust_count"), Some(1.0), "偏差 <5% 死区内 → 不得再次调整");
    assert!(
        orders
            .iter()
            .flatten()
            .flatten()
            .all(|o| o.position_side.as_deref() != Some("short")
                || o.order_type == OrderType::Market),
        "死区内不得产生空头限价单"
    );
}

#[test]
fn test_shannon_hedge_grid_futures_hedge_min_notional_blocks_dust() {
    // dust 防护: hedge_min_notional 设为超大 → 一切调整被名义门槛拦下(含建仓 80%)。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("hedge_enabled", ConfigValue::Float(1.0)),
        ("hedge_min_notional", ConfigValue::Float(1e9)),
    ]);
    let (orders, ctx, st) = run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert!(
        orders.iter().flatten().flatten().all(|o| o.position_side.as_deref() != Some("short")),
        "名义不足 → 不得下空头单"
    );
    assert!(ctx.position_directional("ETHUSDT", OrderSide::Sell).is_none(), "不得持有空头仓");
    assert_eq!(st.global_f64("hedge_adjust_count"), Some(0.0));
}

#[test]
fn test_shannon_hedge_grid_futures_hedge_over_hedge_buys_back_after_sell() {
    // 超对冲回补: 5x 建仓 250@100 → 目标空头 200; 普通网格卖 3.7033@103 → 多头 246.297,
    // 新目标 197.04, 偏差 +1.5% > threshold 1% → 市价买回 2.96。
    let cfg = shannon_fut_cfg_lev5(&[
        ("hedge_enabled", ConfigValue::Float(1.0)),
        ("hedge_adjust_threshold", ConfigValue::Float(0.01)),
    ]);
    let mut bars = univ2_main(19, 200, 100, 10);
    bars.push(bar_at_hour(19, 102, 103, 101, 102));
    let (orders, ctx, st) = run_univ2(cfg, &bars, Some(tf_bars(60)));
    assert_eq!(
        st.global_f64("hedge_adjust_count"),
        Some(2.0),
        "建仓开空 + 网格卖后回补, 共 2 次调整"
    );
    let shorts: Vec<_> = orders
        .iter()
        .flatten()
        .flatten()
        .filter(|o| {
            o.position_side.as_deref() == Some("short") && o.order_type == OrderType::Market
        })
        .collect();
    assert_eq!(shorts.len(), 2, "应有两笔空头市价调整(开空+买回): {shorts:?}");
    assert_eq!(shorts[0].side, OrderSide::Sell, "第一笔 = 开空");
    assert_eq!(shorts[1].side, OrderSide::Buy, "第二笔 = 超对冲买回");
    let short = ctx.position_directional("ETHUSDT", OrderSide::Sell).expect("空头仓应存在");
    let target = 0.8 * 246.2967;
    assert!(
        (short.size.to_f64().unwrap() - target).abs() < 0.05,
        "回补后空头应≈0.8×新多头 {}: {}",
        target,
        short.size
    );
}

#[test]
fn test_shannon_hedge_grid_futures_hedge_short_liq_warns_without_halt() {
    // 空头腿爆仓(合成 LIQ-…-short fill): 只记数/损失 + WARN,**不停机**(fatal 不置位),
    // 网格主体继续; 已实现盈亏按空头均值成本口径扣减。
    let cfg = shannon_fut_cfg(&[
        ("start_price", ConfigValue::Float(150.0)),
        ("hedge_enabled", ConfigValue::Float(1.0)),
    ]);
    let (mut _orders, mut ctx, mut st) =
        run_univ2(cfg, &univ2_main(60, 200, 100, 10), Some(tf_bars(60)));
    assert_eq!(st.global_f64("hedge_avg_entry"), Some(100.0), "前提: 空头均值成本 100");
    let liq_fill = OrderFill {
        trade_id: Some("t1".into()),
        exchange_order_id: "ex1".into(),
        client_order_id: "LIQ-ETHUSDT-short".into(),
        pair: "ETHUSDT".into(),
        side: OrderSide::Buy, // 强平空头 = 买入平空(引擎"追加该方向"口径)
        fill_price: dec!(160),
        fill_size: dec!(80),
        fee: dec!(6.4),
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(60 * 3_600_000).unwrap(),
        position_side: Some("short".into()),
    };
    let follow = st.on_fill(&mut ctx, liq_fill);
    assert!(follow.is_empty(), "空头爆仓不产出订单(不整体停机)");
    assert_eq!(st.global_f64("hedge_liq_count"), Some(1.0), "应记录空头爆仓次数");
    assert_eq!(st.global_f64("fatal"), Some(0.0), "空头腿爆仓不得置停机标记");
    assert!(!st.halted(), "策略不得停机");
    let liq_pnl = st.global_f64("hedge_liq_pnl").expect("空头爆仓损失应记录");
    assert!((liq_pnl + 80.0 * 60.0).abs() < 1.0, "损失 = (160−100)×80 = 4800: {liq_pnl}");
    // stat 导出
    st.on_stop(&mut ctx);
    let snap = st.state_snapshot();
    let get = |key: &str| snap.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    assert!(get("stat_hedge_liq_count").is_some(), "stat_hedge_liq_count 应导出");
    assert!(get("stat_hedge_short_final").is_some(), "stat_hedge_short_final 应导出");
}

// ============================================================================
// 048 合约香农网格 (shannon_grid_futures) 集成测试 —— 去对冲 + 去降杠杆 + 方向 flag 间距放大
// (flag = 卖笔数 − 买笔数, 建仓与强平不计, 符号口径同 paired_grid; |flag|≥2 趋势侧间距 × mult^(|flag|−1))。
// 母本 = shannon_hedge_grid_futures(复用 univ2_main/with_bar/tf_bars/run_univ2 时序)。
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
    // flag=0(建仓不计)且默认 mult=1.2: |flag|≤1 不放大 —— 建仓/首挂与母本对冲网格(纯 1:1)逐值一致:
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

// ============================================================================
// 现货虚拟香农网格 (shannon_virtual_grid) 集成测试 —— EMA 金叉/死叉事件驱动的
// 市价再平衡(不挂限价单): 虚拟账本目标推进 + 真实现金封顶 + 死叉卖量超持仓重置,
// 方向 flag 趋势侧间距指数放大(同合约口径: 卖 +1 / 买 −1, 建仓/种子/重置不计)。
// tf 序列(64 根 1h, 每段 16 根等步长 0.5: 降→升→降→升, TR 恒 1 → ATR 恒 1):
// 金叉@18 / 死叉@34 / 金叉@50, ATR 就绪@15, 间距 = 5×1 = 5。
// ============================================================================

const SHANNON_VIRTUAL_GRID: &str =
    include_str!("../../../strategies/spot/shannon_virtual_grid.lua");

/// 虚拟网格 tf 序列: 16 根降(100→92.5) + 16 根升 + 16 根降 + 16 根升, 步长 0.5。
fn vgrid_tf() -> Vec<Kline> {
    let mut closes: Vec<f64> = (0..16).map(|i| 100.0 - 0.5 * i as f64).collect();
    let mut last = *closes.last().unwrap();
    for _ in 0..16 {
        last += 0.5;
        closes.push(last);
    }
    for _ in 0..16 {
        last -= 0.5;
        closes.push(last);
    }
    for _ in 0..16 {
        last += 0.5;
        closes.push(last);
    }
    closes
        .iter()
        .enumerate()
        .map(|(h, c)| {
            let d = Decimal::from_f64_retain(*c).unwrap();
            Kline {
                open_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    h as i64 * 3_600_000,
                )
                .unwrap(),
                open: d,
                high: d + dec!(0.5),
                low: d - dec!(0.5),
                close: d,
                volume: Decimal::ONE,
                close_time: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                    h as i64 * 3_600_000 + 3_599_999,
                )
                .unwrap(),
            }
        })
        .collect()
}

/// 主时钟序列: 每小时一根 flat bar(open=px), 策略读 price = bar.open(市价成交价)。
fn vgrid_main(closes: &[f64]) -> Vec<Kline> {
    closes.iter().enumerate().map(|(h, p)| bar_at_f(h as i64, *p)).collect()
}

fn vgrid_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("pair", ConfigValue::String("ETHUSDT".into())),
        ("atr_interval", ConfigValue::String("1h".into())),
        ("ema_interval", ConfigValue::String("1h".into())),
        ("atr_period", ConfigValue::Integer(14)),
        ("ema_fast", ConfigValue::Integer(2)),
        ("ema_slow", ConfigValue::Integer(3)),
        ("min_notional", ConfigValue::Float(5.0)),
        ("fee_side", ConfigValue::Float(0.001)),
        ("invest_cash", ConfigValue::Float(10000.0)),
    ];
    params.extend_from_slice(extra);
    config(SHANNON_VIRTUAL_GRID, &params)
}

/// 建仓开关配置(enable_build + build_price=101 + build_amount)。
fn vgrid_build(amount: f64) -> Vec<(&'static str, ConfigValue)> {
    vec![
        ("enable_build", ConfigValue::Boolean(true)),
        ("build_price", ConfigValue::Float(101.0)),
        ("build_amount", ConfigValue::Float(amount)),
    ]
}

#[test]
fn test_shannon_virtual_grid_mult_out_of_range_halts() {
    let mc = vec![100.0; 32];
    let (orders, _ctx, st) = run_univ2(
        vgrid_cfg(&[("virtual_mult", ConfigValue::Float(6.0))]),
        &vgrid_main(&mc),
        Some(vgrid_tf()),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "虚拟倍数越界应 FATAL 停机");
    assert!(orders.iter().all(|b| b.is_empty()), "停机后不得下单");
}

#[test]
fn test_shannon_virtual_grid_requires_build_params() {
    let mc = vec![100.0; 32];
    let (orders, _ctx, st) = run_univ2(
        vgrid_cfg(&[("enable_build", ConfigValue::Boolean(true))]),
        &vgrid_main(&mc),
        Some(vgrid_tf()),
    );
    assert_eq!(st.global_f64("fatal"), Some(1.0), "enable_build 缺 build_price 应 FATAL");
    assert!(orders.iter().all(|b| b.is_empty()));
}

#[test]
fn test_shannon_virtual_grid_golden_cross_virtual_init_no_order() {
    // 建仓关闭(回测默认): 首次金叉(bar18, 价 92)**只做虚拟初始化, 不下任何实际订单** ——
    // 平衡价 := 现价+间距(92+5=97), 虚拟仓位按总资金 1:1 建立, 真实现金全额保留。
    let mut mc = vec![100.0; 18];
    mc.extend(vec![92.0; 12]);
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&[]), &vgrid_main(&mc), Some(vgrid_tf()));
    assert!(orders.iter().all(|b| b.is_empty()), "不建仓口径: 首次金叉虚拟初始化不得下任何订单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "虚拟初始化无真实成交");
    assert_eq!(st.global_f64("flag"), Some(0.0));
    assert!((st.global_f64("balance_price").unwrap() - 97.0).abs() < 1e-6, "平衡价 := 现价+间距");
    // 虚拟账本按总资金在平衡价精确 1:1 建立
    let v_cash = st.global_f64("v_cash").unwrap();
    let v_pos = st.global_f64("v_pos").unwrap();
    assert!((v_cash - 10000.0).abs() < 1e-6, "虚拟现金 = 总资金一半");
    assert!((v_cash - v_pos * 97.0).abs() / 20000.0 < 1e-6, "虚拟仓位 1:1 @平衡价");
    assert_eq!(st.global_f64("m_pos").unwrap(), 0.0, "真实持仓保持 0(未建仓)");
    assert!((st.global_f64("m_cash").unwrap() - 10000.0).abs() < 1e-6, "真实现金全额保留");
}

#[test]
fn test_shannon_virtual_grid_cross_within_band_noop() {
    // 建仓@100(bar15), 金叉@18 价 98: 平衡价−价 = 2 < 买间距(5) → 带内不补仓。
    let mut mc = vec![100.0; 18];
    mc.extend(vec![98.0; 12]);
    let extra = vgrid_build(10000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    let market: Vec<_> =
        orders.iter().flatten().flatten().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(market.len(), 1, "只应有建仓一笔, 带内金叉不补仓");
    assert_eq!(st.global_f64("fill_count"), Some(1.0));
    assert_eq!(st.global_f64("flag"), Some(0.0));
}

#[test]
fn test_shannon_virtual_grid_death_cross_sell() {
    // 建仓@100, 死叉@34 价 115: 价−平衡 = 15 ≥ 卖间距(5) → 市价卖一笔, flag+1。
    let mut mc = vec![100.0; 18];
    mc.extend(vec![103.0, 106.0, 109.0, 112.0, 115.0]);
    mc.extend(vec![115.0; 13]);
    let extra = vgrid_build(10000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    let sells: Vec<_> = orders
        .iter()
        .flatten()
        .flatten()
        .filter(|o| o.order_type == OrderType::Market && o.side == OrderSide::Sell)
        .collect();
    assert_eq!(sells.len(), 1, "死叉越带应市价卖一笔");
    assert_eq!(st.global_f64("flag"), Some(1.0), "卖 +1");
    assert_eq!(st.global_f64("sell_count"), Some(1.0));
}

#[test]
fn test_shannon_virtual_grid_death_cross_no_position_noop() {
    // enable_build 且价恒 150 > build_price=101 → 永不建仓; 死叉@34 无仓 → 不处理。
    let mc = vec![150.0; 52];
    let extra = vgrid_build(10000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    assert!(orders.iter().all(|b| b.is_empty()), "未建仓则死叉无仓不处理, 全程零单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0), "未建仓应零成交");
}

#[test]
fn test_shannon_virtual_grid_flag_spacing_amplified() {
    // 建仓@100(5000), 金叉@18 买@92(flag−1), 金叉@50 买@85(flag−2):
    // 趋势侧(买)间距 ×1.2^(2−1)=×1.2, 卖间距不变 → last_buy/last_sell = 1.2。
    let mut mc = vec![100.0; 18];
    mc.push(92.0);
    mc.extend(vec![85.0; 36]);
    let extra = vgrid_build(5000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (_orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    assert_eq!(st.global_f64("flag"), Some(-2.0), "两笔网格买 → flag=−2");
    assert_eq!(st.global_f64("flag_min"), Some(-2.0));
    let bs = st.global_f64("last_buy_spacing").unwrap();
    let ss = st.global_f64("last_sell_spacing").unwrap();
    assert!((bs / ss - 1.2).abs() < 1e-6, "flag=−2 → 买间距 ×1.2, 卖间距不变: {bs} vs {ss}");
}

#[test]
fn test_shannon_virtual_grid_buy_cash_limited() {
    // 不建仓: 金叉@18 虚拟初始化(平衡价 97, 零现金支出); 价格深跌至 15 后金叉@50 →
    // 虚拟买量 clip ≈704 币(名义 1.06 万)超真实现金 → 实发按现金截断, 计数 cash_limited,
    // 虚拟持仓按目标推进显著高于真实持仓。
    let mut mc = vec![100.0; 18];
    mc.push(92.0);
    mc.extend(vec![80.0; 15]);
    mc.push(60.0);
    mc.extend(vec![15.0; 16]);
    let (_orders, _ctx, st) = run_univ2(
        vgrid_cfg(&[("virtual_mult", ConfigValue::Float(5.0))]),
        &vgrid_main(&mc),
        Some(vgrid_tf()),
    );
    assert_eq!(st.global_f64("cash_limited_buys"), Some(1.0), "金叉买量超现金 → 截断计数");
    assert_eq!(st.global_f64("fill_count"), Some(1.0), "虚拟初始化不下单, 仅这笔截断买单成交");
    let v_pos = st.global_f64("v_pos").unwrap();
    let m_pos = st.global_f64("m_pos").unwrap();
    assert!(v_pos > m_pos * 1.4, "虚拟持仓按目标推进应高于真实(截断)持仓: v={v_pos} m={m_pos}");
}

#[test]
fn test_shannon_virtual_grid_insufficient_sell_resets() {
    // 建仓 1000(真实 10 币, 虚拟 100 币), 死叉@34 价 130: 卖量 ~11.5 > 真实 10 → 重置。
    let mut mc = vec![100.0; 18];
    mc.extend(vec![103.0, 106.0, 109.0, 112.0, 115.0, 118.0, 121.0, 124.0, 127.0, 130.0]);
    mc.extend(vec![130.0; 16]);
    let extra = vgrid_build(1000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    assert_eq!(st.global_f64("reset_count"), Some(1.0), "死叉卖量超实际持仓 → 重置");
    assert_eq!(st.global_f64("flag"), Some(0.0), "重置不发单不计 flag");
    assert!((st.global_f64("balance_price").unwrap() - 130.0).abs() < 1e-6, "新平衡价 = 当前价");
    // 重置回初始承诺: v_total = 投入×倍数 = 20000, v_cash = 一半 = 10000
    assert!((st.global_f64("v_cash").unwrap() - 10000.0).abs() < 1e-6);
    let sells: Vec<_> =
        orders.iter().flatten().flatten().filter(|o| o.side == OrderSide::Sell).collect();
    assert!(sells.is_empty(), "重置本次不发卖单");
}

#[test]
fn test_shannon_virtual_grid_1to1_invariant() {
    // 跑完建仓+死叉卖一轮: 虚拟账本 1:1 恒等 + 真实账本与引擎逐分对齐(回测规范 §C.4)。
    let mut mc = vec![100.0; 18];
    mc.extend(vec![103.0, 106.0, 109.0, 112.0, 115.0]);
    mc.extend(vec![115.0; 13]);
    let extra = vgrid_build(10000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    let (_orders, mut ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    let v_cash = st.global_f64("v_cash").unwrap();
    let v_pos = st.global_f64("v_pos").unwrap();
    let bal = st.global_f64("balance_price").unwrap();
    let vt = st.global_f64("v_total").unwrap();
    assert!((v_cash - v_pos * bal).abs() / vt < 1e-6, "虚拟账本 1:1 恒等应精确闭合");
    let mut st = st;
    st.on_stop(&mut ctx);
    let snap: HashMap<String, String> = st.state_snapshot().into_iter().collect();
    let dc: f64 = snap.get("stat_ledger_diff_cash").unwrap().parse().unwrap();
    let dp: f64 = snap.get("stat_ledger_diff_pos").unwrap().parse().unwrap();
    assert!(dc.abs() < 0.01 && dp.abs() < 0.01, "真实账本与引擎逐分对齐: cash差{dc} 仓差{dp}");
}

#[test]
fn test_shannon_virtual_grid_thin_spacing_halts() {
    // atr_mult≈0 + 禁用下限 → 生效间距 < 4×费率 → 成本门槛停机, 不建仓不交易。
    let mc = vec![100.0; 32];
    let extra = vgrid_build(10000.0);
    let mut refs: Vec<(&str, ConfigValue)> = Vec::new();
    for (k, v) in &extra {
        refs.push((k, v.clone()));
    }
    refs.push(("atr_mult", ConfigValue::Float(0.0001)));
    refs.push(("min_spacing_pct", ConfigValue::Float(-1.0)));
    let (orders, _ctx, st) = run_univ2(vgrid_cfg(&refs), &vgrid_main(&mc), Some(vgrid_tf()));
    assert_eq!(st.global_f64("fatal"), Some(1.0), "间距低于成本门槛 → 停机");
    assert!(orders.iter().all(|b| b.is_empty()), "停机不得下单");
    assert_eq!(st.global_f64("fill_count"), Some(0.0));
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
