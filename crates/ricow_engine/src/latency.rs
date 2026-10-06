//! 038 P1-F: 下单**往返延迟**度量(纯逻辑)。
//!
//! 背景: 此前不采集 `place_order` 的 REST 往返耗时 —— 滑点模型缺真实依据(回测里的
//! `slippage_bps` 是拍的值), 交易所侧卡顿也**无从发现**(只能靠"感觉变慢了")。
//!
//! 设计要点:
//! - 度量点在**引擎调用 `ctx.place_order` 前后**: 覆盖 REST 往返 + 本地对齐/护栏开销。
//!   不含行情推送延迟与策略计算耗时 —— 报告文案里必须写清这一点, 不夸大覆盖面。
//! - 失败(传输错误)的调用**同样计入**: 超时/连不上也是真实的等待成本。
//! - 分位数是**纯函数**(`&[u64]` 微秒), 不碰 `Instant` —— 既满足 CI 红线 5
//!   (生产代码禁对 `Instant` 做裸减法), 也让测试零时钟依赖。
//! - 无样本 → `None` —— 不给 "P50 = 0ms" 这种看起来正常的假数字。

/// 一次运行的下单往返延迟统计(微秒)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyStats {
    /// 样本数(= 本次运行实际发起的下单/撤单类交易所调用次数)。
    pub count: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

impl LatencyStats {
    /// 收尾报告行(毫秒展示; 微秒精度不够读)。
    ///
    /// 文案里明写"不含行情与策略计算" —— 否则读者会把它当成端到端延迟。
    pub fn report_line(&self) -> String {
        format!(
            "下单往返延迟(不含行情推送与策略计算): 样本 {} 次, P50 {:.2}ms, P95 {:.2}ms, P99 {:.2}ms, 最大 {:.2}ms",
            self.count,
            self.p50_us as f64 / 1000.0,
            self.p95_us as f64 / 1000.0,
            self.p99_us as f64 / 1000.0,
            self.max_us as f64 / 1000.0
        )
    }
}

/// 从样本(微秒)算统计; **空样本 → `None`**(不给假数字)。
///
/// 就地排序调用方给的切片 —— 调用方传的是"本次运行用完即弃"的累积缓冲。
pub fn summarize(samples: &mut [u64]) -> Option<LatencyStats> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    Some(LatencyStats {
        count: samples.len() as u64,
        p50_us: percentile(samples, 50, 100),
        p95_us: percentile(samples, 95, 100),
        p99_us: percentile(samples, 99, 100),
        max_us: *samples.last().expect("非空已判"),
    })
}

/// nearest-rank 分位数: `ceil(n × num/den)` 名次对应下标(0 基)。
///
/// 取 nearest-rank 而不是插值: 样本量通常只有几十~几百(下单次数), 插值出来的小数
/// 会让读者误以为精度很高。偶数样本下 P50 取偏低的那一个 —— 保守, 且文档写明。
fn percentile(sorted: &[u64], num: usize, den: usize) -> u64 {
    debug_assert!(den > 0 && num <= den && !sorted.is_empty());
    let rank = (sorted.len() * num).div_ceil(den).max(1);
    sorted[(rank - 1).min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_yields_none() {
        assert_eq!(summarize(&mut []), None, "无样本必须给 None, 不给 P50=0 的假数字");
    }

    #[test]
    fn test_single_sample() {
        let s = summarize(&mut [1234]).expect("有样本");
        assert_eq!(s.count, 1);
        assert_eq!((s.p50_us, s.p95_us, s.p99_us, s.max_us), (1234, 1234, 1234, 1234));
    }

    #[test]
    fn test_all_equal() {
        let s = summarize(&mut [7; 50]).expect("有样本");
        assert_eq!(s.count, 50);
        assert_eq!((s.p50_us, s.p95_us, s.p99_us, s.max_us), (7, 7, 7, 7));
    }

    #[test]
    fn test_percentiles_nearest_rank() {
        // 1..=100 → P50 = 50, P95 = 95, P99 = 99, max = 100。
        let mut v: Vec<u64> = (1..=100).collect();
        let s = summarize(&mut v).expect("有样本");
        assert_eq!(s.p50_us, 50);
        assert_eq!(s.p95_us, 95);
        assert_eq!(s.p99_us, 99);
        assert_eq!(s.max_us, 100);
    }

    #[test]
    fn test_unsorted_input_is_sorted_in_place() {
        let mut v = vec![300, 100, 200];
        let s = summarize(&mut v).expect("有样本");
        assert_eq!(v, vec![100, 200, 300], "应就地排序");
        assert_eq!((s.p50_us, s.max_us), (200, 300));
    }

    #[test]
    fn test_two_samples_p50_takes_lower() {
        // 偶数样本 P50 取偏低者(保守), 文档已写明 —— 锁定该约定, 防止有人"顺手改成插值"。
        let s = summarize(&mut [10, 20]).expect("有样本");
        assert_eq!(s.p50_us, 10);
        assert_eq!(s.max_us, 20);
    }

    #[test]
    fn test_report_line_states_scope_and_units() {
        let s = summarize(&mut [1_500, 4_000, 9_000]).expect("有样本");
        let line = s.report_line();
        assert!(line.contains("样本 3 次"), "{line}");
        assert!(line.contains("P50 4.00ms"), "微秒须换算成毫秒展示: {line}");
        assert!(line.contains("最大 9.00ms"), "{line}");
        // 覆盖面必须写清, 否则会被当成端到端延迟。
        assert!(line.contains("不含行情推送与策略计算"), "{line}");
    }
}
