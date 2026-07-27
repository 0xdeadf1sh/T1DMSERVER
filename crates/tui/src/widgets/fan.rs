//! Prediction fan: the 7-quantile forecast reduced to three nested shaded
//! tiers around the median. It shades the *same* plot rect the [`BgChart`]
//! uses, and is drawn *before* the chart so the median line sits on top.
//!
//! Tiers, innermost → outermost, use `palette.fan[0..3]`:
//! - inner  `[q25, q75]` (the IQR)         — densest shade
//! - middle `[q10, q90]`
//! - outer  `[q05, q95]`                    — lightest shade
//!
//! [`BgChart`]: crate::widgets::chart::BgChart

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use t1dm_core::{BgUnit, QUANTILE_LEVELS};

use crate::theme::Theme;
use crate::widgets::chart::{plot_area, Proj};

/// One column of the fan: a timestamp and the seven quantile values (mg/dL) in
/// ascending [`QUANTILE_LEVELS`] order.
type FanColumn = (i64, [f64; 7]);

#[derive(Default)]
pub struct FanWidget {
    t_left: i64,
    t_right: i64,
    y_min: f64,
    y_max: f64,
    unit: BgUnit,
    points: Vec<FanColumn>,
}

impl FanWidget {
    pub fn new() -> Self {
        FanWidget::default()
    }

    pub fn window(mut self, t_left: i64, t_right: i64) -> Self {
        self.t_left = t_left;
        self.t_right = t_right;
        self
    }

    pub fn y_bounds(mut self, y_min: f64, y_max: f64) -> Self {
        self.y_min = y_min;
        self.y_max = y_max;
        self
    }

    /// The display unit whose axis the quantiles are projected onto. Must match
    /// the chart it shades, or the fan and the median line it wraps land on two
    /// different value axes.
    pub fn unit(mut self, unit: BgUnit) -> Self {
        self.unit = unit;
        self
    }

    /// Fan columns, sorted ascending by timestamp.
    pub fn points(mut self, mut points: Vec<FanColumn>) -> Self {
        points.sort_by_key(|&(ts, _)| ts);
        self.points = points;
        self
    }

    fn proj(&self) -> Proj {
        Proj {
            t_left: self.t_left,
            t_right: self.t_right,
            y_min: self.y_min,
            y_max: self.y_max,
            unit: self.unit,
        }
    }

    /// Linearly interpolate the seven quantiles at `ts`, or `None` if `ts`
    /// lies outside the covered horizon.
    fn quantiles_at(&self, ts: i64) -> Option<[f64; 7]> {
        let first = self.points.first()?;
        let last = self.points.last()?;
        if ts < first.0 || ts > last.0 {
            return None;
        }
        // Locate the bracketing pair.
        let hi = self.points.iter().position(|&(t, _)| t >= ts)?;
        if self.points[hi].0 == ts || hi == 0 {
            return Some(self.points[hi].1);
        }
        let (t0, q0) = self.points[hi - 1];
        let (t1, q1) = self.points[hi];
        let f = if t1 == t0 {
            0.0
        } else {
            (ts - t0) as f64 / (t1 - t0) as f64
        };
        let mut out = [0.0; 7];
        for k in 0..7 {
            out[k] = q0[k] + (q1[k] - q0[k]) * f;
        }
        Some(out)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &dyn Theme) {
        let plot = plot_area(area);
        if plot.width < 1 || plot.height < 1 || self.y_max <= self.y_min || self.points.is_empty() {
            return;
        }
        let pal = theme.palette();
        let proj = self.proj();

        // (lower quantile idx, upper quantile idx, palette fan idx, shade glyph)
        // drawn outer → inner so the denser inner tier wins each cell.
        let tiers: [(usize, usize, usize, char); 3] = [
            (0, 6, 2, '░'), // p05..p95
            (1, 5, 1, '▒'), // p10..p90
            (2, 4, 0, '▓'), // p25..p75 (IQR)
        ];
        debug_assert_eq!(QUANTILE_LEVELS.len(), 7);

        let span = (self.t_right - self.t_left) as f64;
        if span <= 0.0 {
            return;
        }
        let w = plot.width as f64;
        for cx in 0..plot.width {
            let ts = self.t_left + ((cx as f64 / w.max(1.0)) * span) as i64;
            let Some(q) = self.quantiles_at(ts) else {
                continue;
            };
            let x = plot.x + cx;
            for &(lo, up, fan_idx, glyph) in &tiers {
                let color: Color = pal.fan[fan_idx];
                let top = proj.row(plot, q[up]);
                let bot = proj.row(plot, q[lo]);
                let (top, bot) = (top.min(bot), top.max(bot));
                for y in top..=bot {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(glyph);
                        cell.fg = color;
                    }
                }
            }
        }
    }
}
