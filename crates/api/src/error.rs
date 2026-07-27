//! API error type and its HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound(String),
    BadRequest(String),
    PayloadTooLarge(String),
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "unauthorized"),
            ApiError::Forbidden => write!(f, "forbidden"),
            ApiError::NotFound(s) => write!(f, "not found: {s}"),
            ApiError::BadRequest(s) => write!(f, "bad request: {s}"),
            ApiError::PayloadTooLarge(s) => write!(f, "payload too large: {s}"),
            ApiError::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<store::StoreError> for ApiError {
    fn from(e: store::StoreError) -> Self {
        match &e {
            // Caller-correctable input: as a 500 the client would retry with
            // backoff forever instead of fixing the timestamp.
            store::StoreError::OffGrid(_) => ApiError::BadRequest(e.to_string()),
            store::StoreError::Invalid(_) => ApiError::BadRequest(e.to_string()),
            store::StoreError::NotFound(_) => ApiError::NotFound(e.to_string()),
            // Everything left is a genuine server fault. It must stay a 5xx:
            // the client drops a 4xx row permanently, and a clinical record
            // must survive a transient database or filesystem failure.
            _ => ApiError::Internal(e.to_string()),
        }
    }
}

impl From<t1dm_core::CoreError> for ApiError {
    fn from(e: t1dm_core::CoreError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            ApiError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, self.to_string()),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
