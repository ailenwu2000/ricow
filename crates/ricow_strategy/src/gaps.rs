//! 038 P1-D: 回测 K 线的**时间连续性校验**(数据缺口检测)。
//!
//! 背景: 此前回测拉完 K 线**不校验时间连续性** —— 交易所偶发漏 bar、分页拼接出错、
//! 本地缓存残缺, 都会把一段有洞的序列**静默**喂给策略, 指标(EMA / ATR / 网格档位)
//! 就在错误的时间轴上算出来, 却产出一份看起来正常的报告。
//!
//! 与项目纪律一致("错误必须暴露, 不许静默降级"), 这里只做**检测与报告**:
//! 判定为纯函数, 报错文案由 [`gap_error_message`] 生成, 调用方(CLI / Web 回测取数后)
//! 决定是硬报错还是别的处置。
//!
//! 判定口径:
//! - 期望步长由 `interval` 推出(调用方算好 `step_ms` 传入)。
//! - 相邻两根的 `open_time` 差值**四舍五入**到最近整数倍: `missing = round(delta/step) - 1`。
//!   用四舍五入而不是精确相等, 是为了容忍交易所 `open_time` 的 ±1ms 抖动(把 1ms 抖动
//!   报成"缺一根 bar"是误报)。
//! - 差值 ≤ 步长(重复 / 乱序)不算缺口 —— 那是另一类数据问题, 不在本函数职责内。

use ricow_core::Kline;

/// 一处时间缺口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// 缺口**前**一根 bar 的 `open_time`(Unix 毫秒)。
    pub after_ms: i64,
    /// 缺口**后**一根 bar 的 `open_time`(Unix 毫秒)。
    pub before_ms: i64,
    /// 缺失的 bar 根数(≥1)。
    pub missing: u64,
}

/// 扫描 K 线时间连续性, 返回缺口清单(按时间升序)。
///
/// **前提**: 输入按 `open_time` 升序(交易所与本地库两条取数路径都保证)。
/// 重复 / 时间回退属另一类数据问题, 本函数不负责检测 —— 但若输入被"乱序 + 又跳回未来"
/// 污染, 相邻差值可能报出一处**假缺口**; 调用方若怀疑取数乱序, 应先排序再校验。
///
/// `step_ms` ≤ 0 时无从判定 → 返回空(调用方不应把"无从判定"说成"没有缺口";
/// 本函数只负责在有步长时如实列出缺口)。
pub fn find_gaps(klines: &[Kline], step_ms: i64) -> Vec<Gap> {
    if step_ms <= 0 || klines.len() < 2 {
        return Vec::new();
    }
    let half = step_ms / 2;
    let mut out = Vec::new();
    for w in klines.windows(2) {
        let (a, b) = (w[0].open_time.timestamp_millis(), w[1].open_time.timestamp_millis());
        let delta = b - a;
        if delta <= step_ms {
            continue; // 连续 / 重复 / 乱序
        }
        // 四舍五入到最近的整数倍: delta=1.4×step → 1 根(无缺口); 1.5×step → 2 根(缺 1)。
        let steps = (delta + half) / step_ms;
        let missing = steps - 1;
        if missing >= 1 {
            out.push(Gap { after_ms: a, before_ms: b, missing: missing as u64 });
        }
    }
    out
}

/// 缺口清单 → 人类可读的硬报错文案(含**具体时刻**与缺失根数, 不只给一个总数)。
///
/// 只列前 [`MAX_LISTED_GAPS`] 处 —— 一份有几千个洞的数据把所有洞都打印出来没有意义,
/// 但必须**如实说明"还有多少处未列出"**, 不许让人以为就这几处。
pub fn gap_error_message(pair: &str, interval: &str, gaps: &[Gap]) -> String {
    let total_missing: u64 = gaps.iter().map(|g| g.missing).sum();
    let mut s = format!(
        "{pair} {interval} 的 K 线存在 {} 处时间缺口 (合计缺 {total_missing} 根) —— \
         指标会在错误的时间轴上计算, 拒绝用残缺数据回测。\
         处置: 缩短窗口 / 换更粗的 interval / 清掉本地 K 线缓存后重拉。\n缺口明细 (前 {} 处):",
        gaps.len(),
        gaps.len().min(MAX_LISTED_GAPS)
    );
    for g in gaps.iter().take(MAX_LISTED_GAPS) {
        s.push_str(&format!(
            "\n  - {} 之后缺 {} 根, 直到 {}",
            fmt_ms(g.after_ms),
            g.missing,
            fmt_ms(g.before_ms)
        ));
    }
    if gaps.len() > MAX_LISTED_GAPS {
        s.push_str(&format!("\n  - ...另有 {} 处未列出", gaps.len() - MAX_LISTED_GAPS));
    }
    s
}

/// 报错文案里最多列出多少处缺口明细。
pub const MAX_LISTED_GAPS: usize = 5;

/// Unix 毫秒 → `YYYY-MM-DD HH:MM:SS UTC`(只用于报错展示, 不参与判定)。
fn fmt_ms(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => format!("{ms}ms"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use rust_decimal_macros::dec;

    /// 造一根只关心 `open_time` 的 bar。
    fn bar(open_ms: i64) -> Kline {
        Kline {
            open_time: Utc.timestamp_millis_opt(open_ms).unwrap(),
            open: dec!(100),
            high: dec!(101),
            low: dec!(99),
            close: dec!(100),
            volume: dec!(1),
            close_time: Utc.timestamp_millis_opt(open_ms + 3_599_999).unwrap(),
        }
    }

    const H1: i64 = 3_600_000;

    #[test]
    fn test_no_gaps_when_contiguous() {
        let ks: Vec<Kline> = (0..10).map(|i| bar(i * H1)).collect();
        assert!(find_gaps(&ks, H1).is_empty(), "连续序列不得报缺口");
    }

    #[test]
    fn test_single_gap_reports_missing_count() {
        // 缺 2 根: t=0,1,2,3 之后直接跳到 6。
        let ks = vec![bar(0), bar(H1), bar(2 * H1), bar(3 * H1), bar(6 * H1)];
        let gaps = find_gaps(&ks, H1);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].after_ms, 3 * H1);
        assert_eq!(gaps[0].before_ms, 6 * H1);
        assert_eq!(gaps[0].missing, 2);
    }

    #[test]
    fn test_multiple_gaps_are_listed_in_time_order() {
        let ks = vec![bar(0), bar(5 * H1), bar(6 * H1), bar(9 * H1)];
        let gaps = find_gaps(&ks, H1);
        assert_eq!(gaps.len(), 2, "{gaps:?}");
        assert_eq!((gaps[0].missing, gaps[1].missing), (4, 2));
        assert!(gaps[0].after_ms < gaps[1].after_ms, "缺口须按时间升序");
    }

    #[test]
    fn test_ms_jitter_is_not_a_gap() {
        // 交易所 open_time 有 ±1ms 抖动: 不得报成"缺一根"。
        let ks = vec![bar(0), bar(H1 + 1), bar(2 * H1), bar(3 * H1 - 1)];
        assert!(find_gaps(&ks, H1).is_empty(), "亚步长抖动不得算缺口");
    }

    #[test]
    fn test_duplicate_and_out_of_order_are_not_gaps() {
        // 重复 bar / 时间回退是**另一类**数据问题(见函数文档: 本函数假定输入按时间升序),
        // 只管"缺"、不该在这里报。
        let dup = vec![bar(0), bar(0), bar(H1), bar(H1), bar(2 * H1)];
        assert!(find_gaps(&dup, H1).is_empty(), "重复 bar 不算缺口");
        // 整段降序: 相邻差值全为负 → 一个缺口都不报(不是本函数的职责)。
        let desc = vec![bar(2 * H1), bar(H1), bar(0)];
        assert!(find_gaps(&desc, H1).is_empty(), "降序输入不算缺口");
    }

    #[test]
    fn test_degrades_to_empty_on_bad_input() {
        assert!(find_gaps(&[], H1).is_empty());
        assert!(find_gaps(&[bar(0)], H1).is_empty());
        // 步长 ≤ 0 = 无从判定 → 空 (调用方不得把"无从判定"说成"没有缺口")。
        let ks = vec![bar(0), bar(9 * H1)];
        assert!(find_gaps(&ks, 0).is_empty());
        assert!(find_gaps(&ks, -1).is_empty());
    }

    #[test]
    fn test_message_lists_details_and_honest_tail() {
        let ks: Vec<Kline> = (0..20).map(|i| bar(i * 3 * H1)).collect(); // 步长翻三倍 = 每处缺 2 根
        let gaps = find_gaps(&ks, H1);
        assert_eq!(gaps.len(), 19);
        let msg = gap_error_message("BTCUSDT", "1h", &gaps);
        assert!(msg.contains("19 处时间缺口"), "{msg}");
        assert!(msg.contains("合计缺 38 根"), "{msg}");
        assert!(msg.contains("1970-01-01"), "须给出具体时刻: {msg}");
        // 只列前 MAX_LISTED_GAPS 处, 但必须如实说明还有多少处。
        assert!(msg.contains("另有 14 处未列出"), "{msg}");
        assert_eq!(msg.matches("\n  - ").count(), MAX_LISTED_GAPS + 1);
    }

    #[test]
    fn test_message_on_single_gap_has_no_tail() {
        let ks = vec![bar(0), bar(2 * H1)];
        let msg = gap_error_message("ETHUSDT", "1h", &find_gaps(&ks, H1));
        assert!(!msg.contains("未列出"), "只有一处缺口时不该有'另有未列出'的尾巴: {msg}");
    }
}
