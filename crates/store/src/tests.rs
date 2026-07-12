//! Store unit/integration tests: migrations, the `one_rw` token invariant,
//! ingest→read roundtrips, and the statistics reduction. Each test runs
//! against a throwaway store rooted in a unique temp directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use t1dm_core::{
    IngestBundle, IngestPrediction, SampleRow, Series, SeriesPoint, StatsWindow, TokenKind, GRID_MS,
};

use crate::schema::LATEST_VERSION;
use crate::stats::compute;
use crate::{FakeOpts, FakeRange, Store};

/// A store rooted in a fresh temp dir, wiped on drop.
struct TempStore {
    store: Store,
    dir: PathBuf,
}

impl TempStore {
    fn new() -> TempStore {
        static CTR: AtomicU64 = AtomicU64::new(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "t1dm-store-test-{}-{}-{}",
            std::process::id(),
            n,
            crate::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open_at(&dir).expect("open store");
        TempStore { store, dir }
    }
}

impl std::ops::Deref for TempStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Grid timestamp: `n` steps after a fixed on-grid epoch.
fn grid_ts(n: i64) -> i64 {
    // A round on-grid anchor (2021-01-01T00:00:00Z snapped) plus n steps.
    1_609_459_200_000 + n * GRID_MS
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "expected {b}, got {a}");
}

// ---------------------------------------------------------------- migrations

#[test]
fn migrations_apply_once_and_are_idempotent() {
    let ts = TempStore::new();

    // The recorded head version is the latest.
    let head: i64 = ts
        .with_reader(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(head, LATEST_VERSION);

    // Re-running migrate is a no-op and leaves exactly one applied row.
    ts.migrate().unwrap();
    ts.migrate().unwrap();
    let count: i64 = ts
        .with_reader(|c| {
            Ok(c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))?)
        })
        .unwrap();
    assert_eq!(count, LATEST_VERSION);

    // Every core table is present and queryable.
    for tbl in [
        "samples",
        "prediction",
        "note",
        "photo",
        "alert",
        "token",
        "session",
        "model",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {tbl}");
        let n: i64 = ts
            .with_reader(|c| Ok(c.query_row(&sql, [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0, "{tbl} should start empty");
    }

    // Reopening the same directory re-applies cleanly (idempotent open).
    let dir = ts.dir.clone();
    let reopened = Store::open_at(&dir).unwrap();
    assert!(reopened
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()
        .is_empty());
}

// ------------------------------------------------------------- one_rw tokens

#[test]
fn minting_rw_upholds_one_rw_invariant() {
    let ts = TempStore::new();

    let (rw1, secret1) = ts.mint_token(TokenKind::Rw, None).unwrap();
    let (rw2, secret2) = ts.mint_token(TokenKind::Rw, Some("second".into())).unwrap();

    // At most one live RW token, at the DB level.
    let live_rw: i64 = ts
        .with_reader(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM token WHERE kind='rw' AND revoked_at IS NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(live_rw, 1);

    // The replaced token no longer verifies; the current one does.
    assert!(ts.verify_secret(&secret1).unwrap().is_none());
    let resolved = ts.verify_secret(&secret2).unwrap().expect("rw2 live");
    assert_eq!(resolved.id, rw2.id);
    assert_ne!(rw1.id, rw2.id);

    // RO tokens coexist freely; several may be live at once.
    let (_ro_a, sec_a) = ts.mint_token(TokenKind::Ro, Some("phone".into())).unwrap();
    let (ro_b, _sec_b) = ts.mint_token(TokenKind::Ro, Some("tablet".into())).unwrap();
    let live = ts.list_tokens(false).unwrap();
    assert_eq!(live.len(), 3, "1 RW + 2 RO live");
    assert_eq!(live.iter().filter(|t| t.kind == TokenKind::Rw).count(), 1);

    // Revocation is reflected in verification and is idempotent.
    ts.revoke_token(ro_b.id).unwrap();
    ts.revoke_token(ro_b.id).unwrap();
    assert!(ts.verify_secret(&sec_a).unwrap().is_some());
    assert_eq!(ts.list_tokens(false).unwrap().len(), 2);

    // Unknown ids are an error, not a silent success.
    assert!(ts.revoke_token(9_999).is_err());
}

// -------------------------------------------------------- ingest ↔ read

#[test]
fn ingest_bundle_roundtrips_and_rejects_off_grid() {
    let ts = TempStore::new();
    let t = grid_ts(0);

    let bundle = IngestBundle {
        ts: t,
        tz_offset: -300,
        bg: Some(123.5),
        carbs: Some(40.0),
        bolus: Some(4.0),
        basal: Some(0.8),
        hr: Some(72.0),
        prediction: Some(IngestPrediction {
            model_id: "m1".into(),
            horizon_steps: 3,
            line: vec![120.0, 125.0, 130.0],
            fan: vec![vec![110.0, 112.0, 114.0]; 7],
            tod: vec![1.0; 12],
            tod_conf: 0.8,
        }),
        notes: vec!["breakfast".into()],
        ..Default::default()
    };
    ts.ingest_bundle(&bundle).unwrap();

    let rows = ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.ts, t);
    assert_eq!(r.tz_offset, -300);
    approx(r.bg.unwrap(), 123.5);
    approx(r.carbs.unwrap(), 40.0);
    approx(r.total_insulin().unwrap(), 4.8);
    assert!(r.steps.is_none(), "absent field stays NULL");

    // Embedded prediction and note are written in the same transaction.
    let latest = ts.get_prediction_latest().unwrap().expect("prediction");
    assert_eq!(latest.model_id, "m1");
    assert_eq!(latest.made_at, t);
    approx(latest.line[1], 125.0);
    assert_eq!(ts.get_notes(None, None).unwrap().len(), 1);

    // A second ingest with only HR present coalesces onto the existing row.
    let patch = IngestBundle {
        ts: t,
        hr: Some(80.0),
        ..Default::default()
    };
    ts.ingest_bundle(&patch).unwrap();
    let r2 = &ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()[0];
    approx(r2.hr.unwrap(), 80.0);
    approx(r2.bg.unwrap(), 123.5);

    // Off-grid timestamps are refused.
    let bad = IngestBundle {
        ts: t + 1,
        ..Default::default()
    };
    assert!(matches!(
        ts.ingest_bundle(&bad),
        Err(crate::StoreError::OffGrid(_))
    ));
}

#[test]
fn series_upsert_and_cursor_pagination() {
    let ts = TempStore::new();
    let points: Vec<SeriesPoint> = (0..5)
        .map(|i| SeriesPoint {
            ts: grid_ts(i),
            value: 100.0 + i as f64,
        })
        .collect();
    let n = ts.upsert_samples(Series::Bg, &points).unwrap();
    assert_eq!(n, 5);

    // Override in place (backfill semantics): hard-overwrite one column.
    ts.upsert_samples(
        Series::Bg,
        &[SeriesPoint {
            ts: grid_ts(2),
            value: 999.0,
        }],
    )
    .unwrap();
    let all = ts
        .get_samples(&[Series::Bg], None, None, None, None)
        .unwrap();
    assert_eq!(all.len(), 5);
    approx(all[2].bg.unwrap(), 999.0);

    // Forward cursor pagination skips rows with ts <= cursor.
    let page = ts
        .get_samples(&[Series::Bg], None, None, Some(2), Some(grid_ts(1)))
        .unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].ts, grid_ts(2));
    assert_eq!(page[1].ts, grid_ts(3));

    // Integer series truncates to i64.
    ts.upsert_samples(
        Series::Mood,
        &[SeriesPoint {
            ts: grid_ts(0),
            value: 4.7,
        }],
    )
    .unwrap();
    let row0 = &ts
        .get_samples(
            &[Series::Mood],
            Some(grid_ts(0)),
            Some(grid_ts(0)),
            None,
            None,
        )
        .unwrap()[0];
    assert_eq!(row0.mood, Some(4));
}

// -------------------------------------------------------------- statistics

fn bg_hr_row(ts: i64, bg: f64, hr: f64) -> SampleRow {
    SampleRow {
        ts,
        bg: Some(bg),
        hr: Some(hr),
        ..Default::default()
    }
}

#[test]
fn compute_reduces_window_metrics() {
    // Four samples: one hypo, two in-range, one hyper.
    let mut rows = vec![
        bg_hr_row(grid_ts(0), 50.0, 60.0),
        bg_hr_row(grid_ts(1), 100.0, 70.0),
        bg_hr_row(grid_ts(2), 200.0, 90.0),
        bg_hr_row(grid_ts(3), 100.0, 70.0),
    ];
    // hr = 50 + 0.2*bg exactly, so BG↔HR correlation is +1.
    rows[1].carbs = Some(30.0);
    rows[3].carbs = Some(20.0);
    rows[1].bolus = Some(3.0);
    rows[3].bolus = Some(2.0);
    for r in &mut rows {
        r.basal = Some(2.5);
    }

    let s = compute(StatsWindow::D7, &rows);
    approx(s.mean_bg, 112.5);
    approx(s.tir, 0.5);
    approx(s.time_below, 0.25);
    approx(s.time_above, 0.25);
    assert_eq!(s.n_samples, 4);

    // sd² = mean of squared deviations about 112.5.
    approx(s.sd, 2968.75_f64.sqrt());
    approx(s.cv, s.sd / 112.5 * 100.0);
    approx(s.gmi, 3.31 + 0.02392 * 112.5);

    // One contiguous hypo run and one hyper run, each a single 5-min sample.
    assert_eq!(s.hypo_events.count, 1);
    assert_eq!(s.hypo_events.duration_ms, GRID_MS);
    assert_eq!(s.hyper_events.count, 1);
    assert_eq!(s.hyper_events.duration_ms, GRID_MS);

    // Daily aggregates: totals divided by the 7-day window span.
    approx(s.mean_daily_carbs, 50.0 / 7.0);
    approx(s.tdd, 15.0 / 7.0); // (3+2) bolus + (4×2.5) basal, per day over 7d
    approx(s.bolus_basal_ratio, 0.5);
    approx(s.mean_hr, 72.5);
    approx(s.bg_hr_corr, 1.0);

    // No BG at all → the empty block.
    let none = compute(
        StatsWindow::D7,
        &[SampleRow {
            ts: grid_ts(9),
            ..Default::default()
        }],
    );
    assert_eq!(none.n_samples, 0);
    approx(none.tir, 0.0);
}

#[test]
fn stats_over_store_is_bounded() {
    let ts = TempStore::new();
    assert_eq!(ts.stats(StatsWindow::D7).unwrap().n_samples, 0);

    let written = ts
        .generate_fake(FakeRange::last_days(1), FakeOpts::default())
        .unwrap();
    assert!(written > 200, "≈288 grid rows over a day, got {written}");

    let s = ts.stats(StatsWindow::D7).unwrap();
    assert!(s.n_samples > 0);
    assert!((0.0..=1.0).contains(&s.tir));
    assert!((0.0..=1.0).contains(&(s.tir + s.time_below + s.time_above)));
    assert!(s.mean_bg >= 40.0 && s.mean_bg <= 400.0);
    assert!((-1.0..=1.0).contains(&s.bg_hr_corr));

    // The synthetic generator also emits predictions.
    assert!(ts.get_prediction_latest().unwrap().is_some());
}

// -------------------------------------------------------- photos / models

#[test]
fn photo_write_read_and_teardown_clears_them() {
    let ts = TempStore::new();
    let data = b"\x89PNG fake bytes";
    let p = ts.add_photo(grid_ts(0), data, 16, 9, "png").unwrap();
    assert_eq!(p.bytes, data.len() as i64);

    let path = ts.photo_path(p.id).unwrap().expect("path");
    assert!(path.exists());
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(ts.get_photos(None, None).unwrap().len(), 1);

    ts.teardown().unwrap();
    assert!(ts.get_photos(None, None).unwrap().is_empty());
    assert!(!path.exists(), "teardown clears the photos dir");
    // Tables were recreated, not merely dropped.
    assert!(ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()
        .is_empty());
}

// With a live token referenced by a session, teardown must not abort partway
// on the implicit `DELETE FROM token` (foreign_keys = ON) — the whole schema
// has to come back so a follow-up generation writes cleanly.
#[test]
fn teardown_with_referencing_session_recreates_schema_and_allows_generate() {
    let ts = TempStore::new();
    let (tok, _secret) = ts.mint_token(TokenKind::Rw, Some("phone".into())).unwrap();
    ts.upsert_session(tok.id, "10.0.0.2", "agent", "device-a")
        .unwrap();

    ts.teardown().expect("teardown must succeed despite FK edge");

    // Every dropped table is back and empty.
    assert!(ts.list_tokens(true).unwrap().is_empty());
    assert!(ts.list_sessions().unwrap().is_empty());
    assert!(ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()
        .is_empty());

    // The reported failure: generation must now write straight into `samples`.
    let written = ts
        .generate_fake(FakeRange::last_days(1), FakeOpts::default())
        .expect("generate after teardown must not hit a missing table");
    assert!(written > 200, "≈288 grid rows over a day, got {written}");
}

#[test]
fn refresh_models_hashes_and_preserves_opaque_meta() {
    let ts = TempStore::new();
    let dir = ts.models_dir();
    let weights = b"pretend torchscript weights";
    std::fs::write(dir.join("forecaster.pt"), weights).unwrap();
    std::fs::write(
        dir.join("forecaster.json"),
        json!({"arch":"lstm","layers":2}).to_string(),
    )
    .unwrap();

    let models = ts.refresh_models(&dir).unwrap();
    assert_eq!(models.len(), 1);
    let m = &models[0];
    // id is the full filename; ext is exposed; name is the stem.
    assert_eq!(m.id, "forecaster.pt");
    assert_eq!(m.name, "forecaster");
    assert_eq!(m.ext, "pt");
    assert_eq!(m.bytes, weights.len() as i64);
    assert_eq!(m.sha256, crate::writes::sha256_hex(weights));
    assert_eq!(m.meta, json!({"arch":"lstm","layers":2}));

    assert_eq!(
        ts.get_model_meta("forecaster.pt").unwrap(),
        Some(json!({"arch":"lstm","layers":2}))
    );
    assert!(ts.model_path("forecaster.pt").unwrap().is_some());
    assert!(ts.get_model_meta("absent").unwrap().is_none());

    // Re-scan is idempotent (upsert on id), still one row.
    assert_eq!(ts.refresh_models(&dir).unwrap().len(), 1);
}

#[test]
fn refresh_models_registers_any_extension_and_skips_sidecars() {
    let ts = TempStore::new();
    let dir = ts.models_dir();
    // A .pt and an NPU-optimized variant of the same logical model coexist.
    std::fs::write(dir.join("net.pt"), b"torchscript").unwrap();
    std::fs::write(dir.join("net.onnx"), b"onnx graph").unwrap();
    std::fs::write(dir.join("net.json"), json!({"arch":"gru"}).to_string()).unwrap();
    // A dotfile and a bare (extension-less) artifact.
    std::fs::write(dir.join(".gitkeep"), b"").unwrap();
    std::fs::write(dir.join("blob"), b"headerless weights").unwrap();

    let models = ts.refresh_models(&dir).unwrap();
    // net.pt, net.onnx, blob — the .json sidecar and .gitkeep are excluded.
    assert_eq!(models.len(), 3);

    let by_id = |id: &str| models.iter().find(|m| m.id == id).cloned();
    let onnx = by_id("net.onnx").expect("onnx variant registered");
    assert_eq!(onnx.ext, "onnx");
    assert_eq!(onnx.name, "net");
    // Both format variants share the stem-keyed opaque meta sidecar.
    assert_eq!(onnx.meta, json!({"arch":"gru"}));
    assert_eq!(by_id("net.pt").unwrap().meta, json!({"arch":"gru"}));

    let blob = by_id("blob").expect("extension-less artifact registered");
    assert_eq!(blob.ext, "");

    // Both variants remain fetchable by their distinct ids.
    assert!(ts.model_path("net.pt").unwrap().is_some());
    assert!(ts.model_path("net.onnx").unwrap().is_some());
}

// ------------------------------------------------------------ sessions

#[test]
fn sessions_upsert_and_persist() {
    let ts = TempStore::new();
    let (tok, _s) = ts.mint_token(TokenKind::Ro, Some("op".into())).unwrap();

    let a = ts
        .upsert_session(tok.id, "10.0.0.2", "curl/8", "phone")
        .unwrap();
    let a2 = ts
        .upsert_session(tok.id, "10.0.0.2", "curl/9", "phone")
        .unwrap();
    assert_eq!(a.id, a2.id, "same identity upserts one row");
    assert!(a2.last_seen >= a.last_seen);
    assert_eq!(a2.user_agent, "curl/9");

    // A distinct device is a distinct session.
    ts.upsert_session(tok.id, "10.0.0.2", "curl/8", "tablet")
        .unwrap();
    assert_eq!(ts.list_sessions().unwrap().len(), 2);
}
