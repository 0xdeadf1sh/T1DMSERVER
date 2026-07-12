//! Integration tests over a real temp-dir [`Store`], driving the router as a
//! `tower::Service` via `oneshot`. Covers auth accept/reject by kind, the
//! ingest→series roundtrip, cursor pagination, and alert origin-exclusion.

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt; // oneshot

use store::Store;
use t1dm_core::TokenKind;

use crate::hub::Event;
use crate::{build_router, WsHub};

/// A store rooted in a unique temp directory, cleaned up on drop.
struct TempStore {
    store: Store,
    dir: std::path::PathBuf,
}

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_store() -> TempStore {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("t1dm-api-test-{}-{}", std::process::id(), nanos));
    let store = Store::open_at(&dir).expect("open store");
    TempStore { store, dir }
}

fn app(store: Store) -> axum::Router {
    build_router(store, WsHub::new(64))
}

/// A grid-aligned timestamp: `step` five-minute buckets past a fixed epoch
/// that is itself an exact multiple of the 300_000 ms grid.
fn grid_ts(step: i64) -> i64 {
    1_500_000_000_000 + step * 300_000
}

fn get_req(uri: &str, secret: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .body(Body::empty())
        .unwrap()
}

fn body_req(method: &str, uri: &str, secret: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }
}

#[tokio::test]
async fn auth_accept_reject_by_kind() {
    let ts = temp_store();
    let (_rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (_ro, ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("phone".into()))
        .unwrap();

    // No credential -> 401.
    let resp = app(ts.store.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/series")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Unknown secret -> 401.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/series", "deadbeefdeadbeef"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // RO on a read endpoint -> 200.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/series", &ro_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = json!({"ts": grid_ts(0), "tz_offset": 0, "bg": 100.0}).to_string();

    // RO on a write endpoint -> 403.
    let resp = app(ts.store.clone())
        .oneshot(body_req("POST", "/v1/ingest", &ro_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // RW on a write endpoint -> 200.
    let resp = app(ts.store.clone())
        .oneshot(body_req("POST", "/v1/ingest", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ingest_series_roundtrip() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    let t = grid_ts(10);
    let body = json!({"ts": t, "tz_offset": 0, "bg": 123.0, "carbs": 45.0}).to_string();
    let resp = app(ts.store.clone())
        .oneshot(body_req("POST", "/v1/ingest", &rw, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/series?fields=bg,carbs&from={t}&to={t}"),
            &rw,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let rows = v["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ts"], json!(t));
    assert_eq!(rows[0]["bg"], json!(123.0));
    assert_eq!(rows[0]["carbs"], json!(45.0));
    assert_eq!(v["next_cursor"], json!(t));
}

#[tokio::test]
async fn pagination_cursor() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    // Batch-upsert five grid-aligned bg points.
    let samples: Vec<Value> = (0..5)
        .map(|i| json!({"ts": grid_ts(i), "value": i as f64}))
        .collect();
    let body = json!({ "samples": samples }).to_string();
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/series/bg", &rw, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Page 1.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/series?fields=bg&limit=2", &rw))
        .await
        .unwrap();
    let v = body_json(resp).await;
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["ts"], json!(grid_ts(0)));
    assert_eq!(rows[1]["ts"], json!(grid_ts(1)));
    let cursor = v["next_cursor"].as_i64().unwrap();
    assert_eq!(cursor, grid_ts(1));

    // Page 2 — strictly after the cursor, no overlap.
    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/series?fields=bg&limit=2&cursor={cursor}"),
            &rw,
        ))
        .await
        .unwrap();
    let v = body_json(resp).await;
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["ts"], json!(grid_ts(2)));
    assert_eq!(rows[1]["ts"], json!(grid_ts(3)));
}

#[tokio::test]
async fn alert_excludes_origin() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    // Share the hub with the router, subscribe before firing the alert.
    let hub = WsHub::new(64);
    let router = build_router(ts.store.clone(), hub.clone());
    let mut sub = hub.subscribe();

    let body = json!({"ts": grid_ts(0), "kind": "hypo", "payload": {"bg": 55}}).to_string();
    let resp = router
        .oneshot(body_req("POST", "/v1/alerts", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    // The originator's sessions are excluded; every other token still gets it.
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Alert(a) => {
            assert_eq!(a.kind, "hypo");
            assert_eq!(a.origin_token, Some(rw.id));
        }
        other => panic!("expected Alert event, got {other:?}"),
    }
}

#[tokio::test]
async fn health_requires_auth() {
    let ts = temp_store();
    let (_ro, ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("probe".into()))
        .unwrap();

    // No credential -> 401 (host telemetry stays behind auth).
    let resp = app(ts.store.clone())
        .oneshot(
            Request::builder()
                .uri("/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A live RO token -> 200 with the system snapshot.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/health", &ro_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["status"], json!("ok"));
    assert!(v["system"]["uptime_secs"].is_number());
}

#[tokio::test]
async fn model_of_any_extension_is_listed_and_downloadable() {
    let ts = temp_store();
    let (_ro, ro) = ts
        .store
        .mint_token(TokenKind::Ro, Some("phone".into()))
        .unwrap();

    // A non-.pt artifact (an NPU/ONNX build) dropped into models/.
    let dir = ts.store.models_dir();
    let weights = b"onnx graph bytes";
    std::fs::write(dir.join("net.onnx"), weights).unwrap();
    ts.store.refresh_models(&dir).unwrap();

    // It surfaces in the registry with its real extension exposed.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/models", &ro))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let m = &v["models"][0];
    assert_eq!(m["id"], json!("net.onnx"));
    assert_eq!(m["ext"], json!("onnx"));

    // And the extension-neutral download route streams the exact bytes,
    // tagging the response with the artifact's real filename and content hash.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/models/net.onnx/download", &ro))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cd.contains("net.onnx"), "content-disposition: {cd}");
    assert!(resp.headers().get("x-sha256").is_some());
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), weights);
}
