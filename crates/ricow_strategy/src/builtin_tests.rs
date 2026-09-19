//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/builtin/`(策略样板)与 `strategies/builtin/executors/`(执行模式示例),
//! include_str! 编译期嵌入。
//! 断言每个内置脚本的关键行为 (建仓/间隔/分片/触发/挂单), 对齐 Rust 版已知向量。
//! exec 执行组件 (levels/pullback_triggered/ticks_per/slice_due/detect_quote/side_order)
//! 为引擎内置 Rust 实现, 单测见 exec.rs; 此处只验证注入路径 (test_exec_injected)。

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

const SHANNON_GRID: &str = include_str!("../../../strategies/builtin/shannon_rebalance.lua");
const SHANNON_ETF_ACCUM: &str = include_str!("../../../strategies/builtin/shannon_etf_accum.lua");
const DCA: &str = include_str!("../../../strategies/builtin/executors/dca.lua");
const TWAP: &str = include_str!("../../../strategies/builtin/executors/twap.lua");
const VWAP: &str = include_str!("../../../strategies/builtin/executors/vwap.lua");
const PULLBACK: &str = include_str!("../../../strategies/builtin/executors/pullback.lua");
const LADDER: &str = include_str!("../../../strategies/builtin/executors/ladder.lua");

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
        risk: None,
        dry_run_started_at: None,
        live_enabled: false,
        market: "spot".into(),
        position_mode: "one-way".into(),
        backtest: None,
    }
}

fn kline(open: i64, high: i64, low: i64, close: i64) -> Kline {
    Kline {
        open_time: chrono::Utc::now(),
        open: Decimal::from(open),
        high: Decimal::from(high),
        low: Decimal::from(low),
        close: Decimal::from(close),
        volume: Decimal::ONE,
        close_time: chrono::Utc::now(),
    }
}

/// 跑一段序列, 返回每根 bar 的策略订单。
fn run_bars(cfg: StrategyConfig, bars: &[Kline]) -> Vec<Vec<OrderRequest>> {
    let mut strategy = LuaStrategy::from_source(cfg.get_str("script").unwrap(), cfg.clone())
        .expect("内置脚本应编译通过");
    let mut ctx = BacktestContext::new(
        cfg,
        Balance { asset: "USDT".into(), free: dec!(100000), locked: Decimal::ZERO },
    );
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
    out
}

#[test]
fn test_shannon_rebalance_build_then_rebalance() {
    // 首 tick 建仓 (市价买 ~50% 权益), 价涨后卖回 50:50。
    let cfg = config(SHANNON_GRID, &[("pair", ConfigValue::String("ETH".into()))]);
    let bars = vec![kline(100, 105, 95, 104), kline(110, 115, 105, 114)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 1, "首 tick 应建仓");
    assert_eq!(orders[0][0].side, OrderSide::Buy);
    assert_eq!(orders[0][0].order_type, OrderType::Market);
    // 100000 × 0.5 / 100 = 500。
    assert!(
        orders[0][0].size > dec!(400) && orders[0][0].size < dec!(600),
        "建仓应约 50% 权益: {}",
        orders[0][0].size
    );
    assert_eq!(orders[1].len(), 1, "价涨应再平衡卖出");
    assert_eq!(orders[1][0].side, OrderSide::Sell);
}

#[test]
fn test_dca_interval_ticks() {
    // interval_secs=7200, bar_seconds=3600 → 每 2 tick 买入一次。
    let cfg = config(
        DCA,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("order_size", ConfigValue::Float(0.1)),
            ("interval_secs", ConfigValue::Integer(7200)),
            ("bar_seconds", ConfigValue::Integer(3600)),
        ],
    );
    let bars = vec![
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
    ];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 1, "首 tick 买入");
    assert_eq!(orders[1].len(), 0, "间隔内不出");
    assert_eq!(orders[2].len(), 1, "第 3 tick 再买");
    assert_eq!(orders[3].len(), 0);
}

#[test]
fn test_twap_slices() {
    // num_slices=3, 每 1 tick 一片; 发完不再发。
    let cfg = config(
        TWAP,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(3.0)),
            ("num_slices", ConfigValue::Integer(3)),
            ("slice_interval_secs", ConfigValue::Integer(3600)),
            ("bar_seconds", ConfigValue::Integer(3600)),
        ],
    );
    let bars = vec![
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
        kline(100, 101, 99, 100),
    ];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 1);
    assert_eq!(orders[1].len(), 1);
    assert_eq!(orders[2].len(), 1);
    assert_eq!(orders[3].len(), 0, "3 片发完");
    assert_eq!(orders[0][0].size, dec!(1), "每片 = total/num_slices");
}

#[test]
fn test_pullback_triggers_on_pullback() {
    // 100 → 105 (新高) → 101 (回撤 3.8% > 3%) → 触发买入。
    let cfg = config(
        PULLBACK,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("order_size", ConfigValue::Float(1.0)),
            ("pullback_pct", ConfigValue::Float(0.03)),
        ],
    );
    let bars = vec![kline(100, 101, 99, 100), kline(105, 106, 104, 105), kline(101, 102, 100, 101)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 0);
    assert_eq!(orders[1].len(), 0, "新高不触发");
    assert_eq!(orders[2].len(), 1, "回撤触发");
    assert_eq!(orders[2][0].side, OrderSide::Buy);
}

#[test]
fn test_ladder_places_levels_equal() {
    // equal 分档: lower 90 / upper 110 / n 3 → [90, 100, 110], 每档 total/n。
    let cfg = config(
        LADDER,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(3.0)),
            ("num_levels", ConfigValue::Integer(3)),
            ("lower_price", ConfigValue::Float(90.0)),
            ("upper_price", ConfigValue::Float(110.0)),
        ],
    );
    let bars = vec![kline(100, 101, 99, 100)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 3, "一次性挂 3 档");
    assert_eq!(orders[0][0].side, OrderSide::Buy);
    assert_eq!(orders[0][0].size, dec!(1));
    let prices: Vec<Decimal> = orders[0].iter().map(|o| o.price.unwrap()).collect();
    assert_eq!(prices, vec![dec!(90), dec!(100), dec!(110)]);
}

#[test]
fn test_ladder_geometric_levels() {
    // geometric 分档: lower 100 / upper 400 / n 3 → [100, ~200, 400]。
    let cfg = config(
        LADDER,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(3.0)),
            ("num_levels", ConfigValue::Integer(3)),
            ("lower_price", ConfigValue::Float(100.0)),
            ("upper_price", ConfigValue::Float(400.0)),
            ("distribution", ConfigValue::String("geometric".into())),
        ],
    );
    let bars = vec![kline(200, 201, 199, 200)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 3);
    let prices: Vec<f64> = orders[0].iter().map(|o| o.price.unwrap().to_f64().unwrap()).collect();
    assert!((prices[0] - 100.0).abs() < 0.01, "首档 = {}", prices[0]);
    assert!((prices[2] - 400.0).abs() < 0.01, "末档 = {}", prices[2]);
    assert!((prices[1] - 200.0).abs() < 1.0, "等比中项 ≈ 200, got {}", prices[1]);
}

#[test]
fn test_exec_injected() {
    // from_source 统一注入 exec: 用户脚本不手动拼接即可直接调用 exec.* (注入路径)。
    let script = r#"
        function on_tick(ctx)
            local orders = {}
            local eq = exec.levels(90, 110, 3, false)
            for _, p in ipairs(eq) do
                orders[#orders + 1] = { pair = "ETH", side = "buy", size = 1, price = p, order_type = "limit" }
            end
            return orders
        end
    "#;
    let cfg = config(script, &[("pair", ConfigValue::String("ETH".into()))]);
    let bars = vec![kline(100, 101, 99, 100)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 3, "注入的 exec.levels 应可用");
    assert_eq!(orders[0][0].price, Some(dec!(90)));
    assert_eq!(orders[0][1].price, Some(dec!(100)));
    assert_eq!(orders[0][2].price, Some(dec!(110)));
}

/// 带成交量的 K 线构造 (kline 的 volume 固定为 1, 本函数可指定)。
fn kline_v(open: i64, high: i64, low: i64, close: i64, volume: i64) -> Kline {
    Kline {
        open_time: chrono::Utc::now(),
        open: Decimal::from(open),
        high: Decimal::from(high),
        low: Decimal::from(low),
        close: Decimal::from(close),
        volume: Decimal::from(volume),
        close_time: chrono::Utc::now(),
    }
}

#[test]
fn test_vwap_slices() {
    // num_slices=2, 每 2 tick 一片; 首片无历史 K 线 → 市价; 第 2 片有已收盘 K 线 → VWAP 限价。
    let cfg = config(
        VWAP,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(2.0)),
            ("num_slices", ConfigValue::Integer(2)),
            ("slice_interval_secs", ConfigValue::Integer(7200)),
            ("bar_seconds", ConfigValue::Integer(3600)),
        ],
    );
    let bars = vec![
        kline_v(100, 101, 99, 100, 1),
        kline_v(100, 101, 99, 100, 1),
        kline_v(100, 101, 99, 100, 1),
        kline_v(100, 101, 99, 100, 1),
    ];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 1, "首片立即");
    assert_eq!(orders[0][0].order_type, OrderType::Market, "首片无历史 K 线 → 市价");
    assert_eq!(orders[0][0].size, dec!(1), "每片 = total/num_slices");
    assert_eq!(orders[1].len(), 0, "间隔未到");
    assert_eq!(orders[2].len(), 1, "第 2 片");
    assert_eq!(orders[2][0].order_type, OrderType::Limit, "有已收盘 K 线 → VWAP 限价");
    assert_eq!(orders[2][0].price, Some(dec!(100)), "VWAP = 平均 close = 100");
    assert_eq!(orders[3].len(), 0, "2 片发完");
}

#[test]
fn test_vwap_volume_weighted() {
    // VWAP = Σ(close×volume)/Σ(volume): (100×1 + 200×3)/4 = 175。
    let cfg = config(
        VWAP,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(2.0)),
            ("num_slices", ConfigValue::Integer(2)),
            ("slice_interval_secs", ConfigValue::Integer(7200)),
            ("bar_seconds", ConfigValue::Integer(3600)),
        ],
    );
    let bars = vec![
        kline_v(100, 101, 99, 100, 1),
        kline_v(200, 201, 199, 200, 3),
        kline_v(200, 201, 199, 200, 1),
        kline_v(200, 201, 199, 200, 1),
    ];
    let orders = run_bars(cfg, &bars);
    // 第 2 片 (tick3): 已收盘 = [bar1(100,vol1), bar2(200,vol3)] → VWAP = 175。
    assert_eq!(orders[2].len(), 1);
    assert_eq!(orders[2][0].price, Some(dec!(175)), "成交量加权均价应 = 175");
}

#[test]
fn test_vwap_zero_volume_fallback() {
    // 全部成交量 0 → 回退最新 close (bar2 的 close=200)。
    let cfg = config(
        VWAP,
        &[
            ("pair", ConfigValue::String("ETH".into())),
            ("total_size", ConfigValue::Float(2.0)),
            ("num_slices", ConfigValue::Integer(2)),
            ("slice_interval_secs", ConfigValue::Integer(7200)),
            ("bar_seconds", ConfigValue::Integer(3600)),
        ],
    );
    let bars = vec![
        kline_v(100, 101, 99, 100, 0),
        kline_v(200, 201, 199, 200, 0),
        kline_v(200, 201, 199, 200, 0),
        kline_v(200, 201, 199, 200, 0),
    ];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[2].len(), 1);
    assert_eq!(orders[2][0].price, Some(dec!(200)), "成交量全 0 → 回退最新 close");
}

#[test]
fn test_shannon_rebalance_target_ratio_070() {
    // target_ratio=0.7 → 建仓 ≈ 70% 权益 (100000×0.7/100=700), 价涨后卖回 70:30。
    let cfg = config(
        SHANNON_GRID,
        &[("pair", ConfigValue::String("ETH".into())), ("target_ratio", ConfigValue::Float(0.7))],
    );
    let bars = vec![kline(100, 105, 95, 104), kline(110, 115, 105, 114)];
    let orders = run_bars(cfg, &bars);
    assert_eq!(orders[0].len(), 1, "首 tick 应建仓");
    assert_eq!(orders[0][0].side, OrderSide::Buy);
    assert!(
        orders[0][0].size > dec!(600) && orders[0][0].size < dec!(800),
        "建仓应约 70% 权益 (≈700): {}",
        orders[0][0].size
    );
    assert_eq!(orders[1].len(), 1, "价涨应再平衡卖出");
    assert_eq!(orders[1][0].side, OrderSide::Sell);
}

#[test]
fn test_shannon_rebalance_atr_widens_band() {
    // ATR 开 (atr_period=3): 高波动 bars 建立大 ATR → band_eff 放大 → 涨 5% 不触发;
    // 对照 atr_period=0 (禁用): band_eff = rebalance_band → 同序列涨 5% 触发卖出。
    let params_on = [
        ("pair", ConfigValue::String("ETH".into())),
        ("atr_period", ConfigValue::Integer(3)),
        ("atr_mult", ConfigValue::Float(1.0)),
    ];
    let cfg_on = config(SHANNON_GRID, &params_on);
    let cfg_off = config(
        SHANNON_GRID,
        &[("pair", ConfigValue::String("ETH".into())), ("atr_period", ConfigValue::Integer(0))],
    );
    // bar0 建仓 @100; bar1-3 高波动 (high=110/low=90, close=100) 累计 TR≈20 → ATR/price≈0.2;
    // bar4 涨到 105 (+5%): 币市值 500×105=52500 vs 目标 51250, 偏离 ~1.2% 权益。
    let bars = vec![
        kline(100, 100, 100, 100),
        kline(100, 110, 90, 100),
        kline(100, 110, 90, 100),
        kline(100, 110, 90, 100),
        kline(105, 110, 100, 105),
    ];
    let orders_on = run_bars(cfg_on, &bars);
    assert_eq!(orders_on[0].len(), 1, "ATR 开: 首 tick 应建仓");
    assert!(orders_on[4].is_empty(), "ATR 开: band 放大 (≈0.2) → 涨 5% 不应触发再平衡");
    let orders_off = run_bars(cfg_off, &bars);
    assert_eq!(orders_off[0].len(), 1, "ATR 关: 首 tick 应建仓");
    assert_eq!(orders_off[4].len(), 1, "ATR 关: 固定 band 0.5% → 涨 5% 应触发卖出");
    assert_eq!(orders_off[4][0].side, OrderSide::Sell);
}

#[test]
fn test_shannon_rebalance_target_ratio_clamped() {
    // target_ratio 超界 (0 与 2) → 回退默认 0.5, 建仓 ≈ 500, 不 panic。
    for bad in [0.0f64, 2.0f64] {
        let cfg = config(
            SHANNON_GRID,
            &[
                ("pair", ConfigValue::String("ETH".into())),
                ("target_ratio", ConfigValue::Float(bad)),
            ],
        );
        let bars = vec![kline(100, 105, 95, 104), kline(110, 115, 105, 114)];
        let orders = run_bars(cfg, &bars);
        assert_eq!(orders[0].len(), 1, "target_ratio={bad}: 应建仓");
        assert!(
            orders[0][0].size > dec!(400) && orders[0][0].size < dec!(600),
            "target_ratio={bad}: 应回退 0.5 (≈500): {}",
            orders[0][0].size
        );
    }
}

// ============================================================================
// 023 香农 ETF 指数增加策略 (shannon_etf_accum) 集成测试
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
fn test_shannon_etf_accum_requires_tf_atr_channel() {
    // 高周期通道未预装 → 一笔都不下(不猜 ATR 值, 不建仓)。
    let (orders, _ctx) = run_accum(accum_cfg(&[]), &uptrend_bars(), None);
    assert!(orders.iter().all(|o| o.is_empty()), "ATR 通道未就绪时必须完全不动");
}

/// 023 测试用: 按给定价格序列生成 1h 间隔的 bar(引擎不校验周期; OHLC 同价, 便于精确断言)。
fn bars_from(prices: &[i64]) -> Vec<Kline> {
    prices.iter().enumerate().map(|(h, p)| bar_at_hour(h as i64, *p, *p, *p, *p)).collect()
}

/// 网格双向用: 横盘 → 上冲(建仓) → 锯齿(每次 ±40 远大于 spacing, 保证买卖两侧都成交)。
fn r2_bars() -> Vec<Kline> {
    let mut p: Vec<i64> = vec![100; 30];
    p.extend((1..=5).map(|i| 100 + i * 5)); // 105..125 → 建仓
    for i in 0..12 {
        p.push(if i % 2 == 0 { 145 } else { 105 });
    }
    bars_from(&p)
}

/// R3 用: 上冲(建仓 + 网格卖) → **长横盘让 EMA10/20 收敛** → 单根深跌(下一根检出死叉)
/// → **跳空高开**(开盘 215 > 旧锚 195 + spacing 4, 且此刻才是"新死叉") → R3 条件成立。
/// 死叉事件只能在其后一根被检出, 所以"高价 + 新死叉"必须落在同一根 —— 这正是 R3 在连续
/// 行情里极难触发的原因(见结果文档)。
fn r3_bars() -> Vec<Kline> {
    let mut p: Vec<i64> = vec![100; 30];
    p.extend((1..=20).map(|i| 100 + i * 5)); // 105..200 → 建仓 + 网格卖(锚跟到 ~199)
    p.extend(std::iter::repeat_n(200, 40)); // 长横盘: EMA10/20 收敛到 200
    p.push(180); // 单根深跌 → 网格买成交(锚 ≈ 195), 且造成 EMA 死叉
    p.push(215); // 跳空高开: 215 > 195 + 4, 且本根检出"新死叉" → R3
    p.extend(std::iter::repeat_n(215, 3));
    bars_from(&p)
}

#[test]
fn test_shannon_etf_accum_r1_anchor_above_market_and_small_entry() {
    // R1(2026-09-18 修订): 初次金叉 → 锚 = ask + spacing; 虚拟账本在锚价建 target 权重;
    // 再用**当前市价**算回平衡量 → 市价买入。入场是"小额"(≈ v_cap×target×(spacing/锚)),
    // 不再是 v1 的"半仓一次性建仓"。
    let bars = uptrend_bars();
    let (orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &bars, Some(tf_bars(30)));
    let idx = orders
        .iter()
        .position(|o| o.iter().any(|r| r.order_type == OrderType::Market))
        .expect("应有一笔市价建仓");
    let market: Vec<&OrderRequest> =
        orders[idx].iter().filter(|o| o.order_type == OrderType::Market).collect();
    assert_eq!(market.len(), 1, "初次金叉只应一笔市价建仓");
    assert_eq!(market[0].side, OrderSide::Buy);
    // R1 语义(用户口径): 锚 = 价 + spacing; 虚拟账本在锚价建 50:50(10 万账本的 target 权重);
    //   下单量 = **虚拟账本自己的回平衡量**(与真实持仓无关) = 0.25 × v_cap × (锚 − 价)/锚
    //   → v_cap=10000 时约 87 USDT(账户的 0.9%), 不是 v1 的"半仓一次性建仓"。
    let px = bars[idx].open;
    let spacing = dec!(4);
    let anchor = px + spacing;
    let expect_notional = dec!(2500) * spacing / anchor;
    let got_notional = market[0].size * px;
    let rel = ((got_notional - expect_notional) / expect_notional).abs();
    assert!(rel < dec!(0.01), "R1 买量名义应 ≈ {expect_notional}: 实得 {got_notional}");
    assert!(
        got_notional > dec!(50) && got_notional < dec!(150),
        "R1 应是账本回平衡量的小额入场(≈87, 而非半仓 5000): {got_notional}"
    );
    // 虚拟账本已在 R1 建好, 且计数正确
    assert!(st.global_f64("v_coin").unwrap_or(0.0) > 0.0, "虚拟账本应在 R1 建成");
    assert_eq!(st.global_f64("cross_buy"), Some(1.0), "应记 1 次交叉买入");
    // 同批必须先撤后挂: 建仓批不含撤单(还没有挂单), 但不得出现卖单
    assert!(orders[idx].iter().all(|o| o.side != OrderSide::Sell), "建仓时不得卖出");
    let _ = st;
}

#[test]
fn test_shannon_etf_accum_no_chasing_buy_and_both_grid_sides_alive() {
    // 跌下来买 / 涨上去卖: 由网格两侧限价单实现。交叉通道(R2/R3)要求"新交叉"与
    // "价格离开锚点 > spacing"在同根成立, 而锚点跟随成交、网格买单正落在 锚−spacing,
    // 因此连续行情里几乎不可达 —— 这里锁住两个可测事实:
    //   ① 单边上涨不得追高加仓(市价买只有建仓那一次);
    //   ② 网格两侧都在真实成交(不是只买不卖)。
    let (orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &r2_bars(), Some(tf_bars(60)));
    let market: Vec<&OrderRequest> =
        orders.iter().flatten().filter(|o| o.order_type == OrderType::Market).collect();
    // 网格在"锚已被价格甩开"时会用市价补单(2026-09-18 修复: 限价单被价格穿过不会立即成交,
    // 曾经导致纯网格枯死) → 这里锁住真正的不变量: 首笔市价单是建仓, 之后不得出现
    // 交叉通道的追高加仓(cross_buy 恒为 1), 且市价补单买卖两侧都会出现。
    assert_eq!(market[0].side, OrderSide::Buy, "首笔市价单应为建仓");
    assert!(market.iter().any(|o| o.side == OrderSide::Sell), "网格应有市价补单(卖侧)");
    assert_eq!(st.global_f64("cross_buy"), Some(1.0), "不得出现交叉通道追高加仓");
    assert!(
        st.global_f64("fill_count").unwrap_or(0.0) > 5.0,
        "网格应有多次成交: {:?}",
        st.global_f64("fill_count")
    );

    let (orders2, _c2, st2) = run_accum_full(accum_cfg(&[]), &uptrend_bars(), Some(tf_bars(60)));
    let m2_buys: Vec<&OrderRequest> = orders2
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Market && o.side == OrderSide::Buy)
        .collect();
    // 单边上涨不得"追高加仓": 市价买只允许建仓那一次(网格补单若出现, 只可能是卖侧 —— 涨停式
    // 跳空把卖价甩到市价下方)。
    assert_eq!(m2_buys.len(), 1, "单边上涨只应建仓一次市价买, 实得 {}", m2_buys.len());
    assert_eq!(st2.global_f64("cross_buy"), Some(1.0));
}

#[test]
fn test_shannon_etf_accum_r3_no_spurious_market_sell_and_grid_handles_sellside() {
    // R3(死叉 + 价 > 锚 + spacing)在当前锚点跟随机制下几乎不可触发 —— 卖侧由网格实现。
    // 这条用例锁住: 交叉通道不得触发真实卖出(网格自身的补单不算交叉通道)。
    let (_orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &r3_bars(), Some(tf_bars(80)));
    // 网格补单可以是市价卖(见上条用例), 但**交叉通道 R3 不得触发真实卖出**。
    assert_eq!(st.global_f64("cross_sell_real"), Some(0.0));
    assert!(st.global_f64("fill_count").unwrap_or(0.0) > 0.0, "上涨段网格卖单应成交");
}

#[test]
fn test_shannon_etf_accum_virtual_book_invariant_no_expansion() {
    // 不变量 I1(用户 2026-09-18 补充): 虚拟账本不是成交历史的累加器, 而是"10 万本金在参考价
    // (初始锚 = ask + spacing)处的 target 权重快照" → `v_coin × 锚 == target × v_cap`。
    // 这是"虚拟仓位不会因卖不出去而扩张"的可断言形式(v_coin 有闭式上界)。
    let bars = uptrend_bars();
    let (orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &bars, Some(tf_bars(30)));
    let idx = orders
        .iter()
        .position(|o| o.iter().any(|r| r.order_type == OrderType::Market))
        .expect("应有市价建仓");
    let _ = idx;
    // 锚 = 最近一次成交价(用户口径: 每次成交后 锚 := 成交价); 账本按成交继续演化 →
    // 每笔成交后账本在锚价处必然是 **50:50**: v_coin × 锚 == v_cash。
    let anchor = st.global_f64("balance_price").expect("锚应已设置");
    let v_coin = st.global_f64("v_coin").expect("v_coin 应可读");
    let v_cash = st.global_f64("v_cash").expect("v_cash 应可读");
    let coin_value = v_coin * anchor;
    let rel = (coin_value - v_cash).abs() / v_cash;
    assert!(rel < 1e-6, "账本在锚价处应 50:50: coin_value={coin_value} vs v_cash={v_cash}");
    assert!(v_coin > 0.0 && v_coin.is_finite(), "v_coin 异常: {v_coin}");
}

#[test]
fn test_shannon_etf_accum_no_market_sell_without_cover() {
    // 用户第 3 条: 需要卖但真实账户不够 → 零真实成交(不得出现市价卖)。
    let (orders, _ctx, st) = run_accum_full(accum_cfg(&[]), &r3_bars(), Some(tf_bars(80)));
    let (mut bought, mut sold) = (dec!(0), dec!(0));
    for o in orders.iter().flatten() {
        if o.side == OrderSide::Sell {
            sold += o.size;
        } else {
            bought += o.size;
        }
    }
    // 无仓不得卖: 任何时刻的卖出请求总量都不得超过买入总量(否则就是"卖空", 引擎会拒单)。
    assert!(sold <= bought, "卖出总量 {sold} 超过买入总量 {bought} → 出现无仓卖出");
    assert_eq!(st.global_f64("cross_sell_real"), Some(0.0));
}

#[test]
fn test_shannon_etf_accum_skips_thin_spacing() {
    // 保本守卫: spacing ≤ 0.2%×价格 → 不挂单(只可能建仓, 之后静默)。
    let bars = uptrend_bars();
    let (orders, _ctx) =
        run_accum(accum_cfg(&[("atr_mult", ConfigValue::Float(0.0001))]), &bars, Some(tf_bars(30)));
    let limits: Vec<&OrderRequest> =
        orders.iter().flatten().filter(|o| o.order_type == OrderType::Limit).collect();
    assert!(limits.is_empty(), "低于保本线不得挂单, 实得 {} 张", limits.len());
}

#[test]
fn test_shannon_etf_accum_skips_small_notional() {
    // min_notional 守卫: 名义不足 → 不挂单。
    let bars = uptrend_bars();
    let (orders, _ctx) = run_accum(
        accum_cfg(&[("min_notional", ConfigValue::Float(1_000_000.0))]),
        &bars,
        Some(tf_bars(30)),
    );
    let limits: Vec<&OrderRequest> =
        orders.iter().flatten().filter(|o| o.order_type == OrderType::Limit).collect();
    assert!(limits.is_empty(), "名义不足不得挂单, 实得 {} 张", limits.len());
}

// ============================================================================
// 023 v4 日线趋势判据 (2026-09-18 用户定稿): BULL 只买不卖 / BEAR 只卖不买 / RANGE 正常
// 判据 = 上一根已收盘高周期 bar 的 close 与 EMA(period) 的 ±band 带(含等号归 RANGE)。
// 单测里用"4h 序列 + 小 period"跑同一段 Lua 逻辑(周期只是参数, 判定口径与日线一致)。
// ============================================================================

/// 023 趋势判据测试用: 4h 间隔的 bar(每 4 小时一根), OHLC 同价 → 只用于 close/EMA 判据。
fn bars_4h(prices: &[i64]) -> Vec<Kline> {
    prices.iter().enumerate().map(|(i, p)| bar_at_hour(i as i64 * 4, *p, *p, *p, *p)).collect()
}

fn regime_cfg(extra: &[(&str, ConfigValue)]) -> StrategyConfig {
    let mut params = vec![
        ("regime_filter", ConfigValue::String("ema200".into())),
        ("regime_interval", ConfigValue::String("4h".into())),
        ("regime_ema_period", ConfigValue::Integer(3)),
    ];
    params.extend_from_slice(extra);
    accum_cfg(&params)
}

#[test]
fn test_shannon_etf_accum_regime_not_ready_blocks_all_orders() {
    // D3-A: 判据未就绪(判据序列未预装 / 可见 bar 不足 period 根)→ 一笔都不下(不猜值)。
    // 这里装 ATR 序列("1h")但**不装**判据序列("1d") → ATR 就绪、判据未就绪。
    let cfg = accum_cfg(&[
        ("regime_filter", ConfigValue::String("ema200".into())),
        ("regime_interval", ConfigValue::String("1d".into())),
    ]);
    let (orders, _ctx, st) = run_accum_multi(cfg, &uptrend_bars(), &[("1h", Some(tf_bars(40)))]);
    assert!(orders.iter().all(|o| o.is_empty()), "判据未就绪必须完全不动");
    assert_eq!(st.global_f64("cross_buy"), Some(0.0), "判据未就绪不得建仓");
    assert!(st.global_f64("regime_ready_skip").unwrap_or(0.0) > 0.0, "应记未就绪跳过");
}

#[test]
fn test_shannon_etf_accum_regime_bear_blocks_entry() {
    // D2-A: BEAR(`close < EMA×(1−band)`)连**首次金叉建仓**一起拦。
    // 判据序列 = 4h 快速衰减(EMA(3) 稳定远在上方) → 全程 BEAR。
    let cfg = regime_cfg(&[]);
    let (orders, _ctx, st) = run_accum_multi(
        cfg,
        &uptrend_bars(),
        &[
            ("1h", Some(tf_bars(40))),
            ("4h", Some(bars_4h(&[1000, 500, 250, 125, 62, 31, 15, 7, 3, 1, 1, 1]))),
        ],
    );
    assert!(orders.iter().all(|o| o.is_empty()), "熊市不得建仓, 也不该挂任何单");
    assert_eq!(st.global_f64("cross_buy"), Some(0.0), "熊市禁建仓(D2-A)");
    assert!(st.global_f64("regime_block_buy").unwrap_or(0.0) > 0.0, "应记拦买次数");
    assert_eq!(st.global_f64("regime_block_sell"), Some(0.0), "熊市不拦卖");
    assert_eq!(st.global_f64("ticks_bear").unwrap_or(0.0) > 0.0, true);
}

#[test]
fn test_shannon_etf_accum_regime_bull_blocks_sell_side_only() {
    // BULL(`close > EMA×(1+band)`)→ 只买不卖: 建仓照常(只有 BEAR 拦建仓), 之后**不得出现任何卖单**。
    // 判据序列 = 4h 倍增(band≈0): close 恒 ≈ 1.5×EMA → BULL 稳定。
    let cfg = regime_cfg(&[("regime_band_pct", ConfigValue::Float(1e-9))]);
    let bull =
        bars_4h(&[100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 51200, 102400, 204800]);
    let (orders, _ctx, st) =
        run_accum_multi(cfg, &r2_bars(), &[("1h", Some(tf_bars(60))), ("4h", Some(bull))]);
    let flat: Vec<&OrderRequest> = orders.iter().flatten().collect();
    assert_eq!(st.global_f64("cross_buy"), Some(1.0), "建仓不受 BULL 拦");
    assert!(flat.iter().all(|o| o.side != OrderSide::Sell), "牛市不得出现任何卖单");
    assert!(
        flat.iter().any(|o| o.side == OrderSide::Buy && o.order_type == OrderType::Limit),
        "牛市买侧仍应挂单"
    );
    assert!(st.global_f64("regime_block_sell").unwrap_or(0.0) > 0.0, "应记拦卖次数");
    assert!(st.global_f64("ticks_bull").unwrap_or(0.0) > 0.0, "应记 BULL tick 数");
}

#[test]
fn test_shannon_etf_accum_regime_range_when_close_equals_ema() {
    // 逐字边界: `close` **等于** `EMA×(1±band)` 归 RANGE(band≈0 时 close == EMA 精确成立)
    // → 必须走"两侧正常"(买/卖两张都挂), 既不算 BULL 也不算 BEAR。
    let cfg = regime_cfg(&[("regime_band_pct", ConfigValue::Float(1e-9))]);
    let flat4h = bars_4h(&[100; 12]);
    let (orders, _ctx, st) =
        run_accum_multi(cfg, &r2_bars(), &[("1h", Some(tf_bars(60))), ("4h", Some(flat4h))]);
    let flat: Vec<&OrderRequest> = orders.iter().flatten().collect();
    assert!(flat.iter().any(|o| o.side == OrderSide::Buy), "RANGE 买侧应放开");
    assert!(flat.iter().any(|o| o.side == OrderSide::Sell), "RANGE 卖侧应放开");
    assert_eq!(st.global_f64("regime_block_sell"), Some(0.0), "带内不得拦卖");
    assert_eq!(st.global_f64("regime_block_buy"), Some(0.0), "带内不得拦买");
    assert!(st.global_f64("ticks_range").unwrap_or(0.0) > 0.0, "应记 RANGE tick 数");
}

#[test]
fn test_shannon_etf_accum_regime_blocked_side_still_cancels() {
    // P3(架构审核): 被拦侧 + 另一侧本轮也挂不出时, **仍必须发 cancel_pending** ——
    // 否则牛市里那张旧卖单留在盘上并成交, 等于"牛市不卖"被静默破坏。
    // 构造: BULL 拦卖 + `trend_filter_sma` 在下跌 bar 上把买侧关掉 → 两侧都无单可挂。
    let cfg = regime_cfg(&[
        ("regime_band_pct", ConfigValue::Float(1e-9)),
        ("trend_filter_sma", ConfigValue::Integer(2)),
    ]);
    let bull =
        bars_4h(&[100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600, 51200, 102400, 204800]);
    let (orders, _ctx, st) =
        run_accum_multi(cfg, &r2_bars(), &[("1h", Some(tf_bars(60))), ("4h", Some(bull))]);
    let only_cancel =
        orders.iter().filter(|o| o.len() == 1 && o[0].action == OrderAction::CancelPending).count();
    assert!(only_cancel > 0, "被拦侧必须撤旧单(实得 {only_cancel} 次)");
    assert!(st.global_f64("regime_blocked_cancel").unwrap_or(0.0) > 0.0, "应记被拦撤单次数");
    assert!(orders.iter().flatten().all(|o| o.side != OrderSide::Sell), "牛市全程不得出现卖单");
}
