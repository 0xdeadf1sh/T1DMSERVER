//! Aggregate glycemic statistics computed per rolling window.

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// The three supported statistics windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatsWindow {
    #[serde(rename = "7d")]
    D7,
    #[serde(rename = "30d")]
    D30,
    #[serde(rename = "90d")]
    D90,
}

impl StatsWindow {
    /// All windows, ascending in span.
    pub const ALL: [StatsWindow; 3] = [StatsWindow::D7, StatsWindow::D30, StatsWindow::D90];

    /// Canonical query-string label.
    pub fn as_str(self) -> &'static str {
        match self {
            StatsWindow::D7 => "7d",
            StatsWindow::D30 => "30d",
            StatsWindow::D90 => "90d",
        }
    }

    /// Window span in milliseconds.
    pub fn millis(self) -> i64 {
        match self {
            StatsWindow::D7 => 7 * 24 * 3_600_000,
            StatsWindow::D30 => 30 * 24 * 3_600_000,
            StatsWindow::D90 => 90 * 24 * 3_600_000,
        }
    }
}

impl std::str::FromStr for StatsWindow {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "7d" => Ok(StatsWindow::D7),
            "30d" => Ok(StatsWindow::D30),
            "90d" => Ok(StatsWindow::D90),
            other => Err(CoreError::UnknownWindow(other.to_string())),
        }
    }
}

/// A glycemic event (contiguous hypo or hyper excursion): count and total
/// duration in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct EventStat {
    pub count: u32,
    pub duration_ms: i64,
}

/// Full statistics for one window. All BG figures in mg/dL; time-in-range
/// fractions are 0..=1. Recomputed on ingest, cached in the store.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub window: StatsWindow,
    /// Time-in-range 70–180 mg/dL, as a fraction 0..=1.
    pub tir: f64,
    /// Time below 70 mg/dL, fraction 0..=1.
    pub time_below: f64,
    /// Time above 180 mg/dL, fraction 0..=1.
    pub time_above: f64,
    pub mean_bg: f64,
    /// Glucose Management Indicator (%).
    pub gmi: f64,
    /// Coefficient of variation (%).
    pub cv: f64,
    pub sd: f64,
    pub hypo_events: EventStat,
    pub hyper_events: EventStat,
    pub mean_daily_carbs: f64,
    /// Total daily insulin (U/day).
    pub tdd: f64,
    /// Bolus : basal ratio.
    pub bolus_basal_ratio: f64,
    pub mean_hr: f64,
    /// Pearson correlation of BG and HR over the window (-1..=1).
    pub bg_hr_corr: f64,
    /// Number of grid samples that contributed BG to this window.
    pub n_samples: u32,
}

impl Stats {
    /// An all-zero stats block for an empty window.
    pub fn empty(window: StatsWindow) -> Self {
        Stats {
            window,
            tir: 0.0,
            time_below: 0.0,
            time_above: 0.0,
            mean_bg: 0.0,
            gmi: 0.0,
            cv: 0.0,
            sd: 0.0,
            hypo_events: EventStat::default(),
            hyper_events: EventStat::default(),
            mean_daily_carbs: 0.0,
            tdd: 0.0,
            bolus_basal_ratio: 0.0,
            mean_hr: 0.0,
            bg_hr_corr: 0.0,
            n_samples: 0,
        }
    }
}
