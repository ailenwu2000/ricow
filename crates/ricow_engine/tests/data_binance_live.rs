//! 币安区间取数真网络冒烟 (028 T010)。
//!
//! 纪律 (specs/constitution.md §三): 真实调用, 不用替身; 本文件默认 `#[ignore]`,
//! 需显式 `-- --ignored` 才跑(网络失败不算回归)。
//!
//! 运行:
//! ```bash
//! RICOW_BN_BASE_URL=https://data-api.binance.vision \
//!   cargo test -p ricow_engine --test data_binance_live -- --ignored --nocapture
//! ```
//! `api.binance.com` 在本机 DNS 被污染时用官方公开数据域 `data-api.binance.vision`。

use std::sync::Arc;

use ricow_binance::{BinanceClient, BnSpotExchange};
use ricow_core::{Interval, KlineSource};
use ricow_engine::data::source_binance::BinanceSource;

fn spot_source() -> BinanceSource {
    let client = BinanceClient::new().expect("币安客户端");
    BinanceSource::spot(Arc::new(BnSpotExchange::new(client)))
}

/// 单页内: 半开区间 [from, to) 精确覆盖、升序、步长正确。
#[tokio::test]
#[ignore = "真网络冒烟: 需 -- --ignored"]
async fn eth_1h_five_bar_range_is_exact() {
    let src = spot_source();
    // 固定过去窗口: 2026-09-01 00:00 UTC 起 5 小时 (历史数据, 结果稳定可复现)。
    let from =
        chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z").unwrap().timestamp_millis();
    let to = from + 5 * 3_600_000;

    let bars = src.fetch_klines("ETHUSDT", Interval::H1, from, to).await.expect("取数失败");

    let times: Vec<i64> = bars.iter().map(|b| b.open_time.timestamp_millis()).collect();
    println!("{from}..{to} → {} 根: {times:?}", bars.len());
    assert_eq!(bars.len(), 5, "5 小时窗口正好 5 根 1h");
    assert_eq!(times[0], from, "首根 = from (含)");
    assert_eq!(times[4], to - 3_600_000, "末根 = to − 1h (to 不含)");
    for w in times.windows(2) {
        assert_eq!(w[1] - w[0], 3_600_000, "步长 1h, 无重复无缺口");
    }
    for b in &bars {
        assert!(b.close > rust_decimal::Decimal::ZERO, "收盘价为正");
        assert_eq!(
            b.close_time.timestamp_millis() - b.open_time.timestamp_millis(),
            3_599_999,
            "close_time = open + 1h − 1ms"
        );
    }
}

/// 跨页: 1500 根 1m 需要两次分页(单页上限 1000), 拼接后无重复、无缺口。
#[tokio::test]
#[ignore = "真网络冒烟: 需 -- --ignored"]
async fn eth_1m_crosses_pages_without_gaps_or_dupes() {
    let src = spot_source();
    let from =
        chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z").unwrap().timestamp_millis();
    let to = from + 1500 * 60_000;

    let bars = src.fetch_klines("ETHUSDT", Interval::M1, from, to).await.expect("取数失败");
    println!("{} 根 1m (期望 1500)", bars.len());

    assert_eq!(bars.len(), 1500, "1500 分钟窗口正好 1500 根(跨 2 页)");
    let times: Vec<i64> = bars.iter().map(|b| b.open_time.timestamp_millis()).collect();
    assert_eq!(times[0], from);
    assert_eq!(times[times.len() - 1], to - 60_000);
    for w in times.windows(2) {
        assert_eq!(w[1] - w[0], 60_000, "1m 步长连续, 页间无缝");
    }
}

/// 区间为空/反向 → 空结果, 且不发请求(不算错误)。
#[tokio::test]
#[ignore = "真网络冒烟: 需 -- --ignored"]
async fn empty_or_reversed_window_returns_empty() {
    let src = spot_source();
    let from =
        chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z").unwrap().timestamp_millis();
    assert!(src.fetch_klines("ETHUSDT", Interval::H1, from, from).await.unwrap().is_empty());
    assert!(src
        .fetch_klines("ETHUSDT", Interval::H1, from, from - 3_600_000)
        .await
        .unwrap()
        .is_empty());
}
