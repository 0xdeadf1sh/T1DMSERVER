//! t1dm-store — the single in-process SQLite store.
//!
//! One serialized writer (a `Mutex<Connection>`, since SQLite admits one
//! writer) plus an r2d2 read pool over the same WAL database. All public
//! surface is defined here and in the sibling `impl Store` modules; bodies
//! may be minimal but are never panics.

mod error;
mod fake;
mod models;
mod reads;
mod schema;
mod stats;
mod tokens;
mod writes;

#[cfg(test)]
mod tests;

pub use error::{Result, StoreError};
pub use fake::{FakeOpts, FakeRange};
pub use stats::STATS_CACHE_TTL_MS;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

use t1dm_core::Config;

/// Read connection pool type alias.
pub type ReadPool = Pool<SqliteConnectionManager>;

/// Milliseconds since the Unix epoch, monotonic-enough for timestamps.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

struct StoreInner {
    /// The single serialized writer.
    writer: Mutex<Connection>,
    /// WAL read pool for concurrent readers (used under spawn_blocking).
    pool: ReadPool,
    /// Root data directory (`data_dir`), holding db/models/photos/backups.
    data_dir: PathBuf,
}

/// The shared store handle. Cheap to clone (an `Arc`); hand one clone to the
/// api layer and another to the TUI.
#[derive(Clone)]
pub struct Store {
    inner: Arc<StoreInner>,
}

impl Store {
    /// Open (creating if needed) the store described by `cfg`, ensuring the
    /// data directory layout exists and migrations are applied.
    pub fn open(cfg: &Config) -> Result<Store> {
        let data_dir = PathBuf::from(&cfg.storage.data_dir);
        Self::open_at(&data_dir)
    }

    /// Open the store rooted at `data_dir`, independent of a full [`Config`].
    pub fn open_at(data_dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(data_dir)?;
        std::fs::create_dir_all(data_dir.join("models"))?;
        std::fs::create_dir_all(data_dir.join("photos"))?;
        std::fs::create_dir_all(data_dir.join("backups"))?;

        let db_path = data_dir.join("t1dm.db");

        let writer = Connection::open(&db_path)?;
        writer.execute_batch(schema::CONNECTION_PRAGMAS)?;
        schema::migrate(&writer)?;

        let manager = SqliteConnectionManager::file(&db_path)
            .with_init(|c| c.execute_batch(schema::CONNECTION_PRAGMAS));
        let pool = Pool::builder().max_size(8).build(manager)?;

        Ok(Store {
            inner: Arc::new(StoreInner {
                writer: Mutex::new(writer),
                pool,
                data_dir: data_dir.to_path_buf(),
            }),
        })
    }

    /// Root data directory.
    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    /// Path of the models directory.
    pub fn models_dir(&self) -> PathBuf {
        self.inner.data_dir.join("models")
    }

    /// Path of the photos directory.
    pub fn photos_dir(&self) -> PathBuf {
        self.inner.data_dir.join("photos")
    }

    /// Run a closure with the exclusive writer connection.
    pub(crate) fn with_writer<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.inner.writer.lock().unwrap_or_else(|p| p.into_inner());
        f(&conn)
    }

    /// Run a closure with a pooled read connection.
    pub(crate) fn with_reader<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.inner.pool.get()?;
        f(&conn)
    }

    /// Re-run migrations (idempotent; exposed for callers that want an
    /// explicit ensure-schema step).
    pub fn migrate(&self) -> Result<()> {
        self.with_writer(schema::migrate)
    }

    /// Teardown: drop and recreate every table, and clear the photos
    /// directory. The models directory is left untouched.
    pub fn teardown(&self) -> Result<()> {
        self.with_writer(|c| {
            // Drop with FK enforcement off: DROP TABLE performs an implicit
            // DELETE, so dropping a parent (`token`) while a child (`session`)
            // still references it would otherwise abort the batch mid-way and
            // leave the schema half-torn. CONNECTION_PRAGMAS restores it to ON.
            c.execute_batch("PRAGMA foreign_keys = OFF;")?;
            c.execute_batch(schema::DROP_ALL)?;
            c.execute_batch(schema::CONNECTION_PRAGMAS)?;
            schema::migrate(c)?;
            Ok(())
        })?;

        let photos = self.photos_dir();
        if photos.exists() {
            for entry in std::fs::read_dir(&photos)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        // Models are preserved on teardown, but the freshly recreated `model`
        // table is empty and the fs-watcher won't fire for unchanged files —
        // rescan so the surviving artifacts stay registered.
        let _ = self.refresh_models(&self.models_dir());
        Ok(())
    }
}
