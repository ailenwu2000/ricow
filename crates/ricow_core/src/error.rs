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
