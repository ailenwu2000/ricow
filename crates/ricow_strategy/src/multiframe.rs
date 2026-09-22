//! 多周期支持: 由主序列(如 1m)重采样出高周期 K 线(如 1h), 并提供高周期 ATR 缓存。
//!
//! 用途(023 香农 ETF 指数增加策略): 策略在 1m 主序列上决策(金叉建仓 / 挂单撮合分辨率),
//! 但间距用 **1h ATR** —— 引擎侧需要一条"第二序列"通道, 且必须无前视、可缓存(每桶只算一次)。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ricow_core::Kline;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::indicators_api;

const MINUTE_MS: i64 = 60_000;

/// K 线周期标签 → 毫秒(仅支持本引擎既有周期; 未知返回 None)。
pub fn tf_ms_of(tf: &str) -> Option<i64> {
    let ms = match tf {
        "1m" => MINUTE_MS,
        "5m" => 5 * MINUTE_MS,
        "15m" => 15 * MINUTE_MS,
        "30m" => 30 * MINUTE_MS,
        "1h" => 60 * MINUTE_MS,
        "4h" => 4 * 60 * MINUTE_MS,
        "1d" => 24 * 60 * MINUTE_MS,
        _ => return None,
    };
    Some(ms)
}

/// 由主序列(升序)重采样出**完整**的高周期 bar。
///
/// - 桶键 = `open_time.ms.div_euclid(tf_ms)`(周期边界对齐)。
/// - **完整桶判据**: 桶起点 ≥ 序列首根 `open_time`, 且桶终点 ≤ 序列末根 `close_time`
///   —— 首尾半桶一律丢弃(半个桶的 high/low 不能代表该周期)。
/// - **缺口守卫**: 主序列为分钟级时, 桶内 bar 数 < 期望根数 × 0.9 视为缺口, 丢弃并记 debug 日志
///   (防把"只覆盖了半小时的 1h 桶"当完整桶, 算错 ATR)。
/// - 聚合: open = 首根 open, close = 末根 close, high = max, low = min, volume = Σ,
///   `close_time = 桶起点 + tf_ms − 1ms`。
///
/// 输入乱序/空序列 → 返回空(调用方保证升序是引擎既有契约, 此处只做防御)。
pub fn resample_complete(bars: &[Kline], tf_ms: i64) -> Vec<Kline> {
    if tf_ms <= 0 || bars.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&Kline> = bars.iter().collect();
    sorted.sort_by_key(|k| k.open_time);
    let first_ms = sorted[0].open_time.timestamp_millis();
    let last_close_ms = sorted[sorted.len() - 1].close_time.timestamp_millis();

    // 输入粒度: 取前 5000 个相邻差值的**众数**(缺口只会把差值放大, 众数即真实粒度)。
    // **不能把输入硬编码成 1 分钟** —— 主序列本身可能是 5m/15m/1h/4h/1d(023 六周期实测
    // 2026-09-18: 硬编码会让所有桶被判"缺口"→ 0 桶 → ATR 永不就绪 → 全程不下单)。
    let mut diffs: Vec<i64> = sorted
        .windows(2)
        .take(5000)
        .map(|w| w[1].open_time.timestamp_millis() - w[0].open_time.timestamp_millis())
        .filter(|d| *d > 0)
        .collect();
    diffs.sort_unstable();
    let input_ms = if diffs.is_empty() {
        tf_ms
    } else {
        let mut best = (diffs[0], 1usize);
        let mut cur = (diffs[0], 1usize);
        for &d in &diffs[1..] {
            if d == cur.0 {
                cur.1 += 1;
            } else {
                if cur.1 > best.1 {
                    best = cur;
                }
                cur = (d, 1);
            }
        }
        if cur.1 > best.1 {
            best = cur;
        }
        best.0
    };
    // 期望根数 = tf / 输入粒度(输入比 tf 粗时退化为 1, 不做缺口过滤)。
    let expected = ((tf_ms / input_ms.max(1)) as usize).max(1);
    let min_bars = ((expected as f64) * 0.9).ceil() as usize;

    let mut out: Vec<Kline> = Vec::new();
    let mut i = 0usize;
    while i < sorted.len() {
        let bucket_start = sorted[i].open_time.timestamp_millis().div_euclid(tf_ms) * tf_ms;
        let mut j = i;
        while j < sorted.len()
            && sorted[j].open_time.timestamp_millis().div_euclid(tf_ms) * tf_ms == bucket_start
        {
            j += 1;
        }
        let group = &sorted[i..j];
        // 桶终点 = 下一桶起点; 完整判据: 桶起点不早于首根, 且桶终点已被末根 close 覆盖
        // (`+1ms` 因为末根 close_time 是"桶终点 − 1ms", 覆盖到边界即算完整)。
        let bucket_end = bucket_start + tf_ms;
        let complete = bucket_start >= first_ms && bucket_end <= last_close_ms + 1;
        let dense = group.len() >= min_bars;
        if complete && dense {
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
        } else {
            tracing::debug!(
                target: "multiframe",
                bucket = bucket_start,
                bars = group.len(),
                complete,
                dense,
                "高周期桶丢弃(不完整或缺口)"
            );
        }
        i = j;
    }
    out
}

/// 高周期序列缓存 (第二序列)。
///
/// - 持有一条高周期序列(升序, 已由 [`resample_complete`] 剔除不完整/缺口桶)。
/// - **可见性**(无前视): 桶 `open_time + tf_ms ≤ now_ms` 才算已收盘; 与引擎既有
///   "当前 bar 对策略不可见" 的口径一致。
/// - **读数**: ATR(023 网格间距) / EMA(2026-09-18 趋势判据) / 上一根已收盘 bar 的 `close`
///   (趋势判据的输入, 逐字口径"最近的日线 K 线的 close")。三者共用同一可见前缀。
/// - **缓存**: 记忆 `(period) → (最后可见桶 open_time, 值)`; 同桶内重复调用直接返缓存值
///   —— 即"每根高周期 bar 只算一次"; 计算只对可见前缀切片做, **不克隆序列**。
/// - 内部可变(`Cell`/`RefCell`): 三个 ctx 实现都以 `&self` 持有缓存(实盘/模拟盘走
///   `RwLock`, 回测直接存档), 缓存命中判定必须能在 `&self` 下更新。
pub struct TfCache {
    tf_ms: i64,
    bars: Vec<Kline>,
    memo: RefCell<HashMap<usize, (i64, f64)>>,
    ema_memo: RefCell<HashMap<usize, (i64, f64)>>,
    computes: Cell<usize>,
}

/// 高周期缓存的键: `pair|tf` —— 同一 pair 可同时装多套序列(如 4h ATR + 日线趋势判据)。
/// 键格式只此一处定义, 装配层与三个 ctx 实现共用。
pub fn tf_key(pair: &str, tf: &str) -> String {
    format!("{pair}|{tf}")
}

impl TfCache {
    pub fn new(tf_ms: i64, bars: Vec<Kline>) -> Self {
        Self {
            tf_ms,
            bars,
            memo: RefCell::new(HashMap::new()),
            ema_memo: RefCell::new(HashMap::new()),
            computes: Cell::new(0),
        }
    }

    /// 已收盘桶数量(可见前缀长度)。按升序 + 等间距假定, 用二分定位。
    pub fn visible_len(&self, now_ms: i64) -> usize {
        self.bars.partition_point(|k| k.open_time.timestamp_millis() + self.tf_ms <= now_ms)
    }

    /// 可见前缀(无前视); 供 `indicators_api::atr` 等直接切片使用。
    pub fn visible(&self, now_ms: i64) -> &[Kline] {
        &self.bars[..self.visible_len(now_ms)]
    }

    /// 高周期 ATR; 数据不足(可见桶 < period+1)返回 None。
    pub fn atr(&self, period: usize, now_ms: i64) -> Option<f64> {
        if period == 0 {
            return None;
        }
        let n = self.visible_len(now_ms);
        if n < period + 1 {
            return None;
        }
        let last_ts = self.bars[n - 1].open_time.timestamp_millis();
        if let Some((ts, v)) = self.memo.borrow().get(&period) {
            if *ts == last_ts {
                return Some(*v);
            }
        }
        let v = indicators_api::atr(&self.bars[..n], period)?;
        self.computes.set(self.computes.get() + 1);
        self.memo.borrow_mut().insert(period, (last_ts, v));
        Some(v)
    }

    /// **上一根已收盘**高周期 bar 的收盘价 (2026-09-18, 趋势判据输入)。
    /// 逐字口径: "最近的日线 K 线的 close" —— 未收盘的那根 bar 不可见(无前视);
    /// 一根高周期 bar 内重复调用返回同一个值。序列为空 → `None`。
    pub fn close(&self, now_ms: i64) -> Option<f64> {
        let n = self.visible_len(now_ms);
        if n == 0 {
            return None;
        }
        self.bars[n - 1].close.to_f64()
    }

    /// 高周期 EMA (2026-09-18, 趋势判据) —— 对**可见前缀**的收盘序列取 EMA 末值;
    /// 数据不足 (可见桶 < period) 返回 `None`。与 `atr` 同款: 按桶记忆, 每根高周期 bar 只算一次。
    /// 注意 `ta` 的 EMA 用**首值种**, 序列起点越早越准 → 预热长度是装配层责任(见 CLI warmup)。
    pub fn ema(&self, period: usize, now_ms: i64) -> Option<f64> {
        if period == 0 {
            return None;
        }
        let n = self.visible_len(now_ms);
        if n < period {
            return None;
        }
        let last_ts = self.bars[n - 1].open_time.timestamp_millis();
        if let Some((ts, v)) = self.ema_memo.borrow().get(&period) {
            if *ts == last_ts {
                return Some(*v);
            }
        }
        let v = indicators_api::ema(&self.bars[..n], period)?;
        self.computes.set(self.computes.get() + 1);
        self.ema_memo.borrow_mut().insert(period, (last_ts, v));
        Some(v)
    }

    /// 实际计算次数(同桶内缓存命中不递增); 用于断言"每桶只算一次"(ATR 与 EMA 合并计数)。
    pub fn computes(&self) -> usize {
        self.computes.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一根 1m K 线(从 `minute` 分钟起), open=close=base+minute, high/low 上下各 0.5。
    fn bar_minute(minute: i64, base: i64, vol: i64) -> Kline {
        let open_ms = minute * MINUTE_MS;
        let v = Decimal::from(base + minute);
        let half = Decimal::new(5, 1);
        Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
            open: v,
            high: v + half,
            low: v - half,
            close: v,
            volume: Decimal::from(vol),
            close_time: DateTime::<Utc>::from_timestamp_millis(open_ms + MINUTE_MS - 1).unwrap(),
        }
    }

    fn bar_hour(hour: i64, base: i64) -> Kline {
        let open_ms = hour * 60 * MINUTE_MS;
        let v = Decimal::from(base + hour);
        Kline {
            open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
            open: v,
            high: v + Decimal::from(3),
            low: v - Decimal::from(2),
            close: v + Decimal::from(1),
            volume: Decimal::from(10),
            close_time: DateTime::<Utc>::from_timestamp_millis(open_ms + 60 * MINUTE_MS - 1)
                .unwrap(),
        }
    }

    #[test]
    fn tf_ms_of_known_labels() {
        assert_eq!(tf_ms_of("1m"), Some(60_000));
        assert_eq!(tf_ms_of("15m"), Some(900_000));
        assert_eq!(tf_ms_of("1h"), Some(3_600_000));
        assert_eq!(tf_ms_of("4h"), Some(14_400_000));
        assert_eq!(tf_ms_of("1d"), Some(86_400_000));
        assert_eq!(tf_ms_of("3h"), None, "未支持周期必须返回 None, 不能猜");
    }

    #[test]
    fn resample_drops_partial_head_and_tail_buckets() {
        // 分钟 30..149(120 根): 桶0(0-60m)只覆盖 30-59 → 半桶; 桶1(60-120m)完整;
        // 桶2(120-180m)只覆盖 120-149 → 半桶。期望恰好 1 根完整桶。
        let bars: Vec<Kline> = (30..150).map(|m| bar_minute(m, 1000, 1)).collect();
        let tf = tf_ms_of("1h").unwrap();
        let out = resample_complete(&bars, tf);
        assert_eq!(out.len(), 1, "首尾半桶必须丢弃");
        let b = &out[0];
        assert_eq!(b.open_time.timestamp_millis(), 60 * MINUTE_MS, "桶起点 = 60m");
        assert_eq!(b.close_time.timestamp_millis(), 120 * MINUTE_MS - 1, "close_time 口径");
        // 聚合值手算: open=首根(60m)的 open, close=末根(119m)的 close, high/low 取极值。
        assert_eq!(b.open, Decimal::from(1000 + 60));
        assert_eq!(b.close, Decimal::from(1000 + 119));
        assert_eq!(b.high, Decimal::from(1000 + 119) + Decimal::new(5, 1));
        assert_eq!(
            b.low,
            Decimal::from(1000 + 60) - Decimal::new(5, 1),
            "low 取桶内极值(60m), 不是全序列极值"
        );
        assert_eq!(b.volume, Decimal::from(60), "成交量求和");
    }

    #[test]
    fn resample_drops_gap_bucket() {
        // 桶1 完整(60..119), 桶2 缺 10 根(120..169, 只有 50 根, < 54), 桶3 完整(180..239)。
        let mut bars: Vec<Kline> = (60..120).map(|m| bar_minute(m, 1000, 1)).collect();
        bars.extend((120..170).map(|m| bar_minute(m, 1000, 1)));
        bars.extend((180..240).map(|m| bar_minute(m, 1000, 1)));
        let tf = tf_ms_of("1h").unwrap();
        let out = resample_complete(&bars, tf);
        let starts: Vec<i64> = out.iter().map(|k| k.open_time.timestamp_millis()).collect();
        assert_eq!(
            starts,
            vec![60 * MINUTE_MS, 180 * MINUTE_MS],
            "缺口桶(50 根 < 54)必须丢弃, 完整桶保留"
        );
    }

    #[test]
    fn resample_empty_or_invalid_tf() {
        assert!(resample_complete(&[], 3_600_000).is_empty());
        let bars: Vec<Kline> = (0..120).map(|m| bar_minute(m, 100, 1)).collect();
        assert!(resample_complete(&bars, 0).is_empty());
    }

    #[test]
    fn tf_atr_visibility_no_lookahead() {
        // 15 根 1h bar, 每根 high-low 固定 → ATR 有确定值。
        let bars: Vec<Kline> = (0..16).map(|h| bar_hour(h, 100)).collect();
        let tf = tf_ms_of("1h").unwrap();
        let c = TfCache::new(tf, bars);

        // 边界: 第 14 根收盘后(可见 14)不足 period+1=15 → None。
        assert_eq!(c.visible_len(14 * 60 * MINUTE_MS), 14);
        assert_eq!(c.atr(14, 14 * 60 * MINUTE_MS), None, "14 根 < 15 必须 None");
        // 第 15 根收盘后 → 有值。
        let now15 = 15 * 60 * MINUTE_MS;
        assert_eq!(c.visible_len(now15), 15);
        let v15 = c.atr(14, now15).expect("15 根应可算 ATR");
        // 与直接对可见前缀调用指标库一致。
        let direct = indicators_api::atr(c.visible(now15), 14).unwrap();
        assert!((v15 - direct).abs() < 1e-12, "缓存值与指标库直算必须一致");

        // 无前视: 多推一根 bar 后, 同一个 now 的可见段与结果不变。
        let now_inside_next_bucket = 15 * 60 * MINUTE_MS + 30 * MINUTE_MS; // 第 16 桶未收盘
        assert_eq!(c.visible_len(now_inside_next_bucket), 15, "未收盘桶不可见");
        let v_same = c.atr(14, now_inside_next_bucket).unwrap();
        assert!((v_same - v15).abs() < 1e-12, "未收盘桶不得影响结果(无前视)");
    }

    #[test]
    fn tf_atr_computes_once_per_bucket() {
        let bars: Vec<Kline> = (0..20).map(|h| bar_hour(h, 100)).collect();
        let tf = tf_ms_of("1h").unwrap();
        let c = TfCache::new(tf, bars);
        let now = 18 * 60 * MINUTE_MS; // 可见 18 根 ≥ 15
        assert!(c.atr(14, now).is_some());
        assert_eq!(c.computes(), 1);
        for _ in 0..5 {
            assert!(c.atr(14, now).is_some());
        }
        assert_eq!(c.computes(), 1, "同一桶内只算一次");
        // 下一桶可见后重算一次。
        assert!(c.atr(14, 19 * 60 * MINUTE_MS).is_some());
        assert_eq!(c.computes(), 2);
    }

    #[test]
    fn tf_close_is_last_closed_bucket_close() {
        // 12 根 1h bar, close = base + hour + 1 (见 bar_hour) → 可用 close 反查"用到哪一根"。
        let bars: Vec<Kline> = (0..12).map(|h| bar_hour(h, 100)).collect();
        let tf = tf_ms_of("1h").unwrap();
        let c = TfCache::new(tf, bars);

        // 第 5 桶收盘后: 可见 5 根(h=0..4), 最近已收盘 = h=4 → close = 105。
        let now5 = 5 * 60 * MINUTE_MS;
        assert_eq!(c.close(now5), Some(105.0), "取上一根已收盘 bar 的 close");
        // 落在第 6 桶内(未收盘)不得改变读数 → 无前视。
        assert_eq!(c.close(now5 + 30 * MINUTE_MS), Some(105.0));
        // 第 6 桶收盘 → 前移一根。
        assert_eq!(c.close(6 * 60 * MINUTE_MS), Some(106.0));
        // 尚无已收盘桶 → None(不猜值)。
        assert_eq!(c.close(0), None);
    }

    #[test]
    fn tf_ema_no_lookahead_and_cached_per_bucket() {
        let bars: Vec<Kline> = (0..30).map(|h| bar_hour(h, 100)).collect();
        let tf = tf_ms_of("1h").unwrap();
        let c = TfCache::new(tf, bars);

        // 可见 9 根 < period=10 → None。
        assert_eq!(c.ema(10, 9 * 60 * MINUTE_MS), None);
        let now10 = 10 * 60 * MINUTE_MS;
        let v = c.ema(10, now10).expect("可见 10 根应可算 EMA");
        let direct = indicators_api::ema(c.visible(now10), 10).unwrap();
        assert!((v - direct).abs() < 1e-12, "缓存值与指标库直算必须一致");
        assert_eq!(c.computes(), 1);

        // 同桶内重复调用不重算; 未收盘桶不得影响结果。
        for _ in 0..5 {
            assert_eq!(c.ema(10, now10), Some(v));
        }
        assert_eq!(c.computes(), 1, "同一桶内只算一次");
        assert_eq!(c.ema(10, now10 + 30 * MINUTE_MS), Some(v), "未收盘桶不得影响结果(无前视)");
        assert_eq!(c.computes(), 1);
        // 下一桶可见后重算一次。
        assert!(c.ema(10, 11 * 60 * MINUTE_MS).is_some());
        assert_eq!(c.computes(), 2);
    }

    /// 回归(2026-09-18 六周期实测): 缺口守卫曾把输入粒度硬编码为 1 分钟,
    /// 导致 5m/15m/1h/4h/1d 主序列重采样**全部桶被丢弃**(0 桶 → ATR 永不就绪)。
    #[test]
    fn resample_accepts_coarser_input() {
        // 构造指定周期(分钟)的真实 bar: close_time = open + step − 1ms。
        fn bar_step(open_min: i64, step_min: i64, base: i64) -> Kline {
            let open_ms = open_min * MINUTE_MS;
            let v = Decimal::from(base);
            Kline {
                open_time: DateTime::<Utc>::from_timestamp_millis(open_ms).unwrap(),
                open: v,
                high: v,
                low: v,
                close: v,
                volume: Decimal::ONE,
                close_time: DateTime::<Utc>::from_timestamp_millis(
                    open_ms + step_min * MINUTE_MS - 1,
                )
                .unwrap(),
            }
        }
        let tf_1h = tf_ms_of("1h").unwrap();
        // 5m 主序列(2 小时 = 24 根, 每桶 12 根) → 必须产出 2 个 1h 桶。
        let bars5: Vec<Kline> = (0..24).map(|i| bar_step(i * 5, 5, 100 + i)).collect();
        let out = resample_complete(&bars5, tf_1h);
        assert_eq!(out.len(), 2, "5m → 1h 应产出 2 桶(旧实现为 0)");
        assert_eq!(out[0].open, bars5[0].open);
        assert_eq!(out[0].close, bars5[11].close);
        assert_eq!(out[1].close, bars5[23].close);
        // 15m 主序列(2 小时 = 8 根, 每桶 4 根) → 2 桶。
        let bars15: Vec<Kline> = (0..8).map(|i| bar_step(i * 15, 15, 100 + i)).collect();
        assert_eq!(resample_complete(&bars15, tf_1h).len(), 2, "15m → 1h 应产出 2 桶");
        // 同周期(1h 主序列 → 1h ATR): 每桶 1 根, 也必须保留(4h/1d 主序列走同一路径)。
        let bars_h: Vec<Kline> = (0..6).map(|h| bar_hour(h, 100)).collect();
        assert_eq!(resample_complete(&bars_h, tf_1h).len(), 6, "1h → 1h 应逐根保留");
    }
}
