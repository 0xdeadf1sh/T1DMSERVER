//! Display-only channel reconstruction for the TUI.
//!
//! Carbs/bolus/basal are no longer sample columns — each is a first-class curve
//! event (`meal_event`/`dose_event`/`basal_schedule_dose`). To render the
//! time-aligned strips the TUI folds those stored events back onto the fixed
//! 5-minute grid: each row is resolved from its verbatim `custom_curve` when
//! present, else from its stored gamma/Bateman params via the ported
//! [`t1dm_core::curve`] engine, then `bucketize`d onto the grid.
//!
//! The §4-#6 basal XOR is applied here: an active daily schedule (tiled by
//! `extend_basal`) is the SOLE basal source when present; only when no schedule
//! is active do discrete `basal` dose events feed the basal channel. Bolus and
//! basal therefore never double-count a scheduled-plus-injected background.
//!
//! This is display-only and does not participate in the phone's byte-exact
//! IOB/COB guarantee.

use t1dm_core::curve::{
    bateman, bucketize, extend_basal, gamma, BasalDoseSpec, BasalSchedule as CurveSchedule,
    CurveEvent, CurveKind, BASAL_KA_PER_HOUR, BASAL_KE_PER_HOUR, STEP_MS,
};
use t1dm_core::{snap_grid, DoseKind};

use crate::error::Result;
use crate::Store;

/// Per-bucket reconstructed carb-appearance / bolus / basal channels, each a
/// list of `(grid_ts, amount_per_step)` points with a nonzero value. Keyed by
/// grid ts on the fixed 5-minute cadence; display-only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconstructedChannels {
    pub carb: Vec<(i64, f64)>,
    pub bolus: Vec<(i64, f64)>,
    pub basal: Vec<(i64, f64)>,
}

/// Widen the load window backward by this much so an event whose action tail
/// still reaches into `[from, to]` is included (long-acting basal ~42 h; keep a
/// generous margin).
const TAIL_MARGIN_MS: i64 = 3 * 86_400_000;

impl Store {
    /// Reconstruct the carb / bolus / basal display channels over `[from, to]`
    /// (epoch ms) from the stored meal/dose/basal-schedule events. Display-only.
    pub fn reconstruct_channels(&self, from: i64, to: i64) -> Result<ReconstructedChannels> {
        let grid_start = snap_grid(from);
        let grid_end = snap_grid(to);
        if grid_end < grid_start {
            return Ok(ReconstructedChannels::default());
        }
        let n_steps = ((grid_end - grid_start) / STEP_MS) as i32 + 1;

        // --- carbs -------------------------------------------------------
        let meals = self.get_meals(Some(grid_start - TAIL_MARGIN_MS), Some(grid_end))?;
        let carb_events: Vec<CurveEvent> = meals
            .iter()
            .map(|m| {
                let values = m.custom_curve.clone().unwrap_or_else(|| {
                    gamma(m.grams, m.k.unwrap_or(3.0), m.theta.unwrap_or(20.0), m.duration_min)
                });
                CurveEvent {
                    start_ms: m.ts,
                    step_ms: STEP_MS,
                    kind: CurveKind::Carb,
                    total: m.grams,
                    values,
                }
            })
            .collect();
        let carb = grid_points(
            bucketize(carb_events, grid_start, n_steps, CurveKind::Carb)?,
            grid_start,
        );

        // --- bolus + discrete basal injections ---------------------------
        let doses = self.get_doses(Some(grid_start - TAIL_MARGIN_MS), Some(grid_end))?;
        let mut bolus_events: Vec<CurveEvent> = Vec::new();
        let mut basal_inject_events: Vec<CurveEvent> = Vec::new();
        for d in &doses {
            match d.kind {
                DoseKind::Bolus => {
                    let values = d.custom_curve.clone().unwrap_or_else(|| {
                        gamma(d.units, d.k.unwrap_or(3.0), d.theta.unwrap_or(20.0), d.duration_min)
                    });
                    bolus_events.push(CurveEvent {
                        start_ms: d.ts,
                        step_ms: STEP_MS,
                        kind: CurveKind::Insulin,
                        total: d.units,
                        values,
                    });
                }
                DoseKind::Basal => {
                    let values = d.custom_curve.clone().unwrap_or_else(|| {
                        bateman(
                            d.units,
                            d.duration_min,
                            d.ka_per_hour.unwrap_or(BASAL_KA_PER_HOUR),
                            d.ke_per_hour.unwrap_or(BASAL_KE_PER_HOUR),
                        )
                    });
                    basal_inject_events.push(CurveEvent {
                        start_ms: d.ts,
                        step_ms: STEP_MS,
                        kind: CurveKind::Insulin,
                        total: d.units,
                        values,
                    });
                }
            }
        }
        let bolus = grid_points(
            bucketize(bolus_events, grid_start, n_steps, CurveKind::Insulin)?,
            grid_start,
        );

        // --- basal: schedule XOR discrete injections (§4-#6, schedule-primary) ---
        let basal_events = match self.get_basal_schedule()? {
            Some(sched) if !sched.slots.is_empty() => {
                let tz_offset_min = sched.slots.first().map(|s| s.tz_offset).unwrap_or(0);
                let doses = sched
                    .slots
                    .iter()
                    .map(|s| BasalDoseSpec {
                        time_of_day_min: s.time_of_day_min,
                        dose_u: s.dose_u,
                        duration_min: s.duration_min,
                        ka_per_hour: s.ka_per_hour,
                        ke_per_hour: s.ke_per_hour,
                    })
                    .collect();
                extend_basal(
                    CurveSchedule { tz_offset_min, doses },
                    grid_start,
                    grid_end + STEP_MS,
                )?
            }
            _ => basal_inject_events,
        };
        let basal = grid_points(
            bucketize(basal_events, grid_start, n_steps, CurveKind::Insulin)?,
            grid_start,
        );

        Ok(ReconstructedChannels { carb, bolus, basal })
    }
}

/// Pair a bucketized grid with its absolute timestamps, dropping empty buckets.
fn grid_points(grid: Vec<f64>, grid_start: i64) -> Vec<(i64, f64)> {
    grid.into_iter()
        .enumerate()
        .filter(|(_, v)| *v != 0.0)
        .map(|(i, v)| (grid_start + i as i64 * STEP_MS, v))
        .collect()
}
