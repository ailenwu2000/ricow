//! 指标 API: 基于 `ta` crate (0.5) 的向量式计算器。
//!
//! 输入为**已收盘** K 线序列 (回测/实盘同一通道, 无前视), 逐条 `next()` 增量计算,
//! 输出末值或末值结构。数据不足返回 `None` (脚本侧为 nil)。
//! 不持有状态: 每次调用是一次性计算 (tick 频率低, 全量 feed 开销可忽略)。
//!
//! 注: ta 0.5 无 WMA/ADX/MOM, 此三项在模块内自实现 (简单/标准定义, 附公式);
//! stoch 用 FastStochastic (ta 0.5 仅单值输出, 无 k/d 双线结构)。

use mlua::Lua;
use ricow_core::Kline;
use rust_decimal::prelude::ToPrimitive;
use ta::indicators::{
    AverageTrueRange, BollingerBands, CommodityChannelIndex, ExponentialMovingAverage,
    FastStochastic, MovingAverageConvergenceDivergence, RateOfChange, RelativeStrengthIndex,
    SimpleMovingAverage,
};
use ta::{Close, High, Low, Next, Open, Volume};

/// MACD 输出: 主线 / 信号线 / 柱。
#[derive(Debug, Clone, Copy)]
pub struct MacdOutput {
    pub main: f64,
    pub signal: f64,
    pub hist: f64,
}

impl mlua::IntoLua for MacdOutput {
    fn into_lua(self, lua: &Lua) -> mlua::Result<mlua::Value> {
        let t = lua.create_table()?;
        t.set("main", self.main)?;
        t.set("signal", self.signal)?;
        t.set("hist", self.hist)?;
        Ok(mlua::Value::Table(t))
    }
}

/// 布林带输出: 上轨 / 中轨 / 下轨。
#[derive(Debug, Clone, Copy)]
pub struct BollOutput {
    pub upper: f64,
    pub mid: f64,
    pub lower: f64,
}

impl mlua::IntoLua for BollOutput {
    fn into_lua(self, lua: &Lua) -> mlua::Result<mlua::Value> {
        let t = lua.create_table()?;
        t.set("upper", self.upper)?;
        t.set("mid", self.mid)?;
        t.set("lower", self.lower)?;
        Ok(mlua::Value::Table(t))
    }
}

/// OHLC 适配视图 (Kline 是外部类型, 用本地类型实现 ta 的 Open/High/Low/Close/Volume)。
struct CandleView {
    open: f64,
    high: f64,
    low: f64,
    close: f64,
}

impl Open for CandleView {
    fn open(&self) -> f64 {
        self.open
    }
}
impl High for CandleView {
    fn high(&self) -> f64 {
        self.high
    }
}
impl Low for CandleView {
    fn low(&self) -> f64 {
        self.low
    }
}
impl Close for CandleView {
    fn close(&self) -> f64 {
        self.close
    }
}
impl Volume for CandleView {
    fn volume(&self) -> f64 {
        0.0
    }
}

fn to_f64s(klines: &[Kline]) -> Vec<f64> {
    klines.iter().map(|k| k.close.to_f64().unwrap_or(0.0)).collect()
}

fn to_candles(klines: &[Kline]) -> Vec<CandleView> {
    klines
        .iter()
        .map(|k| CandleView {
            open: k.open.to_f64().unwrap_or(0.0),
            high: k.high.to_f64().unwrap_or(0.0),
            low: k.low.to_f64().unwrap_or(0.0),
            close: k.close.to_f64().unwrap_or(0.0),
        })
        .collect()
}

/// NaN 视为数据不足 → None。
fn finite(v: f64) -> Option<f64> {
    if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

macro_rules! feed_last {
    ($ind:expr, $input:expr) => {{
        let mut ind = $ind;
        let mut last = f64::NAN;
        for v in $input {
            last = ind.next(v);
        }
        last
    }};
}

// ============================================================================
// ta 0.5 缺失指标: WMA / MOM / ADX (标准定义, 自实现)
// ============================================================================

/// 加权移动平均: wma = Σ(i·close_i) / Σ(i), 最新权重最大。
fn wma_values(closes: &[f64], period: usize) -> Option<f64> {
    if closes.len() < period || period == 0 {
        return None;
    }
    let tail = &closes[closes.len() - period..];
    let mut num = 0.0;
    let mut den = 0.0;
    for (i, &c) in tail.iter().enumerate() {
        let w = (i + 1) as f64;
        num += w * c;
        den += w;
    }
    finite(num / den)
}

/// 动量: close[last] - close[last - period]。
fn mom_value(closes: &[f64], period: usize) -> Option<f64> {
    if period == 0 || closes.len() <= period {
        return None;
    }
    finite(closes[closes.len() - 1] - closes[closes.len() - 1 - period])
}

/// Wilder ADX (标准定义): TR/+DM/-DM → 平滑 → DI → DX → ADX。
struct AdxState {
    period: usize,
    prev_high: Option<f64>,
    prev_low: Option<f64>,
    prev_close: Option<f64>,
    tr_smooth: Option<f64>,
    plus_dm_smooth: Option<f64>,
    minus_dm_smooth: Option<f64>,
    dx_smooth: Option<f64>,
    adx: f64,
}

fn adx_value(candles: &[CandleView], period: usize) -> Option<f64> {
    if period == 0 || candles.len() < period * 2 {
        return None;
    }
    let mut s = AdxState {
        period,
        prev_high: None,
        prev_low: None,
        prev_close: None,
        tr_smooth: None,
        plus_dm_smooth: None,
        minus_dm_smooth: None,
        dx_smooth: None,
        adx: f64::NAN,
    };
    for c in candles {
        if let (Some(ph), Some(pl), Some(pc)) = (s.prev_high, s.prev_low, s.prev_close) {
            let tr = (c.high - c.low).max((c.high - pc).abs()).max((c.low - pc).abs());
            let up_move = c.high - ph;
            let down_move = pl - c.low;
            let plus_dm = if up_move > down_move && up_move > 0.0 { up_move } else { 0.0 };
            let minus_dm = if down_move > up_move && down_move > 0.0 { down_move } else { 0.0 };
            let p = s.period as f64;
            s.tr_smooth = Some(match s.tr_smooth {
                Some(prev) => prev - prev / p + tr,
                None => tr,
            });
            s.plus_dm_smooth = Some(match s.plus_dm_smooth {
                Some(prev) => prev - prev / p + plus_dm,
                None => plus_dm,
            });
            s.minus_dm_smooth = Some(match s.minus_dm_smooth {
                Some(prev) => prev - prev / p + minus_dm,
                None => minus_dm,
            });
            if let (Some(trs), Some(pds), Some(mds)) =
                (s.tr_smooth, s.plus_dm_smooth, s.minus_dm_smooth)
            {
                if trs > 0.0 {
                    let plus_di = 100.0 * pds / trs;
                    let minus_di = 100.0 * mds / trs;
                    let di_sum = plus_di + minus_di;
                    if di_sum > 0.0 {
                        let dx = 100.0 * (plus_di - minus_di).abs() / di_sum;
                        s.dx_smooth = Some(match s.dx_smooth {
                            Some(prev) => prev - prev / p + dx,
                            None => dx,
                        });
                        if let Some(dxs) = s.dx_smooth {
                            s.adx = dxs;
                        }
                    }
                }
            }
        }
        s.prev_high = Some(c.high);
        s.prev_low = Some(c.low);
        s.prev_close = Some(c.close);
    }
    finite(s.adx)
}

// ============================================================================
// 对外指标 API (12 个)
// ============================================================================

/// EMA 末值。period ≤ 0 或序列不足 → None。
pub fn ema(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period {
        return None;
    }
    let ind = ExponentialMovingAverage::new(period).ok()?;
    finite(feed_last!(ind, to_f64s(klines)))
}

/// SMA 末值。
pub fn sma(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period {
        return None;
    }
    let ind = SimpleMovingAverage::new(period).ok()?;
    finite(feed_last!(ind, to_f64s(klines)))
}

/// WMA 末值 (自实现, ta 0.5 无)。
pub fn wma(klines: &[Kline], period: usize) -> Option<f64> {
    wma_values(&to_f64s(klines), period)
}

/// RSI 末值 (Wilder 平滑)。需 ≥ period+1 根。
pub fn rsi(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period + 1 {
        return None;
    }
    let ind = RelativeStrengthIndex::new(period).ok()?;
    finite(feed_last!(ind, to_f64s(klines)))
}

/// MACD (默认 12/26/9)。需 ≥ 26+9 根。
pub fn macd(klines: &[Kline]) -> Option<MacdOutput> {
    if klines.len() < 26 + 9 {
        return None;
    }
    let mut ind = MovingAverageConvergenceDivergence::new(12, 26, 9).ok()?;
    let mut out = None;
    for v in to_f64s(klines) {
        let r = ind.next(v);
        if r.macd.is_finite() {
            out = Some(MacdOutput { main: r.macd, signal: r.signal, hist: r.histogram });
        }
    }
    out
}

/// 布林带 (默认 20/2.0)。
pub fn boll(klines: &[Kline], period: usize, dev: f64) -> Option<BollOutput> {
    if period == 0 || klines.len() < period {
        return None;
    }
    let mut ind = BollingerBands::new(period, dev).ok()?;
    let mut out = None;
    for v in to_f64s(klines) {
        let r = ind.next(v);
        if r.upper.is_finite() {
            out = Some(BollOutput { upper: r.upper, mid: r.average, lower: r.lower });
        }
    }
    out
}

/// ATR 末值 (默认 14)。需 ≥ period+1 根。
pub fn atr(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period + 1 {
        return None;
    }
    let mut ind = AverageTrueRange::new(period).ok()?;
    let candles = to_candles(klines);
    let mut last = f64::NAN;
    for c in &candles {
        last = ind.next(c);
    }
    finite(last)
}

/// ADX 末值 (默认 14, 自实现, ta 0.5 无)。需 ≥ period*2 根。
pub fn adx(klines: &[Kline], period: usize) -> Option<f64> {
    adx_value(&to_candles(klines), period)
}

/// 随机指标 %K 末值 (默认 14; FastStochastic, ta 0.5 仅单值输出)。需 ≥ period 根。
pub fn stoch(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period {
        return None;
    }
    let mut ind = FastStochastic::new(period).ok()?;
    let candles = to_candles(klines);
    let mut last = f64::NAN;
    for c in &candles {
        last = ind.next(c);
    }
    finite(last)
}

/// CCI 末值 (默认 20)。需 ≥ period 根。
pub fn cci(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period {
        return None;
    }
    let mut ind = CommodityChannelIndex::new(period).ok()?;
    let candles = to_candles(klines);
    let mut last = f64::NAN;
    for c in &candles {
        last = ind.next(c);
    }
    finite(last)
}

/// ROC 末值 (默认 10)。需 ≥ period+1 根。
pub fn roc(klines: &[Kline], period: usize) -> Option<f64> {
    if period == 0 || klines.len() < period + 1 {
        return None;
    }
    let ind = RateOfChange::new(period).ok()?;
    finite(feed_last!(ind, to_f64s(klines)))
}

/// MOM 末值 (默认 10, 自实现, ta 0.5 无)。需 > period 根。
pub fn mom(klines: &[Kline], period: usize) -> Option<f64> {
    mom_value(&to_f64s(klines), period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rust_decimal_macros::dec;

    fn kline(close: f64) -> Kline {
        let d = rust_decimal::Decimal::from_f64_retain(close).unwrap_or_default();
        Kline {
            open_time: Utc::now(),
            open: d,
            high: d,
            low: d,
            close: d,
            volume: dec!(1),
            close_time: Utc::now(),
        }
    }

    fn series(closes: &[f64]) -> Vec<Kline> {
        closes.iter().map(|&c| kline(c)).collect()
    }

    #[test]
    fn test_ema_known_vector() {
        // EMA(3), multiplier = 2/4 = 0.5:
        // 1 → 1; 2 → 1.5; 3 → 2.25; 4 → 3.125; 5 → 4.0625。
        let klines = series(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let v = ema(&klines, 3).unwrap();
        assert!((v - 4.0625).abs() < 1e-9, "ema = {v}");
    }

    #[test]
    fn test_rsi_textbook_sequence() {
        // StockCharts 经典 RSI(14) 例子: 第 14 根 ≈ 70.46; 全序列 (20 根) 末值 ≈ 46.79。
        let closes = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03,
            45.61, 46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64,
        ];
        let v = rsi(&series(&closes), 14).unwrap();
        assert!((v - 46.79).abs() < 0.5, "rsi = {v}");
    }

    #[test]
    fn test_macd_structure() {
        let closes: Vec<f64> = (1..=60).map(|i| i as f64 + (i % 7) as f64 * 0.3).collect();
        let out = macd(&series(&closes)).expect("数据充足应返回");
        assert!(out.main.is_finite());
        assert!(out.signal.is_finite());
        assert!(out.hist.is_finite());
    }

    #[test]
    fn test_boll_structure() {
        let closes: Vec<f64> = (1..=40).map(|i| 100.0 + (i % 5) as f64).collect();
        let out = boll(&series(&closes), 20, 2.0).expect("数据充足应返回");
        assert!(out.upper > out.mid && out.mid > out.lower);
    }

    #[test]
    fn test_wma_mom_known() {
        // WMA(3) on [1,2,3,4]: (1*1+2*2+3*3+4*4)/10? 不 — 只算最后 3 个: (2*1+3*2+4*3)/6 = 20/6。
        let klines = series(&[1.0, 2.0, 3.0, 4.0]);
        let w = wma(&klines, 3).unwrap();
        assert!((w - 20.0 / 6.0).abs() < 1e-9, "wma = {w}");
        // MOM(2) on [1,2,3,4]: 4 - 2 = 2。
        let m = mom(&klines, 2).unwrap();
        assert!((m - 2.0).abs() < 1e-9, "mom = {m}");
    }

    #[test]
    fn test_insufficient_data_returns_none() {
        let klines = series(&[1.0, 2.0, 3.0]);
        assert!(rsi(&klines, 14).is_none());
        assert!(macd(&klines).is_none());
        assert!(atr(&klines, 14).is_none());
        assert!(adx(&klines, 14).is_none());
        assert!(wma(&klines, 5).is_none());
        assert!(mom(&klines, 5).is_none());
        assert!(ema(&klines, 0).is_none());
        assert!(sma(&[], 5).is_none());
    }

    #[test]
    fn test_candle_indicators_work() {
        let closes: Vec<f64> = (1..=50).map(|i| 100.0 + (i % 9) as f64).collect();
        let klines = series(&closes);
        assert!(atr(&klines, 14).is_some());
        assert!(adx(&klines, 14).is_some());
        assert!(stoch(&klines, 14).is_some());
        assert!(cci(&klines, 20).is_some());
        assert!(roc(&klines, 10).is_some());
        assert!(sma(&klines, 5).is_some());
    }
}
