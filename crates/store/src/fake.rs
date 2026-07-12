//! Synthetic dataset generation for the Developer pane — a plausible BG
//! trace (diurnal sine + meal excursions + noise, clamped 40–400) with
//! matching carbs/insulin, a widening prediction fan, and a few notes.

use rand::rngs::SmallRng;
use rand::{RngExt, SeedableRng};

use t1dm_core::{IngestBundle, IngestPrediction, QUANTILE_LEVELS, TOD_BINS};

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
    pub with_notes: bool,
    /// Forecast horizon in grid steps for generated predictions.
    pub horizon_steps: i32,
}

impl Default for FakeOpts {
    fn default() -> Self {
        FakeOpts {
            seed: 0x7431_446D_5345_4544,
            with_predictions: true,
            with_notes: true,
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

        let mut written = 0usize;
        let mut ts = from;
        let mut last_pred_ts = from;

        while ts <= to {
            let tod = ((ts % 86_400_000) as f64) / day_ms; // 0..1 fraction of day
            let bg = synth_bg(tod, &mut rng);
            let (carbs, bolus) = meal_at(tod, &mut rng);
            let basal = 0.8 + 0.1 * (tod * std::f64::consts::TAU).sin();
            let hr =
                62.0 + 18.0 * (tod * std::f64::consts::TAU).sin() + rng.random_range(-4.0..4.0);
            let steps = if carbs > 0.0 {
                rng.random_range(0.0..120.0)
            } else {
                0.0
            };

            let bundle = IngestBundle {
                ts,
                tz_offset: 0,
                bg: Some(bg),
                carbs: (carbs > 0.0).then_some(carbs),
                bolus: (bolus > 0.0).then_some(bolus),
                basal: Some(basal),
                hr: Some(hr),
                steps: Some(steps),
                sleep: Some(if tod < 0.30 { 1.0 } else { 0.0 }),
                exercise: Some(0.0),
                mood: Some(rng.random_range(3..=5)),
                prediction: None,
                notes: Vec::new(),
            };
            self.ingest_bundle(&bundle)?;
            written += 1;

            if opts.with_predictions && ts - last_pred_ts >= 6 * t1dm_core::GRID_MS {
                let pred = synth_prediction(bg, opts.horizon_steps, &mut rng);
                self.put_predictions(std::slice::from_ref(&pred))?;
                last_pred_ts = ts;
            }

            if opts.with_notes && rng.random_bool(0.01) {
                let _ = self.add_note(ts, 0, sample_note(&mut rng));
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

/// Carbs and bolus fired at meal centers.
fn meal_at(tod: f64, rng: &mut SmallRng) -> (f64, f64) {
    let centers = [0.30, 0.51, 0.78];
    for &c in &centers {
        if (tod - c).abs() < 0.004 {
            let carbs: f64 = rng.random_range(30.0..70.0);
            let bolus: f64 = carbs / 10.0 + rng.random_range(-0.5..0.5);
            return (carbs.round(), (bolus.max(0.5) * 10.0).round() / 10.0);
        }
    }
    (0.0, 0.0)
}

/// A widening quantile fan around a plausible near-future median.
fn synth_prediction(bg_now: f64, horizon: i32, rng: &mut SmallRng) -> IngestPrediction {
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

    let tod: Vec<f64> = (0..TOD_BINS).map(|_| rng.random_range(0.0..2.0)).collect();

    IngestPrediction {
        model_id: "synthetic".to_string(),
        horizon_steps: horizon,
        line,
        fan,
        tod,
        tod_conf: rng.random_range(0.4..0.95),
    }
}

fn sample_note(rng: &mut SmallRng) -> &'static str {
    const NOTES: [&str; 5] = [
        "felt low before lunch",
        "long walk after dinner",
        "stressful meeting",
        "corrected a spike",
        "new sensor applied",
    ];
    NOTES[rng.random_range(0..NOTES.len())]
}
