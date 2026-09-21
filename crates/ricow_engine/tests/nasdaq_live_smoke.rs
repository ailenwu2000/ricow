//! Nasdaq 真网络冒烟 (测试纪律: 真实调用; 默认 #[ignore] 手动跑, 网络失败不算回归)。
//!
//! 2026-09-20 (028 T011): `get_daily_klines` 改为显式 `[fromdate, todate]` 区间
//! (原实现写死起始日 2016-01-01); 日期口径 `YYYY-MM-DD`(实测该接口不接受 `MM/DD/YYYY`)。
use ricow_engine::{AssetClass, NasdaqClient};

#[tokio::test]
#[ignore = "真网络冒烟: cargo test -p ricow_engine --test nasdaq_live_smoke -- --ignored"]
async fn tsla_10y_split_adjusted() {
    let c = NasdaqClient::new();
    let klines = c
        .get_daily_klines_between("TSLA", AssetClass::Stock, "2016-01-01", "2026-09-07")
        .await
        .expect("TSLA 拉取");
    assert!(klines.len() >= 2500, "10 年日线 ≈2514, got {}", klines.len());
    let first = &klines[0];
    let last = &klines[klines.len() - 1];
    // 拆股复权: 2016-09 close ≈ $13.52 (连三次拆股连续, Nasdaq 官方数据事实)。
    println!(
        "TSLA {} 根, 最老 {} close={}, 最新 {} close={}",
        klines.len(),
        first.open_time.date_naive(),
        first.close,
        last.open_time.date_naive(),
        last.close
    );
    assert!(last.close > rust_decimal::Decimal::from(300), "最新价 >300");
}

#[tokio::test]
#[ignore = "真网络冒烟"]
async fn spy_etf_assetclass() {
    let c = NasdaqClient::new();
    let klines = c
        .get_daily_klines_between("SPY", AssetClass::Etf, "2016-01-01", "2026-09-07")
        .await
        .expect("SPY 拉取");
    assert!(klines.len() >= 2500);
    println!("SPY {} 根", klines.len());
}

/// 区间语义: 适配器只交回落在窗口内的 bar (服务端对较早 `todate` 会回 0 行,
/// 故适配器统一请求到"今天"再本地裁剪 —— 这里正是验证这条路径) (028 T011)。
#[tokio::test]
#[ignore = "真网络冒烟"]
async fn short_window_is_clipped_by_adapter() {
    use ricow_core::{Interval, KlineSource};
    use ricow_engine::data::source_nasdaq::NasdaqSource;

    let src = NasdaqSource::new();
    let from =
        chrono::DateTime::parse_from_rfc3339("2026-08-03T00:00:00Z").unwrap().timestamp_millis();
    let to =
        chrono::DateTime::parse_from_rfc3339("2026-08-15T00:00:00Z").unwrap().timestamp_millis();

    let bars = src.fetch_klines("QQQ", Interval::D1, from, to).await.expect("QQQ 拉取");
    println!("QQQ 08/03~08/14 → {} 根", bars.len());
    assert!(!bars.is_empty(), "两周窗口应有交易日");
    assert!(bars.len() <= 10, "两周交易日 ≤10, got {}", bars.len());
    let first = bars[0].open_time.date_naive().to_string();
    let last = bars[bars.len() - 1].open_time.date_naive().to_string();
    println!("首根 {first} / 末根 {last}");
    assert!(first.as_str() >= "2026-08-03", "首根 {first} 应在窗口内");
    assert!(last.as_str() <= "2026-08-14", "末根 {last} 应在窗口内");
}
