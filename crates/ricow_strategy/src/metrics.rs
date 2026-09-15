//! 回测风险/收益指标纯函数。
//!
//! 输入: 权益曲线 (每 bar 收盘估值, 含未实现盈亏) + 时间跨度 (秒) + 无风险利率 (年化 %)。
//! 输出: 年化收益/波动率/夏普/索提诺/Calmar/盈亏比/平均盈亏; 数据不足返回 None。
//! 全部 f64 计算; 年化因子基于平均每根 bar 时长, 精确到秒。

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

/// 每年秒数 (365.25 天)。
pub const SECONDS_PER_YEAR: f64 = 365.25 * 86400.0;

/// 无风险利率默认值 (年化 %): 3M 美国国债收益率, 2026-08-31 实测 3.70~3.82% 取中值。
/// 来源: 美联储 H.15 / Trading Economics。可用策略参数 `risk_free_rate` 覆盖。
pub const DEFAULT_RISK_FREE_RATE_PCT: f64 = 3.8;

/// 权益曲线 → 相邻点收益率序列。曲线 <2 点或前值 <=0 返回 None。
fn returns(equity: &[Decimal]) -> Option<Vec<f64>> {
    if equity.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(equity.len() - 1);
    for w in equity.windows(2) {
        let prev = w[0].to_f64()?;
        let cur = w[1].to_f64()?;
        if prev <= 0.0 {
            return None;
        }
        out.push((cur - prev) / prev);
    }
    Some(out)
}

/// 平均每根 bar 时长 (秒) = span / 收益率数量。span <= 0 → None。
fn bar_span(span_seconds: f64, n: usize) -> Option<f64> {
    if span_seconds <= 0.0 || n == 0 {
        return None;
    }
    let s = span_seconds / n as f64;
    if s <= 0.0 {
        None
    } else {
        Some(s)
    }
}

/// 每 bar 无风险收益率: (1 + rf_annual)^(bar_span/SPY) − 1。
fn rf_per_bar(rf_annual_pct: f64, bar_span_sec: f64) -> f64 {
    (1.0 + rf_annual_pct / 100.0).powf(bar_span_sec / SECONDS_PER_YEAR) - 1.0
}

/// 年化收益率 (几何, 行业标准口径): (期末/期初)^(年/span) − 1。
/// 注: 旧实现为算术均值复利 (1+mean)^bars_per_year − 1, 波动 >0 时系统性高估几何收益
/// (R1 十年波动 ~60% 实例: 算术 102% vs 几何 ~74%); 2026-09-09 修正为几何口径。
pub fn annual_return(equity: &[Decimal], span_seconds: f64) -> Option<f64> {
    if equity.len() < 2 || span_seconds <= 0.0 {
        return None;
    }
    let first = equity.first()?.to_f64()?;
    let last = equity.last()?.to_f64()?;
    if first <= 0.0 || last <= 0.0 {
        return None; // 期初/期末权益非正 → 无意义
    }
    Some((last / first).powf(SECONDS_PER_YEAR / span_seconds) - 1.0)
}

/// 年化波动率: 样本标准差 × sqrt(bars_per_year)。收益率 <2 个或方差为 0 → None。
pub fn annual_volatility(equity: &[Decimal], span_seconds: f64) -> Option<f64> {
    let rs = returns(equity)?;
    let n = rs.len();
    let span = bar_span(span_seconds, n)?;
    if n < 2 {
        return None;
    }
    let mean = rs.iter().sum::<f64>() / n as f64;
    let var = rs.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    // 方差容差 1e-18: 等比序列经 Decimal→f64 转换后方差是 ~5e-34 而非 0, 视为无波动。
    if var < 1e-18 {
        return None;
    }
    Some(var.sqrt() * (SECONDS_PER_YEAR / span).sqrt())
}

/// 夏普比率: (mean(r) − rf_per_bar) / σ × sqrt(bars_per_year)。
pub fn sharpe(equity: &[Decimal], span_seconds: f64, rf_annual_pct: f64) -> Option<f64> {
    let rs = returns(equity)?;
    let n = rs.len();
    let span = bar_span(span_seconds, n)?;
    if n < 2 {
        return None;
    }
    let mean = rs.iter().sum::<f64>() / n as f64;
    let var = rs.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    if var < 1e-18 {
        return None;
    }
    let rf = rf_per_bar(rf_annual_pct, span);
    Some((mean - rf) / var.sqrt() * (SECONDS_PER_YEAR / span).sqrt())
}

/// 索提诺比率: (mean(r) − rf_per_bar) / 下行偏差 × sqrt(bars_per_year)。
/// 下行偏差 = sqrt(mean(min(r − rf, 0)²)), 以 rf 为最小可接受收益。
pub fn sortino(equity: &[Decimal], span_seconds: f64, rf_annual_pct: f64) -> Option<f64> {
    let rs = returns(equity)?;
    let n = rs.len();
    let span = bar_span(span_seconds, n)?;
    let rf = rf_per_bar(rf_annual_pct, span);
    let mean = rs.iter().sum::<f64>() / n as f64;
    let downside = rs.iter().map(|r| (r - rf).min(0.0).powi(2)).sum::<f64>() / n as f64;
    if downside < 1e-18 {
        return None;
    }
    Some((mean - rf) / downside.sqrt() * (SECONDS_PER_YEAR / span).sqrt())
}

/// Calmar 比率: 年化收益 / |最大回撤|。年化为 None 或回撤为 0 → None。
pub fn calmar(annual_ret: Option<f64>, max_drawdown: f64) -> Option<f64> {
    let ar = annual_ret?;
    if max_drawdown <= 0.0 {
        return None;
    }
    Some(ar / max_drawdown)
}

/// 盈亏比 (profit factor): 总盈利 / 总亏损。无亏损 → None。
pub fn profit_factor(gross_profit: f64, gross_loss: f64) -> Option<f64> {
    if gross_loss <= 0.0 {
        return None;
    }
    Some(gross_profit / gross_loss)
}

/// 平均盈利 / 平均亏损。无任何交易 → None。
pub fn avg_win_loss(
    gross_profit: f64,
    win_count: u64,
    gross_loss: f64,
    loss_count: u64,
) -> Option<(f64, f64)> {
    if win_count == 0 && loss_count == 0 {
        return None;
    }
    let avg_win = if win_count > 0 { gross_profit / win_count as f64 } else { 0.0 };
    let avg_loss = if loss_count > 0 { gross_loss / loss_count as f64 } else { 0.0 };
    Some((avg_win, avg_loss))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn eq(e: &[Decimal]) -> Vec<Decimal> {
        e.to_vec()
    }

    #[test]
    fn test_annual_return_known_vectors() {
        // [100, 110] 跨度 1 年 → 10%。
        let a = annual_return(&eq(&[dec!(100), dec!(110)]), SECONDS_PER_YEAR).unwrap();
        assert!((a - 0.10).abs() < 1e-9, "annual_return = {a}");
        // [100, 110] 跨度 90 天 → (1.1)^(365.25/90) − 1 ≈ 47.2%。
        let b = annual_return(&eq(&[dec!(100), dec!(110)]), 90.0 * 86400.0).unwrap();
        assert!((b - 0.472).abs() < 0.005, "annual_return 90d = {b}");
        // 几何口径防回归: [100,110,99] 两年总收益 −1% → 年化 ≈ √0.99 − 1 = −0.50%
        // (旧算术均值口径: mean=(10%−10%)/2=0 → 年化 0%, 会漏报亏损年化)。
        let c =
            annual_return(&eq(&[dec!(100), dec!(110), dec!(99)]), 2.0 * SECONDS_PER_YEAR).unwrap();
        assert!(
            (c - (0.99f64.sqrt() - 1.0)).abs() < 1e-9,
            "annual_return 几何 = {c} (应 ≈ −0.50%)"
        );
        // 曲线 <2 点 / 跨度 <=0 → None。
        assert!(annual_return(&eq(&[dec!(100)]), SECONDS_PER_YEAR).is_none());
        assert!(annual_return(&eq(&[dec!(100), dec!(110)]), 0.0).is_none());
    }

    #[test]
    fn test_annual_volatility_known_vector() {
        // 收益率 [1%, 3%]: mean=2%, 样本方差 = ((−0.01)²+(0.01)²)/1 = 0.0002,
        // σ = 1.4142%; span=1 年 2 根 → bar_span=SPY/2, bars_per_year=2 → σ年化 = σ×√2 = 2%。
        let e = eq(&[dec!(100), dec!(101), dec!(104.03)]);
        let v = annual_volatility(&e, SECONDS_PER_YEAR).unwrap();
        assert!((v - 0.02).abs() < 1e-9, "vol = {v}");
        // 等比序列 (收益率恒 1%) → 方差 0 → None。
        assert!(annual_volatility(&eq(&[dec!(100), dec!(101), dec!(102.01)]), SECONDS_PER_YEAR)
            .is_none());
        // 2 点曲线 (1 个收益率) → 样本方差无定义 → None。
        assert!(annual_volatility(&eq(&[dec!(100), dec!(110)]), SECONDS_PER_YEAR).is_none());
    }

    #[test]
    fn test_sharpe_known_vectors() {
        // 收益率 [1%, 3%]: rf=0 → sharpe = mean/σ × √2 = 0.02/0.014142 × 1.4142 = 2.0。
        let e = eq(&[dec!(100), dec!(101), dec!(104.03)]);
        let s0 = sharpe(&e, SECONDS_PER_YEAR, 0.0).unwrap();
        assert!((s0 - 2.0).abs() < 1e-9, "sharpe rf=0 = {s0}");
        // rf=3.8% 年化: rf_per_bar = 1.038^0.5 − 1 ≈ 0.018808 → sharpe 明显下降。
        let s38 = sharpe(&e, SECONDS_PER_YEAR, 3.8).unwrap();
        assert!(s38 < s0 && s38 > 0.0, "sharpe rf=3.8 = {s38}");
        // 期望值: (0.02 − 0.018808)/0.014142 × 1.4142 ≈ 0.1192。
        assert!((s38 - 0.1192).abs() < 0.01, "sharpe rf=3.8 = {s38}");
    }

    #[test]
    fn test_sortino_known_vector() {
        // 收益率 [1%, −2%]: rf=0, mean=−0.5%, 下行偏差 = √(0.02²/2) = 1.4142%。
        // sortino = −0.005/0.014142 × √2 = −0.5。
        let e = eq(&[dec!(100), dec!(101), dec!(98.98)]);
        let so = sortino(&e, SECONDS_PER_YEAR, 0.0).unwrap();
        assert!((so + 0.5).abs() < 1e-6, "sortino = {so}");
        // 无下行 → None。
        assert!(
            sortino(&eq(&[dec!(100), dec!(101), dec!(104.03)]), SECONDS_PER_YEAR, 0.0).is_none()
        );
    }

    #[test]
    fn test_calmar_known_vector() {
        // 年化 20% / 回撤 10% → 2.0。
        let c = calmar(Some(0.20), 0.10).unwrap();
        assert!((c - 2.0).abs() < 1e-9);
        // 回撤 0 → None; 年化 None → None。
        assert!(calmar(Some(0.20), 0.0).is_none());
        assert!(calmar(None, 0.10).is_none());
    }

    #[test]
    fn test_profit_factor_and_avg() {
        assert_eq!(profit_factor(300.0, 100.0), Some(3.0));
        assert!(profit_factor(300.0, 0.0).is_none());
        let (aw, al) = avg_win_loss(300.0, 3, 100.0, 2).unwrap();
        assert!((aw - 100.0).abs() < 1e-9);
        assert!((al - 50.0).abs() < 1e-9);
        assert!(avg_win_loss(0.0, 0, 0.0, 0).is_none());
        // 只有盈利: 平均亏损为 0。
        let (aw2, al2) = avg_win_loss(100.0, 1, 0.0, 0).unwrap();
        assert!((aw2 - 100.0).abs() < 1e-9);
        assert_eq!(al2, 0.0);
    }
}
