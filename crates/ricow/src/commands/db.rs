//! `ricow db` — K 线库管理 (sync / stats / export)。

use clap::{Args, Subcommand};
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::Database;

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
    },
    /// 统计
    Stats,
    /// 导出 K 线 (CSV)
    Export {
        pair: String,
        #[arg(long)]
        interval: Option<String>,
    },
}

pub async fn run(args: DbArgs) -> CoreResult<()> {
    let db_path = crate::commands::default_db_path();
    let db = Database::open(&db_path).await.map_err(|e| CoreError::Exchange(e.to_string()))?;

    match args.cmd {
        DbCmd::Sync { pair, interval } => {
            let interval = interval.unwrap_or_else(|| "1h".into());
            let exchange = crate::commands::bn_exchange()?;
            let klines = exchange.get_klines(&pair, &interval, 2000).await?;
            let mut n = 0u32;
            for k in &klines {
                db.insert_kline(&pair, &interval, k)
                    .await
                    .map_err(|e| CoreError::Exchange(e.to_string()))?;
                n += 1;
            }
            println!("已同步 {pair} {interval} K 线 {n} 根 → {}", db_path.display());
        }
        DbCmd::Stats => {
            let k = db.kline_count().await.map_err(|e| CoreError::Exchange(e.to_string()))?;
            let f = db.fill_count().await.map_err(|e| CoreError::Exchange(e.to_string()))?;
            println!("数据库: {}", db_path.display());
            println!("  K 线总数: {k}");
            println!("  成交总数: {f}");
        }
        DbCmd::Export { pair, interval } => {
            let interval = interval.unwrap_or_else(|| "1h".into());
            let klines = db
                .get_klines(&pair, &interval, 100_000)
                .await
                .map_err(|e| CoreError::Exchange(e.to_string()))?;
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
