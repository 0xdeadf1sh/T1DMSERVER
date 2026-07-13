//! Phone-pushed windowed statistics blocks.
//!
//! Decision #6: the server no longer computes glycemic statistics. The phone
//! is the sole author; it pushes a self-contained stats block per window
//! (`7d`/`30d`/`90d`) which the server stores and serves back verbatim — never
//! re-derived, never re-stamped. The phone's `updated_at` is kept exactly as
//! sent; `received_at` is the server's internal arrival clock and never
//! crosses the wire.

use rusqlite::OptionalExtension;

use crate::error::Result;
use crate::{now_ms, Store};

impl Store {
    /// Upsert the phone-pushed stats block for `window`, storing `json`
    /// verbatim.
    ///
    /// Idempotent on `window` and monotone in the phone clock: a byte-identical
    /// redelivery (equal `updated_at`) or a reordered stale redelivery (older
    /// `updated_at`) is a no-op; a genuine phone-side edit (newer `updated_at`)
    /// replaces in place. `updated_at` is stored exactly as sent; `received_at`
    /// is the server's internal arrival stamp and is not serialized on any wire
    /// shape.
    pub fn put_stats_block(&self, window: &str, json: &str, updated_at: i64) -> Result<()> {
        let received_at = now_ms();
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO stats_block(window, json, updated_at, received_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(window) DO UPDATE SET
                    json        = excluded.json,
                    updated_at  = excluded.updated_at,
                    received_at = excluded.received_at
                 WHERE excluded.updated_at > stats_block.updated_at",
                (window, json, updated_at, received_at),
            )?;
            Ok(())
        })
    }

    /// Return the most recently pushed stats block JSON for `window`, verbatim,
    /// or `None` when the phone has pushed none for it. The server never
    /// synthesizes a block; an all-zero default is the caller's concern.
    pub fn get_stats_block(&self, window: &str) -> Result<Option<String>> {
        self.with_reader(|conn| {
            Ok(conn
                .query_row(
                    "SELECT json FROM stats_block WHERE window = ?1",
                    [window],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })
    }
}
