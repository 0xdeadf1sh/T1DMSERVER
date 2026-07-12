//! Store error type wrapping SQLite, the pool, JSON, and core errors.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Pool(#[from] r2d2::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Core(#[from] t1dm_core::CoreError),

    #[error("no live RW token to satisfy request")]
    NoLiveRw,

    #[error("token {0} not found")]
    TokenNotFound(i64),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    Invalid(String),

    #[error("timestamp {0} is not on the 5-minute grid")]
    OffGrid(i64),
}
