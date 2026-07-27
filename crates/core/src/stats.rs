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

/// A glycemic excursion tally, as the phone defines one: a maximal run of at
/// least two consecutive BG-bearing samples past the configured target edge.
/// `count` is the number of such runs; `duration_ms` is their total, each run
/// measured last timestamp minus first — one grid step short of the span it
/// covers. A dropout of up to 30 minutes is bridged into a single run; a longer
/// one splits it, and either fragment shorter than two samples is discarded.
/// A lone out-of-range sample therefore contributes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct EventStat {
    pub count: u32,
    pub duration_ms: i64,
}

/// Full statistics for one window. All BG figures in mg/dL; time-in-range
/// fractions are 0..=1. Computed on the phone and stored by the server verbatim.
///
/// Five fields the schema carries are not populated by the current phone build
/// and so always arrive as `0.0`, indistinguishable from a genuine zero:
/// `mean_hr` and `bg_hr_corr` are not computed there at all, and
/// `mean_daily_carbs`, `tdd` and `bolus_basal_ratio` reduce over sample columns
/// that no longer exist — carbohydrate and insulin totals travel as meal and
/// dose curve events, so the sums they reduce are empty. Do not present any of
/// the five as a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub window: StatsWindow,
    /// Phone millisecond clock at which the block was computed.
    pub updated_at: i64,
    /// Time in the target range configured on the phone (default 70–180 mg/dL),
    /// as a fraction 0..=1. The edges are not carried on the wire, so this
    /// fraction cannot be reinterpreted against any other pair.
    pub tir: f64,
    /// Time below the configured target low, fraction 0..=1.
    pub time_below: f64,
    /// Time above the configured target high, fraction 0..=1.
    pub time_above: f64,
    pub mean_bg: f64,
    /// Glucose Management Indicator (%).
    pub gmi: f64,
    /// Coefficient of variation (%).
    pub cv: f64,
    pub sd: f64,
    pub hypo_events: EventStat,
    pub hyper_events: EventStat,
    /// Nominally g/day. Not populated by the current phone build; see the type docs.
    pub mean_daily_carbs: f64,
    /// Nominally total daily insulin (U/day). Not populated; see the type docs.
    pub tdd: f64,
    /// Nominally the bolus : basal ratio. Not populated; see the type docs.
    pub bolus_basal_ratio: f64,
    /// Nominally a mean heart rate. Not computed on the phone at all.
    pub mean_hr: f64,
    /// Nominally the Pearson correlation of BG and HR over the window (-1..=1).
    /// Not computed on the phone at all.
    pub bg_hr_corr: f64,
    /// Number of grid samples that contributed BG to this window.
    pub n_samples: u32,
}

impl Stats {
    /// An all-zero stats block for an empty window.
    pub fn empty(window: StatsWindow) -> Self {
        Stats {
            window,
            updated_at: 0,
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
