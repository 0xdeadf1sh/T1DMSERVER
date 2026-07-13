//! Integration tests over a real temp-dir [`Store`], driving the router as a
//! `tower::Service` via `oneshot`. Covers auth accept/reject by kind, the
//! six-scalar ingest→series roundtrip and its `sample` fan-out, cursor
//! pagination, the first-class meal/dose/basal-schedule/prediction/stats
//! curve-event endpoints (round-trip, verbatim phone `updated_at`, idempotent
//! upsert-on-newer), every write fanning out to every session *except* its
//! origin token, the phone-pushed statistics block served verbatim, and
//! `GET /v1/health` carrying the `store_epoch` re-mirror marker.

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

/// A router over a throwaway hub — for reads and for writes whose fan-out a
/// test does not inspect.
fn app(store: Store) -> axum::Router {
    build_router(store, WsHub::new(64))
}

/// A router that shares `hub`, so a test can subscribe and observe the fan-out
/// a write triggers.
fn app_on(store: &Store, hub: &WsHub) -> axum::Router {
    build_router(store.clone(), hub.clone())
}

/// A grid-aligned timestamp: `step` five-minute buckets past a fixed epoch
/// that is itself an exact multiple of the 300_000 ms grid.
fn grid_ts(step: i64) -> i64 {
    1_500_000_000_000 + step * 300_000
}

/// A phone wall-clock `updated_at` value, distinct from any grid `ts` and from
/// the server clock, so a verbatim round-trip proves the server never
/// re-stamps it.
fn phone_clock(k: i64) -> i64 {
    1_700_000_000_000 + k
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

/// The single stored meal's `grams` over the whole event window — asserts the
/// `client_id`-keyed table holds exactly one row (idempotency).
async fn meal_grams(store: Store, secret: &str) -> Value {
    let resp = app(store)
        .oneshot(get_req(
            &format!("/v1/meals?from={}&to={}", grid_ts(0), grid_ts(10)),
            secret,
        ))
        .await
        .unwrap();
    let v = body_json(resp).await;
    let meals = v["meals"].as_array().unwrap().clone();
    assert_eq!(meals.len(), 1, "exactly one row per client_id");
    meals[0]["grams"].clone()
}

/// The currently-served `tir` for the 7d stats window.
async fn stats_tir(store: Store, secret: &str) -> Value {
    let resp = app(store)
        .oneshot(get_req("/v1/stats?window=7d", secret))
        .await
        .unwrap();
    let v = body_json(resp).await;
    v["stats"]["tir"].clone()
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

    // A six-scalar ingest bundle carrying the required phone `updated_at`.
    let body =
        json!({"ts": grid_ts(0), "tz_offset": 0, "updated_at": phone_clock(0), "bg": 100.0})
            .to_string();

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
async fn ingest_scalar_roundtrip() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    let t = grid_ts(10);
    let ua = phone_clock(10);
    // Only the six demoted scalars — carbs/bolus/basal are first-class events.
    let body =
        json!({"ts": t, "tz_offset": 0, "updated_at": ua, "bg": 123.0, "hr": 68.0}).to_string();
    let resp = app(ts.store.clone())
        .oneshot(body_req("POST", "/v1/ingest", &rw, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/series?fields=bg,hr&from={t}&to={t}"),
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
    assert_eq!(rows[0]["hr"], json!(68.0));
    // The phone `updated_at` is served back verbatim, never re-stamped.
    assert_eq!(rows[0]["updated_at"], json!(ua));
    // carbs/bolus/basal are gone from the sample shape entirely.
    assert!(rows[0].get("carbs").is_none());
    assert!(rows[0].get("bolus").is_none());
    assert!(rows[0].get("basal").is_none());
    // The server-internal arrival stamp never crosses the wire.
    assert!(rows[0].get("received_at").is_none());
    assert_eq!(v["next_cursor"], json!(t));
}

#[tokio::test]
async fn ingest_fans_out_sample_except_origin() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    let t = grid_ts(4);
    let ua = phone_clock(4);
    let body = json!({"ts": t, "tz_offset": 0, "updated_at": ua, "bg": 142.0}).to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("POST", "/v1/ingest", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Sample(row) => {
            assert_eq!(row.ts, t);
            assert_eq!(row.bg, Some(142.0));
            // The phone clock is carried verbatim through the fan-out.
            assert_eq!(row.updated_at, ua);
        }
        other => panic!("expected Sample event, got {other:?}"),
    }
}

#[tokio::test]
async fn pagination_cursor() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    // Ingest five grid-aligned bg points — the sole sample-write path now.
    for i in 0..5 {
        let body = json!({
            "ts": grid_ts(i), "tz_offset": 0, "updated_at": phone_clock(i), "bg": i as f64
        })
        .to_string();
        let resp = app(ts.store.clone())
            .oneshot(body_req("POST", "/v1/ingest", &rw, &body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

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
    let mut sub = hub.subscribe();

    let body =
        json!({"client_id": "alert-1", "ts": grid_ts(0), "kind": "hypo", "payload": {"bg": 55}})
            .to_string();
    let resp = app_on(&ts.store, &hub)
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
            assert_eq!(a.client_id, "alert-1");
            assert_eq!(a.kind, "hypo");
            assert_eq!(a.origin_token, Some(rw.id));
        }
        other => panic!("expected Alert event, got {other:?}"),
    }
}

#[tokio::test]
async fn meals_roundtrip_and_fanout() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    // A batch of two parametric (gamma) meal-curve events.
    let body = json!([
        {"client_id": "meal-1", "ts": grid_ts(5), "tz_offset": 0, "updated_at": phone_clock(5),
         "grams": 30.0, "duration_min": 180.0, "gi": 1.0, "k": 2.0, "theta": 3.0},
        {"client_id": "meal-2", "ts": grid_ts(6), "tz_offset": 0, "updated_at": phone_clock(6),
         "grams": 55.0, "duration_min": 240.0, "gi": 1.0, "k": 2.0, "theta": 3.0}
    ])
    .to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("PUT", "/v1/meals", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["ids"].as_array().unwrap().len(), 2);

    // Each stored meal fans out to every session except the origin token's.
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..2 {
        let msg = sub.recv().await.expect("hub message");
        assert_eq!(msg.exclude_token, Some(rw.id));
        assert!(!msg.delivers_to(rw.id));
        assert!(msg.delivers_to(ro.id));
        match msg.event {
            Event::Meal(m) => seen.push(m.client_id),
            other => panic!("expected Meal event, got {other:?}"),
        }
    }
    assert!(seen.contains(&"meal-1".to_string()));
    assert!(seen.contains(&"meal-2".to_string()));

    // GET returns both, keyed by phone `client_id`, `updated_at` verbatim,
    // with no server-internal `received_at` leaked onto the wire.
    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/meals?from={}&to={}", grid_ts(0), grid_ts(10)),
            &rw_secret,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let meals = v["meals"].as_array().expect("meals array");
    assert_eq!(meals.len(), 2);
    let m = meals
        .iter()
        .find(|m| m["client_id"] == json!("meal-1"))
        .expect("meal-1 present");
    assert_eq!(m["ts"], json!(grid_ts(5)));
    assert_eq!(m["grams"], json!(30.0));
    assert_eq!(m["gi"], json!(1.0));
    assert_eq!(m["updated_at"], json!(phone_clock(5)));
    assert!(m.get("received_at").is_none());
}

#[tokio::test]
async fn meal_idempotent_upsert_on_newer() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    let put = |updated_at: i64, grams: f64| {
        json!([{
            "client_id": "meal-x", "ts": grid_ts(5), "tz_offset": 0, "updated_at": updated_at,
            "grams": grams, "duration_min": 180.0, "gi": 1.0, "k": 2.0, "theta": 3.0
        }])
        .to_string()
    };

    // First write.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/meals", &rw, &put(1000, 30.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(meal_grams(ts.store.clone(), &rw).await, json!(30.0));

    // Byte-identical redelivery (equal `updated_at`) — a no-op.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/meals", &rw, &put(1000, 30.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(meal_grams(ts.store.clone(), &rw).await, json!(30.0));

    // Stale reorder (older `updated_at`) — rejected, prior value survives.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/meals", &rw, &put(999, 99.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(meal_grams(ts.store.clone(), &rw).await, json!(30.0));

    // Genuine phone-side edit (newer `updated_at`) — replaces in place.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/meals", &rw, &put(2000, 45.0)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(meal_grams(ts.store.clone(), &rw).await, json!(45.0));
}

#[tokio::test]
async fn doses_roundtrip_and_fanout() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    // A single fast bolus (gamma) dose-curve event.
    let body = json!([{
        "client_id": "dose-1", "ts": grid_ts(7), "tz_offset": 0, "updated_at": phone_clock(7),
        "kind": "bolus", "units": 4.5, "duration_min": 300.0, "k": 2.0, "theta": 40.0
    }])
    .to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("PUT", "/v1/doses", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["ids"], json!(["dose-1"]));

    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Dose(d) => {
            assert_eq!(d.client_id, "dose-1");
            assert_eq!(d.units, 4.5);
        }
        other => panic!("expected Dose event, got {other:?}"),
    }

    // GET returns it with its kind, verbatim `updated_at`, and no `received_at`.
    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/doses?from={}&to={}", grid_ts(0), grid_ts(10)),
            &rw_secret,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let doses = v["doses"].as_array().expect("doses array");
    assert_eq!(doses.len(), 1);
    assert_eq!(doses[0]["client_id"], json!("dose-1"));
    assert_eq!(doses[0]["kind"], json!("bolus"));
    assert_eq!(doses[0]["units"], json!(4.5));
    assert_eq!(doses[0]["updated_at"], json!(phone_clock(7)));
    assert!(doses[0].get("received_at").is_none());
}

#[tokio::test]
async fn basal_schedule_replace_and_fanout() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    // Schedule A: two daily slots. Full-replaces the active schedule.
    let body = json!({
        "schedule_id": "sched-A", "active": true, "slots": [
            {"client_id": "slot-1", "label": "night", "time_of_day_min": 0, "dose_u": 0.8,
             "duration_min": 720.0, "ka_per_hour": 0.5, "ke_per_hour": 0.4, "tz_offset": 0,
             "updated_at": phone_clock(1)},
            {"client_id": "slot-2", "label": "day", "time_of_day_min": 720, "dose_u": 0.6,
             "duration_min": 720.0, "ka_per_hour": 0.5, "ke_per_hour": 0.4, "tz_offset": 0,
             "updated_at": phone_clock(2)}
        ]
    })
    .to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("PUT", "/v1/basal-schedule", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["ids"].as_array().unwrap().len(), 2);

    // The whole schedule fans out once, except-origin.
    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::BasalSchedule(s) => {
            assert_eq!(s.schedule_id, "sched-A");
            assert_eq!(s.slots.len(), 2);
            assert!(s.slots.iter().any(|sl| sl.client_id == "slot-1"));
        }
        other => panic!("expected BasalSchedule event, got {other:?}"),
    }

    // Schedule B: a single slot — a full replace, not a merge.
    let body = json!({
        "schedule_id": "sched-B", "active": true, "slots": [
            {"client_id": "slot-b", "label": "flat", "time_of_day_min": 0, "dose_u": 1.2,
             "duration_min": 1440.0, "ka_per_hour": 0.5, "ke_per_hour": 0.4, "tz_offset": 0,
             "updated_at": phone_clock(3)}
        ]
    })
    .to_string();
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/basal-schedule", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET returns only the active (replaced) schedule.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/basal-schedule", &rw_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let sched = &v["basal_schedule"];
    assert_eq!(sched["schedule_id"], json!("sched-B"));
    let slots = sched["slots"].as_array().expect("slots array");
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["client_id"], json!("slot-b"));
    assert_eq!(slots[0]["dose_u"], json!(1.2));
    assert_eq!(slots[0]["updated_at"], json!(phone_clock(3)));
    assert!(slots[0].get("received_at").is_none());
}

#[tokio::test]
async fn predictions_roundtrip_and_fanout() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    let made_at = grid_ts(3);
    // One row per quantile level (7) × horizon_steps (2).
    let fan: Vec<Vec<f64>> = vec![vec![95.0, 96.0]; 7];
    let body = json!([{
        "made_at": made_at, "model_id": "m1", "updated_at": phone_clock(3),
        "horizon_steps": 2, "line": [100.0, 105.0],
        "fan": fan,
        "circadian": {"probs": [0.1, 0.2], "predicted_hour": 7.5, "resultant_r": 0.8,
                      "n_bins": 12, "bin_hours": 2.0}
    }])
    .to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("PUT", "/v1/predictions", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The newest prediction fans out, except-origin, with `made_at` verbatim
    // (the phone cycle timestamp, never the server clock).
    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Prediction(p) => {
            assert_eq!(p.made_at, made_at);
            assert_eq!(p.model_id, "m1");
        }
        other => panic!("expected Prediction event, got {other:?}"),
    }

    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/predictions/latest", &rw_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let p = &v["prediction"];
    assert_eq!(p["made_at"], json!(made_at));
    assert_eq!(p["model_id"], json!("m1"));
    assert_eq!(p["circadian"]["predicted_hour"], json!(7.5));
}

#[tokio::test]
async fn notes_fanout_and_client_id() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    let body = json!({
        "client_id": "note-1", "ts": grid_ts(2), "tz_offset": 0,
        "text": "felt low after run", "updated_at": phone_clock(2)
    })
    .to_string();
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("POST", "/v1/notes", &rw_secret, &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["id"], json!("note-1"));

    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Note(n) => {
            assert_eq!(n.client_id, "note-1");
            assert_eq!(n.text, "felt low after run");
            assert_eq!(n.updated_at, phone_clock(2));
        }
        other => panic!("expected Note event, got {other:?}"),
    }

    let resp = app(ts.store.clone())
        .oneshot(get_req(
            &format!("/v1/notes?from={}&to={}", grid_ts(0), grid_ts(10)),
            &rw_secret,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let notes = v["notes"].as_array().expect("notes array");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["client_id"], json!("note-1"));
    assert_eq!(notes[0]["updated_at"], json!(phone_clock(2)));
}

#[tokio::test]
async fn stats_push_served_verbatim() {
    let ts = temp_store();
    let (rw, rw_secret) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
    let (ro, _ro_secret) = ts
        .store
        .mint_token(TokenKind::Ro, Some("watcher".into()))
        .unwrap();

    let hub = WsHub::new(64);
    let mut sub = hub.subscribe();

    // The opaque phone `Stats` block for one window, carrying an extra field the
    // server cannot know about — proving it stores/serves the body verbatim
    // rather than re-deriving a fixed schema.
    let block = json!({
        "window": "7d", "updated_at": phone_clock(10),
        "tir": 0.72, "time_below": 0.03, "time_above": 0.25,
        "mean_bg": 145.0, "gmi": 6.8, "cv": 0.32, "sd": 46.0,
        "hypo_events": {"count": 1, "duration_ms": 600000},
        "hyper_events": {"count": 2, "duration_ms": 1200000},
        "mean_daily_carbs": 180.0, "tdd": 42.0, "bolus_basal_ratio": 0.6,
        "mean_hr": 72.0, "bg_hr_corr": 0.1, "n_samples": 2016,
        "custom_marker": 424242
    });
    let resp = app_on(&ts.store, &hub)
        .oneshot(body_req("PUT", "/v1/stats", &rw_secret, &block.to_string()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["window"], json!("7d"));

    // Fans out except-origin, carrying the opaque block whole.
    let msg = sub.recv().await.expect("hub message");
    assert_eq!(msg.exclude_token, Some(rw.id));
    assert!(!msg.delivers_to(rw.id));
    assert!(msg.delivers_to(ro.id));
    match msg.event {
        Event::Stats(s) => {
            assert_eq!(s.window, "7d");
            assert_eq!(s.updated_at, phone_clock(10));
            assert_eq!(s.json["tir"], json!(0.72));
            assert_eq!(s.json["custom_marker"], json!(424242));
        }
        other => panic!("expected Stats event, got {other:?}"),
    }

    // GET serves the pushed block back byte-for-byte, extra field and all.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/stats?window=7d", &rw_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let stats = &v["stats"];
    assert_eq!(stats["window"], json!("7d"));
    assert_eq!(stats["updated_at"], json!(phone_clock(10)));
    assert_eq!(stats["tir"], json!(0.72));
    assert_eq!(stats["mean_bg"], json!(145.0));
    assert_eq!(stats["custom_marker"], json!(424242));

    // A window the phone never pushed serves an all-zero block, not an error.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/stats?window=30d", &rw_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["stats"]["window"], json!("30d"));
    assert_eq!(v["stats"]["tir"], json!(0.0));
}

#[tokio::test]
async fn stats_upsert_on_newer() {
    let ts = temp_store();
    let (_rw, rw) = ts.store.mint_token(TokenKind::Rw, None).unwrap();

    let push = |updated_at: i64, tir: f64| {
        json!({"window": "7d", "updated_at": updated_at, "tir": tir}).to_string()
    };

    // First push.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/stats", &rw, &push(phone_clock(10), 0.72)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stats_tir(ts.store.clone(), &rw).await, json!(0.72));

    // Stale push (older `updated_at`) — rejected, prior block survives.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/stats", &rw, &push(phone_clock(9), 0.10)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stats_tir(ts.store.clone(), &rw).await, json!(0.72));

    // Newer push — replaces the window's block.
    let resp = app(ts.store.clone())
        .oneshot(body_req("PUT", "/v1/stats", &rw, &push(phone_clock(20), 0.90)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stats_tir(ts.store.clone(), &rw).await, json!(0.90));
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

    // A live RO token -> 200 with the system snapshot and the re-mirror marker.
    let resp = app(ts.store.clone())
        .oneshot(get_req("/v1/health", &ro_secret))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["status"], json!("ok"));
    assert!(v["system"]["uptime_secs"].is_number());
    // A freshly-created store mints a `store_epoch` so the phone can detect a
    // wiped/new server and re-mirror its authoritative history (§3.8).
    assert!(
        v.get("store_epoch").is_some() && !v["store_epoch"].is_null(),
        "health must carry a non-null store_epoch"
    );
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
