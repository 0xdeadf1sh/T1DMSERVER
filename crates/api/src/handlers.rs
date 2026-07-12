//! REST + WS handlers. Every store touch is shunted onto the blocking pool
//! via [`crate::util::blocking`] so the async workers never stall on rusqlite.

use std::net::SocketAddr;
use std::str::FromStr;

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use t1dm_core::{IngestBundle, IngestPrediction, Series, SeriesUpsert, StatsWindow};

use crate::auth::{Auth, RwAuth};
use crate::error::{ApiError, ApiResult};
use crate::hub::Event;
use crate::util::blocking;
use crate::AppState;

// ----- shared query types -------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SeriesQuery {
    /// Comma-separated field list, e.g. `bg,carbs,hr`.
    pub fields: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub window: Option<String>,
    /// Force a fresh compute, bypassing the daily cache. Any of `1`/`true`/
    /// `yes`/`on` (case-insensitive) is truthy; parsed as a string because the
    /// query deserializer only accepts `true`/`false` for a bare `bool`.
    pub refresh: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct NoteBody {
    pub ts: i64,
    #[serde(default)]
    pub tz_offset: i32,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct AlertBody {
    pub ts: i64,
    pub kind: String,
    #[serde(default)]
    pub payload: Value,
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
    _auth: RwAuth,
    Json(bundle): Json<IngestBundle>,
) -> ApiResult<Json<Value>> {
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
        app.hub.broadcast(Event::Sample(row));
    }
    Ok(Json(json!({ "ok": true, "ts": ts })))
}

pub async fn put_series(
    State(app): State<AppState>,
    _auth: RwAuth,
    Path(name): Path<String>,
    Json(body): Json<SeriesUpsert>,
) -> ApiResult<Json<Value>> {
    let series = Series::from_str(&name)?;
    let store = app.store.clone();
    let n = blocking(move || store.upsert_samples(series, &body.samples)).await?;
    Ok(Json(json!({ "ok": true, "written": n })))
}

pub async fn put_predictions(
    State(app): State<AppState>,
    _auth: RwAuth,
    Json(preds): Json<Vec<IngestPrediction>>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let (ids, latest) = blocking(move || {
        let ids = store.put_predictions(&preds)?;
        let latest = store.get_prediction_latest()?;
        Ok((ids, latest))
    })
    .await?;
    // Surface the newest prediction to live listeners.
    if let Some(latest) = latest {
        app.hub.broadcast(Event::Prediction(latest));
    }
    Ok(Json(json!({ "ok": true, "ids": ids })))
}

pub async fn post_note(
    State(app): State<AppState>,
    _auth: RwAuth,
    Json(body): Json<NoteBody>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let note = blocking(move || store.add_note(body.ts, body.tz_offset, &body.text)).await?;
    app.hub.broadcast(Event::Note(note.clone()));
    Ok(Json(json!({ "ok": true, "id": note.id })))
}

pub async fn post_photo(
    State(app): State<AppState>,
    _auth: RwAuth,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    let mut ts: Option<i64> = None;
    let mut data: Option<Bytes> = None;
    let mut ext = String::from("jpg");

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("ts") => {
                let txt = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
                ts = txt.trim().parse().ok();
            }
            Some("image") | Some("file") | Some("photo") => {
                if let Some(fname) = field.file_name() {
                    if let Some((_, e)) = fname.rsplit_once('.') {
                        ext = e.to_lowercase();
                    }
                }
                data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?,
                );
            }
            _ => {}
        }
    }

    let ts = ts.ok_or_else(|| ApiError::BadRequest("missing ts".into()))?;
    let data = data.ok_or_else(|| ApiError::BadRequest("missing image".into()))?;
    let store = app.store.clone();
    // Dimensions are decoded by the TUI/importer; unknown at upload time.
    let photo = blocking(move || store.add_photo(ts, &data, 0, 0, &ext)).await?;
    app.hub.broadcast(Event::Photo(photo.clone()));
    Ok(Json(
        json!({ "ok": true, "id": photo.id, "sha256": photo.sha256 }),
    ))
}

pub async fn post_alert(
    State(app): State<AppState>,
    auth: RwAuth,
    Json(body): Json<AlertBody>,
) -> ApiResult<Json<Value>> {
    let origin = auth.0.token.id;
    let store = app.store.clone();
    let alert =
        blocking(move || store.add_alert(body.ts, &body.kind, &body.payload, Some(origin))).await?;
    // Fan out to every session except the origin token's.
    app.hub
        .broadcast_except(Event::Alert(alert.clone()), origin);
    Ok(Json(json!({ "ok": true, "id": alert.id })))
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
    let next = rows.last().map(|r| r.ts);
    Ok(Json(json!({ "rows": rows, "next_cursor": next })))
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

pub async fn get_notes(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Value>> {
    let store = app.store.clone();
    let notes = blocking(move || store.get_notes(q.from, q.to)).await?;
    Ok(Json(json!({ "notes": notes })))
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
    let model = blocking(move || Ok(store.list_models()?.into_iter().find(|m| m.id == idc)))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("model {id}")))?;

    let file = tokio::fs::File::open(&model.path)
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

pub async fn get_stats(
    State(app): State<AppState>,
    _auth: Auth,
    Query(q): Query<StatsQuery>,
) -> ApiResult<Json<Value>> {
    let window = match q.window {
        Some(w) => StatsWindow::from_str(&w)?,
        None => StatsWindow::D7,
    };
    let fresh = q
        .refresh
        .as_deref()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let store = app.store.clone();
    let stats = blocking(move || {
        if fresh {
            store.stats(window)
        } else {
            store.stats_cached(window)
        }
    })
    .await?;
    Ok(Json(json!({ "stats": stats })))
}

/// Liveness plus a cheap system snapshot (memory, uptime, load) sampled via
/// `sysinfo` on the blocking pool. Requires a live token (RO or RW) so host
/// telemetry is never exposed to unauthenticated callers.
pub async fn get_health(State(app): State<AppState>, _auth: Auth) -> Json<Value> {
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

    Json(json!({
        "status": "ok",
        "ws_clients": app.hub.receiver_count(),
        "time_ms": store::now_ms(),
        "system": system,
    }))
}

// ----- websocket ----------------------------------------------------------

pub async fn ws_stream(
    State(app): State<AppState>,
    Query(q): Query<WsQuery>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let secret = q.token;
    let store = app.store.clone();
    let token = match tokio::task::spawn_blocking(move || store.verify_secret(&secret)).await {
        Ok(Ok(Some(t))) => t,
        _ => return (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    };
    let token_id = token.id;

    // Register/refresh a session; it persists across WS reconnects. Auth on
    // the socket rides the `?token=` query, not an Authorization header, so
    // this cannot reuse the `Auth` extractor.
    let ip = addr.ip().to_string();
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("websocket")
        .to_string();
    let store = app.store.clone();
    let _ =
        tokio::task::spawn_blocking(move || store.upsert_session(token_id, &ip, &ua, &ua)).await;

    ws.on_upgrade(move |socket| ws_task(socket, app, token_id))
}

async fn ws_task(mut socket: WebSocket, app: AppState, token_id: i64) {
    let mut rx = app.hub.subscribe();
    loop {
        tokio::select! {
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
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
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
