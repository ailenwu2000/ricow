//! `ricow_binance` — Binance 现货 + USDT-M 合约适配。
//!
//! - `client`: `BinanceClient` 现货 REST 客户端 (HMAC-SHA256 签名)
//! - `spot`: `BnSpotExchange` (实现 `Exchange` trait)
//! - `futures_client`: `FuturesClient` 合约签名 REST 客户端 (下单/撤单/账户/持仓/杠杆/保证金/listenKey)
//! - `futures`: `BnFuturesExchange` (合约实现 `Exchange`, 012)
//! - `ws`: 现货 WebSocket (市场流 + WebSocket API 用户数据流)
//! - `futures_ws`: 合约 WebSocket (fapi 盘口 + listenKey 用户数据流)
//! - `futures_data`: USDT-M 合约 (fapi) 公共数据源 (K 线 + MMR 元数据, 无密钥)

mod client;
mod futures;
mod futures_client;
mod futures_data;
mod futures_ws;
mod spot;
mod ws;

pub use client::{validate_quantity, BinanceClient};
pub use futures::{
    directional_positions_from_risk, fapi_position_side, is_benign_change_error,
    position_from_risk, BnFuturesExchange,
};
pub use futures_client::{
    parse_available_balance, position_amount, position_liquidation_price, FuturesClient,
};
pub use futures_data::{tier1_mmr_pct, FuturesDataClient, PERP_CONTRACT_TYPES};
pub use futures_ws::parse_futures_user_event;
pub use spot::BnSpotExchange;
