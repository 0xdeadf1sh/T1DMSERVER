//! Request-body extractors. These exist so an extraction failure lands in
//! [`ApiError`] and answers with the same `{"error": …}` envelope as every
//! other error, rather than axum's bare plain-text rejection.

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// `axum::Json` in a request position, with its rejection folded into
/// [`ApiError`]: malformed JSON, a missing field, and a missing/incorrect
/// `Content-Type` all answer 400, an overrun of the router's body limit
/// answers 413, and a body stream that failed mid-transfer answers 500 so the
/// client retries it. Responses keep using `axum::Json` directly.
pub struct JsonBody<T>(pub T);

impl<T, S> FromRequest<S> for JsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(from_json_rejection(rejection)),
        }
    }
}

fn from_json_rejection(rejection: JsonRejection) -> ApiError {
    match rejection {
        JsonRejection::BytesRejection(inner) => from_bytes_rejection(inner),
        other if other.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::PayloadTooLarge(other.body_text())
        }
        other => ApiError::BadRequest(other.body_text()),
    }
}

/// The unparsed body, for the one route that must persist a client's JSON
/// verbatim rather than round-trip it through a typed struct. Its rejection is
/// folded into [`ApiError`] for the same reason as [`JsonBody`].
pub struct RawBody(pub Bytes);

impl<S> FromRequest<S> for RawBody
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Bytes::from_request(req, state).await {
            Ok(bytes) => Ok(RawBody(bytes)),
            Err(rejection) => Err(from_bytes_rejection(rejection)),
        }
    }
}

fn from_bytes_rejection(rejection: BytesRejection) -> ApiError {
    match rejection {
        BytesRejection::FailedToBufferBody(FailedToBufferBody::LengthLimitError(e)) => {
            ApiError::PayloadTooLarge(e.body_text())
        }
        // The body stream itself errored mid-transfer — a flapping link, not a
        // malformed request. axum calls it 400, but the client drops a 4xx row
        // permanently, so a clinical record would be lost to a dropped packet.
        BytesRejection::FailedToBufferBody(FailedToBufferBody::UnknownBodyError(e)) => {
            ApiError::Internal(e.body_text())
        }
        other if other.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            ApiError::PayloadTooLarge(other.body_text())
        }
        other => ApiError::BadRequest(other.body_text()),
    }
}
