//! Time-aligned strips (insulin / carbs / HR). Each strip is a short
//! braille area-plot sharing the chart's time axis: a label at the top of the
//! left gutter, the in-window peak value at the bottom of the gutter, a faint
//! back-grid, and the value envelope drawn as a 2×4 braille silhouette so a
//! six-row strip resolves far more shape than eighth-block bars would. Values
//! are normalised to the strip's own in-window peak so each series fills its
//! rows regardless of magnitude.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::theme::{Palette, Theme};
use crate::widgets::chart::{plot_area, Y_GUTTER};

/// Braille dot bit for a dot at (col ∈ {0,1}, row ∈ 0..4) within a cell,
/// numbered off the U+2800 base — the same convention the BG chart uses.
const BRAILLE: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Fractions of the strip height at which to lay faint horizontal gridlines.
const H_GRID: [f64; 2] = [1.0 / 3.0, 2.0 / 3.0];

#[derive(Default)]
pub struct StripWidget {
    t_left: i64,
    t_right: i64,
    label: String,
    color: Option<Color>,
    data: Vec<(i64, f64)>,
    /// Absolute epoch-ms tick timestamps, projected each frame through the
    /// strip's own window so the vertical back-grid pans with the data and
    /// stays column-aligned with the BG chart's grid and hh:mm axis.
    ticks: Vec<i64>,
}

impl StripWidget {
    pub fn new() -> Self {
        StripWidget::default()
    }

    pub fn window(mut self, t_left: i64, t_right: i64) -> Self {
        self.t_left = t_left;
        self.t_right = t_right;
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn data(mut self, data: Vec<(i64, f64)>) -> Self {
        self.data = data;
        self
    }

    pub fn ticks(mut self, ticks: Vec<i64>) -> Self {
        self.ticks = ticks;
        self
    }

    /// Sub-cell dot column (2× the cell resolution) for `ts`, or `None` when
    /// the timestamp falls outside the window.
    fn dot_col(&self, ts: i64, dw: usize) -> Option<usize> {
        let span = (self.t_right - self.t_left) as f64;
        if span <= 0.0 || dw == 0 {
            return None;
        }
        let f = (ts - self.t_left) as f64 / span;
        if !(0.0..=1.0).contains(&f) {
            return None;
        }
        Some((f * (dw as f64 - 1.0)).round() as usize)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &dyn Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let pal = theme.palette();
        let color = self.color.unwrap_or(pal.secondary);
        let gutter = Y_GUTTER.min(area.width) as usize;
        let label_style = Style::default().fg(pal.dim).bg(pal.bg);
        // The gutter name wears the strip's own colour so a glance tells the
        // series apart; the peak-value scale below it stays faint.
        let name_style = Style::default().fg(color).bg(pal.bg);

        // Label at the top of the gutter, right-aligned.
        if gutter >= 2 {
            let text = format!("{:>w$}", truncate(&self.label, gutter - 1), w = gutter - 1);
            buf.set_string(area.x, area.y, &text, name_style);
        }

        let plot = plot_area(area);
        if plot.width == 0 || plot.height == 0 {
            return;
        }

        // Bucket the per-sub-column peak at 2× horizontal resolution so the
        // silhouette's leading/trailing edges land on the correct half-cell.
        let dw = plot.width as usize * 2;
        let dh = plot.height as usize * 4;
        let mut cols = vec![0.0f64; dw];
        let mut peak = 0.0f64;
        for &(ts, v) in &self.data {
            if v <= 0.0 {
                continue;
            }
            if let Some(dx) = self.dot_col(ts, dw) {
                if v > cols[dx] {
                    cols[dx] = v;
                }
                if v > peak {
                    peak = v;
                }
            }
        }
        if peak <= 0.0 {
            return;
        }

        // Peak value at the bottom of the gutter, so the axis has a scale.
        if gutter >= 2 && area.height >= 2 {
            let text = format!(
                "{:>w$}",
                truncate(&fmt_peak(peak), gutter - 1),
                w = gutter - 1
            );
            buf.set_string(area.x, area.bottom() - 1, &text, label_style);
        }

        // Faint back-grid first, so the braille silhouette lays over it.
        draw_grid(plot, buf, pal, self.t_left, self.t_right, &self.ticks);

        // Braille area silhouette: for each occupied sub-column fill a run of
        // dots from the baseline up to the value's height. Adjacent columns
        // merge into a continuous envelope; a lone impulse stays a hairline.
        let mut bits = vec![0u8; plot.width as usize * plot.height as usize];
        let baseline = dh as i32 - 1;
        for (dx, &v) in cols.iter().enumerate() {
            if v <= 0.0 {
                continue;
            }
            let frac = (v / peak).clamp(0.0, 1.0);
            let top = ((1.0 - frac) * (dh as f64 - 1.0)).round() as i32;
            for py in top..=baseline {
                set_dot(&mut bits, plot.width, dx as i32, py);
            }
        }
        blit(&bits, plot, buf, color);
    }
}

/// Set one braille dot in the packed per-cell bit buffer, bounds-checked.
fn set_dot(bits: &mut [u8], w: u16, px: i32, py: i32) {
    if px < 0 || py < 0 {
        return;
    }
    let (cx, cy) = (px as usize / 2, py as usize / 4);
    if cx >= w as usize {
        return;
    }
    let idx = cy * w as usize + cx;
    if idx >= bits.len() {
        return;
    }
    bits[idx] |= BRAILLE[(py % 4) as usize][(px % 2) as usize];
}

/// Emit the packed braille cells into `buf`, one fg colour throughout.
fn blit(bits: &[u8], plot: Rect, buf: &mut Buffer, color: Color) {
    for cy in 0..plot.height {
        for cx in 0..plot.width {
            let b = bits[cy as usize * plot.width as usize + cx as usize];
            if b == 0 {
                continue;
            }
            if let Some(ch) = char::from_u32(0x2800 + b as u32) {
                if let Some(cell) = buf.cell_mut((plot.x + cx, plot.y + cy)) {
                    cell.set_char(ch);
                    cell.fg = color;
                }
            }
        }
    }
}

/// Faint back-grid: a couple of horizontal value levels and a few vertical
/// time markers, painted only onto still-blank cells so the braille curve
/// laid on top always wins the shared cells.
fn draw_grid(
    plot: Rect,
    buf: &mut Buffer,
    pal: &Palette,
    t_left: i64,
    t_right: i64,
    ticks: &[i64],
) {
    for &f in &H_GRID {
        let row = plot.y + ((1.0 - f) * (plot.height as f64 - 1.0)).round() as u16;
        if row >= plot.bottom() {
            continue;
        }
        for x in plot.x..plot.right() {
            if let Some(cell) = buf.cell_mut((x, row)) {
                if cell.symbol() == " " {
                    cell.set_char('┈');
                    cell.fg = pal.grid;
                }
            }
        }
    }
    // Vertical time markers projected through the strip's own window, so they
    // pan with the data and stay column-aligned with the BG chart's grid.
    let span = (t_right - t_left) as f64;
    if span <= 0.0 {
        return;
    }
    for &ts in ticks {
        let f = (ts - t_left) as f64 / span;
        if !(0.0..=1.0).contains(&f) {
            continue;
        }
        let col = plot.x + (f * (plot.width as f64 - 1.0)).round() as u16;
        if col >= plot.right() {
            continue;
        }
        for y in plot.y..plot.bottom() {
            if let Some(cell) = buf.cell_mut((col, y)) {
                if cell.symbol() == " " {
                    cell.set_char('┊');
                    cell.fg = pal.grid;
                }
            }
        }
    }
}

/// Compact rendering of a strip's in-window peak for the gutter scale.
fn fmt_peak(peak: f64) -> String {
    if peak >= 10.0 {
        format!("{peak:.0}")
    } else {
        format!("{peak:.1}")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}
