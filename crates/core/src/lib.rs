//! t1dm-core — shared domain contract for T1DMSERVER.
//!
//! These types are the cross-crate contract locked by the Foundation.
//! Everything physiologic lives on a fixed 5-minute epoch-ms grid; sample
//! storage is scalar (BG in mg/dL plus HR, steps, sleep, exercise, mood),
//! while meals and doses are self-describing curves keyed by phone
//! `client_id`, and display conversion is a pure function here.

pub mod config;
pub mod curve;
pub mod domain;
pub mod error;
pub mod events;
pub mod ingest;
pub mod stats;
pub mod units;

pub use config::{
    BackupConfig, Config, LogConfig, QrConfig, ServerConfig, StorageConfig, UiConfig,
};
pub use domain::{
    Alert, CgmSourceRow, Model, Photo, PredictionEvent, SampleRow, Series, Session, StatsBlock,
    Token, TokenKind, QUANTILE_LEVELS, TOD_BINS,
};
pub use error::{CoreError, Result};
pub use events::{
    BasalSchedule, BasalSlot, Circadian, DoseEvent, DoseKind, MealEvent, PredictionWrite,
};
pub use ingest::{IngestBundle, QrPayload};
pub use stats::{Stats, StatsWindow};
pub use units::{BgUnit, MGDL_PER_MMOL};

/// Physiologic grid spacing in milliseconds (5 minutes).
pub const GRID_MS: i64 = 300_000;

/// Number of grid steps per hour on the 5-minute grid.
pub const STEPS_PER_HOUR: i64 = 12;

/// Returns true when `ts` (epoch ms) sits exactly on the 5-minute grid.
#[inline]
pub fn on_grid(ts: i64) -> bool {
    ts.rem_euclid(GRID_MS) == 0
}

/// Snap an arbitrary epoch-ms timestamp down to the nearest grid point.
#[inline]
pub fn snap_grid(ts: i64) -> i64 {
    ts - ts.rem_euclid(GRID_MS)
}
