//! Store unit/integration tests: migrations, the `one_rw` token invariant,
//! six-scalar ingest→read roundtrips, the first-class meal/dose/basal-schedule
//! curve events, prediction idempotency, phone-pushed stats blocks, and the
//! `store_epoch` identity. Each test runs against a throwaway store rooted in a
//! unique temp directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use t1dm_core::{
    BasalSchedule, BasalSlot, DoseEvent, DoseKind, IngestBundle, MealEvent, PredictionWrite, Series,
    StatsWindow, TokenKind, GRID_MS,
};

use crate::schema::LATEST_VERSION;
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

    // Every core table is present and queryable. `meta` is excluded: it may
    // legitimately hold the minted `store_epoch` row (see the store_epoch test).
    for tbl in [
        "samples",
        "meal_event",
        "dose_event",
        "basal_schedule_dose",
        "prediction",
        "stats_block",
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

    // The note feature is withdrawn: v2 drops the table v1 created, so a fresh
    // store must not carry it.
    let notes: i64 = ts
        .with_reader(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='note'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(notes, 0, "the note table must not survive migration");

    // Reopening the same directory re-applies cleanly (idempotent open).
    let dir = ts.dir.clone();
    let reopened = Store::open_at(&dir).unwrap();
    assert!(reopened
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()
        .is_empty());
}

/// A store as it stood before the note feature was withdrawn: version 1
/// recorded, the `note` table present and populated. The store is keep-forever,
/// so those rows go only if a migration drops them — this pins that it does.
#[test]
fn migration_drops_the_note_table_of_a_v1_store() {
    let dir = std::env::temp_dir().join(format!(
        "t1dm-v1-store-{}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t1dm.db")).unwrap();
    conn.execute_batch(crate::schema::CONNECTION_PRAGMAS).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations (
             version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
         INSERT INTO schema_migrations(version, applied_at) VALUES (1, 0);
         CREATE TABLE note (
             id INTEGER PRIMARY KEY, client_id TEXT NOT NULL,
             ts INTEGER NOT NULL, tz_offset INTEGER NOT NULL DEFAULT 0,
             text TEXT NOT NULL, updated_at INTEGER NOT NULL,
             created_at INTEGER NOT NULL);
         CREATE UNIQUE INDEX note_client ON note(client_id);
         INSERT INTO note(client_id, ts, tz_offset, text, updated_at, created_at)
             VALUES ('n-1', 0, 0, 'felt low before lunch', 1, 1);",
    )
    .unwrap();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM note", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, 1, "the v1 fixture starts with one stored note");

    crate::schema::migrate(&conn).unwrap();

    let head: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(head, LATEST_VERSION);
    let tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='note'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0, "the note table and its rows must be dropped");

    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
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

// ---------------------------------------------------- six-scalar ingest ↔ read

#[test]
fn ingest_bundle_roundtrips_coalesces_and_rejects_off_grid() {
    let ts = TempStore::new();
    let t = grid_ts(0);

    // Six demoted scalars only — carbs/bolus/basal are first-class curve events
    // now, and predictions have their own endpoint. `updated_at` is the phone
    // clock, stored verbatim.
    let bundle = IngestBundle {
        ts: t,
        tz_offset: -300,
        updated_at: 1_000,
        bg: Some(123.5),
        hr: Some(72.0),
        steps: Some(410.0),
        mood: Some(4),
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
    assert_eq!(r.updated_at, 1_000, "phone clock stored verbatim");
    approx(r.bg.unwrap(), 123.5);
    approx(r.hr.unwrap(), 72.0);
    approx(r.steps.unwrap(), 410.0);
    assert_eq!(r.mood, Some(4));
    assert!(
        r.sleep.is_none() && r.exercise.is_none(),
        "absent fields stay NULL"
    );
    // `received_at` is the server's internal arrival stamp: populated on read,
    // never serialized onto the wire (`#[serde(skip)]`).
    assert!(r.received_at > 0, "server arrival stamp is set internally");

    // A later partial fill (HR only, newer `updated_at`) coalesces onto the
    // existing row: present columns survive via COALESCE, `updated_at` advances.
    let patch = IngestBundle {
        ts: t,
        updated_at: 2_000,
        hr: Some(80.0),
        ..Default::default()
    };
    ts.ingest_bundle(&patch).unwrap();
    let r2 = &ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap()[0];
    approx(r2.hr.unwrap(), 80.0);
    approx(r2.bg.unwrap(), 123.5);
    assert_eq!(r2.updated_at, 2_000);

    // Idempotency: a stale redelivery (older `updated_at`) is a no-op — the
    // `excluded.updated_at >= samples.updated_at` guard rejects it, so neither
    // the value nor `updated_at` regresses. The `ts` primary key keeps the row
    // count at one throughout (no duplicate bucket).
    let stale = IngestBundle {
        ts: t,
        updated_at: 1_500,
        hr: Some(999.0),
        ..Default::default()
    };
    ts.ingest_bundle(&stale).unwrap();
    let r3 = ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap();
    assert_eq!(r3.len(), 1, "one bucket, no duplicate");
    approx(r3[0].hr.unwrap(), 80.0);
    assert_eq!(r3[0].updated_at, 2_000);

    // Off-grid timestamps are refused.
    let bad = IngestBundle {
        ts: t + 1,
        updated_at: 3_000,
        ..Default::default()
    };
    assert!(matches!(
        ts.ingest_bundle(&bad),
        Err(crate::StoreError::OffGrid(_))
    ));
}

#[test]
fn ingest_cursor_pagination_and_mood_column() {
    let ts = TempStore::new();
    for i in 0..5 {
        ts.ingest_bundle(&IngestBundle {
            ts: grid_ts(i),
            tz_offset: 0,
            updated_at: 1_000 + i,
            bg: Some(100.0 + i as f64),
            mood: Some(3),
            ..Default::default()
        })
        .unwrap();
    }

    // Forward cursor pagination skips rows with ts <= cursor.
    let page = ts
        .get_samples(&[Series::Bg], None, None, Some(2), Some(grid_ts(1)))
        .unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].ts, grid_ts(2));
    assert_eq!(page[1].ts, grid_ts(3));
    approx(page[0].bg.unwrap(), 102.0);

    // Mood is the one integer-valued scalar and round-trips as an i64.
    let row0 = &ts
        .get_samples(&[Series::Mood], Some(grid_ts(0)), Some(grid_ts(0)), None, None)
        .unwrap()[0];
    assert_eq!(row0.mood, Some(3));
}

// --------------------------------------------------------- meal curve events

#[test]
fn meal_event_roundtrips_and_upserts_on_newer() {
    let ts = TempStore::new();

    // A parametric meal: gamma params, no resolved curve.
    let m = MealEvent {
        client_id: "meal-abc".into(),
        ts: grid_ts(1),
        tz_offset: -300,
        updated_at: 1_000,
        grams: 60.0,
        duration_min: 180.0,
        gi: Some(0.75),
        k: Some(2.0),
        theta: Some(20.0),
        custom_curve: None,
        note: Some("breakfast".into()),
    };
    let out = ts.put_meals(std::slice::from_ref(&m)).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0], m, "the canonical row echoes the input verbatim");

    // Readable by range and by phone `client_id`.
    assert_eq!(ts.get_meals(None, None, None).unwrap(), vec![m.clone()]);
    assert_eq!(ts.get_meal("meal-abc").unwrap(), Some(m.clone()));

    // A mixed/builder meal is stored only as its resolved appearance curve —
    // there is no per-component breakdown.
    let mixed = MealEvent {
        client_id: "meal-mix".into(),
        ts: grid_ts(2),
        tz_offset: 0,
        updated_at: 1_000,
        grams: 45.0,
        duration_min: 120.0,
        gi: None,
        k: None,
        theta: None,
        custom_curve: Some(vec![0.0, 1.5, 3.0, 2.0, 0.5]),
        note: None,
    };
    ts.put_meals(std::slice::from_ref(&mixed)).unwrap();
    assert_eq!(
        ts.get_meal("meal-mix").unwrap().unwrap().custom_curve,
        mixed.custom_curve,
        "the resolved curve round-trips through the JSON column"
    );

    // Idempotency: a redelivery (equal `updated_at`) is a no-op — no duplicate.
    ts.put_meals(std::slice::from_ref(&m)).unwrap();
    assert_eq!(ts.get_meals(None, None, None).unwrap().len(), 2, "no duplicate row");

    // A reordered stale redelivery (older `updated_at`) is rejected in place.
    let stale = MealEvent {
        grams: 999.0,
        updated_at: 500,
        ..m.clone()
    };
    ts.put_meals(std::slice::from_ref(&stale)).unwrap();
    approx(ts.get_meal("meal-abc").unwrap().unwrap().grams, 60.0);

    // A genuine phone-side edit (newer `updated_at`) replaces in place.
    let edited = MealEvent {
        grams: 75.0,
        updated_at: 2_000,
        note: Some("bigger".into()),
        ..m.clone()
    };
    let echoed = ts.put_meals(std::slice::from_ref(&edited)).unwrap();
    approx(echoed[0].grams, 75.0);
    let got = ts.get_meal("meal-abc").unwrap().unwrap();
    approx(got.grams, 75.0);
    assert_eq!(got.updated_at, 2_000);
    assert_eq!(got.note.as_deref(), Some("bigger"));
}

// --------------------------------------------------------- dose curve events

#[test]
fn dose_event_roundtrips_bolus_and_basal_and_upserts_on_newer() {
    let ts = TempStore::new();

    // A bolus carries gamma params; a basal carries Bateman ka/ke params.
    let bolus = DoseEvent {
        client_id: "dose-bolus".into(),
        ts: grid_ts(1),
        tz_offset: 0,
        updated_at: 1_000,
        kind: DoseKind::Bolus,
        units: 4.5,
        duration_min: 240.0,
        k: Some(2.0),
        theta: Some(25.0),
        ka_per_hour: None,
        ke_per_hour: None,
        custom_curve: None,
        note: None,
    };
    let basal = DoseEvent {
        client_id: "dose-basal".into(),
        ts: grid_ts(2),
        tz_offset: 0,
        updated_at: 1_000,
        kind: DoseKind::Basal,
        units: 12.0,
        duration_min: 1440.0,
        k: None,
        theta: None,
        ka_per_hour: Some(1.0),
        ke_per_hour: Some(0.5),
        custom_curve: None,
        note: Some("overnight".into()),
    };

    let out = ts.put_doses(&[bolus.clone(), basal.clone()]).unwrap();
    assert_eq!(
        out,
        vec![bolus.clone(), basal.clone()],
        "canonical rows echo the inputs in request order"
    );

    // The dose kind survives the text round-trip through the CHECK column.
    assert_eq!(
        ts.get_dose("dose-bolus").unwrap().unwrap().kind,
        DoseKind::Bolus
    );
    assert_eq!(
        ts.get_dose("dose-basal").unwrap().unwrap().kind,
        DoseKind::Basal
    );
    assert_eq!(
        ts.get_doses(None, None, None).unwrap(),
        vec![bolus.clone(), basal.clone()]
    );

    // Redelivery (equal `updated_at`) is a no-op; no duplicate rows.
    ts.put_doses(std::slice::from_ref(&bolus)).unwrap();
    assert_eq!(ts.get_doses(None, None, None).unwrap().len(), 2);

    // Stale (older `updated_at`) rejected; newer replaces in place.
    let stale = DoseEvent {
        units: 99.0,
        updated_at: 500,
        ..bolus.clone()
    };
    ts.put_doses(std::slice::from_ref(&stale)).unwrap();
    approx(ts.get_dose("dose-bolus").unwrap().unwrap().units, 4.5);

    let edited = DoseEvent {
        units: 6.0,
        updated_at: 2_000,
        ..bolus.clone()
    };
    ts.put_doses(std::slice::from_ref(&edited)).unwrap();
    let got = ts.get_dose("dose-bolus").unwrap().unwrap();
    approx(got.units, 6.0);
    assert_eq!(got.updated_at, 2_000);
}

// ------------------------------------------------- curve-event read bounding

#[test]
fn curve_event_reads_bound_only_when_a_limit_is_asked_for() {
    let ts = TempStore::new();
    let meals: Vec<MealEvent> = (0..5)
        .map(|i| MealEvent {
            client_id: format!("meal-{i}"),
            ts: grid_ts(i),
            updated_at: 1_000 + i,
            grams: 30.0,
            duration_min: 180.0,
            ..Default::default()
        })
        .collect();
    ts.put_meals(&meals).unwrap();
    let doses: Vec<DoseEvent> = (0..5)
        .map(|i| DoseEvent {
            client_id: format!("dose-{i}"),
            ts: grid_ts(i),
            updated_at: 1_000 + i,
            kind: DoseKind::Bolus,
            units: 2.0,
            duration_min: 240.0,
            ..Default::default()
        })
        .collect();
    ts.put_doses(&doses).unwrap();

    // No limit stays unbounded: the phone's desync-forced full resync asks for
    // the whole history in one shot and does not page.
    assert_eq!(ts.get_meals(None, None, None).unwrap().len(), 5);
    assert_eq!(ts.get_doses(None, None, None).unwrap().len(), 5);

    // A limit takes the oldest rows first, so the last `ts` of a page is the
    // cursor the next one resumes from.
    let page = ts.get_meals(None, None, Some(2)).unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].ts, grid_ts(0));
    assert_eq!(page[1].ts, grid_ts(1));
    let next = ts.get_meals(Some(page[1].ts + 1), None, Some(2)).unwrap();
    assert_eq!(next[0].ts, grid_ts(2));
    assert_eq!(next[1].ts, grid_ts(3));

    let dpage = ts.get_doses(None, None, Some(3)).unwrap();
    assert_eq!(dpage.len(), 3);
    assert_eq!(dpage[2].ts, grid_ts(2));

    // A limit past the end neither errors nor pads; a zero limit yields nothing.
    assert_eq!(ts.get_doses(None, None, Some(99)).unwrap().len(), 5);
    assert!(ts.get_meals(None, None, Some(0)).unwrap().is_empty());

    // The bound composes with the range rather than replacing it.
    let ranged = ts
        .get_meals(Some(grid_ts(1)), Some(grid_ts(3)), Some(2))
        .unwrap();
    assert_eq!(ranged.len(), 2);
    assert_eq!(ranged[0].ts, grid_ts(1));
}

// ------------------------------------------------- curve-event grid discipline

#[test]
fn curve_events_reject_off_grid_timestamps_without_writing() {
    let ts = TempStore::new();

    let meal = MealEvent {
        client_id: "meal-off".into(),
        ts: grid_ts(1) + 1,
        tz_offset: 0,
        updated_at: 1_000,
        grams: 30.0,
        duration_min: 120.0,
        gi: Some(0.5),
        k: None,
        theta: None,
        custom_curve: None,
        note: None,
    };
    assert!(matches!(
        ts.put_meals(std::slice::from_ref(&meal)),
        Err(crate::StoreError::OffGrid(_))
    ));
    assert!(ts.get_meals(None, None, None).unwrap().is_empty());

    let dose = DoseEvent {
        client_id: "dose-off".into(),
        ts: grid_ts(1) - 1,
        tz_offset: 0,
        updated_at: 1_000,
        kind: DoseKind::Bolus,
        units: 3.0,
        duration_min: 240.0,
        k: Some(2.0),
        theta: Some(25.0),
        ka_per_hour: None,
        ke_per_hour: None,
        custom_curve: None,
        note: None,
    };
    assert!(matches!(
        ts.put_doses(std::slice::from_ref(&dose)),
        Err(crate::StoreError::OffGrid(_))
    ));
    assert!(ts.get_doses(None, None, None).unwrap().is_empty());

    // The rejection precedes the transaction, so an on-grid sibling batched with
    // an off-grid row is not half-written.
    let good_meal = MealEvent {
        client_id: "meal-ok".into(),
        ts: grid_ts(1),
        ..meal.clone()
    };
    assert!(ts.put_meals(&[good_meal, meal]).is_err());
    assert!(ts.get_meals(None, None, None).unwrap().is_empty());
}

// ------------------------------------------------------ basal schedule replace

#[test]
fn basal_schedule_full_replace_keeps_one_active() {
    let ts = TempStore::new();
    assert!(ts.get_basal_schedule().unwrap().is_none());

    let slot = |cid: &str, tod: i32, dose: f64| BasalSlot {
        client_id: cid.into(),
        label: format!("slot-{tod}"),
        time_of_day_min: tod,
        dose_u: dose,
        duration_min: 180.0,
        ka_per_hour: 1.0,
        ke_per_hour: 0.4,
        tz_offset: 0,
        updated_at: 1_000,
    };

    let sched = BasalSchedule {
        schedule_id: "sched-A".into(),
        active: true,
        slots: vec![slot("a1", 360, 0.8), slot("a2", 720, 1.0)],
    };
    let stored = ts.put_basal_schedule(&sched).unwrap();
    assert_eq!(stored.schedule_id, "sched-A");
    assert!(stored.active);
    // Slots come back ordered by `time_of_day_min`.
    assert_eq!(stored.slots.len(), 2);
    assert_eq!(stored.slots[0].client_id, "a1");
    assert_eq!(stored.slots[1].client_id, "a2");

    let live = ts.get_basal_schedule().unwrap().unwrap();
    assert_eq!(live.schedule_id, "sched-A");
    assert_eq!(live.slots.len(), 2);

    // A full-replace with fewer slots drops the omitted one (a true replace),
    // while the surviving slot's edit needs a newer `updated_at` to land.
    let a1_edit = BasalSlot {
        updated_at: 2_000,
        ..slot("a1", 360, 0.9)
    };
    let shrunk = BasalSchedule {
        schedule_id: "sched-A".into(),
        active: true,
        slots: vec![a1_edit],
    };
    let after = ts.put_basal_schedule(&shrunk).unwrap();
    assert_eq!(after.slots.len(), 1, "the omitted slot is dropped");
    assert_eq!(after.slots[0].client_id, "a1");
    approx(after.slots[0].dose_u, 0.9);

    // Activating a second schedule retires the first — exactly one stays live.
    let sched_b = BasalSchedule {
        schedule_id: "sched-B".into(),
        active: true,
        slots: vec![slot("b1", 300, 0.5)],
    };
    ts.put_basal_schedule(&sched_b).unwrap();
    let live2 = ts.get_basal_schedule().unwrap().unwrap();
    assert_eq!(
        live2.schedule_id, "sched-B",
        "only the newest activation is live"
    );
    assert_eq!(live2.slots.len(), 1);
}

#[test]
fn basal_schedule_reactivation_survives_unchanged_slots() {
    let ts = TempStore::new();

    let slot = |cid: &str, tod: i32| BasalSlot {
        client_id: cid.into(),
        label: format!("slot-{tod}"),
        time_of_day_min: tod,
        dose_u: 0.7,
        duration_min: 180.0,
        ka_per_hour: 1.0,
        ke_per_hour: 0.4,
        tz_offset: 0,
        updated_at: 1_000,
    };
    let sched_a = BasalSchedule {
        schedule_id: "sched-A".into(),
        active: true,
        slots: vec![slot("a1", 360), slot("a2", 720)],
    };
    let sched_b = BasalSchedule {
        schedule_id: "sched-B".into(),
        active: true,
        slots: vec![slot("b1", 300)],
    };
    ts.put_basal_schedule(&sched_a).unwrap();
    ts.put_basal_schedule(&sched_b).unwrap();

    // Re-PUT A verbatim: every slot upsert no-ops on the `updated_at` guard, so
    // A's activation has to be asserted outright — otherwise B is retired, A
    // stays retired, and the scheduled basal silently vanishes.
    let stored = ts.put_basal_schedule(&sched_a).unwrap();
    assert!(stored.active);
    assert_eq!(stored.slots.len(), 2);

    let live = ts
        .get_basal_schedule()
        .unwrap()
        .expect("a re-PUT of the live schedule keeps it live");
    assert_eq!(live.schedule_id, "sched-A");
    assert_eq!(live.slots.len(), 2);

    let active_schedules: i64 = ts
        .with_reader(|c| {
            Ok(c.query_row(
                "SELECT COUNT(DISTINCT schedule_id) FROM basal_schedule_dose WHERE active = 1",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(active_schedules, 1, "exactly one schedule stays active");

    // Deactivating a schedule retires its rows without dropping them.
    let retired = BasalSchedule {
        active: false,
        ..sched_a.clone()
    };
    let stored = ts.put_basal_schedule(&retired).unwrap();
    assert!(!stored.active);
    assert_eq!(stored.slots.len(), 2, "retiring keeps the slots");
    assert!(ts.get_basal_schedule().unwrap().is_none());
}

#[test]
fn basal_schedule_stale_redelivery_cannot_resurrect_a_retired_schedule() {
    let ts = TempStore::new();

    let slot = |cid: &str, tod: i32, updated_at: i64| BasalSlot {
        client_id: cid.into(),
        label: format!("slot-{tod}"),
        time_of_day_min: tod,
        dose_u: 0.7,
        duration_min: 180.0,
        ka_per_hour: 1.0,
        ke_per_hour: 0.4,
        tz_offset: 0,
        updated_at,
    };
    let sched_a = BasalSchedule {
        schedule_id: "sched-A".into(),
        active: true,
        slots: vec![slot("a1", 360, 1_000), slot("a2", 720, 1_000)],
    };
    let sched_b = BasalSchedule {
        schedule_id: "sched-B".into(),
        active: true,
        slots: vec![slot("b1", 300, 2_000)],
    };
    ts.put_basal_schedule(&sched_a).unwrap();
    ts.put_basal_schedule(&sched_b).unwrap();

    // A queued retry of the superseded A lands late, and with one slot fewer
    // than the stored A. Every slot upsert no-ops on the `updated_at` guard and
    // A's version (its newest slot) predates B's, so the whole PUT is a no-op:
    // the activation must not flip back to a schedule the phone has replaced.
    let retry = BasalSchedule {
        slots: vec![slot("a1", 360, 1_000)],
        ..sched_a.clone()
    };
    let echoed = ts.put_basal_schedule(&retry).unwrap();
    assert!(!echoed.active, "echoed as stored, not as sent");
    assert_eq!(echoed.slots.len(), 2, "a stale replace drops no slot either");

    let live = ts.get_basal_schedule().unwrap().unwrap();
    assert_eq!(live.schedule_id, "sched-B");
    assert_eq!(live.slots.len(), 1);

    // A genuine phone-side edit of A (newer than B) does take activation back.
    let revived = BasalSchedule {
        slots: vec![slot("a1", 360, 3_000)],
        ..sched_a.clone()
    };
    let stored = ts.put_basal_schedule(&revived).unwrap();
    assert!(stored.active);
    assert_eq!(stored.slots.len(), 1, "the newer replace does drop a2");
    assert_eq!(
        ts.get_basal_schedule().unwrap().unwrap().schedule_id,
        "sched-A"
    );
}

#[test]
fn basal_stale_retry_cannot_prune_newer_slots_of_the_same_schedule() {
    let ts = TempStore::new();

    let slot = |cid: &str, tod: i32, updated_at: i64| BasalSlot {
        client_id: cid.into(),
        label: format!("slot-{tod}"),
        time_of_day_min: tod,
        dose_u: 0.8,
        duration_min: 180.0,
        ka_per_hour: 1.0,
        ke_per_hour: 0.4,
        tz_offset: 0,
        updated_at,
    };
    let sched = |slots: Vec<BasalSlot>| BasalSchedule {
        schedule_id: "sched-only".into(),
        active: true,
        slots,
    };

    // The phone keeps ONE stable schedule_id, so there is never a rival holding
    // activation — the case a rival-only staleness check cannot see.
    ts.put_basal_schedule(&sched(vec![slot("s1", 360, 1_000)]))
        .unwrap();
    let grown = ts
        .put_basal_schedule(&sched(vec![
            slot("s1", 360, 2_000),
            slot("s2", 720, 2_000),
            slot("s3", 1080, 2_000),
        ]))
        .unwrap();
    assert_eq!(grown.slots.len(), 3);

    // A queued retry of the superseded one-slot revision arrives late. Its slot
    // upsert no-ops on the `updated_at` guard; the replace must not follow
    // through and delete the two slots the phone has since added.
    let echoed = ts
        .put_basal_schedule(&sched(vec![slot("s1", 360, 1_000)]))
        .unwrap();
    assert_eq!(echoed.slots.len(), 3, "stale retry pruned newer slots");

    let live = ts.get_basal_schedule().unwrap().unwrap();
    assert_eq!(live.schedule_id, "sched-only");
    assert_eq!(live.slots.len(), 3);
    assert!(live.active);
}

#[test]
fn basal_stale_replay_cannot_resurrect_a_slot_the_newer_put_deleted() {
    let ts = TempStore::new();

    let slot = |cid: &str, tod: i32, dose_u: f64, updated_at: i64| BasalSlot {
        client_id: cid.into(),
        label: format!("slot-{tod}"),
        time_of_day_min: tod,
        dose_u,
        duration_min: 1440.0,
        ka_per_hour: 0.30,
        ke_per_hour: 0.07,
        tz_offset: 0,
        updated_at,
    };
    let sched = |slots: Vec<BasalSlot>| BasalSchedule {
        schedule_id: "sched-S".into(),
        active: true,
        slots,
    };

    // Two slots at revision 1000; the user then deletes B and the revision-2000
    // PUT (carrying A alone) drains first, pruning B.
    ts.put_basal_schedule(&sched(vec![
        slot("A", 360, 8.0, 1_000),
        slot("B", 1320, 6.0, 1_000),
    ]))
    .unwrap();
    let pruned = ts
        .put_basal_schedule(&sched(vec![slot("A", 360, 8.0, 2_000)]))
        .unwrap();
    assert_eq!(pruned.slots.len(), 1);

    // The backed-off revision-1000 PUT now redelivers. B no longer exists, so its
    // upsert is an unguarded fresh INSERT — which must NOT be read as "this write
    // landed, therefore it is current" and re-prune the schedule to the stale set.
    let echoed = ts
        .put_basal_schedule(&sched(vec![
            slot("A", 360, 8.0, 1_000),
            slot("B", 1320, 6.0, 1_000),
        ]))
        .unwrap();
    assert_eq!(echoed.slots.len(), 1, "stale replay resurrected a deleted slot");

    let live = ts.get_basal_schedule().unwrap().unwrap();
    assert_eq!(live.slots.len(), 1);
    assert_eq!(live.slots[0].client_id, "A");
    assert_eq!(live.slots[0].updated_at, 2_000);
    approx(live.slots[0].dose_u, 8.0);
}

// ------------------------------------------------- display reconstruction

#[test]
fn parametric_meal_gamma_is_derived_from_its_glycaemic_index() {
    let ts = TempStore::new();

    // A parametric meal with neither k/theta nor gi: the phone resolves its gamma
    // through carbGammaForGi(DEFAULT_GI = 50) ⇒ (k 3.25, θ 22.5), peak (k-1)·θ =
    // 50.6 min — NOT the invented (3.0, 20.0) whose peak is 40 min.
    let meal = MealEvent {
        client_id: "meal-gi".into(),
        ts: grid_ts(0),
        tz_offset: 0,
        updated_at: 1_000,
        grams: 60.0,
        duration_min: 240.0,
        gi: None,
        k: None,
        theta: None,
        custom_curve: None,
        note: None,
    };
    ts.put_meals(std::slice::from_ref(&meal)).unwrap();

    let ch = ts.reconstruct_channels(grid_ts(0), grid_ts(60)).unwrap();
    let total: f64 = ch.carb.iter().map(|(_, v)| v).sum();
    approx(total, 60.0);
    let (peak_ts, peak_v) = ch
        .carb
        .iter()
        .cloned()
        .fold((0i64, f64::MIN), |acc, p| if p.1 > acc.1 { p } else { acc });
    assert_eq!(peak_ts, grid_ts(9), "carb appearance must peak at 50 min");
    assert!((peak_v - 3.4246).abs() < 1e-3, "peak Ra {peak_v} g/step");

    // An explicit GI overrides the default: GI 100 ⇒ (k 2.0, θ 15.0), peak 15 min.
    let fast_store = TempStore::new();
    let fast = MealEvent {
        client_id: "meal-fast".into(),
        gi: Some(100.0),
        ..meal.clone()
    };
    fast_store.put_meals(std::slice::from_ref(&fast)).unwrap();
    let ch = fast_store
        .reconstruct_channels(grid_ts(0), grid_ts(60))
        .unwrap();
    let peak = ch
        .carb
        .iter()
        .cloned()
        .fold((0i64, f64::MIN), |acc, p| if p.1 > acc.1 { p } else { acc });
    assert_eq!(peak.0, grid_ts(2), "GI 100 pulls the peak to 15 min");
}

#[test]
fn bolus_without_both_gamma_params_falls_to_the_exp_action_kernel() {
    let ts = TempStore::new();

    let bolus = DoseEvent {
        client_id: "dose-exp".into(),
        ts: grid_ts(0),
        tz_offset: 0,
        updated_at: 1_000,
        kind: DoseKind::Bolus,
        units: 5.0,
        duration_min: 360.0,
        k: None,
        theta: None,
        ka_per_hour: None,
        ke_per_hour: None,
        custom_curve: None,
        note: None,
    };
    ts.put_doses(std::slice::from_ref(&bolus)).unwrap();

    let ch = ts.reconstruct_channels(grid_ts(0), grid_ts(100)).unwrap();
    approx(ch.bolus.iter().map(|(_, v)| v).sum::<f64>(), 5.0);
    let peak = ch
        .bolus
        .iter()
        .cloned()
        .fold((0i64, f64::MIN), |acc, p| if p.1 > acc.1 { p } else { acc });
    assert_eq!(peak.0, grid_ts(14), "exp-action peaks at min(75, 0.4·DIA)");
    // The first hour: 1.172 U under the exp-action kernel, 3.022 U under the
    // gamma the port invented — the divergence that summed to the same 5.0 U.
    let first_hour: f64 = ch
        .bolus
        .iter()
        .filter(|(t, _)| *t <= grid_ts(11))
        .map(|(_, v)| v)
        .sum();
    assert!((first_hour - 1.1718).abs() < 1e-3, "first-hour action {first_hour} U");

    // BOTH params present ⇒ the stored gamma is honoured (peak (k-1)·θ = 50 min).
    let gamma_store = TempStore::new();
    gamma_store
        .put_doses(std::slice::from_ref(&DoseEvent {
            client_id: "dose-gamma".into(),
            k: Some(3.0),
            theta: Some(25.0),
            ..bolus.clone()
        }))
        .unwrap();
    let ch = gamma_store
        .reconstruct_channels(grid_ts(0), grid_ts(100))
        .unwrap();
    let peak = ch
        .bolus
        .iter()
        .cloned()
        .fold((0i64, f64::MIN), |acc, p| if p.1 > acc.1 { p } else { acc });
    assert_eq!(peak.0, grid_ts(9));

    // Only one of the two ⇒ still exp-action, never a half-defaulted gamma.
    let half_store = TempStore::new();
    half_store
        .put_doses(std::slice::from_ref(&DoseEvent {
            client_id: "dose-half".into(),
            k: Some(3.0),
            theta: None,
            ..bolus.clone()
        }))
        .unwrap();
    let ch = half_store
        .reconstruct_channels(grid_ts(0), grid_ts(100))
        .unwrap();
    let peak = ch
        .bolus
        .iter()
        .cloned()
        .fold((0i64, f64::MIN), |acc, p| if p.1 > acc.1 { p } else { acc });
    assert_eq!(peak.0, grid_ts(14), "one of two params must not half-default");
}

// ---------------------------------------------------------------- predictions

#[test]
fn prediction_upserts_on_newer_and_rejects_stale_redelivery() {
    let ts = TempStore::new();

    let pred = PredictionWrite {
        made_at: grid_ts(3),
        model_id: "forecaster.pt".into(),
        updated_at: 1_000,
        horizon_steps: 3,
        line: vec![120.0, 125.0, 130.0],
        fan: vec![vec![110.0, 130.0], vec![112.0, 138.0], vec![115.0, 145.0]],
        circadian: None,
    };
    let ids = ts.put_predictions(std::slice::from_ref(&pred)).unwrap();
    assert_eq!(ids.len(), 1);

    // A queued stale redelivery must not roll the forecast — or its
    // `updated_at` — backwards, and still reports the canonical row id.
    let stale = PredictionWrite {
        updated_at: 500,
        line: vec![999.0, 999.0, 999.0],
        ..pred.clone()
    };
    assert_eq!(ts.put_predictions(std::slice::from_ref(&stale)).unwrap(), ids);
    let got = ts.get_prediction_latest().unwrap().unwrap();
    approx(got.line[0], 120.0);
    assert_eq!(got.updated_at, 1_000);

    // A genuine re-run of the same cycle (newer `updated_at`) replaces in place.
    let newer = PredictionWrite {
        updated_at: 2_000,
        line: vec![140.0, 145.0, 150.0],
        ..pred.clone()
    };
    assert_eq!(ts.put_predictions(std::slice::from_ref(&newer)).unwrap(), ids);
    let got = ts.get_prediction_latest().unwrap().unwrap();
    approx(got.line[0], 140.0);
    assert_eq!(got.updated_at, 2_000);
    assert_eq!(
        ts.get_predictions(None, None).unwrap().len(),
        1,
        "idempotent on (made_at, model_id) — no duplicate row"
    );
}

#[test]
fn prediction_latest_tiebreaks_within_one_cycle_deterministically() {
    let ts = TempStore::new();

    // The phone pushes EVERY running model of a cycle in one batch, so several
    // rows share one `made_at` (the table's identity is (made_at, model_id)).
    // The wire carries no selection flag, so the server cannot pick the model the
    // phone displays — but the pick must not vary between reads.
    let pred = |model_id: &str, updated_at: i64, first: f64| PredictionWrite {
        made_at: grid_ts(3),
        model_id: model_id.into(),
        updated_at,
        horizon_steps: 2,
        line: vec![first, first + 5.0],
        fan: vec![vec![first - 10.0, first + 10.0], vec![first - 12.0, first + 12.0]],
        circadian: None,
    };
    ts.put_predictions(&[
        pred("alpha.pte", 1_000, 100.0),
        pred("beta.pte", 1_000, 200.0),
        pred("gamma.pte", 2_000, 300.0),
    ])
    .unwrap();

    // Newest cycle, then newest write, then newest row: gamma.pte, every time.
    for _ in 0..4 {
        let got = ts.get_prediction_latest().unwrap().unwrap();
        assert_eq!(got.model_id, "gamma.pte");
        approx(got.line[0], 300.0);
    }

    // A newer cycle still wins outright, whatever the within-cycle stamps.
    ts.put_predictions(&[PredictionWrite {
        made_at: grid_ts(4),
        ..pred("alpha.pte", 500, 400.0)
    }])
    .unwrap();
    let got = ts.get_prediction_latest().unwrap().unwrap();
    assert_eq!(got.made_at, grid_ts(4));
    approx(got.line[0], 400.0);
}

// ------------------------------------------------------- phone-pushed stats

#[test]
fn stats_block_roundtrips_verbatim_and_upserts_on_newer() {
    let ts = TempStore::new();
    assert!(ts.get_stats_block("7d").unwrap().is_none());

    // The server stores the phone's block byte-for-byte and never re-derives it.
    let block = json!({
        "window": "7d",
        "tir": 0.72,
        "time_below": 0.05,
        "time_above": 0.23,
        "mean_bg": 141.0,
        "n_samples": 288
    })
    .to_string();
    assert!(
        ts.put_stats_block(StatsWindow::D7, &block, 1_000).unwrap(),
        "the first push lands"
    );
    assert_eq!(
        ts.get_stats_block("7d").unwrap().as_deref(),
        Some(block.as_str())
    );

    // A distinct window is stored independently and leaves 7d untouched.
    let block90 = json!({"window":"90d","tir":0.66}).to_string();
    assert!(ts
        .put_stats_block(StatsWindow::D90, &block90, 1_000)
        .unwrap());
    assert_eq!(
        ts.get_stats_block("90d").unwrap().as_deref(),
        Some(block90.as_str())
    );
    assert_eq!(
        ts.get_stats_block("7d").unwrap().as_deref(),
        Some(block.as_str())
    );

    // Idempotency: a redelivery (equal `updated_at`) is a no-op even with a
    // different body; a stale one is likewise rejected. The rejection is
    // reported back, so the api layer can withhold the fan-out frame.
    let other = json!({"window":"7d","tir":0.0}).to_string();
    assert!(
        !ts.put_stats_block(StatsWindow::D7, &other, 1_000).unwrap(),
        "a redelivery reports that nothing landed"
    );
    assert_eq!(
        ts.get_stats_block("7d").unwrap().as_deref(),
        Some(block.as_str())
    );
    assert!(!ts.put_stats_block(StatsWindow::D7, &other, 500).unwrap());
    assert_eq!(
        ts.get_stats_block("7d").unwrap().as_deref(),
        Some(block.as_str())
    );

    // A newer `updated_at` replaces the block verbatim.
    let newer = json!({"window":"7d","tir":0.80,"n_samples":300}).to_string();
    assert!(ts.put_stats_block(StatsWindow::D7, &newer, 2_000).unwrap());
    assert_eq!(
        ts.get_stats_block("7d").unwrap().as_deref(),
        Some(newer.as_str())
    );
}

// ------------------------------------------------------------- store_epoch

#[test]
fn store_epoch_is_minted_once_and_refreshes_on_wipe() {
    let ts = TempStore::new();

    // Minting is idempotent: one stable identity per store.
    let e1 = ts.ensure_store_epoch().unwrap();
    assert!(!e1.is_empty());
    assert_eq!(ts.ensure_store_epoch().unwrap(), e1, "idempotent mint");
    assert_eq!(ts.store_epoch().unwrap().as_deref(), Some(e1.as_str()));

    // Persisted in `meta`: reopening the same data dir recovers the same epoch.
    {
        let reopened = Store::open_at(&ts.dir).unwrap();
        assert_eq!(reopened.store_epoch().unwrap(), Some(e1.clone()));
    }

    // A teardown wipes `meta`, so the next mint yields a fresh, distinct
    // identity — the signal the phone uses to trigger a full re-mirror (§3.8).
    ts.teardown().unwrap();
    let e2 = ts.ensure_store_epoch().unwrap();
    assert_ne!(e2, e1, "a wiped store is a new identity");
}

// ------------------------------------------------------ synthetic generator

#[test]
fn generate_fake_writes_scalar_samples_and_predictions() {
    let ts = TempStore::new();
    let written = ts
        .generate_fake(FakeRange::last_days(1), FakeOpts::default())
        .unwrap();
    assert!(written > 200, "≈288 grid rows over a day, got {written}");

    let rows = ts
        .get_samples(&Series::ALL, None, None, None, None)
        .unwrap();
    assert_eq!(rows.len(), written, "one row per generated grid step");
    // Every generated row sits on the grid and carries the scalar BG series.
    assert!(rows.iter().all(|r| t1dm_core::on_grid(r.ts)));
    assert!(rows.iter().all(|r| r.bg.is_some()));

    // The synthetic generator also emits forecasts.
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

    // Identity is the ATTACHMENT, `(sha256, ts)`: an exact re-POST is idempotent,
    // but the same saved image attached to a later meal is its own event. The
    // phone mints no photo id and discards the ack, so a collapse here is
    // invisible to it and hangs the photo on the wrong meal.
    let again = ts.add_photo(grid_ts(0), data, 16, 9, "png").unwrap();
    assert_eq!(again.id, p.id, "an exact re-POST stays idempotent");
    assert_eq!(ts.get_photos(None, None).unwrap().len(), 1);

    let later = ts.add_photo(grid_ts(132), data, 16, 9, "png").unwrap();
    assert_ne!(later.id, p.id, "a second attachment is a distinct row");
    assert_eq!(later.ts, grid_ts(132));
    assert_eq!(later.path, p.path, "both rows share the content-addressed file");
    let listed = ts.get_photos(None, None).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].ts, grid_ts(132), "newest first");

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
    // The served `path` is documented as relative to `storage.data_dir`, so the
    // registry never hands a read-only client the appliance's fs layout.
    assert_eq!(m.path, "models/forecaster.pt");
    let abs = ts.model_path("forecaster.pt").unwrap().expect("resolved path");
    assert!(abs.starts_with(&ts.dir), "the download path rejoins data_dir");
    assert_eq!(std::fs::read(&abs).unwrap(), weights);
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

#[test]
fn refresh_models_reads_the_client_descriptor_sidecar() {
    let ts = TempStore::new();
    let dir = ts.models_dir();
    // The phone writes and consumes `<name>.descriptor.json`, so a descriptor
    // carried over from its models directory must be found — otherwise the row is
    // served with `meta: null` and the phone skips the artifact entirely.
    std::fs::write(dir.join("large-real.xnnpack.pte"), b"executorch").unwrap();
    std::fs::write(
        dir.join("large-real.xnnpack.descriptor.json"),
        json!({"engine":"executorch","normalization_stats":{"bg_mean":140.0}}).to_string(),
    )
    .unwrap();

    let models = ts.refresh_models(&dir).unwrap();
    assert_eq!(models.len(), 1, "the descriptor sidecar is not an artifact");
    assert_eq!(models[0].id, "large-real.xnnpack.pte");
    assert_eq!(
        models[0].meta,
        json!({"engine":"executorch","normalization_stats":{"bg_mean":140.0}})
    );

    // The server's own `<stem>.json` spelling still resolves, and the client
    // spelling wins when both are present.
    std::fs::write(dir.join("plain.pte"), b"weights").unwrap();
    std::fs::write(dir.join("plain.json"), json!({"src":"server"}).to_string()).unwrap();
    std::fs::write(dir.join("both.pte"), b"weights").unwrap();
    std::fs::write(dir.join("both.json"), json!({"src":"server"}).to_string()).unwrap();
    std::fs::write(
        dir.join("both.descriptor.json"),
        json!({"src":"client"}).to_string(),
    )
    .unwrap();

    let models = ts.refresh_models(&dir).unwrap();
    assert_eq!(models.len(), 3);
    let meta_of = |id: &str| models.iter().find(|m| m.id == id).unwrap().meta.clone();
    assert_eq!(meta_of("plain.pte"), json!({"src":"server"}));
    assert_eq!(meta_of("both.pte"), json!({"src":"client"}));
}

#[test]
fn refresh_models_prunes_rows_whose_artifact_is_gone() {
    let ts = TempStore::new();
    let dir = ts.models_dir();
    std::fs::write(dir.join("old.pte"), b"retired").unwrap();
    std::fs::write(dir.join("old.json"), json!({"arch":"risk-v2"}).to_string()).unwrap();
    std::fs::write(dir.join("new.pte"), b"current").unwrap();
    assert_eq!(ts.refresh_models(&dir).unwrap().len(), 2);

    // Retiring a model is a file deletion; the registry must mirror it, or the
    // row keeps advertising a /download that 404s.
    std::fs::remove_file(dir.join("old.pte")).unwrap();
    std::fs::remove_file(dir.join("old.json")).unwrap();
    let models = ts.refresh_models(&dir).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "new.pte");
    assert!(ts.model_path("old.pte").unwrap().is_none());
    assert!(ts.get_model_meta("old.pte").unwrap().is_none());
}

#[test]
fn refresh_models_leaves_registry_alone_when_dir_is_absent() {
    let ts = TempStore::new();
    let dir = ts.models_dir();
    std::fs::write(dir.join("net.pte"), b"weights").unwrap();
    assert_eq!(ts.refresh_models(&dir).unwrap().len(), 1);

    // An unmounted / not-yet-created models dir must not be read as "every
    // model was retired".
    assert_eq!(ts.refresh_models(&dir.join("not-here")).unwrap().len(), 1);
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
