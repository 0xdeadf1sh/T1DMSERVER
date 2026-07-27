//! Schema DDL and the migration runner. The whole schema is one migration
//! for now.

use rusqlite::Connection;

use crate::error::{Result, StoreError};

/// PRAGMAs applied to every connection (writer and pooled readers).
pub const CONNECTION_PRAGMAS: &str = "\
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
";

/// Version 1: the complete schema — the single clean-break migration. Demoted
/// scalar `samples` grid, first-class curve-event tables (meal/dose/basal),
/// phone-pushed stats blocks, and all side tables.
const MIGRATION_V1: &str = r#"
-- (#5) demoted scalars; carbs/bolus/basal GONE (they are events)
CREATE TABLE IF NOT EXISTS samples (
    ts          INTEGER PRIMARY KEY,          -- epoch ms, == 0 mod 300000
    tz_offset   INTEGER NOT NULL DEFAULT 0,
    bg          REAL, hr REAL, steps REAL, sleep REAL, exercise REAL,
    mood        INTEGER,
    updated_at  INTEGER NOT NULL,             -- (#2) CLIENT clock, VERBATIM
    received_at INTEGER NOT NULL              -- (#2) SERVER clock, INTERNAL only
);

-- (#4) a meal is a curve, keyed by phone id
CREATE TABLE IF NOT EXISTS meal_event (
    id           INTEGER PRIMARY KEY,
    client_id    TEXT    NOT NULL,
    ts           INTEGER NOT NULL,            -- grid-snapped (#1)
    tz_offset    INTEGER NOT NULL DEFAULT 0,
    grams        REAL    NOT NULL,
    gi REAL, k REAL, theta REAL,
    duration_min REAL    NOT NULL,
    custom_curve TEXT,                         -- JSON f64[]: the resolved (mixed/builder) appearance curve, or NULL for a parametric meal
    note         TEXT,
    updated_at   INTEGER NOT NULL,
    received_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS meal_event_client ON meal_event(client_id);
CREATE INDEX        IF NOT EXISTS meal_event_ts     ON meal_event(ts);

-- (#4) a dose is a curve: bolus (gamma) or basal (Bateman), keyed by phone id
CREATE TABLE IF NOT EXISTS dose_event (
    id            INTEGER PRIMARY KEY,
    client_id     TEXT    NOT NULL,
    ts            INTEGER NOT NULL,
    tz_offset     INTEGER NOT NULL DEFAULT 0,
    kind          TEXT    NOT NULL CHECK(kind IN ('bolus','basal')),
    units         REAL    NOT NULL,
    duration_min  REAL    NOT NULL,
    k REAL, theta REAL, ka_per_hour REAL, ke_per_hour REAL,
    custom_curve  TEXT, note TEXT,
    updated_at    INTEGER NOT NULL,
    received_at   INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS dose_event_client ON dose_event(client_id);
CREATE INDEX        IF NOT EXISTS dose_event_ts     ON dose_event(ts);

-- (#4) daily-repeating basal template; TUI tiles it. Full-replace on PUT.
CREATE TABLE IF NOT EXISTS basal_schedule_dose (
    id              INTEGER PRIMARY KEY,
    client_id       TEXT    NOT NULL,
    schedule_id     TEXT    NOT NULL,
    label           TEXT    NOT NULL,
    time_of_day_min INTEGER NOT NULL,
    dose_u          REAL    NOT NULL,
    duration_min    REAL    NOT NULL,
    ka_per_hour     REAL    NOT NULL,
    ke_per_hour     REAL    NOT NULL,
    tz_offset       INTEGER NOT NULL DEFAULT 0,
    active          INTEGER NOT NULL,          -- 1 for the live schedule
    updated_at      INTEGER NOT NULL,
    received_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS basal_sched_client ON basal_schedule_dose(client_id);
CREATE INDEX        IF NOT EXISTS basal_sched_sid    ON basal_schedule_dose(schedule_id);
CREATE INDEX        IF NOT EXISTS basal_sched_active ON basal_schedule_dose(active);

-- (#7,#17) UNIQUE(made_at,model_id); real circadian; client made_at verbatim
CREATE TABLE IF NOT EXISTS prediction (
    id            INTEGER PRIMARY KEY,
    made_at       INTEGER NOT NULL,            -- CLIENT cycle ts, VERBATIM (was server now_ms)
    model_id      TEXT    NOT NULL,
    horizon_steps INTEGER NOT NULL,
    line          TEXT    NOT NULL,            -- JSON f64[]
    fan           TEXT    NOT NULL,            -- JSON 7 × horizon (QUANTILE_LEVELS order)
    circadian     TEXT,                         -- JSON {probs,predicted_hour,resultant_r,n_bins,bin_hours} or NULL
    updated_at    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL             -- INTERNAL server stamp
);
CREATE UNIQUE INDEX IF NOT EXISTS prediction_ident   ON prediction(made_at, model_id);
CREATE INDEX        IF NOT EXISTS prediction_made_at ON prediction(made_at);

-- (#6) phone-pushed, served verbatim (replaces stats_cache)
CREATE TABLE IF NOT EXISTS stats_block (
    window      TEXT PRIMARY KEY,             -- '7d'/'30d'/'90d'
    json        TEXT    NOT NULL,             -- phone Stats block, VERBATIM
    updated_at  INTEGER NOT NULL,
    received_at INTEGER NOT NULL
);

-- (#7) note/alert gain client_id; note gains updated_at (editable)
CREATE TABLE IF NOT EXISTS note (
    id INTEGER PRIMARY KEY, client_id TEXT NOT NULL,
    ts INTEGER NOT NULL, tz_offset INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL, updated_at INTEGER NOT NULL, created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS note_client ON note(client_id);
CREATE INDEX        IF NOT EXISTS note_ts     ON note(ts);

CREATE TABLE IF NOT EXISTS photo (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    path       TEXT NOT NULL,
    sha256     TEXT NOT NULL,
    width      INTEGER NOT NULL,
    height     INTEGER NOT NULL,
    bytes      INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS photo_ts ON photo(ts);

CREATE TABLE IF NOT EXISTS alert (
    id INTEGER PRIMARY KEY, client_id TEXT NOT NULL,
    ts INTEGER NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
    origin_token INTEGER, created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS alert_client ON alert(client_id);
CREATE INDEX        IF NOT EXISTS alert_ts     ON alert(ts);

CREATE TABLE IF NOT EXISTS token (
    id          INTEGER PRIMARY KEY,
    secret_hash TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK(kind IN ('rw','ro')),
    label       TEXT,
    created_at  INTEGER NOT NULL,
    revoked_at  INTEGER
);
-- At most one live RW token.
CREATE UNIQUE INDEX IF NOT EXISTS one_rw
    ON token(kind) WHERE kind = 'rw' AND revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS session (
    id         INTEGER PRIMARY KEY,
    token_id   INTEGER NOT NULL REFERENCES token(id),
    ip         TEXT NOT NULL,
    user_agent TEXT NOT NULL,
    device     TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS session_token ON session(token_id);
CREATE UNIQUE INDEX IF NOT EXISTS session_ident
    ON session(token_id, ip, device);

CREATE TABLE IF NOT EXISTS model (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    path          TEXT NOT NULL,
    meta          TEXT NOT NULL,             -- OPAQUE JSON, never interpreted
    sha256        TEXT NOT NULL,
    bytes         INTEGER NOT NULL,
    discovered_at INTEGER NOT NULL
);

-- (H7) store identity kv. A random `store_epoch` minted on fresh create and
-- surfaced in GET /v1/health; when it differs from the phone's persisted value
-- (a wiped/replaced server), the phone runs a full authoritative re-mirror.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);
"#;

/// The highest migration version defined by this schema.
pub const LATEST_VERSION: i64 = 1;

/// All migrations in ascending version order.
const MIGRATIONS: &[(i64, &str)] = &[(1, MIGRATION_V1)];

/// Run all outstanding migrations against `conn`. Idempotent. Afterwards the
/// recorded head must have reached [`LATEST_VERSION`] — a bumped constant with
/// no matching entry in `MIGRATIONS` would otherwise leave the store silently
/// short of the schema every caller assumes.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let mut head = current;
    for &(version, sql) in MIGRATIONS {
        if current < version {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                (version, crate::now_ms()),
            )?;
        }
        head = head.max(version);
    }

    if head < LATEST_VERSION {
        return Err(StoreError::Invalid(format!(
            "schema head {head} short of LATEST_VERSION {LATEST_VERSION}: a migration is missing"
        )));
    }
    Ok(())
}

/// Drop every table (teardown), including `schema_migrations` — so the caller's
/// immediate re-`migrate` rebuilds the whole schema from version 0. Children are
/// listed before parents (`session` before `token`) and the caller disables
/// foreign-key enforcement for the batch, so the implicit per-drop DELETE can't
/// abort on a dangling reference. Dropping `meta` too makes teardown mint a
/// fresh `store_epoch` on re-`migrate`, matching a from-scratch data dir (H7).
pub const DROP_ALL: &str = r#"
DROP TABLE IF EXISTS samples;
DROP TABLE IF EXISTS meal_event;
DROP TABLE IF EXISTS dose_event;
DROP TABLE IF EXISTS basal_schedule_dose;
DROP TABLE IF EXISTS prediction;
DROP TABLE IF EXISTS stats_block;
DROP TABLE IF EXISTS note;
DROP TABLE IF EXISTS photo;
DROP TABLE IF EXISTS alert;
DROP TABLE IF EXISTS session;
DROP TABLE IF EXISTS token;
DROP TABLE IF EXISTS model;
DROP TABLE IF EXISTS meta;
DROP TABLE IF EXISTS schema_migrations;
"#;
