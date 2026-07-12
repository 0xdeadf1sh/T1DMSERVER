//! Write path — all mutations flow through the single serialized writer.

use rusqlite::params;
use serde_json::Value;

use t1dm_core::{Alert, IngestBundle, IngestPrediction, Note, Photo, Series, SeriesPoint};

use crate::error::{Result, StoreError};
use crate::{now_ms, Store};

/// Upsert SQL that hard-overwrites the whole wide row but preserves any
/// column the bundle left absent (COALESCE keeps the existing value).
const INGEST_UPSERT: &str = r#"
INSERT INTO samples (ts, tz_offset, bg, carbs, bolus, basal, hr, steps, sleep, exercise, mood, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
ON CONFLICT(ts) DO UPDATE SET
    tz_offset  = excluded.tz_offset,
    bg         = COALESCE(excluded.bg, samples.bg),
    carbs      = COALESCE(excluded.carbs, samples.carbs),
    bolus      = COALESCE(excluded.bolus, samples.bolus),
    basal      = COALESCE(excluded.basal, samples.basal),
    hr         = COALESCE(excluded.hr, samples.hr),
    steps      = COALESCE(excluded.steps, samples.steps),
    sleep      = COALESCE(excluded.sleep, samples.sleep),
    exercise   = COALESCE(excluded.exercise, samples.exercise),
    mood       = COALESCE(excluded.mood, samples.mood),
    updated_at = excluded.updated_at
"#;

impl Store {
    /// Ingest one atomic 5-minute bundle: the wide sample row plus any
    /// embedded prediction and notes, all in a single transaction.
    pub fn ingest_bundle(&self, bundle: &IngestBundle) -> Result<()> {
        if !t1dm_core::on_grid(bundle.ts) {
            return Err(StoreError::OffGrid(bundle.ts));
        }
        let now = now_ms();
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                INGEST_UPSERT,
                params![
                    bundle.ts,
                    bundle.tz_offset,
                    bundle.bg,
                    bundle.carbs,
                    bundle.bolus,
                    bundle.basal,
                    bundle.hr,
                    bundle.steps,
                    bundle.sleep,
                    bundle.exercise,
                    bundle.mood,
                    now,
                ],
            )?;

            if let Some(pred) = &bundle.prediction {
                insert_prediction(&tx, pred, bundle.ts, now)?;
            }
            for text in &bundle.notes {
                tx.execute(
                    "INSERT INTO note(ts, tz_offset, text, created_at) VALUES (?1,?2,?3,?4)",
                    params![bundle.ts, bundle.tz_offset, text, now],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Batch upsert/override + backfill for a single named series.
    pub fn upsert_samples(&self, series: Series, points: &[SeriesPoint]) -> Result<usize> {
        if points.is_empty() {
            return Ok(0);
        }
        let col = series.column();
        let now = now_ms();
        let sql = format!(
            "INSERT INTO samples (ts, tz_offset, {col}, updated_at) VALUES (?1, 0, ?2, ?3)
             ON CONFLICT(ts) DO UPDATE SET {col} = excluded.{col}, updated_at = excluded.updated_at"
        );
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut n = 0usize;
            {
                let mut stmt = tx.prepare(&sql)?;
                for p in points {
                    if !t1dm_core::on_grid(p.ts) {
                        return Err(StoreError::OffGrid(p.ts));
                    }
                    if series.is_integer() {
                        stmt.execute(params![p.ts, p.value as i64, now])?;
                    } else {
                        stmt.execute(params![p.ts, p.value, now])?;
                    }
                    n += 1;
                }
            }
            tx.commit()?;
            Ok(n)
        })
    }

    /// Persist a batch of predictions; returns the assigned row ids.
    pub fn put_predictions(&self, preds: &[IngestPrediction]) -> Result<Vec<i64>> {
        let now = now_ms();
        self.with_writer(|conn| {
            let tx = conn.unchecked_transaction()?;
            let mut ids = Vec::with_capacity(preds.len());
            for pred in preds {
                ids.push(insert_prediction(&tx, pred, now, now)?);
            }
            tx.commit()?;
            Ok(ids)
        })
    }

    /// Append a free-text note.
    pub fn add_note(&self, ts: i64, tz_offset: i32, text: &str) -> Result<Note> {
        let now = now_ms();
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO note(ts, tz_offset, text, created_at) VALUES (?1,?2,?3,?4)",
                params![ts, tz_offset, text, now],
            )?;
            Ok(Note {
                id: conn.last_insert_rowid(),
                ts,
                tz_offset,
                text: text.to_string(),
                created_at: now,
            })
        })
    }

    /// Store a meal photo: write the binary under `photos/<sha>.<ext>` and
    /// record its metadata. `width`/`height` are supplied by the decoder.
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
        std::fs::write(&full, data)?;
        let now = now_ms();
        let rel = format!("photos/{file_name}");
        self.with_writer(|conn| {
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

    /// Record an application-originated alert. `origin_token` is excluded
    /// from the WS broadcast fan-out by the api hub.
    pub fn add_alert(
        &self,
        ts: i64,
        kind: &str,
        payload: &Value,
        origin_token: Option<i64>,
    ) -> Result<Alert> {
        let now = now_ms();
        let payload_txt = serde_json::to_string(payload)?;
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO alert(ts, kind, payload, origin_token, created_at)
                 VALUES (?1,?2,?3,?4,?5)",
                params![ts, kind, payload_txt, origin_token, now],
            )?;
            Ok(Alert {
                id: conn.last_insert_rowid(),
                ts,
                kind: kind.to_string(),
                payload: payload.clone(),
                origin_token,
                created_at: now,
            })
        })
    }
}

/// Insert one prediction row within a transaction; returns its id.
fn insert_prediction(
    conn: &rusqlite::Connection,
    pred: &IngestPrediction,
    made_at: i64,
    now: i64,
) -> Result<i64> {
    let line = serde_json::to_string(&pred.line)?;
    let fan = serde_json::to_string(&pred.fan)?;
    let tod = serde_json::to_string(&pred.tod)?;
    conn.execute(
        "INSERT INTO prediction(made_at, model_id, horizon_steps, line, fan, tod, tod_conf, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            made_at,
            pred.model_id,
            pred.horizon_steps,
            line,
            fan,
            tod,
            pred.tod_conf,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
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
