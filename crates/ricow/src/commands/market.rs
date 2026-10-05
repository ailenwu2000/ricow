//! `ricow ticker` / `orderbook` — 实时行情 (仅现货; 合约行情走回测数据源/视野 `ricow pairs`)。

use clap::Args;
use ricow_core::{CoreError, CoreResult};

#[derive(Args)]
pub struct TickerArgs {
    pub pair: String,
}

#[derive(Args)]
pub struct OrderbookArgs {
    pub pair: String,
}

/// 现货入口的交易对预检 (#010 范围校验 + #011 合约命名提示)。
///
/// #011: `ricow ticker TSLAUSDT` 这类**合约命名**打到现货入口会直接冒泡交易所原始
/// 400 报文 —— 在通用视野报错之上追加"现货写法"提示, 让用户一步改对。
async fn ensure_spot_pair(pair: &str) -> CoreResult<()> {
    let root = crate::commands::project_root();
    if let Err(e) = crate::commands::pairs::ensure_pair_in_scope(&root, pair, "spot").await {
        if let Ok(view) = crate::commands::pairs::current_view(&root, false).await {
            if view.futures.iter().any(|s| s.eq_ignore_ascii_case(pair)) {
                let base = pair.trim_end_matches("USDT").trim_end_matches("USDC");
                return Err(CoreError::InvalidArgument(format!(
                    "{e}\n提示: {pair} 是**合约(美股永续)**符号; ticker/orderbook 只支持现货交易对, 现货写法通常是 {base}B + USDT (如 TSLAUSDT → TSLABUSDT)。"
                )));
            }
        }
        return Err(e);
    }
    Ok(())
}

pub async fn ticker(args: TickerArgs) -> CoreResult<()> {
    ensure_spot_pair(&args.pair).await?;
    let exchange = crate::commands::bn_exchange()?;
    let ob = exchange.get_orderbook(&args.pair, 1).await?;
    match ob.mid_price() {
        Some(mid) => println!("{} 中间价: {}", args.pair, mid),
        None => println!("{} 无盘口数据", args.pair),
    }
    Ok(())
}

pub async fn orderbook(args: OrderbookArgs) -> CoreResult<()> {
    ensure_spot_pair(&args.pair).await?;
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
