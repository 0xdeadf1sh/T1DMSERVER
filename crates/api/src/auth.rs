//! Bearer authentication. The middleware resolves the secret to a token row,
//! upserts a session (ip + user-agent/device), and enforces the token kind.
//! RW endpoints require [`RwAuth`]; read endpoints accept either kind.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Query};
use axum::http::request::Parts;

use t1dm_core::{Token, TokenKind};

use crate::error::ApiError;
use crate::AppState;

/// An authenticated request context: the resolved token and its session.
#[derive(Debug, Clone)]
pub struct Auth {
    pub token: Token,
    pub session_id: i64,
}

/// Extract the `Authorization: Bearer <secret>` value.
fn bearer_secret(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(axum::http::header::AUTHORIZATION)?;
    let s = value.to_str().ok()?;
    let rest = s
        .strip_prefix("Bearer ")
        .or_else(|| s.strip_prefix("bearer "))?;
    Some(rest.trim().to_string())
}

fn client_ip(parts: &Parts) -> String {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn user_agent(parts: &Parts) -> String {
    parts
        .headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let secret = bearer_secret(parts).ok_or(ApiError::Unauthorized)?;
        let store = state.store.clone();
        let token = crate::util::blocking(move || store.verify_secret(&secret))
            .await?
            .ok_or(ApiError::Unauthorized)?;

        let ip = client_ip(parts);
        let ua = user_agent(parts);
        let token_id = token.id;
        let store = state.store.clone();
        let session =
            crate::util::blocking(move || store.upsert_session(token_id, &ip, &ua, &ua)).await?;

        Ok(Auth {
            token,
            session_id: session.id,
        })
    }
}

/// A stream request context. The socket's credential rides `?token=<secret>`
/// rather than an `Authorization` header, so it cannot reuse [`Auth`]. It is an
/// extractor rather than an in-handler check so that an absent, unknown, or
/// revoked token answers 401 *before* the upgrade handshake is evaluated, as
/// the contract promises — otherwise a bad token on a malformed handshake would
/// answer 426 instead.
#[derive(Debug, Clone)]
pub struct WsAuth(pub Auth);

impl FromRequestParts<AppState> for WsAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Query(q) = Query::<crate::handlers::WsQuery>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::Unauthorized)?;
        let secret = q.token.ok_or(ApiError::Unauthorized)?;
        let store = state.store.clone();
        let token = crate::util::blocking(move || store.verify_secret(&secret))
            .await?
            .ok_or(ApiError::Unauthorized)?;

        // The session persists across WS reconnects.
        let ip = client_ip(parts);
        let ua = parts
            .headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("websocket")
            .to_string();
        let token_id = token.id;
        let store = state.store.clone();
        let session =
            crate::util::blocking(move || store.upsert_session(token_id, &ip, &ua, &ua)).await?;

        Ok(WsAuth(Auth {
            token,
            session_id: session.id,
        }))
    }
}

/// A write-authorized request context. Rejects RO tokens with 403.
#[derive(Debug, Clone)]
pub struct RwAuth(pub Auth);

impl FromRequestParts<AppState> for RwAuth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = Auth::from_request_parts(parts, state).await?;
        if auth.token.kind != TokenKind::Rw {
            return Err(ApiError::Forbidden);
        }
        Ok(RwAuth(auth))
    }
}
