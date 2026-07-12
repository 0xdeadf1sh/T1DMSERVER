//! Read path — served from the WAL read pool. Callers on the async side run
//! these under `spawn_blocking`.

use rusqlite::{params, OptionalExtension, Row};

use t1dm_core::{Alert, Note, Photo, Prediction, SampleRow, Series, TOD_BINS};

use crate::error::Result;
use crate::Store;

const SAMPLE_COLS: &str =
    "ts, tz_offset, bg, carbs, bolus, basal, hr, steps, sleep, exercise, mood, updated_at";

fn map_sample(row: &Row<'_>) -> rusqlite::Result<SampleRow> {
    Ok(SampleRow {
        ts: row.get(0)?,
        tz_offset: row.get(1)?,
        bg: row.get(2)?,
        carbs: row.get(3)?,
        bolus: row.get(4)?,
        basal: row.get(5)?,
        hr: row.get(6)?,
        steps: row.get(7)?,
        sleep: row.get(8)?,
        exercise: row.get(9)?,
        mood: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn map_note(row: &Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        ts: row.get(1)?,
        tz_offset: row.get(2)?,
        text: row.get(3)?,
        created_at: row.get(4)?,
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

fn map_alert(row: &Row<'_>) -> rusqlite::Result<Alert> {
    let payload_txt: String = row.get(3)?;
    let payload = serde_json::from_str(&payload_txt).unwrap_or(serde_json::Value::Null);
    Ok(Alert {
        id: row.get(0)?,
        ts: row.get(1)?,
        kind: row.get(2)?,
        payload,
        origin_token: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn map_prediction(row: &Row<'_>) -> rusqlite::Result<Prediction> {
    let line_txt: String = row.get(4)?;
    let fan_txt: String = row.get(5)?;
    let tod_txt: String = row.get(6)?;
    Ok(Prediction {
        id: row.get(0)?,
        made_at: row.get(1)?,
        model_id: row.get(2)?,
        horizon_steps: row.get(3)?,
        line: serde_json::from_str(&line_txt).unwrap_or_default(),
        fan: serde_json::from_str(&fan_txt).unwrap_or_default(),
        tod: serde_json::from_str(&tod_txt).unwrap_or_else(|_| vec![0.0; TOD_BINS]),
        tod_conf: row.get(7)?,
        created_at: row.get(8)?,
    })
}

const PRED_COLS: &str =
    "id, made_at, model_id, horizon_steps, line, fan, tod, tod_conf, created_at";

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
    pub fn get_predictions(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<Prediction>> {
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

    /// The single most recent prediction, if any.
    pub fn get_prediction_latest(&self) -> Result<Option<Prediction>> {
        self.with_reader(|conn| {
            let sql = format!("SELECT {PRED_COLS} FROM prediction ORDER BY made_at DESC LIMIT 1");
            let out = conn.query_row(&sql, [], map_prediction).optional()?;
            Ok(out)
        })
    }

    /// Notes with `ts` in `[from, to]`, newest first.
    pub fn get_notes(&self, from: Option<i64>, to: Option<i64>) -> Result<Vec<Note>> {
        let lo = from.unwrap_or(i64::MIN);
        let hi = to.unwrap_or(i64::MAX);
        self.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, ts, tz_offset, text, created_at FROM note
                 WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts DESC",
            )?;
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
            let mut stmt = conn.prepare(
                "SELECT id, ts, kind, payload, origin_token, created_at FROM alert
                 WHERE ts >= ?1 AND ts <= ?2 ORDER BY ts DESC",
            )?;
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
}
