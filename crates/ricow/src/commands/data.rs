//! `ricow data pull` — 把历史 K 线从数据源拉进本地库 (028 T013)。
//!
//! 为什么需要它: 回测**只读本地库**(拍板 D3), 保证同一份数据断网重跑结果逐位一致;
//! 需要更长历史 / 第三方来源(美股日线)时, 先显式拉一次, 再跑回测。
//!
//! 增量语义: 已缓存的区间不重复拉(缺口才回源, 见 `DataHub::ensure_cached`)。

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use clap::{Args, Subcommand};
use ricow_core::{CoreError, CoreResult, Exchange, Interval, SeriesKey};
use ricow_engine::DataHub;
use ricow_strategy::Database;

/// 本命令的源级节流间隔(覆盖 Yahoo 的 ≈2 req/s 上限; 对其他源只是略微放慢请求)。
const THROTTLE: Duration = Duration::from_millis(500);

/// 未给 `--start` 时的默认回看天数。
const DEFAULT_DAYS: i64 = 365;

#[derive(Args)]
pub struct DataArgs {
    #[command(subcommand)]
    pub cmd: DataCmd,
}

#[derive(Subcommand)]
pub enum DataCmd {
    /// 拉取区间 K 线入库 (增量补齐; 已缓存区间不重复拉)
    Pull {
        /// 数据源注册名 (binance_spot / binance_futures / nasdaq / yahoo)
        #[arg(long)]
        source: String,
        /// 源原生写法 (如 ETHUSDT / QQQ / SPY)
        #[arg(long)]
        symbol: String,
        /// 周期标签 (1m/5m/15m/30m/1h/2h/4h/6h/8h/12h/1d/3d/1w)
        #[arg(long)]
        interval: String,
        /// 起始日期 YYYY-MM-DD (UTC, 含); 与 --days 二选一
        #[arg(long)]
        start: Option<String>,
        /// 结束日期 YYYY-MM-DD (UTC, **不含**); 缺省 = 现在
        #[arg(long)]
        end: Option<String>,
        /// 相对现在的回看天数 (缺省 365; 与 --start 同给时以 --start 为准)
        #[arg(long)]
        days: Option<i64>,
    },
}

pub async fn run(args: DataArgs) -> CoreResult<()> {
    match args.cmd {
        DataCmd::Pull { source, symbol, interval, start, end, days } => {
            pull(source, symbol, interval, start, end, days).await
        }
    }
}

async fn pull(
    source: String,
    symbol: String,
    interval: String,
    start: Option<String>,
    end: Option<String>,
    days: Option<i64>,
) -> CoreResult<()> {
    let iv = Interval::from_label(&interval).ok_or_else(|| {
        CoreError::InvalidArgument(format!(
            "不支持的周期 '{interval}'; 支持: {}",
            all_interval_labels()
        ))
    })?;
    let key = SeriesKey::new(&source, &symbol, iv)?;
    let now_ms = Utc::now().timestamp_millis();
    let (from_ms, to_ms) = window_of(start.as_deref(), end.as_deref(), days, now_ms)?;

    let db_path = crate::commands::default_db_path();
    let db = Database::open(&db_path).await.map_err(|e| CoreError::Exchange(e.to_string()))?;
    let hub = DataHub::new(build_registry()?, db).with_min_interval(THROTTLE);

    println!(
        "数据源 {} | {} {} | 窗口 {} → {} (UTC, 含起不含止)",
        key.source,
        key.symbol,
        iv.label(),
        ymd(from_ms),
        ymd(to_ms)
    );
    let fetched = hub.ensure_cached(&key, from_ms, to_ms).await?;
    let bars = hub.load_cached(&key, from_ms, to_ms).await?;

    println!("本次新增 {fetched} 根; 本地该窗口共 {} 根", bars.len());
    if let (Some(f), Some(l)) = (bars.first(), bars.last()) {
        println!(
            "首 {} / 末 {} (close {})",
            ymd(f.kline.open_time.timestamp_millis()),
            ymd(l.kline.open_time.timestamp_millis()),
            l.kline.close
        );
    }
    println!("本地库: {}", db_path.display());

    if bars.is_empty() {
        return Err(CoreError::InvalidArgument(
            "该窗口取不到数据: 检查标的/周期/日期; 第三方源有窗口限制(如 Yahoo 1m 仅约 7 天), 也可能是该源在本机不可达"
                .to_string(),
        ));
    }
    Ok(())
}

/// 默认装配: 公开现货 + 公开合约(都不需要密钥) + Nasdaq + Yahoo。
pub(crate) fn build_registry() -> CoreResult<ricow_core::SourceRegistry> {
    let spot = crate::commands::bn_exchange()?;
    let futures_client = ricow_binance::FuturesClient::new()?;
    let futures: Arc<dyn Exchange> =
        Arc::new(ricow_binance::BnFuturesExchange::new(futures_client));
    Ok(ricow_engine::data::default_registry(spot, Some(futures)))
}

/// 计算半开窗口 `[from_ms, to_ms)`。
fn window_of(
    start: Option<&str>,
    end: Option<&str>,
    days: Option<i64>,
    now_ms: i64,
) -> CoreResult<(i64, i64)> {
    let to_ms = match end {
        Some(s) => parse_ymd(s)?,
        None => now_ms,
    };
    let from_ms = match start {
        Some(s) => parse_ymd(s)?,
        None => to_ms - days.unwrap_or(DEFAULT_DAYS).max(1) * 86_400_000,
    };
    if from_ms >= to_ms {
        return Err(CoreError::InvalidArgument(format!(
            "起始 {} 不早于结束 {} —— 窗口为空",
            ymd(from_ms),
            ymd(to_ms)
        )));
    }
    Ok((from_ms, to_ms))
}

/// `YYYY-MM-DD`(UTC 00:00) → 毫秒。
fn parse_ymd(s: &str) -> CoreResult<i64> {
    let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| CoreError::InvalidArgument(format!("日期需 YYYY-MM-DD: {s} ({e})")))?;
    Ok(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("00:00 合法")).timestamp_millis())
}

fn ymd(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".into())
}

/// 全部受支持周期标签(报错文案用, 与 `Interval` 同源)。
fn all_interval_labels() -> String {
    Interval::ALL.iter().map(|i| i.label()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ymd_ok_and_bad() {
        assert_eq!(parse_ymd("1970-01-02").unwrap(), 86_400_000);
        assert!(parse_ymd("2026/09/01").is_err(), "只认 YYYY-MM-DD");
        assert!(parse_ymd("").is_err());
    }

    #[test]
    fn test_window_default_is_365_days_back() {
        let now = 1_800_000_000_000i64;
        let (from, to) = window_of(None, None, None, now).unwrap();
        assert_eq!(to, now);
        assert_eq!(to - from, 365 * 86_400_000);
    }

    #[test]
    fn test_window_start_end_are_utc_half_open() {
        let (from, to) = window_of(Some("2026-09-01"), Some("2026-09-03"), None, 0).unwrap();
        assert_eq!(ymd(from), "2026-09-01");
        assert_eq!(ymd(to), "2026-09-03");
        assert_eq!(to - from, 2 * 86_400_000);
    }

    #[test]
    fn test_window_start_wins_over_days() {
        let now = parse_ymd("2026-09-20").unwrap();
        let (from, _) = window_of(Some("2026-01-01"), None, Some(7), now).unwrap();
        assert_eq!(ymd(from), "2026-01-01", "--start 优先于 --days");
    }

    #[test]
    fn test_window_empty_is_rejected() {
        let err = window_of(Some("2026-09-03"), Some("2026-09-01"), None, 0).unwrap_err();
        assert!(err.to_string().contains("窗口为空"), "{err}");
    }

    #[test]
    fn test_all_interval_labels_matches_interval_table() {
        let s = all_interval_labels();
        assert!(s.starts_with("1m/"), "{s}");
        assert_eq!(s.split('/').count(), 14, "14 个周期: {s}");
    }
}
