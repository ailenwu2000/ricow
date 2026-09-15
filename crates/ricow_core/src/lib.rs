//! `ricow_core` — ricow 核心类型与交易所抽象层。
//!
//! 提供跨交易所共享的:
//! - 核心类型([`types`]): 行情 / 订单 / 持仓 / 余额等
//! - 交易所抽象([`exchange`]): [`Exchange`] trait
//! - 错误类型([`error`]): [`CoreError`] / [`CoreResult`]
//! - 配置与密钥: 见 `ricow::commands::config_file`(`$RICOW_ROOT/ricow.toml`, 0600, 无 keyring)

mod error;
mod exchange;
mod types;
#[cfg(test)]
mod types_tests;

pub use error::{CoreError, CoreResult};
pub use exchange::Exchange;
pub use types::*;
