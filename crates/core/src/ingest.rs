//! Request/wire structs shared between api and store: the atomic 5-minute
//! ingest bundle and the QR login payload.

use serde::{Deserialize, Serialize};

/// Atomic 5-minute bundle accepted by `POST /v1/ingest`. Carries the demoted
/// scalar series only — carbs/bolus/basal are now first-class curve events
/// (`meal_event`/`dose_event`) and predictions/notes have their own endpoints.
/// Every physiologic field is optional; an absent field leaves the stored
/// column untouched (COALESCE upsert keyed on `ts`). `updated_at` is the phone
/// clock, stored verbatim (never re-stamped by the server).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct IngestBundle {
    pub ts: i64,
    pub tz_offset: i32,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<f64>,
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
