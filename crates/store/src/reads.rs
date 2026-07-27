//! Read path — served from the WAL read pool. Callers on the async side run
//! these under `spawn_blocking`.

use rusqlite::{params, OptionalExtension, Row};

use t1dm_core::{Alert, Circadian, Note, Photo, PredictionEvent, SampleRow, Series};

use crate::error::Result;
use crate::Store;

// (#5) carbs/bolus/basal demoted to curve events; `received_at` is the
// internal server clock and is NOT served (skipped on the wire in the
// `SampleRow` serde definition, §2.3) — it is read here only to populate the
// struct's internal field.
const SAMPLE_COLS: &str =
    "ts, tz_offset, bg, hr, steps, sleep, exercise, mood, updated_at, received_at";

fn map_sample(row: &Row<'_>) -> rusqlite::Result<SampleRow> {
    Ok(SampleRow {
        ts: row.get(0)?,
        tz_offset: row.get(1)?,
        bg: row.get(2)?,
        hr: row.get(3)?,
        steps: row.get(4)?,
        sleep: row.get(5)?,
        exercise: row.get(6)?,
        mood: row.get(7)?,
        updated_at: row.get(8)?,
        received_at: row.get(9)?,
    })
}

const NOTE_COLS: &str = "id, client_id, ts, tz_offset, text, updated_at, created_at";

fn map_note(row: &Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        client_id: row.get(1)?,
        ts: row.get(2)?,
        tz_offset: row.get(3)?,
        text: row.get(4)?,
        updated_at: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn map_photo(row: &Row<'_>) -> rusqlite::Result<Photo> {
    Ok(Photo {
        id: row.get(0)?,
        ts: row.get(1)?,
        path: row.get(2)?,
        sha256: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        bytes: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const ALERT_COLS: &str = "id, client_id, ts, kind, payload, origin_token, created_at";

fn map_alert(row: &Row<'_>) -> rusqlite::Result<Alert> {
    let payload_txt: String = row.get(4)?;
    let payload = serde_json::from_str(&payload_txt).unwrap_or(serde_json::Value::Null);
    Ok(Alert {
        id: row.get(0)?,
        client_id: row.get(1)?,
        ts: row.get(2)?,
        kind: row.get(3)?,
        payload,
        origin_token: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn map_prediction(row: &Row<'_>) -> rusqlite::Result<PredictionEvent> {
    let line_txt: String = row.get(3)?;
    let fan_txt: String = row.get(4)?;
    let circadian_txt: Option<String> = row.get(5)?;
    Ok(PredictionEvent {
        made_at: row.get(0)?,
        model_id: row.get(1)?,
        updated_at: row.get(2)?,
        horizon_steps: row.get(6)?,
        line: serde_json::from_str(&line_txt).unwrap_or_default(),
        fan: serde_json::from_str(&fan_txt).unwrap_or_default(),
        circadian: circadian_txt
            .and_then(|s| serde_json::from_str::<Circadian>(&s).ok()),
    })
}

const PRED_COLS: &str = "made_at, model_id, updated_at, line, fan, circadian, horizon_steps";

impl Store {
    /// Fetch wide sample rows in `[from, to]` (epoch ms), paginating forward
    /// by `ts` cursor. `fields` selects which series a caller cares about;
    /// the store returns whole rows and lets the caller project. Rows with
    /// `ts <= cursor` are skipped.
    pub fn get_samples(
        &self,
        _fields: &[Series],
        from: Option<i64>,
        to: Option<i64>,
        limit: Option<usize>,
        cursor: Option<i64>,
    ) -> Result<Vec<SampleRow>> {
        let lo = cursor.map(|c| c + 1).or(from).unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        let lim = limit.unwrap_or(10_000) as i64;
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {SAMPLE_COLS} FROM samples WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts ASC LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi, lim], map_sample)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Predictions with `made_at` in `[from, to]`, newest first.
    pub fn get_predictions(
        &self,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<Vec<PredictionEvent>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {PRED_COLS} FROM prediction WHERE made_at >= ?1 AND made_at <= ?2 ORDER BY made_at DESC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi], map_prediction)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// The single most recent prediction, if any. The table's identity is
    /// `(made_at, model_id)` and the phone pushes every running model of a cycle
    /// in one batch, so several rows legitimately share one `made_at`; the
    /// `updated_at, id` tiebreak makes the pick the LAST-WRITTEN model of the
    /// newest cycle, deterministic across repeated reads of an unchanged store.
    /// It is not necessarily the model the phone displays — the wire carries no
    /// selection/status flag, so the server cannot know which that is.
    pub fn get_prediction_latest(&self) -> Result<Option<PredictionEvent>> {
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {PRED_COLS} FROM prediction
                 ORDER BY made_at DESC, updated_at DESC, id DESC LIMIT 1"
            );
            let out = conn.query_row(&sql, [], map_prediction).optional()?;
            Ok(out)
        })
    }

    /// Notes with `ts` in `[from, to]`, newest first.
    pub fn get_notes(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<Note>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {NOTE_COLS} FROM note WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts DESC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi], map_note)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Alerts with `ts` in `[from, to]`, newest first.
    pub fn get_alerts(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<Alert>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let sql = format!(
                "SELECT {ALERT_COLS} FROM alert WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts DESC"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![lo, hi], map_alert)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Photo metadata with `ts` in `[from, to]`, newest first.
    pub fn get_photos(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<Photo>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, path, sha256, width, height, bytes, created_at FROM photo
                 WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts DESC",
            )?;
            let rows = stmt
                .query_map(params![lo, hi], map_photo)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Absolute filesystem path of a stored photo binary, if the id exists.
    pub fn photo_path(&self, id: i64) -> Result<Option<std::path::PathBuf>> {
        let rel: Option<String> = self.with_reader(|conn| {
            Ok(conn
                .query_row("SELECT path FROM photo WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .optional()?)
        })?;
        Ok(rel.map(|r| self.data_dir().join(r)))
    }

    /// The server's `store_epoch` marker, minted once when the schema is
    /// first created and surfaced (authed) via `GET /v1/health` (§3.8).
    /// `None` before the first mint. It is an opaque, clock-free identity —
    /// not a timestamp — that the phone compares against its last-mirrored
    /// epoch to detect a freshly-wiped/new server and trigger a full
    /// re-mirror of its authoritative history.
    pub fn store_epoch(&self) -> Result<Option<String>> {
        self.with_reader(|conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM meta WHERE key = 'store_epoch'",
                    [],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }
}
