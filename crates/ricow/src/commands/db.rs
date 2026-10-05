//! `ricow db` — K 线库管理 (sync / stats / export)。

use clap::{Args, Subcommand};
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{Database, SqlxResultExt};

#[derive(Args)]
pub struct DbArgs {
    #[command(subcommand)]
    pub cmd: DbCmd,
}

#[derive(Subcommand)]
pub enum DbCmd {
    /// 从交易所拉取历史 K 线到本地库
    Sync {
        pair: String,
        #[arg(long)]
        interval: Option<String>,
        /// 市场 spot|futures (默认 spot)
        #[arg(long)]
        market: Option<String>,
    },
    /// 统计
    Stats,
    /// 导出 K 线 (CSV)
    Export {
        pair: String,
        #[arg(long)]
        interval: Option<String>,
        /// 市场 spot|futures (默认 spot)
        #[arg(long)]
        market: Option<String>,
    },
}

/// 按市场取 K 线: 现货走 `bn_exchange`, 合约走 fapi 公共数据源 (035)。
async fn fetch_klines(
    market: &str,
    pair: &str,
    interval: &str,
    limit: u32,
) -> CoreResult<Vec<ricow_core::Kline>> {
    if market == "futures" {
        ricow_binance::FuturesDataClient::new()?.get_klines(pair, interval, limit).await
    } else {
        crate::commands::bn_exchange()?.get_klines(pair, interval, limit).await
    }
}

/// 校验并规范化 `--market`。
fn parse_market(market: Option<String>) -> CoreResult<String> {
    let m = market.unwrap_or_else(|| "spot".into());
    if m != "spot" && m != "futures" {
        return Err(CoreError::InvalidArgument(format!(
            "--market 仅支持 spot|futures, 收到 '{m}'"
        )));
    }
    Ok(m)
}

pub async fn run(args: DbArgs) -> CoreResult<()> {
    let db_path = crate::commands::default_db_path();
    let db = Database::open(&db_path).await.core()?;

    match args.cmd {
        DbCmd::Sync { pair, interval, market } => {
            let interval = interval.unwrap_or_else(|| "1h".into());
            let market = parse_market(market)?;
            // #010: 与 ticker/backtest 同一份视野校验(默认仅股票类), 不放行越界交易对。
            crate::commands::pairs::ensure_pair_in_scope(
                &crate::commands::project_root(),
                &pair,
                &market,
            )
            .await?;
            let klines = fetch_klines(&market, &pair, &interval, 2000).await?;
            let inserted = db.insert_klines(&market, &pair, &interval, &klines).await.core()?;
            println!(
                "已同步 {market} {pair} {interval} K 线 {} 根 (新增 {inserted}) → {}",
                klines.len(),
                db_path.display()
            );
            // #022: 只报根数用户无从判断"是不是全部" —— 如实说明取数口径与更早历史的拉法
            // (回测按窗口自动向前翻页取数并回填本地缓存, 见 backtest.rs 的分页取数)。
            println!(
                "口径: 单次 sync 只拉**最近 {} 根**(单次上限), 不是全部可得历史; 本地库根数直接决定可回测的窗口大小。",
                klines.len()
            );
            println!(
                "增量: 重复执行按时间戳去重、只补新增; 需要更早的历史时, 用 `ricow backtest --start <日期>` 按窗口自动向前翻页取数(会回填本地缓存, 下次回测即命中)。"
            );
        }
        DbCmd::Stats => {
            let k = db.kline_count().await.core()?;
            let f = db.fill_count().await.core()?;
            println!("数据库: {}", db_path.display());
            println!("  K 线总数: {k}");
            println!("  成交总数: {f}");
        }
        DbCmd::Export { pair, interval, market } => {
            let interval = interval.unwrap_or_else(|| "1h".into());
            let market = parse_market(market)?;
            let klines = db.get_klines(&market, &pair, &interval, 100_000).await.core()?;
            println!("open_time,open,high,low,close,volume");
            for k in klines {
                println!(
                    "{},{},{},{},{},{}",
                    k.open_time.timestamp_millis(),
                    k.open,
                    k.high,
                    k.low,
                    k.close,
                    k.volume
                );
            }
        }
    }
    Ok(())
}
