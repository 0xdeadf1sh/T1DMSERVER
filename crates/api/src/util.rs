//! Small async plumbing shared by the handlers and auth extractors.
//!
//! The store is synchronous rusqlite (a serialized writer plus a blocking
//! read pool); calling it directly from an async task would stall a tokio
//! worker. [`blocking`] shunts a store closure onto the blocking pool and
//! flattens the join-and-store error pair into a single [`ApiError`].

use crate::error::{ApiError, ApiResult};

/// Run a blocking store closure on tokio's blocking pool.
pub(crate) async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    F: FnOnce() -> store::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(format!("blocking task join: {e}")))?
        .map_err(ApiError::from)
}
