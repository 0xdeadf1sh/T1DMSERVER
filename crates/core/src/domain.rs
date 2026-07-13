//! Core domain records. Field layout mirrors the SQLite schema; all
//! timestamps are epoch milliseconds and physiologic ones sit on the
//! 5-minute grid.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::CoreError;
use crate::events::Circadian;

/// Prediction quantile levels, ascending. The `fan` matrix has one row per
/// level in this exact order.
pub const QUANTILE_LEVELS: [f64; 7] = [0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95];

/// Index of the median (0.5) row within [`QUANTILE_LEVELS`].
pub const MEDIAN_QUANTILE: usize = 3;

/// Number of circadian time-of-day bins (2 hours each over 24h).
pub const TOD_BINS: usize = 12;

/// The six scalar physiologic series that share the wide `samples` table.
///
/// Each maps to exactly one column; `column()` is the authoritative
/// column-name mapping used by the store for generic per-series writes.
/// Carbs, bolus, and basal are no longer scalar series — they are first-class
/// curve events (`meal_event`/`dose_event`), so they do not appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Series {
    Bg,
    Hr,
    Steps,
    Sleep,
    Exercise,
    Mood,
}

impl Series {
    /// All series in canonical column order.
    pub const ALL: [Series; 6] = [
        Series::Bg,
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
/// `ts % 300000 == 0`). Every physiologic scalar is optional; a gap is an
/// explicit `None`/NULL. `updated_at` is the phone clock, stored verbatim;
/// `received_at` is the server's internal arrival stamp and never crosses the
/// wire (`#[serde(skip)]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SampleRow {
    pub ts: i64,
    pub tz_offset: i32,
    pub bg: Option<f64>,
    pub hr: Option<f64>,
    pub steps: Option<f64>,
    pub sleep: Option<f64>,
    pub exercise: Option<f64>,
    pub mood: Option<i64>,
    pub updated_at: i64,
    #[serde(skip)]
    pub received_at: i64,
}

impl SampleRow {
    /// Read one series column as an f64 (mood widened).
    pub fn get(&self, series: Series) -> Option<f64> {
        match series {
            Series::Bg => self.bg,
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
            Series::Hr => self.hr = value,
            Series::Steps => self.steps = value,
            Series::Sleep => self.sleep = value,
            Series::Exercise => self.exercise = value,
            Series::Mood => self.mood = value.map(|v| v as i64),
        }
    }
}

/// A model forecast served on `GET /v1/predictions`: the median line, a
/// quantile fan, and the optional circadian belief. `made_at` is the phone's
/// cycle timestamp, stored verbatim; the server-internal `id`/`created_at`
/// never cross the wire, so this read shape omits them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PredictionEvent {
    pub made_at: i64,
    pub model_id: String,
    pub updated_at: i64,
    pub horizon_steps: i32,
    /// Predicted median series, length `horizon_steps` (mg/dL).
    pub line: Vec<f64>,
    /// Quantile fan: 7 rows (one per [`QUANTILE_LEVELS`]) × `horizon_steps`.
    pub fan: Vec<Vec<f64>>,
    /// Circadian belief from the model's time head; explicit `null` when the
    /// model has no time head.
    pub circadian: Option<Circadian>,
}

/// A free-text note pinned to a grid timestamp, keyed by the phone's
/// `client_id`. `updated_at` is the phone clock, stored verbatim; a phone-side
/// edit carries a newer `updated_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Note {
    pub id: i64,
    pub client_id: String,
    pub ts: i64,
    pub tz_offset: i32,
    pub text: String,
    pub updated_at: i64,
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

/// An application-originated alert, keyed by the phone's `client_id`. `payload`
/// is opaque JSON; `origin_token` is the minting caller's token id, excluded
/// from broadcast fan-out. Immutable once written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    pub client_id: String,
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
            client_id: String::new(),
            ts: 0,
            kind: String::new(),
            payload: Value::Null,
            origin_token: None,
            created_at: 0,
        }
    }
}

/// A phone-pushed statistics block for one window (`7d`/`30d`/`90d`), stored
/// and served verbatim. `json` is the opaque phone `Stats` block echoed
/// byte-for-byte — the server never computes or re-derives it. `window` is the
/// primary key; the server-internal `received_at` never crosses the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsBlock {
    pub window: String,
    pub updated_at: i64,
    pub json: Value,
}

impl Default for StatsBlock {
    fn default() -> Self {
        StatsBlock {
            window: String::new(),
            updated_at: 0,
            json: Value::Null,
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
