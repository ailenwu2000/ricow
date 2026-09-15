//! Nasdaq 真网络冒烟 (测试纪律: 真实调用; 默认 #[ignore] 手动跑, 网络失败不算回归)。
use ricow_engine::{NasdaqClient, AssetClass};

#[tokio::test]
#[ignore = "真网络冒烟: cargo test -p ricow_engine --test nasdaq_live_smoke -- --ignored"]
async fn tsla_10y_split_adjusted() {
    let c = NasdaqClient::new();
    let klines = c.get_daily_klines("TSLA", AssetClass::Stock, "2026-09-07").await.expect("TSLA 拉取");
    assert!(klines.len() >= 2500, "10 年日线 ≈2514, got {}", klines.len());
    let first = &klines[0];
    let last = &klines[klines.len() - 1];
    // 拆股复权: 2016-09 close ≈ $13.52 (连三次拆股连续, Nasdaq 官方数据事实)。
    println!("TSLA {} 根, 最老 {} close={}, 最新 {} close={}",
        klines.len(), first.open_time.date_naive(), first.close, last.open_time.date_naive(), last.close);
    assert!(last.close > rust_decimal::Decimal::from(300), "最新价 >300");
}

#[tokio::test]
#[ignore = "真网络冒烟"]
async fn spy_etf_assetclass() {
    let c = NasdaqClient::new();
    let klines = c.get_daily_klines("SPY", AssetClass::Etf, "2026-09-07").await.expect("SPY 拉取");
    assert!(klines.len() >= 2500);
    println!("SPY {} 根", klines.len());
}
