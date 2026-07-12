//! Schema DDL and the migration runner. The whole schema is one migration
//! for now; add higher-versioned steps below and bump [`LATEST_VERSION`].

use rusqlite::Connection;

use crate::error::Result;

/// The current head schema version.
pub const LATEST_VERSION: i64 = 2;

/// PRAGMAs applied to every connection (writer and pooled readers).
pub const CONNECTION_PRAGMAS: &str = "\
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
";

/// Version 1: the complete schema. Wide `samples` grid + all side tables.
const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS samples (
    ts         INTEGER PRIMARY KEY,          -- epoch ms, == 0 mod 300000
    tz_offset  INTEGER NOT NULL DEFAULT 0,   -- minutes east of UTC (e.g. -300 = UTC-5)
    bg         REAL,                         -- mg/dL
    carbs      REAL,                         -- grams ingested this 5-min bucket
    bolus      REAL,                         -- units delivered this 5-min bucket
    basal      REAL,                         -- units delivered this 5-min bucket
    hr         REAL,                         -- bpm
    steps      REAL,
    sleep      REAL,
    exercise   REAL,
    mood       INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS prediction (
    id            INTEGER PRIMARY KEY,
    made_at       INTEGER NOT NULL,
    model_id      TEXT NOT NULL,
    horizon_steps INTEGER NOT NULL,
    line          TEXT NOT NULL,             -- JSON f64[]
    fan           TEXT NOT NULL,             -- JSON 7 x horizon matrix
    tod           TEXT NOT NULL,             -- JSON f64[12]
    tod_conf      REAL NOT NULL,
    created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS prediction_made_at ON prediction(made_at);

CREATE TABLE IF NOT EXISTS note (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    tz_offset  INTEGER NOT NULL DEFAULT 0,
    text       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS note_ts ON note(ts);

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
    id           INTEGER PRIMARY KEY,
    ts           INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL,              -- JSON
    origin_token INTEGER,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS alert_ts ON alert(ts);

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
"#;

/// Version 2 (additive): the daily statistics cache. One row per window,
/// holding the last computed [`t1dm_core::Stats`] block as JSON.
const MIGRATION_V2: &str = r#"
CREATE TABLE IF NOT EXISTS stats_cache (
    window      TEXT PRIMARY KEY,          -- StatsWindow::as_str (7d/30d/90d)
    computed_at INTEGER NOT NULL,          -- epoch ms of the compute
    json        TEXT NOT NULL              -- serialized Stats
);
"#;

/// All migrations in ascending version order.
const MIGRATIONS: &[(i64, &str)] = &[(1, MIGRATION_V1), (2, MIGRATION_V2)];

/// Run all outstanding migrations against `conn`. Idempotent.
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

    for &(version, sql) in MIGRATIONS {
        if current < version {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                (version, crate::now_ms()),
            )?;
        }
    }

    Ok(())
}

/// Drop every data table (teardown). `schema_migrations` is preserved so the
/// recreate path re-runs cleanly; callers immediately re-`migrate`.
pub const DROP_ALL: &str = r#"
DROP TABLE IF EXISTS samples;
DROP TABLE IF EXISTS prediction;
DROP TABLE IF EXISTS note;
DROP TABLE IF EXISTS photo;
DROP TABLE IF EXISTS alert;
DROP TABLE IF EXISTS token;
DROP TABLE IF EXISTS session;
DROP TABLE IF EXISTS model;
DROP TABLE IF EXISTS stats_cache;
DROP TABLE IF EXISTS schema_migrations;
"#;
