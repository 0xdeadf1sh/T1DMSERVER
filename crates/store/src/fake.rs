//! Synthetic dataset generation for the Developer pane — a plausible BG
//! trace (diurnal sine + meal excursions + noise, clamped 40–400) over the
//! demoted scalar series (bg/hr/steps/sleep/exercise/mood), plus a widening
//! prediction fan with a synthetic circadian head. Carbs and insulin are now
//! first-class curve events, generated elsewhere, not here.

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use t1dm_core::{Circadian, IngestBundle, PredictionWrite, QUANTILE_LEVELS};

use crate::error::Result;
use crate::Store;

/// Inclusive epoch-ms range (snapped to the grid) to populate.
#[derive(Debug, Clone, Copy)]
pub struct FakeRange {
    pub from: i64,
    pub to: i64,
}

impl FakeRange {
    /// A range of the last `days` days ending now.
    pub fn last_days(days: i64) -> Self {
        let now = t1dm_core::snap_grid(crate::now_ms());
        FakeRange {
            from: now - days * 86_400_000,
            to: now,
        }
    }
}

/// Knobs for the generator.
#[derive(Debug, Clone, Copy)]
pub struct FakeOpts {
    pub seed: u64,
    pub with_predictions: bool,
    /// Forecast horizon in grid steps for generated predictions.
    pub horizon_steps: i32,
}

impl Default for FakeOpts {
    fn default() -> Self {
        FakeOpts {
            seed: 0x7431_446D_5345_4544,
            with_predictions: true,
            horizon_steps: 24,
        }
    }
}

impl Store {
    /// Generate a synthetic dataset over `range`, writing straight into the
    /// store. Returns the number of grid rows written.
    pub fn generate_fake(&self, range: FakeRange, opts: FakeOpts) -> Result<usize> {
        let from = t1dm_core::snap_grid(range.from);
        let to = t1dm_core::snap_grid(range.to);
        if to <= from {
            return Ok(0);
        }
        let mut rng = SmallRng::seed_from_u64(opts.seed);
        let day_ms = 86_400_000.0;

        let now = crate::now_ms();
        let mut written = 0usize;
        let mut ts = from;
        let mut last_pred_ts = from;

        while ts <= to {
            let tod = ((ts % 86_400_000) as f64) / day_ms; // 0..1 fraction of day
            let bg = synth_bg(tod, &mut rng);
            let hr =
                62.0 + 18.0 * (tod * std::f64::consts::TAU).sin() + rng.random_range(-4.0..4.0);
            // A burst of steps within each post-prandial window.
            let steps = if meal_excursion(tod) > 20.0 {
                rng.random_range(0.0..120.0)
            } else {
                0.0
            };

            let bundle = IngestBundle {
                ts,
                tz_offset: 0,
                updated_at: now,
                bg: Some(bg),
                hr: Some(hr),
                steps: Some(steps),
                sleep: Some(if tod < 0.30 { 1.0 } else { 0.0 }),
                exercise: Some(0.0),
                mood: Some(rng.random_range(3..=5)),
            };
            self.ingest_bundle(&bundle)?;
            written += 1;

            if opts.with_predictions && ts - last_pred_ts >= 6 * t1dm_core::GRID_MS {
                let pred = synth_prediction(bg, ts, now, opts.horizon_steps, &mut rng);
                self.put_predictions(std::slice::from_ref(&pred))?;
                last_pred_ts = ts;
            }


            ts += t1dm_core::GRID_MS;
        }
        Ok(written)
    }
}

/// Diurnal BG with meal bumps and noise, clamped to [40, 400].
fn synth_bg(tod: f64, rng: &mut SmallRng) -> f64 {
    let base = 120.0 + 25.0 * (tod * std::f64::consts::TAU - 1.2).sin();
    let meal = meal_excursion(tod);
    let noise = rng.random_range(-8.0..8.0);
    (base + meal + noise).clamp(40.0, 400.0)
}

/// Post-prandial excursions around breakfast/lunch/dinner.
fn meal_excursion(tod: f64) -> f64 {
    let peaks = [(0.32, 60.0), (0.53, 70.0), (0.80, 80.0)];
    peaks
        .iter()
        .map(|&(c, amp)| {
            let d = (tod - c) / 0.04;
            amp * (-d * d).exp()
        })
        .sum()
}

/// A widening quantile fan around a plausible near-future median, keyed at
/// `made_at` with the phone `updated_at` carried verbatim.
fn synth_prediction(
    bg_now: f64,
    made_at: i64,
    updated_at: i64,
    horizon: i32,
    rng: &mut SmallRng,
) -> PredictionWrite {
    let h = horizon.max(1) as usize;
    let drift = rng.random_range(-1.5..1.5);
    let line: Vec<f64> = (0..h)
        .map(|i| (bg_now + drift * i as f64).clamp(40.0, 400.0))
        .collect();

    // Fan: one row per quantile level; spread grows with horizon.
    let fan: Vec<Vec<f64>> = QUANTILE_LEVELS
        .iter()
        .map(|&q| {
            let z = (q - 0.5) * 2.0; // -0.9..0.9
            (0..h)
                .map(|i| {
                    let spread = 6.0 + 2.2 * i as f64;
                    (line[i] + z * spread).clamp(40.0, 400.0)
                })
                .collect()
        })
        .collect();

    PredictionWrite {
        made_at,
        model_id: "synthetic".to_string(),
        updated_at,
        horizon_steps: horizon,
        line,
        fan,
        circadian: Some(synth_circadian(rng)),
    }
}

/// A synthetic circadian belief over twelve two-hour bins.
fn synth_circadian(rng: &mut SmallRng) -> Circadian {
    const N_BINS: i32 = 12;
    const BIN_HOURS: f64 = 2.0;
    let raw: Vec<f64> = (0..N_BINS).map(|_| rng.random_range(0.0..1.0)).collect();
    let sum: f64 = raw.iter().sum::<f64>().max(1e-9);
    let probs: Vec<f64> = raw.iter().map(|p| p / sum).collect();
    let arg = probs
        .iter()
        .enumerate()
        .fold((0usize, f64::MIN), |(bi, bv), (i, &v)| {
            if v > bv {
                (i, v)
            } else {
                (bi, bv)
            }
        })
        .0;
    Circadian {
        probs,
        predicted_hour: (arg as f64 + 0.5) * BIN_HOURS,
        resultant_r: rng.random_range(0.3..0.95),
        n_bins: N_BINS,
        bin_hours: BIN_HOURS,
    }
}

