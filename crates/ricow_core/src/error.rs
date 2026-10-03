//! 错误类型: 跨 crate 共享的统一错误枚举。

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error, serde::Serialize, serde::Deserialize)]
pub enum CoreError {
    #[error("network error: {0}")]
    Network(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("exchange error: {0}")]
    Exchange(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("order not found: {0}")]
    OrderNotFound(String),

    #[error("insufficient balance: {0}")]
    InsufficientBalance(String),

    #[error("rate limit: {0}")]
    RateLimit(String),

    /// 本地数据库错误 (`ricow_strategy` 的 SQLite / sqlx 层)。
    ///
    /// 与 [`CoreError::Exchange`] 分开是**有意的**: 过去所有库错误在边界上被一律
    /// `Exchange(e.to_string())`, 于是"本地库打不开 / 表结构不符"和"交易所拒单 / 网络抖动"
    /// 长得一模一样 —— 日志分不出, 提示也就没法引导用户做对的事 (修权限 vs 检查密钥)。
    #[error("db error: {0}")]
    Db(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Parse(e.to_string())
    }
}

impl From<url::ParseError> for CoreError {
    fn from(e: url::ParseError) -> Self {
        CoreError::InvalidArgument(e.to_string())
    }
}
