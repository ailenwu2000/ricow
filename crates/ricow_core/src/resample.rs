//! 周期重采样: 由主序列重采样出**完整**的高周期 K 线 (028 T007)。
//!
//! 来源: 分支 `feat/shannon-atr-grid` 的 `crates/ricow_strategy/src/multiframe.rs::resample_complete`
//! (2026-09-18 实测定稿: 完整桶 + 缺口守卫 + 输入粒度众数探测)。按 028 plan d12 迁入 `ricow_core`
//! —— 它是纯函数, 策略侧(序列句柄)与引擎侧(装配预装)都要用, 放 core 才不产生反向依赖。
//!
//! 迁入时的两处调整(与分支版本的差异, 如实记录):
//! 1. 周期换算不再自带表 —— 统一走 [`Interval`](`tf_ms` 由调用方传入, 或经
//!    [`resample_to_interval`]);
//! 2. 丢弃桶不再打 `tracing::debug!` —— `ricow_core` 无 `tracing` 依赖(不新增依赖),
//!    需要日志的调用方按 `输入桶数 − 输出根数` 自行记录;
//! 3. (2026-09-21) 缺口守卫从"理论根数 = tf/输入粒度 × 0.9"改为"**实测桶大小中位数** × 0.6",
//!    并删掉不再需要的输入粒度众数探测: 原判据假设市场 7×24 连续交易, 对美股(5 交易日/周)
//!    会把所有周桶判成缺口 → 重采样恒为空(真机踩到, 见 `test_equity_weekly_buckets_survive_five_day_weeks`)。

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::interval::Interval;
use crate::types::Kline;

/// 由主序列重采样出**完整**的高周期 bar (升序返回)。
///
/// - 桶键 = `open_time.ms.div_euclid(tf_ms)`(周期边界对齐);
/// - **完整桶判据**: 桶起点 ≥ 序列首根 `open_time`, 且桶终点 ≤ 序列末根 `close_time`
///   —— 首尾半桶一律丢弃(半个桶的 high/low 不能代表该周期);
/// - **缺口守卫**: 桶内 bar 数 < **实测桶大小中位数** × 0.6 视为缺口, 丢弃
///   (防把"只覆盖了半小时的 1h 桶"当完整桶, 算错 ATR);
///   ⚠️ 为什么不用"理论根数 = tf / 输入粒度 × 0.9": 那条判据假设市场 7×24 连续交易。
///   美股一周只有 5 个交易日 → 每个周桶都凑不满 7 根 → **整条序列重采样为 0 根**
///   (2026-09-21 真机踩到: nasdaq QQQ 声明 `interval="1w"` 报"本地库没有可用数据")。
///   改用实测中位数后, 真缺口桶(10/60 根)依旧被丢, 而"市场必然休市"造成的固定缩水不再被误判。
/// - 聚合: open = 首根 open, close = 末根 close, high = max, low = min, volume = Σ,
///   `close_time = 桶起点 + tf_ms − 1ms`;
///
/// 输入乱序/空序列 → 返回空(升序是引擎既有契约, 此处只做防御); `tf_ms <= 0` → 返回空。
pub fn resample_complete(bars: &[Kline], tf_ms: i64) -> Vec<Kline> {
    if tf_ms <= 0 || bars.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&Kline> = bars.iter().collect();
    sorted.sort_by_key(|k| k.open_time);
    let first_ms = sorted[0].open_time.timestamp_millis();
    let last_close_ms = sorted[sorted.len() - 1].close_time.timestamp_millis();

    let bucket_of = |k: &Kline| k.open_time.timestamp_millis().div_euclid(tf_ms) * tf_ms;

    // 第一遍: 分组(密度阈值要用**实测**桶大小, 所以必须先分组再判)。
    let mut groups: Vec<(i64, usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < sorted.len() {
        let bucket_start = bucket_of(sorted[i]);
        let mut j = i;
        while j < sorted.len() && bucket_of(sorted[j]) == bucket_start {
            j += 1;
        }
        groups.push((bucket_start, i, j));
        i = j;
    }

    // 缺口守卫阈值 = 桶大小中位数 × 0.6(至少 1) —— 见函数文档注记: 不用理论根数,
    // 否则"每个周期内必然休市"的市场(美股 5/7 交易日)会把所有桶判成缺口。
    let min_bars = {
        let mut sizes: Vec<usize> = groups.iter().map(|(_, a, b)| b - a).collect();
        sizes.sort_unstable();
        let median = sizes[sizes.len() / 2];
        ((median as f64) * 0.6).ceil().max(1.0) as usize
    };

    let mut out: Vec<Kline> = Vec::new();
    for (bucket_start, a, b) in groups {
        let group = &sorted[a..b];
        // 桶终点 = 下一桶起点; 完整判据: 桶起点不早于首根, 且桶终点已被末根 close 覆盖
        // (`+1ms` 因为末根 close_time 是"桶终点 − 1ms", 覆盖到边界即算完整)。
        let bucket_end = bucket_start + tf_ms;
        let complete = bucket_start >= first_ms && bucket_end <= last_close_ms + 1;
        if !complete || group.len() < min_bars {
            continue;
        }
        let open = group[0].open;
        let close = group[group.len() - 1].close;
        let mut high = group[0].high;
        let mut low = group[0].low;
        let mut volume = Decimal::ZERO;
        for k in group {
            if k.high > high {
                high = k.high;
            }
            if k.low < low {
                low = k.low;
            }
            volume += k.volume;
        }
        out.push(Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(bucket_start)
                .unwrap_or(group[0].open_time),
            open,
            high,
            low,
            close,
            volume,
            close_time: DateTime::<Utc>::from_timestamp_millis(bucket_end - 1)
                .unwrap_or(group[group.len() - 1].close_time),
        });
    }
    out
}

/// 按 [`Interval`] 重采样(周期表唯一定义在 [`Interval`], 本函数只做转发)。
pub fn resample_to_interval(bars: &[Kline], interval: Interval) -> Vec<Kline> {
    resample_complete(bars, interval.ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    const MINUTE_MS: i64 = 60_000;

    fn bar(open_min: i64, price: i64) -> Kline {
        bar_step(open_min, price, 1)
    }

    /// 指定粒度的 bar(5m/1h 输入必须自带正确的 `close_time`, 否则末桶会被判不完整)。
    fn bar_step(open_min: i64, price: i64, step_min: i64) -> Kline {
        let open_ms = open_min * MINUTE_MS;
        let close_ms = open_ms + step_min * MINUTE_MS - 1;
        Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
            open: Decimal::from(price),
            high: Decimal::from(price),
            low: Decimal::from(price),
            close: Decimal::from(price),
            volume: dec!(1),
            close_time: DateTime::<Utc>::from_timestamp_millis(close_ms).unwrap(),
        }
    }

    /// 三小时 1m 序列(0..180 分钟), 重采样成 1h → 中间一小时完整, 首尾半桶丢弃。
    fn one_minute_bars(n: i64) -> Vec<Kline> {
        (0..n).map(|m| bar(m, 1000 + m)).collect()
    }

    #[test]
    fn test_first_and_last_partial_buckets_dropped() {
        // 0..180 分钟 = 3 个 1h 桶, 但首桶(0-60)起点=序列首根且覆盖到 60 → 完整;
        // 用 30..150 的序列构造真正的首尾半桶。
        let bars: Vec<Kline> = (30..150).map(|m| bar(m, 1000 + m)).collect();
        let out = resample_complete(&bars, 60 * MINUTE_MS);
        assert_eq!(out.len(), 1, "首尾半桶必须丢弃, 只剩中间的完整小时");
        let b = &out[0];
        assert_eq!(b.open_time.timestamp_millis(), 60 * MINUTE_MS, "桶起点 = 60m");
        assert_eq!(b.close_time.timestamp_millis(), 120 * MINUTE_MS - 1, "close_time 口径");
        assert_eq!(b.open, Decimal::from(1000 + 60), "open = 桶内首根 open");
        assert_eq!(b.close, Decimal::from(1000 + 119), "close = 桶内末根 close");
        assert_eq!(b.volume, Decimal::from(60), "成交量求和");
    }

    #[test]
    fn test_high_low_are_min_max_within_bucket() {
        let mut bars: Vec<Kline> = (0..180).map(|m| bar(m, 1000 + m)).collect();
        // 在第二个桶(60..120)内造一个尖峰与一个深谷。
        bars[70].high = Decimal::from(2000);
        bars[80].low = Decimal::from(500);
        let out = resample_complete(&bars, 60 * MINUTE_MS);
        assert_eq!(out.len(), 3);
        assert_eq!(out[1].high, Decimal::from(2000), "high = 桶内最大值");
        assert_eq!(out[1].low, Decimal::from(500), "low = 桶内最小值");
    }

    #[test]
    fn test_gap_bucket_is_dropped() {
        // 0..180 分钟, 但抽掉 60..120 桶内一半的 bar → 密度不足, 该桶丢弃。
        let bars: Vec<Kline> =
            (0..180).filter(|m| !(60..110).contains(m)).map(|m| bar(m, 1000 + m)).collect();
        let out = resample_complete(&bars, 60 * MINUTE_MS);
        assert_eq!(out.len(), 2, "缺口桶(仅剩 10/60 根)必须丢弃");
        assert_eq!(out[0].open_time.timestamp_millis(), 0);
        assert_eq!(out[1].open_time.timestamp_millis(), 120 * MINUTE_MS);
    }

    #[test]
    fn test_input_granularity_is_detected_not_hardcoded() {
        // 输入是 5m 序列(不是 1m): 0..24 小时按 5m 步进, 重采样成 4h。
        let five_min: Vec<Kline> = (0..288).map(|i| bar_step(i * 5, 1000 + i, 5)).collect();
        let out = resample_complete(&five_min, 4 * 60 * MINUTE_MS);
        assert_eq!(out.len(), 6, "24h / 4h = 6 个完整桶 (硬编码 1m 会全判缺口 → 0 桶)");
        assert!(out.iter().all(|b| b.volume == Decimal::from(48)), "每桶 48 根 5m");

        // 输入是 1h 序列, 重采样成 1d: 每天 24 根。
        let hourly: Vec<Kline> = (0..96).map(|i| bar_step(i * 60, 2000 + i, 60)).collect();
        let daily = resample_complete(&hourly, 24 * 60 * MINUTE_MS);
        assert_eq!(daily.len(), 4, "96h / 24h = 4 天");
        assert_eq!(daily[0].close, Decimal::from(2000 + 23));
        assert_eq!(daily[3].close, Decimal::from(2000 + 95));
    }

    /// 2026-09-21 真机踩到的回归: 美股日线每周只有 5 个交易日, 旧判据(理论 7 根 × 0.9)
    /// 会把每个周桶都判成缺口 → 重采样恒为 0 根 → 声明 `interval="1w"` 报"没有可用数据"。
    #[test]
    fn test_equity_weekly_buckets_survive_five_day_weeks() {
        let day = 24 * 60 * MINUTE_MS;
        let mut bars: Vec<Kline> = Vec::new();
        for week in 0..4i64 {
            for d in 0..5i64 {
                // 周一..周五(周末休市), 每天一根日线。
                bars.push(bar_step((week * 7 + d) * 24 * 60, 100 + week * 10 + d, 24 * 60));
            }
        }
        let weekly = resample_complete(&bars, 7 * day);
        assert_eq!(weekly.len(), 3, "4 个自然周里末周不完整 → 3 个周桶(旧判据得 0)");
        assert_eq!(weekly[0].open, bars[0].open, "首周 open = 该周首根 open");
        assert_eq!(weekly[0].volume, Decimal::from(5), "首周 = 5 个交易日");
        assert!(weekly.windows(2).all(|w| w[0].open_time < w[1].open_time), "输出必须升序");
    }

    #[test]
    fn test_empty_and_non_positive_tf_return_empty() {
        assert!(resample_complete(&[], 3_600_000).is_empty());
        let bars = one_minute_bars(60);
        assert!(resample_complete(&bars, 0).is_empty());
        assert!(resample_complete(&bars, -1).is_empty());
    }

    #[test]
    fn test_unsorted_input_is_sorted_first() {
        let mut bars: Vec<Kline> = (0..180).map(|m| bar(m, 1000 + m)).collect();
        bars.reverse();
        let out = resample_complete(&bars, 60 * MINUTE_MS);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].open_time.timestamp_millis(), 0);
        assert!(out.windows(2).all(|w| w[0].open_time < w[1].open_time), "输出必须升序");
    }

    #[test]
    fn test_resample_to_interval_matches_ms_variant() {
        let bars = one_minute_bars(180);
        assert_eq!(
            resample_to_interval(&bars, Interval::H1),
            resample_complete(&bars, Interval::H1.ms())
        );
        assert_eq!(resample_to_interval(&bars, Interval::D1).len(), 0, "3 小时不足一天");
    }

    #[test]
    fn test_single_bar_input_yields_nothing() {
        // 一根 bar 永远构不成完整桶(首尾都不完整)。
        let bars = vec![bar(0, 100)];
        assert!(resample_complete(&bars, 60 * MINUTE_MS).is_empty());
    }
}
