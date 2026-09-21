//! Yahoo v8 chart 真网络冒烟 (028 T012; 默认 `#[ignore]`)。
//!
//! 运行:
//! ```bash
//! cargo test -p ricow_engine --test yahoo_live_smoke -- --ignored --nocapture
//! ```
//!
//! 🔴 环境限制 (2026-09-20 本机实测, 如实记录): 本机网络访问 Yahoo 的
//! `query1/query2.finance.yahoo.com/{v8/finance/chart, v7/finance/download}` **一律 HTTP 403**
//! (返回 Yahoo 中文拦截页), 换浏览器 UA、加 `Accept`/`Referer` 均无效, 浏览器守护不可用。
//! 因此本文件的用例在**本机必然失败** —— 它保留下来是为了在能访问 Yahoo 的环境里一键验证,
//! 不作为本机通过的证据。解析逻辑由 `src/data/source_yahoo.rs` 的单元测试覆盖。

use ricow_core::{Interval, KlineSource};
use ricow_engine::data::source_yahoo::YahooSource;

fn days_ago(days: i64) -> i64 {
    (chrono::Utc::now() - chrono::Duration::days(days)).timestamp_millis()
}

/// QQQ 日线 10 年: 根数与首末时间(记录, 不断言精确值 —— 服务端窗口随上市/交易日变化)。
#[tokio::test]
#[ignore = "真网络冒烟(本机被 403 拦截, 见文件头): 需可访问 Yahoo 的网络"]
async fn qqq_daily_ten_years() {
    let src = YahooSource::new();
    let to = chrono::Utc::now().timestamp_millis();
    let bars = src.fetch_bars("QQQ", Interval::D1, days_ago(3650), to).await.expect("QQQ 日线");
    assert!(!bars.is_empty(), "应有数据");
    println!(
        "QQQ 1d 10 年 → {} 根, 首 {} / 末 {}, 复权列={}",
        bars.len(),
        bars[0].kline.open_time.date_naive(),
        bars[bars.len() - 1].kline.open_time.date_naive(),
        if bars[0].adj_close.is_some() { "有" } else { "无" }
    );
    assert!(bars.len() >= 2000, "10 年日线应 ≥2000 根, got {}", bars.len());
    assert!(bars.iter().all(|b| b.adj_close.is_some()), "Yahoo 应提供 adjclose");
    // 升序 + 无重复
    for w in bars.windows(2) {
        assert!(w[0].kline.open_time < w[1].kline.open_time, "升序");
    }
}

/// QQQ 近期 1h(服务端 1h 只保约 730 天, 这里取最近 30 天)。
#[tokio::test]
#[ignore = "真网络冒烟(本机被 403 拦截, 见文件头): 需可访问 Yahoo 的网络"]
async fn qqq_hourly_recent() {
    let src = YahooSource::new();
    let to = chrono::Utc::now().timestamp_millis();
    let bars = src.fetch_bars("QQQ", Interval::H1, days_ago(30), to).await.expect("QQQ 1h");
    assert!(!bars.is_empty(), "应有数据");
    println!(
        "QQQ 1h 30 天 → {} 根, 首 {} / 末 {}",
        bars.len(),
        bars[0].kline.open_time,
        bars[bars.len() - 1].kline.open_time
    );
    assert!(bars.len() >= 100, "30 天 1h 应 ≥100 根, got {}", bars.len());
}
