//! App-authored write shapes for the first-class curve events: meals, doses,
//! basal schedules, and forecasts. Each physiologic entity crosses the wire as
//! a self-describing curve keyed by a phone-assigned `client_id`; the server
//! stores it verbatim and never re-stamps the phone `updated_at`.
//!
//! These are the *write* shapes accepted on `PUT /v1/{meals,doses,
//! basal-schedule,predictions}`, so an absent optional field follows the same
//! house style as [`crate::ingest`]: `skip_serializing_if` omits it. The read
//! shapes served back on the matching `GET`s omit `skip_serializing_if`, so a
//! gap serializes as an explicit `null`, and they carry no server-internal
//! `id`/`received_at`/`created_at`.

use serde::{Deserialize, Serialize};

/// A meal as an appearance (Ra) curve, keyed by the phone's `client_id`.
///
/// `ts` is grid-snapped (`ts % 300000 == 0`). A parametric meal carries its
/// gamma parameters (`gi`/`k`/`theta`) with `duration_min`; a mixed or builder
/// meal instead carries its resolved appearance curve in `custom_curve` (a
/// per-bucket `f64` vector on the 5-minute grid). There is no per-component
/// breakdown — a builder meal is stored only as its summed `custom_curve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MealEvent {
    pub client_id: String,
    pub ts: i64,
    pub tz_offset: i32,
    pub updated_at: i64,
    pub grams: f64,
    pub duration_min: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gi: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_curve: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Whether a [`DoseEvent`] is a fast bolus (gamma action) or a background
/// basal (Bateman ka/ke action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoseKind {
    Bolus,
    Basal,
}

impl Default for DoseKind {
    fn default() -> Self {
        DoseKind::Bolus
    }
}

/// An insulin dose as a PK action curve, keyed by the phone's `client_id`.
///
/// A `Bolus` carries gamma parameters (`k`/`theta`); a `Basal` carries Bateman
/// parameters (`ka_per_hour`/`ke_per_hour`). Either may instead resolve to an
/// explicit `custom_curve` on the 5-minute grid. `ts` is grid-snapped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DoseEvent {
    pub client_id: String,
    pub ts: i64,
    pub tz_offset: i32,
    pub updated_at: i64,
    pub kind: DoseKind,
    pub units: f64,
    pub duration_min: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theta: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ka_per_hour: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ke_per_hour: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_curve: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A daily-repeating basal template, full-replaced on `PUT /v1/basal-schedule`.
/// The TUI tiles the active schedule's slots across the display window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasalSchedule {
    pub schedule_id: String,
    pub active: bool,
    pub slots: Vec<BasalSlot>,
}

/// One slot of a [`BasalSchedule`]: a background dose delivered every day at
/// `time_of_day_min` minutes past local midnight, keyed by `client_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasalSlot {
    pub client_id: String,
    pub label: String,
    pub time_of_day_min: i32,
    pub dose_u: f64,
    pub duration_min: f64,
    pub ka_per_hour: f64,
    pub ke_per_hour: f64,
    pub tz_offset: i32,
    pub updated_at: i64,
}

/// The circadian (time-of-day) belief emitted by the model's optional time
/// head, carried verbatim on a [`PredictionWrite`]. Absent (serialized as an
/// explicit `null`) when the model has no time head.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Circadian {
    pub probs: Vec<f64>,
    pub predicted_hour: f64,
    pub resultant_r: f64,
    pub n_bins: i32,
    pub bin_hours: f64,
}

/// A forecast written by `PUT /v1/predictions`, idempotent on
/// `(made_at, model_id)`. `made_at` is the phone's cycle timestamp, stored
/// verbatim; `fan` is one row per quantile level × `horizon_steps`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PredictionWrite {
    pub made_at: i64,
    pub model_id: String,
    pub updated_at: i64,
    pub horizon_steps: i32,
    pub line: Vec<f64>,
    pub fan: Vec<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circadian: Option<Circadian>,
}
