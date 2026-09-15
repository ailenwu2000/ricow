//! `ricow ticker` / `orderbook` — 实时行情。

use clap::Args;
use ricow_core::CoreResult;

#[derive(Args)]
pub struct TickerArgs {
    pub pair: String,
}

#[derive(Args)]
pub struct OrderbookArgs {
    pub pair: String,
}

pub async fn ticker(args: TickerArgs) -> CoreResult<()> {
    let exchange = crate::commands::bn_exchange()?;
    let ob = exchange.get_orderbook(&args.pair, 1).await?;
    match ob.mid_price() {
        Some(mid) => println!("{} 中间价: {}", args.pair, mid),
        None => println!("{} 无盘口数据", args.pair),
    }
    Ok(())
}

pub async fn orderbook(args: OrderbookArgs) -> CoreResult<()> {
    let exchange = crate::commands::bn_exchange()?;
    let ob = exchange.get_orderbook(&args.pair, 5).await?;
    println!("{} 盘口:", args.pair);
    for b in ob.bids.iter().take(5) {
        println!("  bid {} x {}", b.price, b.size);
    }
    for a in ob.asks.iter().take(5) {
        println!("  ask {} x {}", a.price, a.size);
    }
    Ok(())
}
