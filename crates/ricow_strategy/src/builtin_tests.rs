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
const SHANNON_ETF_ACCUM: &str = include_str!("../../../strategies/builtin/shannon_spot_grid.lua");
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

/// 030 测试用: 主序列 —— 前 `at` 根 @ `base`, 之后 @ `then`。
fn step_main(n: i64, base: i64, at: i64, then: i64) -> Vec<Kline> {
    (0..n)
        .map(|h| {
            let p = if h < at { base } else { then };
            bar_at_hour(h, p, p, p, p)
        })
        .collect()
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

/// 030 测试用: 高周期序列 —— 横盘 → 上行(金叉) → 回落(死叉)。
fn tf_bars_up_down(flat: i64, up: i64, down: i64, step: i64) -> Vec<Kline> {
    let mut v = Vec::new();
    let mut px = 100;
    for h in 0..(flat + up + down) {
        if h >= flat && h < flat + up {
            px += step;
        } else if h >= flat + up {
            px -= step;
        }
        v.push(bar_at_hour(h, px, px + 1, px - 1, px));
    }
    v
}

fn market_orders(orders: &[Vec<OrderRequest>]) -> Vec<OrderRequest> {
    orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Market)
        .cloned()
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
    assert!(
        orders.iter().all(|o| o.is_empty()),
        "信号 EMA 通道未就绪时必须完全不动"
    );
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
    assert!(
        orders.iter().all(|o| o.is_empty()),
        "成本门槛不满足时必须停机且不下任何单"
    );
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
    assert!(
        orders.iter().all(|o| o.is_empty()),
        "缺必填 start_price 时不得下任何单"
    );
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
    let market: Vec<_> = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Market)
        .collect();
    assert_eq!(market.len(), 1, "激活时应恰有一笔市价初始建仓");
    assert_eq!(market[0].side, OrderSide::Buy);
    assert_eq!(
        st.global_f64("balance_price"),
        Some(100.0),
        "初始建仓成交价应成为第一次平衡价"
    );
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
    assert_eq!(
        st.global_f64("balance_price"),
        Some(100.0),
        "未设初始仓位 -> 平衡价 := 激活时现价"
    );
    let limits: Vec<_> = orders
        .iter()
        .flatten()
        .filter(|o| o.order_type == OrderType::Limit && o.price.is_some())
        .collect();
    assert!(!limits.is_empty(), "未设初始仓位也应挂出买单");
    assert_eq!(
        limits[0].price.expect("限价单必须带价格"),
        dec!(96),
        "买单价位 = 平衡价 − 2×ATR"
    );
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
    assert!(
        snap.iter().any(|(k, _)| k == "v_cap"),
        "状态快照应含账本规模"
    );
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
    let (orders, _ctx, st) = run_accum_full(cfg, &bull.clone(), Some(bull));
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
