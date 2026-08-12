//! t1dm-api — the axum HTTP/WS surface. `build_router` wires every route to
//! a handler; auth is enforced per-route by the [`auth::Auth`] /
//! [`auth::RwAuth`] extractors. The server is driven by tokio in the root
//! binary.

pub mod auth;
pub mod error;
pub mod extract;
pub mod handlers;
pub mod hub;
pub(crate) mod util;

#[cfg(test)]
mod tests;

pub use error::{ApiError, ApiResult};
pub use hub::{Event, HubMsg, WsHub};

use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post, put};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use store::Store;

/// Shared application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub hub: WsHub,
}

/// Ceiling on any request body. axum's own default is 2 MiB, under which a
/// routine phone camera JPEG posted to `/v1/photos` — the client uploads the
/// picked image unrecompressed — fails outright, and the photo path is not
/// queued, so the image is lost rather than retried.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Build the complete `/v1` router. The returned [`Router`] already carries
/// its state and is ready to hand to `axum::serve`.
pub fn build_router(store: Store, hub: WsHub) -> Router {
    let state = AppState { store, hub };

    let v1 = Router::new()
        .route("/ingest", post(handlers::ingest))
        .route("/series", get(handlers::get_series))
        .route("/meals", put(handlers::put_meals).get(handlers::get_meals))
        .route("/doses", put(handlers::put_doses).get(handlers::get_doses))
        .route(
            "/basal-schedule",
            put(handlers::put_basal_schedule).get(handlers::get_basal_schedule),
        )
        .route(
            "/cgm-sources",
            put(handlers::put_cgm_sources).get(handlers::get_cgm_sources),
        )
        .route(
            "/predictions",
            put(handlers::put_predictions).get(handlers::get_predictions),
        )
        .route("/predictions/latest", get(handlers::get_prediction_latest))
        .route(
            "/photos",
            post(handlers::post_photo).get(handlers::get_photos),
        )
        .route("/photos/{id}", get(handlers::get_photo_binary))
        .route(
            "/alerts",
            post(handlers::post_alert).get(handlers::get_alerts),
        )
        .route("/models", get(handlers::get_models))
        .route("/models/{id}/meta", get(handlers::get_model_meta))
        .route("/models/{id}/download", get(handlers::get_model_file))
        .route("/stats", put(handlers::put_stats).get(handlers::get_stats))
        .route("/health", get(handlers::get_health))
        .route("/stream", get(handlers::ws_stream));

    Router::new()
        .nest("/v1", v1)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Bind `addr` and serve `router`, threading per-connection socket info so
/// the auth middleware can record client IPs. Blocks until the server stops.
pub async fn serve(router: Router, addr: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}
