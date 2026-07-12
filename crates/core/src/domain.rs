//! Core domain records. Field layout mirrors the SQLite schema; all
//! timestamps are epoch milliseconds and physiologic ones sit on the
//! 5-minute grid.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CoreError;

/// Prediction quantile levels, ascending. The `fan` matrix has one row per
/// level in this exact order.
pub const QUANTILE_LEVELS: [f64; 7] = [0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95];

/// Index of the median (0.5) row within [`QUANTILE_LEVELS`].
pub const MEDIAN_QUANTILE: usize = 3;

/// Number of circadian time-of-day bins (2 hours each over 24h).
pub const TOD_BINS: usize = 12;

/// The nine scalar physiologic series that share the wide `samples` table.
///
/// Each maps to exactly one column; `column()` is the authoritative
/// column-name mapping used by the store for generic per-series writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Series {
    Bg,
    Carbs,
    Bolus,
    Basal,
    Hr,
    Steps,
    Sleep,
    Exercise,
    Mood,
}

impl Series {
    /// All series in canonical column order.
    pub const ALL: [Series; 9] = [
        Series::Bg,
        Series::Carbs,
        Series::Bolus,
        Series::Basal,
        Series::Hr,
        Series::Steps,
        Series::Sleep,
        Series::Exercise,
        Series::Mood,
    ];

    /// The SQLite column name backing this series.
    pub fn column(self) -> &'static str {
        match self {
            Series::Bg => "bg",
            Series::Carbs => "carbs",
            Series::Bolus => "bolus",
            Series::Basal => "basal",
            Series::Hr => "hr",
            Series::Steps => "steps",
            Series::Sleep => "sleep",
            Series::Exercise => "exercise",
            Series::Mood => "mood",
        }
    }

    /// True for the one integer-valued series (`mood`); all others are REAL.
    pub fn is_integer(self) -> bool {
        matches!(self, Series::Mood)
    }
}

impl std::str::FromStr for Series {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "bg" => Series::Bg,
            "carbs" => Series::Carbs,
            "bolus" => Series::Bolus,
            "basal" => Series::Basal,
            "hr" => Series::Hr,
            "steps" => Series::Steps,
            "sleep" => Series::Sleep,
            "exercise" => Series::Exercise,
            "mood" => Series::Mood,
            other => return Err(CoreError::UnknownSeries(other.to_string())),
        })
    }
}

impl std::fmt::Display for Series {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.column())
    }
}

/// One wide row on the 5-minute grid. `ts` is the primary key (epoch ms,
/// `ts % 300000 == 0`). Every physiologic field is optional; a gap is an
/// explicit `None`/NULL. Total insulin (`bolus + basal`) is never stored —
/// it is derived at display time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SampleRow {
    pub ts: i64,
    pub tz_offset: i32,
    pub bg: Option<f64>,
    pub carbs: Option<f64>,
    pub bolus: Option<f64>,
    pub basal: Option<f64>,
    pub hr: Option<f64>,
    pub steps: Option<f64>,
    pub sleep: Option<f64>,
    pub exercise: Option<f64>,
    pub mood: Option<i64>,
    pub updated_at: i64,
}

impl SampleRow {
    /// Display-derived total insulin (bolus + basal), None when both absent.
    pub fn total_insulin(&self) -> Option<f64> {
        match (self.bolus, self.basal) {
            (None, None) => None,
            (b, s) => Some(b.unwrap_or(0.0) + s.unwrap_or(0.0)),
        }
    }

    /// Read one series column as an f64 (mood widened).
    pub fn get(&self, series: Series) -> Option<f64> {
        match series {
            Series::Bg => self.bg,
            Series::Carbs => self.carbs,
            Series::Bolus => self.bolus,
            Series::Basal => self.basal,
            Series::Hr => self.hr,
            Series::Steps => self.steps,
            Series::Sleep => self.sleep,
            Series::Exercise => self.exercise,
            Series::Mood => self.mood.map(|m| m as f64),
        }
    }

    /// Write one series column from an f64 (mood truncated to i64).
    pub fn set(&mut self, series: Series, value: Option<f64>) {
        match series {
            Series::Bg => self.bg = value,
            Series::Carbs => self.carbs = value,
            Series::Bolus => self.bolus = value,
            Series::Basal => self.basal = value,
            Series::Hr => self.hr = value,
            Series::Steps => self.steps = value,
            Series::Sleep => self.sleep = value,
            Series::Exercise => self.exercise = value,
            Series::Mood => self.mood = value.map(|v| v as i64),
        }
    }
}

/// A model forecast: the median line plus a quantile fan, and a circadian
/// (time-of-day) distribution with a confidence scalar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    pub id: i64,
    pub made_at: i64,
    pub model_id: String,
    pub horizon_steps: i32,
    /// Predicted median series, length `horizon_steps` (mg/dL).
    pub line: Vec<f64>,
    /// Quantile fan: 7 rows (one per [`QUANTILE_LEVELS`]) × `horizon_steps`.
    pub fan: Vec<Vec<f64>>,
    /// Circadian distribution over 12 two-hour bins (units: hours).
    pub tod: Vec<f64>,
    pub tod_conf: f64,
    pub created_at: i64,
}

impl Default for Prediction {
    fn default() -> Self {
        Prediction {
            id: 0,
            made_at: 0,
            model_id: String::new(),
            horizon_steps: 0,
            line: Vec::new(),
            fan: Vec::new(),
            tod: vec![0.0; TOD_BINS],
            tod_conf: 0.0,
            created_at: 0,
        }
    }
}

/// A free-text note pinned to a grid timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Note {
    pub id: i64,
    pub ts: i64,
    pub tz_offset: i32,
    pub text: String,
    pub created_at: i64,
}

/// A meal photo. The binary lives under `<data_dir>/photos/`; only metadata
/// is stored in the DB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Photo {
    pub id: i64,
    pub ts: i64,
    pub path: String,
    pub sha256: String,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
    pub created_at: i64,
}

/// An application-originated alert. `payload` is opaque JSON; `origin_token`
/// is the minting caller's token id, excluded from broadcast fan-out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    pub ts: i64,
    pub kind: String,
    pub payload: Value,
    pub origin_token: Option<i64>,
    pub created_at: i64,
}

impl Default for Alert {
    fn default() -> Self {
        Alert {
            id: 0,
            ts: 0,
            kind: String::new(),
            payload: Value::Null,
            origin_token: None,
            created_at: 0,
        }
    }
}

/// Access class of a token. At most one live `Rw` token may exist (DB
/// invariant `one_rw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Rw,
    Ro,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Rw => "rw",
            TokenKind::Ro => "ro",
        }
    }

    /// True when this token may satisfy a write endpoint.
    pub fn can_write(self) -> bool {
        matches!(self, TokenKind::Rw)
    }
}

impl std::str::FromStr for TokenKind {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rw" => Ok(TokenKind::Rw),
            "ro" => Ok(TokenKind::Ro),
            other => Err(CoreError::UnknownTokenKind(other.to_string())),
        }
    }
}

/// A persisted opaque bearer token. The raw secret is never stored — only
/// `secret_hash = sha256(salt || secret)`. Immortal unless revoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub id: i64,
    #[serde(skip_serializing)]
    pub secret_hash: String,
    pub kind: TokenKind,
    pub label: Option<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

impl Token {
    pub fn is_live(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// A client session, upserted by the auth middleware and persisted across WS
/// reconnects. `device` is the app/user-agent string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Session {
    pub id: i64,
    pub token_id: i64,
    pub ip: String,
    pub user_agent: String,
    pub device: String,
    pub first_seen: i64,
    pub last_seen: i64,
}

/// A discovered forecasting model. `meta` is OPAQUE JSON — stored, served,
/// and rendered verbatim, never interpreted semantically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// Artifact filename extension (lowercased, no dot), e.g. `pt`, `onnx`,
    /// `tflite`. Empty when the file has none. Lets a consumer know the format
    /// without parsing the server-local `path`.
    pub ext: String,
    pub path: String,
    pub meta: Value,
    pub sha256: String,
    pub bytes: i64,
    pub discovered_at: i64,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            id: String::new(),
            name: String::new(),
            ext: String::new(),
            path: String::new(),
            meta: Value::Null,
            sha256: String::new(),
            bytes: 0,
            discovered_at: 0,
        }
    }
}
