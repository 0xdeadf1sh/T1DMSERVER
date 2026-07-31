//! REST + WS handlers. Every store touch is shunted onto the blocking pool
//! via [`crate::util::blocking`] so the async workers never stall on rusqlite.

use std::str::FromStr;

use axum::body::{Body, Bytes};
use axum::extract::multipart::MultipartError;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use t1dm_core::{
    BasalSchedule, DoseEvent, IngestBundle, MealEvent, PredictionWrite, Series, Stats, StatsBlock,
    StatsWindow,
};

use crate::auth::{Auth, RwAuth, WsAuth};
use crate::error::{ApiError, ApiResult};
use crate::extract::{JsonBody, RawBody};
use crate::hub::Event;
use crate::util::blocking;
use crate::AppState;

// ----- shared query types -------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// Row cap, not a page: these endpoints expose no cursor, because `ts` is
    /// not unique on the event tables. Absent means the whole range.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SeriesQuery {
    /// Comma-separated field list, e.g. `bg,hr,steps`.
    pub fields: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub window: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AlertBody {
    pub client_id: String,
    pub ts: i64,
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
}

/// Thin key extractor for the opaque stats block: the phone `Stats` JSON is
/// stored verbatim, but the server needs `window` (the primary key) and the
/// phone `updated_at` (the idempotency guard) to persist it.
#[derive(Debug, Deserialize)]
struct StatsKey {
    window: String,
    updated_at: i64,
}

fn parse_fields(spec: &Option<String>) -> Result<Vec<Series>, ApiError> {
    match spec {
        None => Ok(Series::ALL.to_vec()),
        Some(s) if s.trim().is_empty() => Ok(Series::ALL.to_vec()),
        Some(s) => s
            .split(',')
            .map(|f| Series::from_str(f.trim()).map_err(ApiError::from))
            .collect(),
    }
}

// ----- writes (RW) --------------------------------------------------------

pub async fn ingest(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(bundle): JsonBody<IngestBundle>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let ts = bundle.ts;
    let store = app.store.clone();
    // Ingest and read the resulting row back in one blocking hop.
    let row = blocking(move || {
        store.ingest_bundle(&bundle)?;
        Ok(store
            .get_samples(&[], Some(ts), Some(ts), Some(1), None)?
            .into_iter()
            .next())
    })
    .await?;
    if let Some(row) = row {
        app.hub.broadcast_except(Event::Sample(row), origin);
    }
    Ok(Json(json!({ "ok": true, "ts": ts })))
}

/// Idempotent batch upsert of meal-curve events, keyed by phone `client_id`.
/// A redelivery is a no-op; a newer `updated_at` replaces in place. Every
/// stored row fans out to every session except the origin token's.
pub async fn put_meals(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(meals): JsonBody<Vec<MealEvent>>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let stored = blocking(move || store.put_meals(&meals)).await?;
    let ids: Vec<String> = stored.iter().map(|m| m.client_id.clone()).collect();
    for meal in stored {
        app.hub.broadcast_except(Event::Meal(meal), origin);
    }
    Ok(Json(json!({ "ok": true, "ids": ids })))
}

/// Idempotent batch upsert of dose-curve events (bolus/basal), keyed by phone
/// `client_id`. Fans out except-origin.
pub async fn put_doses(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(doses): JsonBody<Vec<DoseEvent>>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let stored = blocking(move || store.put_doses(&doses)).await?;
    let ids: Vec<String> = stored.iter().map(|d| d.client_id.clone()).collect();
    for dose in stored {
        app.hub.broadcast_except(Event::Dose(dose), origin);
    }
    Ok(Json(json!({ "ok": true, "ids": ids })))
}

/// Full-replace the active basal schedule (a daily-repeating template the TUI
/// tiles). Idempotent per-slot on `client_id`; fans out except-origin.
pub async fn put_basal_schedule(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(schedule): JsonBody<BasalSchedule>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let stored = blocking(move || store.put_basal_schedule(&schedule)).await?;
    let ids: Vec<String> = stored.slots.iter().map(|s| s.client_id.clone()).collect();
    app.hub
        .broadcast_except(Event::BasalSchedule(stored), origin);
    Ok(Json(json!({ "ok": true, "ids": ids })))
}

pub async fn put_predictions(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(preds): JsonBody<Vec<PredictionWrite>>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let (ids, latest) = blocking(move || {
        let ids = store.put_predictions(&preds)?;
        let latest = store.get_prediction_latest()?;
        Ok((ids, latest))
    })
    .await?;
    // Surface the newest prediction to live listeners, except the origin.
    if let Some(latest) = latest {
        app.hub.broadcast_except(Event::Prediction(latest), origin);
    }
    Ok(Json(json!({ "ok": true, "ids": ids })))
}

/// Store a phone-pushed statistics block for one window verbatim. The body is
/// the opaque phone `Stats` JSON; only `window` and `updated_at` are peeled
/// off to key and guard the upsert. `window` must name a window the readers
/// know (`7d`/`30d`/`90d`) or the push is rejected with 400; it is stored and
/// echoed in its canonical spelling. Fans out except-origin.
pub async fn put_stats(
    State(app): State<AppState>,
    auth: RwAuth,
    RawBody(body): RawBody,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let raw = std::str::from_utf8(body.as_ref())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .to_string();
    let key: StatsKey =
        serde_json::from_str(&raw).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let window = StatsWindow::from_str(&key.window)?;
    let updated_at = key.updated_at;
    let store = app.store.clone();
    // Persist the phone clock verbatim; `put_stats_block` guards on `updated_at`.
    let applied = blocking(move || store.put_stats_block(window, &raw, updated_at)).await?;
    // A push the guard rejected is stale: fanning out the request body would
    // stream a block older than the one every viewer already holds.
    if applied {
        app.hub.broadcast_except(
            Event::Stats(StatsBlock {
                window: window.as_str().to_string(),
                updated_at,
                json: value,
            }),
            origin,
        );
    }
    Ok(Json(json!({ "ok": true, "window": window.as_str() })))
}

/// A body that overran the router's limit must answer 413, not 400: the photo
/// upload is not queued, so a caller that reads it as a permanent client error
/// discards the image instead of retrying it smaller.
fn multipart_err(e: MultipartError) -> ApiError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::PayloadTooLarge(e.body_text())
    } else {
        ApiError::BadRequest(e.to_string())
    }
}

pub async fn post_photo(
    State(app): State<AppState>,
    auth: RwAuth,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let mut ts: Option<i64> = None;
    let mut data: Option<Bytes> = None;
    let mut ext = String::from("jpg");

    while let Some(field) = multipart.next_field().await.map_err(multipart_err)? {
        match field.name() {
            Some("ts") => {
                let txt = field.text().await.map_err(multipart_err)?;
                ts = txt.trim().parse().ok();
            }
            Some("image") | Some("file") | Some("photo") => {
                if let Some(fname) = field.file_name() {
                    if let Some((_, e)) = fname.rsplit_once('.') {
                        ext = e.to_lowercase();
                    }
                }
                data = Some(field.bytes().await.map_err(multipart_err)?);
            }
            _ => {}
        }
    }

    let ts = ts.ok_or_else(|| ApiError::BadRequest("missing ts".into()))?;
    let data = data.ok_or_else(|| ApiError::BadRequest("missing image".into()))?;
    let store = app.store.clone();
    // Dimensions are decoded by the TUI/importer; unknown at upload time.
    let photo = blocking(move || store.add_photo(ts, &data, 0, 0, &ext)).await?;
    app.hub.broadcast_except(Event::Photo(photo.clone()), origin);
    Ok(Json(
        json!({ "ok": true, "id": photo.id, "sha256": photo.sha256 }),
    ))
}

pub async fn post_alert(
    State(app): State<AppState>,
    auth: RwAuth,
    JsonBody(body): JsonBody<AlertBody>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let alert = blocking(move || {
        store.add_alert(&body.client_id, body.ts, &body.kind, &body.payload, Some(origin))
    })
    .await?;
    // Fan out to every session except the origin token's.
    app.hub
        .broadcast_except(Event::Alert(alert.clone()), origin);
    Ok(Json(json!({ "ok": true, "id": alert.client_id })))
}

// ----- reads (RO or RW) ---------------------------------------------------

pub async fn get_series(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<SeriesQuery>,
) -> ApiResult<Json<Value>> {
    let fields = parse_fields(&q.fields)?;
    let store = app.store.clone();
    let rows =
        blocking(move || store.get_samples(&fields, q.from, q.to, q.limit, q.cursor)).await?;
    // A short page is the last one; advertising a cursor there costs the caller
    // an extra round trip that can only come back empty.
    let next = match q.limit {
        Some(lim) if rows.len() < lim => None,
        _ => rows.last().map(|r| r.ts),
    };
    Ok(Json(json!({ "rows": rows, "next_cursor": next })))
}

pub async fn get_meals(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let meals = blocking(move || store.get_meals(q.from, q.to, q.limit)).await?;
    Ok(Json(json!({ "meals": meals })))
}

pub async fn get_doses(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let doses = blocking(move || store.get_doses(q.from, q.to, q.limit)).await?;
    Ok(Json(json!({ "doses": doses })))
}

/// The active schedule as a bare top-level object, or JSON `null` when none is
/// active — no envelope key. The client decodes the body straight into its
/// schedule type, and the `basal_schedule` stream frame inlines the same fields.
pub async fn get_basal_schedule(
    State(app): State<AppState>,
    _auth: Auth,
) -> ApiResult<Json<Option<BasalSchedule>>> {
    let store = app.store.clone();
    let schedule = blocking(move || store.get_basal_schedule()).await?;
    Ok(Json(schedule))
}

pub async fn get_predictions(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let preds = blocking(move || store.get_predictions(q.from, q.to)).await?;
    Ok(Json(json!({ "predictions": preds })))
}

pub async fn get_prediction_latest(
    State(app): State<AppState>,
    _auth: Auth,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let pred = blocking(move || store.get_prediction_latest()).await?;
    Ok(Json(json!({ "prediction": pred })))
}

pub async fn get_alerts(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let alerts = blocking(move || store.get_alerts(q.from, q.to)).await?;
    Ok(Json(json!({ "alerts": alerts })))
}

pub async fn get_photos(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let photos = blocking(move || store.get_photos(q.from, q.to)).await?;
    Ok(Json(json!({ "photos": photos })))
}

pub async fn get_photo_binary(
    State(app): State<AppState>,
    _auth: Auth,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let store = app.store.clone();
    let path = blocking(move || store.photo_path(id))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("photo {id}")))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let ctype = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/jpeg",
    };
    Ok(([(header::CONTENT_TYPE, ctype)], bytes).into_response())
}

pub async fn get_models(State(app): State<AppState>, _auth: Auth) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let models = blocking(move || store.list_models()).await?;
    Ok(Json(json!({ "models": models })))
}

pub async fn get_model_meta(
    State(app): State<AppState>,
    _auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let idc = id.clone();
    let meta = blocking(move || store.get_model_meta(&idc))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("model {id}")))?;
    Ok(Json(meta))
}

/// Stream a model artifact of any format in bounded chunks so a large file
/// never has to be buffered whole in RAM (the Pi Zero has 512MB). Emits
/// `Content-Length`, `X-SHA256`, and a `Content-Disposition` filename (carrying
/// the artifact's real extension) alongside the streamed body.
pub async fn get_model_file(
    State(app): State<AppState>,
    _auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let store = app.store.clone();
    let idc = id.clone();
    // `Model::path` is stored relative to the data dir; `model_path` rejoins
    // the root, so the artifact is opened from an absolute path.
    let (model, path) = blocking(move || {
        let model = store.list_models()?.into_iter().find(|m| m.id == idc);
        let path = store.model_path(&idc)?;
        Ok(model.zip(path))
    })
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("model {id}")))?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ApiError::NotFound(format!("model artifact unavailable: {e}")))?;
    let len = file
        .metadata()
        .await
        .map(|m| m.len())
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let stream = futures::stream::try_unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        let n = file.read(&mut buf).await?;
        if n == 0 {
            Ok::<_, std::io::Error>(None)
        } else {
            buf.truncate(n);
            Ok(Some((Bytes::from(buf), file)))
        }
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    if let Ok(v) = len.to_string().parse() {
        headers.insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = model.sha256.parse() {
        headers.insert("x-sha256", v);
    }
    // `id` is the artifact filename, so it already carries the real extension.
    if let Ok(v) = format!("attachment; filename=\"{}\"", model.id).parse() {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok((headers, Body::from_stream(stream)).into_response())
}

/// Serve the most recent phone-pushed statistics block for `window`
/// (`7d`/`30d`/`90d`, default `7d`) verbatim, or an all-zero block when the
/// phone has pushed none. The server never computes or re-derives statistics.
pub async fn get_stats(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<StatsQuery>,
) -> ApiResult<Json<Value>> {
    let window = match q.window.as_deref() {
        Some(w) => StatsWindow::from_str(w)?,
        None => StatsWindow::D7,
    };
    let store = app.store.clone();
    let raw = blocking(move || store.get_stats_block(window.as_str())).await?;
    let block = match raw {
        Some(txt) => serde_json::from_str::<Value>(&txt)
            .unwrap_or_else(|_| serde_json::to_value(Stats::empty(window)).unwrap_or(Value::Null)),
        None => serde_json::to_value(Stats::empty(window)).unwrap_or(Value::Null),
    };
    Ok(Json(json!({ "stats": block })))
}

/// Liveness plus a cheap system snapshot (memory, uptime, load) sampled via
/// `sysinfo` on the blocking pool. Requires a live token (RO or RW) so host
/// telemetry is never exposed to unauthenticated callers. Carries the
/// `store_epoch` marker so the phone can detect a freshly-wiped/new server and
/// re-mirror its authoritative history (§3.8).
pub async fn get_health(State(app): State<AppState>, _auth: Auth) -> ApiResult<Json<Value>> {
    let system = tokio::task::spawn_blocking(|| {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_memory();
        let load = System::load_average();
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0);
        json!({
            "mem_total_bytes": sys.total_memory(),
            "mem_used_bytes": sys.used_memory(),
            "mem_available_bytes": sys.available_memory(),
            "cpus": cpus,
            "uptime_secs": System::uptime(),
            "load_avg": [load.one, load.five, load.fifteen],
        })
    })
    .await
    .unwrap_or(Value::Null);

    // A failed read must not surface as `null`: the phone reads `null` as "no
    // marker" and silently skips the re-mirror until the next reconnect, so a
    // read failure would look exactly like the one case the marker exists for.
    let store = app.store.clone();
    let store_epoch = blocking(move || store.store_epoch())
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "health: store_epoch read failed"))?;

    Ok(Json(json!({
        "status": "ok",
        // The TUI bridge holds one permanent in-process subscriber that is not a
        // connected client; main.rs discounts it the same way for its own probe.
        "ws_clients": app.hub.receiver_count().saturating_sub(1),
        "time_ms": store::now_ms(),
        "store_epoch": store_epoch,
        "system": system,
    })))
}

// ----- websocket ----------------------------------------------------------

/// How often an upgraded stream re-checks that its token is still live. Auth
/// is only resolved at upgrade time, so this bounds how long a revoked token
/// keeps receiving events.
const WS_LIVENESS_PERIOD: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn ws_stream(
    State(app): State<AppState>,
    auth: WsAuth,
    ws: WebSocketUpgrade,
) -> Response {
    let token_id = auth.0.token.id;
    ws.on_upgrade(move |socket| ws_task(socket, app, token_id))
}

async fn ws_task(mut socket: WebSocket, app: AppState, token_id: i64) {
    let mut rx = app.hub.subscribe();
    let mut liveness = tokio::time::interval_at(
        tokio::time::Instant::now() + WS_LIVENESS_PERIOD,
        WS_LIVENESS_PERIOD,
    );
    loop {
        tokio::select! {
            _ = liveness.tick() => {
                let store = app.store.clone();
                let live = blocking(move || {
                    Ok(store.list_tokens(false)?.iter().any(|t| t.id == token_id))
                })
                .await;
                // Fail closed: a revoked or unreadable token loses the stream.
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
            recv = rx.recv() => {
                match recv {
                    Ok(msg) => {
                        if !msg.delivers_to(token_id) {
                            continue;
                        }
                        match serde_json::to_string(&msg.event) {
                            Ok(txt) => {
                                if socket.send(Message::Text(txt.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => continue,
                        }
                    }
                    // The buffer overran and events are already gone. Announce
                    // the gap, then close: a reconnect re-pages the REST reads
                    // from the client's own high-water marks, which is the only
                    // way what was skipped comes back.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, token_id, "ws lagged; closing to force resync");
                        let notice = json!({ "type": "lagged", "missed": missed }).to_string();
                        let _ = socket.send(Message::Text(notice.into())).await;
                        break;
                    }
                    Err(_) => break,
                }
            }
            client = socket.recv() => {
                match client {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => { /* server->client only; ignore inbound */ }
                    Some(Err(_)) => break,
                }
            }
        }
    }
}
