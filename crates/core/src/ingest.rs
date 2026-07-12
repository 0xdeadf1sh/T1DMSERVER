//! Request/wire structs shared between api and store: the atomic 5-minute
//! ingest bundle, series batch upserts, and the QR login payload.

use serde::{Deserialize, Serialize};

/// Atomic 5-minute bundle accepted by `POST /v1/ingest`. Every physiologic
/// field is optional; a present `prediction`/`notes` are written in the same
/// transaction. Writes hard-overwrite the row at `ts` (upsert).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestBundle {
    pub ts: i64,
    pub tz_offset: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carbs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bolus: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basal: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercise: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prediction: Option<IngestPrediction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Prediction payload embedded in an ingest bundle or `PUT /v1/predictions`.
/// `fan` is 7 × `horizon_steps` (quantile rows), `tod` is 12 circadian bins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestPrediction {
    pub model_id: String,
    pub horizon_steps: i32,
    pub line: Vec<f64>,
    pub fan: Vec<Vec<f64>>,
    pub tod: Vec<f64>,
    pub tod_conf: f64,
}

/// One point in a series batch upsert.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub ts: i64,
    pub value: f64,
}

/// Body of `PUT /v1/series/{name}` — batch upsert/override + backfill for a
/// single named series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SeriesUpsert {
    pub samples: Vec<SeriesPoint>,
}

/// Payload embedded in the login QR code rendered by the Sessions pane.
/// Serialized as `{type, token, addr, port}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QrPayload {
    #[serde(rename = "type")]
    pub kind: String,
    pub token: String,
    pub addr: String,
    pub port: u16,
}

impl QrPayload {
    /// Standard payload kind tag for a T1DM login QR.
    pub const KIND: &'static str = "t1dm-login";

    pub fn new(token: impl Into<String>, addr: impl Into<String>, port: u16) -> Self {
        QrPayload {
            kind: Self::KIND.to_string(),
            token: token.into(),
            addr: addr.into(),
            port,
        }
    }
}
