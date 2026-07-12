//! Core error type. Kept dependency-light; store/api/tui wrap this as needed.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("unknown series: {0}")]
    UnknownSeries(String),

    #[error("unknown token kind: {0}")]
    UnknownTokenKind(String),

    #[error("unknown stats window: {0}")]
    UnknownWindow(String),

    #[error("timestamp {0} is not on the 5-minute grid")]
    OffGrid(i64),

    #[error("invalid value: {0}")]
    Invalid(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
