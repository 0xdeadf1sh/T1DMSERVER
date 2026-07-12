//! Windowed glycemic statistics. Loads the window's samples and reduces
//! them (rayon across cores for the heavier folds). Recomputed on ingest by
//! callers and cheap enough to compute on demand here.

use rayon::prelude::*;

use t1dm_core::{Series, Stats, StatsWindow};

use crate::error::Result;
use crate::{now_ms, Store};

const HYPO: f64 = 70.0;
const HYPER: f64 = 180.0;
/// Each grid sample spans 5 minutes.
const SAMPLE_MS: i64 = t1dm_core::GRID_MS;

/// Freshness horizon of a cached stats block: recompute at most once per day.
pub const STATS_CACHE_TTL_MS: i64 = 86_400_000;

impl Store {
    /// Compute [`Stats`] for `window`, ending at the current wall clock. Always
    /// a fresh reduction over the window's samples.
    pub fn stats(&self, window: StatsWindow) -> Result<Stats> {
        let now = now_ms();
        let from = now - window.millis();
        let rows = self.get_samples(&Series::ALL, Some(from), Some(now), Some(200_000), None)?;
        if rows.is_empty() {
            return Ok(Stats::empty(window));
        }
        Ok(compute(window, &rows))
    }

    /// Return the cached [`Stats`] for `window` when it was computed within
    /// [`STATS_CACHE_TTL_MS`]; otherwise compute fresh, persist to the cache,
    /// and return. This is the default read path for the API.
    pub fn stats_cached(&self, window: StatsWindow) -> Result<Stats> {
        let now = now_ms();
        if let Some((computed_at, json)) = self.read_stats_cache(window)? {
            if now - computed_at < STATS_CACHE_TTL_MS {
                if let Ok(stats) = serde_json::from_str::<Stats>(&json) {
                    return Ok(stats);
                }
            }
        }
        let stats = self.stats(window)?;
        self.write_stats_cache(window, now, &stats)?;
        Ok(stats)
    }

    /// Recompute every window and refresh its cache row (the once-per-day
    /// fan-out driven by the server's background task).
    pub fn refresh_stats_cache(&self) -> Result<()> {
        let now = now_ms();
        for window in StatsWindow::ALL {
            let stats = self.stats(window)?;
            self.write_stats_cache(window, now, &stats)?;
        }
        Ok(())
    }

    /// Read a cached `(computed_at, json)` pair for `window`, if present.
    fn read_stats_cache(&self, window: StatsWindow) -> Result<Option<(i64, String)>> {
        use rusqlite::OptionalExtension;
        self.with_reader(|conn| {
            Ok(conn
                .query_row(
                    "SELECT computed_at, json FROM stats_cache WHERE window = ?1",
                    [window.as_str()],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?)
        })
    }

    /// Upsert the cache row for `window`.
    fn write_stats_cache(&self, window: StatsWindow, computed_at: i64, stats: &Stats) -> Result<()> {
        let json = serde_json::to_string(stats)?;
        self.with_writer(|conn| {
            conn.execute(
                "INSERT INTO stats_cache(window, computed_at, json) VALUES (?1, ?2, ?3)
                 ON CONFLICT(window) DO UPDATE SET
                    computed_at = excluded.computed_at,
                    json = excluded.json",
                (window.as_str(), computed_at, &json),
            )?;
            Ok(())
        })
    }
}

/// Pure reduction from a window's sample rows to a [`Stats`] block.
pub(crate) fn compute(window: StatsWindow, rows: &[t1dm_core::SampleRow]) -> Stats {
    let bgs: Vec<f64> = rows.iter().filter_map(|r| r.bg).collect();
    let n = bgs.len();
    if n == 0 {
        return Stats::empty(window);
    }

    let sum: f64 = bgs.par_iter().sum();
    let mean = sum / n as f64;
    let var: f64 = bgs.par_iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let sd = var.sqrt();
    let cv = if mean != 0.0 { sd / mean * 100.0 } else { 0.0 };
    let gmi = 3.31 + 0.02392 * mean;

    let in_range = bgs.iter().filter(|&&v| (HYPO..=HYPER).contains(&v)).count();
    let below = bgs.iter().filter(|&&v| v < HYPO).count();
    let above = bgs.iter().filter(|&&v| v > HYPER).count();
    let tir = in_range as f64 / n as f64;
    let time_below = below as f64 / n as f64;
    let time_above = above as f64 / n as f64;

    let hypo_events = count_events(&bgs, |v| v < HYPO);
    let hyper_events = count_events(&bgs, |v| v > HYPER);

    let days = (window.millis() as f64 / 86_400_000.0).max(1.0);
    let total_carbs: f64 = rows.iter().filter_map(|r| r.carbs).sum();
    let total_bolus: f64 = rows.iter().filter_map(|r| r.bolus).sum();
    let total_basal: f64 = rows.iter().filter_map(|r| r.basal).sum();
    let mean_daily_carbs = total_carbs / days;
    let tdd = (total_bolus + total_basal) / days;
    let bolus_basal_ratio = if total_basal != 0.0 {
        total_bolus / total_basal
    } else {
        0.0
    };

    let hrs: Vec<f64> = rows.iter().filter_map(|r| r.hr).collect();
    let mean_hr = if hrs.is_empty() {
        0.0
    } else {
        hrs.iter().sum::<f64>() / hrs.len() as f64
    };

    let bg_hr_corr = pearson_paired(rows);

    Stats {
        window,
        tir,
        time_below,
        time_above,
        mean_bg: mean,
        gmi,
        cv,
        sd,
        hypo_events,
        hyper_events,
        mean_daily_carbs,
        tdd,
        bolus_basal_ratio,
        mean_hr,
        bg_hr_corr,
        n_samples: n as u32,
    }
}

/// Count contiguous excursion runs matching `pred`, summing their durations.
fn count_events(bgs: &[f64], pred: impl Fn(f64) -> bool) -> t1dm_core::stats::EventStat {
    let mut count = 0u32;
    let mut run = 0i64;
    let mut duration = 0i64;
    let mut open = false;
    for &v in bgs {
        if pred(v) {
            if !open {
                count += 1;
                open = true;
                run = 0;
            }
            run += SAMPLE_MS;
            duration += SAMPLE_MS;
            let _ = run;
        } else {
            open = false;
        }
    }
    t1dm_core::stats::EventStat {
        count,
        duration_ms: duration,
    }
}

/// Pearson correlation of BG and HR over rows where both are present.
fn pearson_paired(rows: &[t1dm_core::SampleRow]) -> f64 {
    let pairs: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|r| match (r.bg, r.hr) {
            (Some(b), Some(h)) => Some((b, h)),
            _ => None,
        })
        .collect();
    let n = pairs.len();
    if n < 2 {
        return 0.0;
    }
    let nf = n as f64;
    let mx = pairs.iter().map(|p| p.0).sum::<f64>() / nf;
    let my = pairs.iter().map(|p| p.1).sum::<f64>() / nf;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (x, y) in &pairs {
        let dx = x - mx;
        let dy = y - my;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    let denom = (sxx * syy).sqrt();
    if denom == 0.0 {
        0.0
    } else {
        sxy / denom
    }
}
