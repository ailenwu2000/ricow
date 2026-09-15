//! 内置脚本 Lua 集成测试 (回测冒烟, 不依赖网络)。
//!
//! 脚本源: `strategies/builtin/`(策略样板)与 `strategies/builtin/executors/`(执行模式示例),
//! include_str! 编译期嵌入。
//! 断言每个内置脚本的关键行为 (建仓/间隔/分片/触发/挂单), 对齐 Rust 版已知向量。
//! exec 执行组件 (levels/pullback_triggered/ticks_per/slice_due/detect_quote/side_order)
//! 为引擎内置 Rust 实现, 单测见 exec.rs; 此处只验证注入路径 (test_exec_injected)。

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

const SHANNON_GRID: &str = include_str!("../../../strategies/builtin/shannon_grid.lua");
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
fn test_shannon_grid_build_then_rebalance() {
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
fn test_shannon_grid_target_ratio_070() {
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
fn test_shannon_grid_atr_widens_band() {
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
fn test_shannon_grid_target_ratio_clamped() {
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
