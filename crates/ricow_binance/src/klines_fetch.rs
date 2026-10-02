//! 050 回测提速: K 线并发分页拉数。
//!
//! 根因 (2026-10-02 实测): 年级 1m 回测 527s 里 96.8% 耗在串行翻页 —— 币安 klines
//! 单请求上限 1000 根, 52.7 万根 = 528 次请求 × ~0.95s 跨境 RTT。接口支持显式
//! startTime/endTime 单页查询 → 把窗口切成固定时间片**并发**拉取, 收齐后按 open_time
//! 排序拼接, 结果与串行逐位一致 (确定性可复现, 验收锚点)。
//!
//! 纪律: 单片失败重试 2 次 (退避), 仍失败 → 整次报错。缺 bar 会悄悄改变回测结果,
//! 绝不静默降级 (同"预热不足硬报错"口径)。

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ricow_core::{CoreError, CoreResult, Kline};

/// 每页最大根数 (币安现货/合约 klines 单次请求上限)。
pub const KLINE_PAGE_BARS: u32 = 1000;

/// 单片失败重试退避 (毫秒): 首次失败后等 300ms 重试, 再失败等 1s 重试, 第三次仍失败 → 报错。
const RETRY_BACKOFF_MS: [u64; 2] = [300, 1000];

/// 把 `[start_ms, end_ms)` 按 `page_bars × interval_ms` 切成并发时间片 (纯函数, 便于单测)。
///
/// - 片起点升序、互不重叠、覆盖全窗口 (末片右端钉到 `end_ms`)。
/// - `end_ms <= start_ms` 或参数非法 → 返回空 (调用方按"无数据"处理)。
pub fn plan_windows(
    start_ms: i64,
    end_ms: i64,
    interval_ms: i64,
    page_bars: u32,
) -> Vec<(i64, i64)> {
    if interval_ms <= 0 || page_bars == 0 || end_ms <= start_ms {
        return Vec::new();
    }
    let span = interval_ms * page_bars as i64;
    let total = end_ms - start_ms;
    let pages = ((total + span - 1) / span) as usize;
    (0..pages)
        .map(|i| {
            let s = start_ms + i as i64 * span;
            let e = (s + span).min(end_ms);
            (s, e)
        })
        .collect()
}

/// 合并并发拉回的各页: open_time 升序 + 去重 + 截断到 `cap`。
///
/// 与旧串行游标语义逐位一致: 旧实现从窗口起点向前翻页、取满 `cap` 根即停 ——
/// 等价于"全窗口 bar 排序后取前 cap 根"。`endTime` 闭区间可能让相邻片边界重复一根, 去重兜底。
pub fn merge_pages(pages: Vec<Vec<Kline>>, cap: usize) -> Vec<Kline> {
    let mut all: Vec<Kline> = pages.into_iter().flatten().collect();
    all.sort_by_key(|k| k.open_time);
    all.dedup_by(|a, b| a.open_time == b.open_time);
    all.truncate(cap);
    all
}

/// 并发拉取全部时间片 (`buffer_unordered`), 返回与 `windows` 同序的页向量。
///
/// - `fetch_one(page_idx, start_ms, end_ms)`: 单片取数 (调用方注入, 现货/合约各自实现);
/// - 单片失败按 [`RETRY_BACKOFF_MS`] 重试, 最终失败 → 整次 `Err` (不静默缺数据);
/// - 每完成 50 片打一条进度日志 (长窗口不再"长时间无输出")。
pub async fn fetch_klines_concurrent<F, Fut>(
    windows: &[(i64, i64)],
    concurrency: usize,
    fetch_one: F,
) -> CoreResult<Vec<Vec<Kline>>>
where
    F: Fn(usize, i64, i64) -> Fut + Send + Sync,
    Fut: Future<Output = CoreResult<Vec<Kline>>> + Send,
{
    use futures::stream::{self, StreamExt};

    let total = windows.len();
    let done = AtomicUsize::new(0usize);
    let items: Vec<(usize, i64, i64)> =
        windows.iter().enumerate().map(|(i, (s, e))| (i, *s, *e)).collect();
    let collected: Vec<(usize, CoreResult<Vec<Kline>>)> = stream::iter(items)
        .map(|(i, s, e)| {
            let fetch_one = &fetch_one;
            let done = &done;
            async move {
                let mut attempt = 0usize;
                loop {
                    match fetch_one(i, s, e).await {
                        Ok(v) => {
                            let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                            if n % 50 == 0 || n == total {
                                tracing::info!(
                                    target: "kfetch",
                                    done = n,
                                    total,
                                    "K 线并发拉数进度"
                                );
                            }
                            return (i, Ok(v));
                        }
                        Err(err) if attempt < RETRY_BACKOFF_MS.len() => {
                            tracing::warn!(
                                target: "kfetch",
                                page = i,
                                attempt = attempt + 1,
                                error = %err,
                                "K 线单片拉取失败, 重试"
                            );
                            tokio::time::sleep(Duration::from_millis(RETRY_BACKOFF_MS[attempt]))
                                .await;
                            attempt += 1;
                        }
                        Err(err) => return (i, Err(err)),
                    }
                }
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    let mut pages: Vec<Vec<Kline>> = vec![Vec::new(); total];
    for (i, r) in collected {
        pages[i] = r.map_err(|e| CoreError::Network(format!("K 线第 {i} 片拉取失败: {e}")))?;
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use std::sync::atomic::AtomicUsize;

    fn bar_ms(open_ms: i64) -> Kline {
        let t = |ms: i64| DateTime::<Utc>::from_timestamp_millis(ms).unwrap();
        Kline {
            open_time: t(open_ms),
            close_time: t(open_ms + 59_999),
            open: Decimal::ONE,
            high: Decimal::ONE,
            low: Decimal::ONE,
            close: Decimal::ONE,
            volume: Decimal::ONE,
        }
    }

    #[test]
    fn plan_windows_covers_whole_window_disjoint_and_ordered() {
        // 一年 1m: 527040 根 → 528 片, 末片钉到窗口终点。
        let step = 60_000_i64;
        let n = 527_040_i64;
        let start = 1_760_000_000_000;
        let end = start + n * step;
        let w = plan_windows(start, end, step, KLINE_PAGE_BARS);
        assert_eq!(w.len(), 528);
        assert_eq!(w[0], (start, start + 1000 * step));
        assert_eq!(*w.last().unwrap(), (end - 40 * step, end)); // 527040 = 527×1000 + 40
        for pair in w.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "片必须首尾相接不重叠");
        }
    }

    #[test]
    fn plan_windows_edge_cases() {
        assert!(plan_windows(100, 100, 60_000, 1000).is_empty());
        assert!(plan_windows(200, 100, 60_000, 1000).is_empty());
        assert!(plan_windows(100, 200, 0, 1000).is_empty());
        // 不足一页 → 单片钉到 end。
        assert_eq!(plan_windows(0, 5, 1, 1000), vec![(0, 5)]);
    }

    #[test]
    fn merge_pages_sorts_dedups_and_caps() {
        let pages = vec![
            vec![bar_ms(200), bar_ms(300)],
            vec![bar_ms(0), bar_ms(100)],
            vec![bar_ms(300), bar_ms(400)], // 边界重复一根 (endTime 闭区间)
        ];
        let all = merge_pages(pages, usize::MAX);
        let ts: Vec<i64> = all.iter().map(|k| k.open_time.timestamp_millis()).collect();
        assert_eq!(ts, vec![0, 100, 200, 300, 400]);
        // cap = 旧串行"取满即停": 排序后取前 N。
        let capped = merge_pages(
            vec![vec![bar_ms(200)], vec![bar_ms(0)], vec![bar_ms(100)]],
            2,
        );
        assert_eq!(capped.len(), 2);
        assert_eq!(capped[0].open_time.timestamp_millis(), 0);
        assert_eq!(capped[1].open_time.timestamp_millis(), 100);
    }

    #[tokio::test]
    async fn fetch_concurrent_retries_then_succeeds() {
        let windows = vec![(0i64, 100i64), (100, 200), (200, 300)];
        let tries = AtomicUsize::new(0);
        let pages = fetch_klines_concurrent(&windows, 8, |i, s, _e| {
            let tries = &tries;
            async move {
                let n = tries.fetch_add(1, Ordering::SeqCst);
                // 第 1 片首次失败一次, 其余直接成功。
                if i == 1 && n == 0 {
                    return Err(CoreError::Network("模拟抖动".into()));
                }
                Ok(vec![bar_ms(s)])
            }
        })
        .await
        .unwrap();
        assert_eq!(pages.len(), 3);
        let merged = merge_pages(pages, usize::MAX);
        let ts: Vec<i64> = merged.iter().map(|k| k.open_time.timestamp_millis()).collect();
        assert_eq!(ts, vec![0, 100, 200]);
    }

    #[tokio::test]
    async fn fetch_concurrent_hard_failure_reports_error() {
        let windows = vec![(0i64, 100i64), (100, 200)];
        let err = fetch_klines_concurrent(&windows, 4, |i, s, _e| async move {
            if i == 1 {
                Err(CoreError::Network("永久失败".into()))
            } else {
                Ok(vec![bar_ms(s)])
            }
        })
        .await
        .unwrap_err();
        assert!(format!("{err}").contains("第 1 片"), "错误必须点名失败片: {err}");
    }
}
