//! Write path — all mutations flow through the single serialized writer.

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use t1dm_core::{Alert, IngestBundle, Photo, PredictionWrite};

use crate::error::{Result, StoreError};
use crate::{now_ms, Store};

/// Upsert SQL for one demoted-scalar sample row. Carbs/bolus/basal are no longer
/// columns here (they are first-class curve events), so the row carries only the
/// six scalars. COALESCE preserves any column the bundle left absent (a partial
/// fill), `updated_at` is the phone clock stored verbatim, `received_at` is the
/// server's internal arrival stamp. The `WHERE excluded.updated_at >= …` guard
/// makes a stale redelivery a no-op while still admitting multiple partial fills
/// that share one bucket-tick's `updated_at` (`>=`, not `>`).
const INGEST_UPSERT: &str = r#"
INSERT INTO samples (ts, tz_offset, bg, hr, steps, sleep, exercise, mood, updated_at, received_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(ts) DO UPDATE SET
    tz_offset   = excluded.tz_offset,
    bg          = COALESCE(excluded.bg, samples.bg),
    hr          = COALESCE(excluded.hr, samples.hr),
    steps       = COALESCE(excluded.steps, samples.steps),
    sleep       = COALESCE(excluded.sleep, samples.sleep),
    exercise    = COALESCE(excluded.exercise, samples.exercise),
    mood        = COALESCE(excluded.mood, samples.mood),
    updated_at  = excluded.updated_at,
    received_at = excluded.received_at
WHERE excluded.updated_at >= samples.updated_at
"#;

impl Store {
    /// Ingest one atomic 5-minute bundle: a single demoted-scalar sample row.
    /// Carbs/bolus/basal are first-class curve events and predictions have their
    /// own endpoint, so a bundle is now one guarded upsert (no embedded
    /// records, no enclosing transaction needed). `updated_at` is stored verbatim
    /// (the phone clock, never re-stamped); `received_at` is the server's
    /// internal arrival stamp and never crosses the wire.
    pub fn ingest_bundle(&self, bundle: &IngestBundle) -> Result<()> {
        if !t1dm_core::on_grid(bundle.ts) {
            return Err(StoreError::OffGrid(bundle.ts));
        }
        let received_at = now_ms();
        self.with_writer(|conn| {
            conn.execute(
                INGEST_UPSERT,
                params![
                    bundle.ts,
                    bundle.tz_offset,
                    bundle.bg,
                    bundle.hr,
                    bundle.steps,
                    bundle.sleep,
                    bundle.exercise,
                    bundle.mood,
                    bundle.updated_at, // (#2) phone clock, VERBATIM
                    received_at,       // (#2) server clock, INTERNAL only
                ],
            )?;
            Ok(())
        })
    }

    /// Persist a batch of predictions, idempotent on `(made_at, model_id)`;
    /// returns the canonical row ids. A re-run of the same cycle overwrites with
    /// the newer forecast; a byte-identical redelivery is a no-op in effect.
    pub fn put_predictions(&self, preds: &[PredictionWrite]) -> Result<Vec<i64>> {
        let now = now_ms();
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut ids = Vec::with_capacity(preds.len());
            for pred in preds {
                ids.push(insert_prediction(&tx, pred, now)?);
            }
            tx.commit()?;
            Ok(ids)
        })
    }

    /// Store a meal photo: write the binary under `photos/<sha>.<ext>` and record
    /// its metadata. `width`/`height` are supplied by the decoder. A photo's
    /// identity is the ATTACHMENT — `(sha256, ts)`, not the content hash alone —
    /// because the phone mints no id for it and re-attaching one saved image to a
    /// later meal is a genuinely distinct event. Idempotent on that pair (#7): an
    /// exact re-POST returns the canonical existing row rather than inserting a
    /// duplicate, while the same bytes at another `ts` get their own row sharing
    /// the content-addressed file. The `photo` table carries no UNIQUE index, so
    /// this guards by SELECT rather than `ON CONFLICT`.
    pub fn add_photo(
        &self,
        ts: i64,
        data: &[u8],
        width: i64,
        height: i64,
        ext: &str,
    ) -> Result<Photo> {
        let sha = sha256_hex(data);
        let ext = safe_ext(ext);
        let file_name = format!("{sha}.{ext}");
        let full = self.photos_dir().join(&file_name);
        // Content-addressed name ⇒ re-writing identical bytes is harmless.
        std::fs::write(&full, data)?;
        let now = now_ms();
        let rel = format!("photos/{file_name}");
        self.with_writer(|conn| {
            if let Some(existing) = conn
                .query_row(
                    "SELECT id, ts, path, sha256, width, height, bytes, created_at
                     FROM photo WHERE sha256 = ?1 AND ts = ?2",
                    params![sha, ts],
                    |r| {
                        Ok(Photo {
                            id: r.get(0)?,
                            ts: r.get(1)?,
                            path: r.get(2)?,
                            sha256: r.get(3)?,
                            width: r.get(4)?,
                            height: r.get(5)?,
                            bytes: r.get(6)?,
                            created_at: r.get(7)?,
                        })
                    },
                )
                .optional()?
            {
                return Ok(existing);
            }
            conn.execute(
                "INSERT INTO photo(ts, path, sha256, width, height, bytes, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![ts, rel, sha, width, height, data.len() as i64, now],
            )?;
            Ok(Photo {
                id: conn.last_insert_rowid(),
                ts,
                path: rel,
                sha256: sha,
                width,
                height,
                bytes: data.len() as i64,
                created_at: now,
            })
        })
    }

    /// Record an application-originated alert, keyed by the phone's `client_id`.
    /// Alerts are immutable: a redelivery is a no-op and returns the canonical
    /// first-written row. `origin_token` is the minting caller's token id,
    /// excluded from the WS broadcast fan-out by the api hub.
    pub fn add_alert(
        &self,
        client_id: &str,
        ts: i64,
        kind: &str,
        payload: &Value,
        origin_token: Option<i64>,
    ) -> Result<Alert> {
        let now = now_ms();
        let payload_txt = serde_json::to_string(payload)?;
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO alert(client_id, ts, kind, payload, origin_token, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(client_id) DO NOTHING",
                params![client_id, ts, kind, payload_txt, origin_token, now],
            )?;
            // Immutable upsert may no-op; return the canonical stored row, never
            // last_insert_rowid() (0 on a no-op).
            let alert = conn.query_row(
                "SELECT id, client_id, ts, kind, payload, origin_token, created_at
                 FROM alert WHERE client_id = ?1",
                params![client_id],
                |r| {
                    let payload_txt: String = r.get(4)?;
                    let payload =
                        serde_json::from_str(&payload_txt).unwrap_or(serde_json::Value::Null);
                    Ok(Alert {
                        id: r.get(0)?,
                        client_id: r.get(1)?,
                        ts: r.get(2)?,
                        kind: r.get(3)?,
                        payload,
                        origin_token: r.get(5)?,
                        created_at: r.get(6)?,
                    })
                },
            )?;
            Ok(alert)
        })
    }

    /// Mint (once) and return this store's `store_epoch` — a random identity
    /// token written to the `meta` table on a freshly created schema. Idempotent:
    /// an existing epoch is returned unchanged, so calling it on every open is
    /// safe; only a wiped `meta` (teardown drops it) yields a fresh one. Surfaced
    /// in `GET /v1/health` so the phone can detect a replaced/wiped server and
    /// re-mirror its authoritative history (§3.8, H7).
    ///
    /// NOTE (cross-file, flagged in the return): `Store::open`/`open_at`
    /// (`lib.rs`) must call this after `schema::migrate`, and `teardown()` after
    /// its internal re-migrate, so the epoch actually gets minted — this method
    /// only defines the write; nothing in `writes.rs` invokes it.
    pub fn ensure_store_epoch(&self) -> Result<String> {
        self.with_writer(|conn| {
            if let Some(epoch) = conn
                .query_row(
                    "SELECT value FROM meta WHERE key = 'store_epoch'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(epoch);
            }
            let epoch = mint_store_epoch_value();
            // OR IGNORE keeps the first writer's value should two opens race.
            conn.execute(
                "INSERT OR IGNORE INTO meta(key, value) VALUES ('store_epoch', ?1)",
                params![epoch],
            )?;
            let epoch: String = conn.query_row(
                "SELECT value FROM meta WHERE key = 'store_epoch'",
                [],
                |r| r.get(0),
            )?;
            Ok(epoch)
        })
    }
}

/// Insert or replace one prediction row within a transaction, idempotent on
/// `(made_at, model_id)`; returns the canonical row id. `made_at` is the phone's
/// cycle timestamp, stored verbatim (never the server clock); the mutable
/// forecast columns are overwritten on a re-run carrying a newer `updated_at`
/// while the internal `created_at` stamp is preserved. The circadian belief is
/// stored as its JSON object, or SQL NULL when the model has no time head.
fn insert_prediction(
    conn: &rusqlite::Connection,
    pred: &PredictionWrite,
    now: i64,
) -> Result<i64> {
    let line = serde_json::to_string(&pred.line)?;
    let fan = serde_json::to_string(&pred.fan)?;
    let circadian = pred
        .circadian
        .as_ref()
        .map(|c| serde_json::to_string(c))
        .transpose()?;
    conn.execute(
        "INSERT INTO prediction(made_at, model_id, horizon_steps, line, fan, circadian, updated_at, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(made_at, model_id) DO UPDATE SET
            horizon_steps = excluded.horizon_steps,
            line          = excluded.line,
            fan           = excluded.fan,
            circadian     = excluded.circadian,
            updated_at    = excluded.updated_at
         WHERE excluded.updated_at > prediction.updated_at",
        params![
            pred.made_at,
            pred.model_id,
            pred.horizon_steps,
            line,
            fan,
            circadian,
            pred.updated_at,
            now,
        ],
    )?;
    // last_insert_rowid() is stale on the DO UPDATE path; fetch the canonical id.
    let id: i64 = conn.query_row(
        "SELECT id FROM prediction WHERE made_at = ?1 AND model_id = ?2",
        params![pred.made_at, pred.model_id],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Mint a fresh, effectively-unique `store_epoch` as a 32-char hex string. The
/// RNG is seeded from the high-resolution clock XOR a per-process counter, so two
/// mints in the same nanosecond (a teardown immediately followed by re-open in a
/// test) still diverge.
fn mint_store_epoch_value() -> String {
    use rand::rngs::SmallRng;
    use rand::{RngExt, SeedableRng};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut rng = SmallRng::seed_from_u64(nanos ^ bump.rotate_left(32));
    let a: u64 = rng.random_range(0..u64::MAX);
    let b: u64 = rng.random_range(0..u64::MAX);
    format!("{a:016x}{b:016x}")
}

/// Map a client-supplied filename extension onto a fixed allow-list. The raw
/// value flows from the multipart filename, so concatenating it into a path
/// unchecked would permit traversal (`foo.pt/../../etc`); anything unrecognized
/// or path-bearing collapses to `jpg`.
fn safe_ext(ext: &str) -> &'static str {
    match ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "png",
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        _ => "jpg",
    }
}

/// Lowercase hex-encode a byte slice.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `data` as lowercase hex.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    to_hex(&hasher.finalize())
}
