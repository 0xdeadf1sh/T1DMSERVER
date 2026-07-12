//! Custom braille BG chart: the actual glucose line (past + future) drawn on
//! a 2×4 braille sub-cell canvas, threshold reference lines, a "now" divider,
//! a pulsing live point, and note/photo markers along the baseline. The
//! prediction fan is a sibling widget ([`super::fan`]) that shades the same
//! plot rect *before* this widget lays the median line on top.
//!
//! The projection/colour helpers here are shared (`pub(crate)`) with the fan
//! and strip widgets so every band aligns to one time axis.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use t1dm_core::BgUnit;

use crate::anim::{lerp, pulse};
use crate::theme::Theme;

/// Left gutter reserved for y-axis / strip labels. Every time-aligned band
/// insets this same amount so their columns coincide.
pub(crate) const Y_GUTTER: u16 = 6;

/// Glucose thresholds (mg/dL) drawn as horizontal reference lines.
pub(crate) const HYPO_MGDL: f64 = 70.0;
pub(crate) const HYPER_MGDL: f64 = 180.0;

/// The plotting rect for a band: the given region minus the label gutter.
pub(crate) fn plot_area(area: Rect) -> Rect {
    let gutter = Y_GUTTER.min(area.width);
    Rect {
        x: area.x + gutter,
        y: area.y,
        width: area.width.saturating_sub(gutter),
        height: area.height,
    }
}

/// Time→x and value→y mapping shared by chart, fan, and strips.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Proj {
    pub t_left: i64,
    pub t_right: i64,
    pub y_min: f64,
    pub y_max: f64,
}

impl Proj {
    /// Fraction 0..=1 across the window for `ts`, or `None` if out of range.
    fn x_frac(&self, ts: i64) -> Option<f64> {
        let span = (self.t_right - self.t_left) as f64;
        if span <= 0.0 {
            return None;
        }
        let f = (ts - self.t_left) as f64 / span;
        (0.0..=1.0).contains(&f).then_some(f)
    }

    /// Fraction 0..=1 up the value axis (clamped).
    fn y_frac(&self, v: f64) -> f64 {
        let span = (self.y_max - self.y_min).max(1e-6);
        ((v - self.y_min) / span).clamp(0.0, 1.0)
    }

    /// Cell column within `plot` for `ts`, or `None` if outside the window.
    pub(crate) fn col(&self, plot: Rect, ts: i64) -> Option<u16> {
        let f = self.x_frac(ts)?;
        let w = plot.width.max(1) as f64;
        Some(plot.x + (f * (w - 1.0)).round() as u16)
    }

    /// Cell row within `plot` for a value (clamped into the plot).
    pub(crate) fn row(&self, plot: Rect, v: f64) -> u16 {
        let f = self.y_frac(v);
        let h = plot.height.max(1) as f64;
        plot.y + ((1.0 - f) * (h - 1.0)).round() as u16
    }

    /// Braille dot coordinates (2× cols, 4× rows) for a data point.
    fn dot(&self, plot: Rect, ts: i64, v: f64) -> Option<(i32, i32)> {
        let f = self.x_frac(ts)?;
        let dw = (plot.width as i32 * 2).max(1);
        let dh = (plot.height as i32 * 4).max(1);
        let px = (f * (dw - 1) as f64).round() as i32;
        let py = ((1.0 - self.y_frac(v)) * (dh - 1) as f64).round() as i32;
        Some((px, py))
    }
}

/// Pick a round mg/dL interval for the horizontal back-grid so the visible
/// value span holds roughly two to six lines.
fn grid_step(y_min: f64, y_max: f64) -> f64 {
    let range = (y_max - y_min).max(1.0);
    for &s in &[20.0, 25.0, 50.0, 100.0, 200.0] {
        if range / s <= 6.0 {
            return s;
        }
    }
    250.0
}

/// Split a [`Color::Rgb`] into channels, else `None`.
fn rgb(c: Color) -> Option<(f32, f32, f32)> {
    match c {
        Color::Rgb(r, g, b) => Some((r as f32, g as f32, b as f32)),
        _ => None,
    }
}

/// Linear RGB blend `a`→`b` by `t` (falls back to `a` for non-RGB colours).
pub(crate) fn mix(a: Color, b: Color, t: f32) -> Color {
    match (rgb(a), rgb(b)) {
        (Some((ar, ag, ab)), Some((br, bg, bb))) => Color::Rgb(
            lerp(ar, br, t) as u8,
            lerp(ag, bg, t) as u8,
            lerp(ab, bb, t) as u8,
        ),
        _ => a,
    }
}

/// Braille bit for a dot at (col ∈ {0,1}, row ∈ 0..4) within a cell.
const BRAILLE: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// A braille sub-cell canvas: 2×4 dots per terminal cell, one fg colour per
/// cell (last writer wins).
struct Braille {
    w: u16,
    h: u16,
    bits: Vec<u8>,
    color: Vec<Color>,
}

impl Braille {
    fn new(w: u16, h: u16) -> Self {
        let n = w as usize * h as usize;
        Braille {
            w,
            h,
            bits: vec![0u8; n],
            color: vec![Color::Reset; n],
        }
    }

    fn plot(&mut self, px: i32, py: i32, color: Color) {
        if px < 0 || py < 0 {
            return;
        }
        let (cx, cy) = (px as u16 / 2, py as u16 / 4);
        if cx >= self.w || cy >= self.h {
            return;
        }
        let idx = cy as usize * self.w as usize + cx as usize;
        self.bits[idx] |= BRAILLE[(py % 4) as usize][(px % 2) as usize];
        self.color[idx] = color;
    }

    /// Bresenham segment in dot space.
    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.plot(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn blit(&self, plot: Rect, buf: &mut Buffer) {
        for cy in 0..self.h {
            for cx in 0..self.w {
                let idx = cy as usize * self.w as usize + cx as usize;
                let bits = self.bits[idx];
                if bits == 0 {
                    continue;
                }
                if let Some(ch) = char::from_u32(0x2800 + bits as u32) {
                    if let Some(cell) = buf.cell_mut((plot.x + cx, plot.y + cy)) {
                        cell.set_char(ch);
                        cell.fg = self.color[idx];
                    }
                }
            }
        }
    }
}

/// The BG time-series chart. Data and window are set through the builder; the
/// locked surface is [`BgChart::new`] and [`BgChart::render`].
#[derive(Default)]
pub struct BgChart {
    t_left: i64,
    t_right: i64,
    y_min: f64,
    y_max: f64,
    unit: BgUnit,
    now_ms: i64,
    pulse: f32,
    bg: Vec<(i64, f64)>,
    median: Vec<(i64, f64)>,
    notes: Vec<i64>,
    photos: Vec<i64>,
    /// Timestamps of the vertical gridlines / time-axis ticks (epoch ms).
    ticks: Vec<i64>,
    /// Latest heart rate in bpm for the top-right beating heart; None => idle.
    hr: Option<f64>,
    /// Beat phase 0..1, advanced by the pane at bpm/60 Hz.
    heart_phase: f32,
}

impl BgChart {
    pub fn new() -> Self {
        BgChart::default()
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

    pub fn unit(mut self, unit: BgUnit) -> Self {
        self.unit = unit;
        self
    }

    pub fn now(mut self, now_ms: i64) -> Self {
        self.now_ms = now_ms;
        self
    }

    pub fn pulse(mut self, pulse: f32) -> Self {
        self.pulse = pulse;
        self
    }

    pub fn bg(mut self, bg: Vec<(i64, f64)>) -> Self {
        self.bg = bg;
        self
    }

    pub fn median(mut self, median: Vec<(i64, f64)>) -> Self {
        self.median = median;
        self
    }

    pub fn notes(mut self, notes: Vec<i64>) -> Self {
        self.notes = notes;
        self
    }

    pub fn photos(mut self, photos: Vec<i64>) -> Self {
        self.photos = photos;
        self
    }

    /// Timestamps (epoch ms) at which to drop faint vertical gridlines. These
    /// coincide with the time-axis tick labels the dashboard paints beneath the
    /// chart, so the two align column-for-column.
    pub fn ticks(mut self, ticks: Vec<i64>) -> Self {
        self.ticks = ticks;
        self
    }

    /// The current heart rate (bpm) for the top-right beating-heart indicator.
    pub fn hr(mut self, hr: Option<f64>) -> Self {
        self.hr = hr;
        self
    }

    /// Beat phase 0..1 (the caller advances it at bpm/60 Hz).
    pub fn heart_phase(mut self, phase: f32) -> Self {
        self.heart_phase = phase;
        self
    }

    fn proj(&self) -> Proj {
        Proj {
            t_left: self.t_left,
            t_right: self.t_right,
            y_min: self.y_min,
            y_max: self.y_max,
        }
    }

    /// Severity colour for a glucose value.
    fn bg_color(&self, pal: &crate::theme::Palette, v: f64) -> Color {
        if v < HYPO_MGDL {
            pal.hypo
        } else if v > HYPER_MGDL {
            pal.hyper
        } else {
            pal.primary
        }
    }

    /// Draw the widget into `area`. `area` includes the label gutter; the plot
    /// occupies the region to its right.
    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &dyn Theme) {
        let plot = plot_area(area);
        if plot.width < 2 || plot.height < 2 || self.y_max <= self.y_min {
            return;
        }
        let pal = theme.palette();
        let glyphs = theme.glyphs();
        let proj = self.proj();

        self.draw_labels(area, plot, buf, pal);
        self.draw_grid(area, plot, buf, proj, pal);
        self.draw_reference_lines(plot, buf, proj, pal);
        self.draw_now_divider(plot, buf, proj, pal);
        self.draw_line(&self.bg, plot, buf, proj, pal, false);
        self.draw_line(&self.median, plot, buf, proj, pal, true);
        self.draw_live_point(plot, buf, proj, pal, glyphs);
        self.draw_markers(plot, buf, proj, pal, glyphs);
        self.draw_heart(plot, buf, theme);
    }

    fn draw_labels(&self, area: Rect, plot: Rect, buf: &mut Buffer, pal: &crate::theme::Palette) {
        let gutter = Y_GUTTER.min(area.width) as usize;
        if gutter < 2 {
            return;
        }
        let style = Style::default().fg(pal.dim).bg(pal.bg);
        let proj = self.proj();
        let mut seen: Vec<u16> = Vec::new();
        let label = |value: f64, buf: &mut Buffer, seen: &mut Vec<u16>| {
            let row = proj.row(plot, value);
            if seen.contains(&row) {
                return;
            }
            seen.push(row);
            let text = self.unit.format(value);
            let text = format!("{text:>w$}", w = gutter.saturating_sub(1));
            buf.set_string(area.x, row, &text, style);
        };
        label(self.y_max, buf, &mut seen);
        if self.y_max > HYPER_MGDL && HYPER_MGDL > self.y_min {
            label(HYPER_MGDL, buf, &mut seen);
        }
        if self.y_max > HYPO_MGDL && HYPO_MGDL > self.y_min {
            label(HYPO_MGDL, buf, &mut seen);
        }
        label(self.y_min, buf, &mut seen);
    }

    /// Faint back-grid: horizontal gridlines at round glucose levels (with
    /// small left-gutter value labels) and vertical gridlines at the time
    /// ticks. Drawn *after* labels/fan but *before* the reference lines, the
    /// now-divider and the data, and only onto still-blank cells, so nothing it
    /// paints ever competes with the series on top.
    fn draw_grid(
        &self,
        area: Rect,
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
    ) {
        let gutter = Y_GUTTER.min(area.width) as usize;
        let hypo_row =
            (HYPO_MGDL > self.y_min && HYPO_MGDL < self.y_max).then(|| proj.row(plot, HYPO_MGDL));
        let hyper_row = (HYPER_MGDL > self.y_min && HYPER_MGDL < self.y_max)
            .then(|| proj.row(plot, HYPER_MGDL));
        let now_col = proj.col(plot, self.now_ms);
        // Rows already spoken for by draw_labels / reference lines: skip both
        // the gridline and its label there so nothing is drawn twice.
        let skip_rows = [
            Some(plot.y),
            Some(plot.bottom().saturating_sub(1)),
            hypo_row,
            hyper_row,
        ];

        // --- horizontal gridlines at round mg/dL levels --------------------
        let step = grid_step(self.y_min, self.y_max);
        let mut level = (self.y_min / step).ceil() * step;
        while level < self.y_max {
            if level > self.y_min {
                let row = proj.row(plot, level);
                if !skip_rows.contains(&Some(row)) {
                    for x in plot.x..plot.right() {
                        if let Some(cell) = buf.cell_mut((x, row)) {
                            if cell.symbol() == " " {
                                cell.set_char('┈');
                                cell.fg = pal.grid;
                            }
                        }
                    }
                    if gutter >= 2 {
                        let text = self.unit.format(level);
                        let text = format!("{text:>w$}", w = gutter.saturating_sub(1));
                        crate::boot::put_str(buf, area, area.x, row, &text, pal.dim, pal.bg);
                    }
                }
            }
            level += step;
        }

        // --- vertical gridlines at the time ticks --------------------------
        for &ts in &self.ticks {
            let Some(col) = proj.col(plot, ts) else {
                continue;
            };
            if Some(col) == now_col {
                continue; // the now-divider already owns this column
            }
            for y in plot.y..plot.bottom() {
                if Some(y) == hypo_row || Some(y) == hyper_row {
                    continue; // keep the reference lines unbroken
                }
                if let Some(cell) = buf.cell_mut((col, y)) {
                    if cell.symbol() == " " {
                        cell.set_char('┊');
                        cell.fg = pal.grid;
                    }
                }
            }
        }
    }

    fn draw_reference_lines(
        &self,
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
    ) {
        for &level in &[HYPO_MGDL, HYPER_MGDL] {
            if level <= self.y_min || level >= self.y_max {
                continue;
            }
            let row = proj.row(plot, level);
            for x in plot.x..plot.right() {
                if let Some(cell) = buf.cell_mut((x, row)) {
                    if cell.symbol() == " " {
                        cell.set_char('┈');
                        cell.fg = pal.grid;
                    }
                }
            }
        }
    }

    fn draw_now_divider(
        &self,
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
    ) {
        let Some(col) = proj.col(plot, self.now_ms) else {
            return;
        };
        for y in plot.y..plot.bottom() {
            if let Some(cell) = buf.cell_mut((col, y)) {
                if cell.symbol() == " " || cell.symbol() == "┈" {
                    cell.set_char('│');
                    cell.fg = pal.dim;
                }
            }
        }
    }

    fn draw_line(
        &self,
        points: &[(i64, f64)],
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
        predicted: bool,
    ) {
        if points.is_empty() {
            return;
        }
        let mut grid = Braille::new(plot.width, plot.height);
        let mut prev: Option<(i32, i32)> = None;
        for &(ts, v) in points {
            let Some((px, py)) = proj.dot(plot, ts, v.clamp(self.y_min, self.y_max)) else {
                prev = None;
                continue;
            };
            let color = if predicted {
                pal.secondary
            } else {
                self.bg_color(pal, v)
            };
            match prev {
                Some((qx, qy)) => grid.line(qx, qy, px, py, color),
                None => grid.plot(px, py, color),
            }
            prev = Some((px, py));
        }
        grid.blit(plot, buf);
    }

    fn draw_live_point(
        &self,
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
        glyphs: &crate::theme::Glyphs,
    ) {
        let Some(&(ts, v)) = self.bg.last() else {
            return;
        };
        let Some(col) = proj.col(plot, ts) else {
            return;
        };
        let row = proj.row(plot, v.clamp(self.y_min, self.y_max));
        let base = self.bg_color(pal, v);
        let color = mix(base, pal.fg, pulse(self.pulse));
        if let Some(cell) = buf.cell_mut((col, row)) {
            cell.set_char(glyphs.live_point);
            cell.fg = color;
        }
    }

    fn draw_markers(
        &self,
        plot: Rect,
        buf: &mut Buffer,
        proj: Proj,
        pal: &crate::theme::Palette,
        glyphs: &crate::theme::Glyphs,
    ) {
        let row = plot.bottom().saturating_sub(1);
        for &ts in &self.notes {
            if let Some(col) = proj.col(plot, ts) {
                if let Some(cell) = buf.cell_mut((col, row)) {
                    cell.set_char(glyphs.note_marker);
                    cell.fg = pal.secondary;
                }
            }
        }
        for &ts in &self.photos {
            if let Some(col) = proj.col(plot, ts) {
                if let Some(cell) = buf.cell_mut((col, row)) {
                    cell.set_char(glyphs.photo_marker);
                    cell.fg = pal.accent;
                }
            }
        }
    }

    /// A beating heart pinned to the plot's top-right corner. Its systolic flare
    /// is keyed to `heart_phase`, which the caller advances at bpm/60 Hz, so the
    /// on-screen beat matches the live heart rate; the bpm is printed beneath it.
    fn draw_heart(&self, plot: Rect, buf: &mut Buffer, theme: &dyn Theme) {
        let pal = theme.palette();
        const HEART: [&str; 6] = [
            " ██  ██ ",
            "████████",
            "████████",
            " ██████ ",
            "  ████  ",
            "   ██   ",
        ];
        let hw = HEART[0].chars().count() as u16;
        let hh = HEART.len() as u16;
        if plot.width < hw + 2 || plot.height < hh + 1 {
            return;
        }
        let x0 = plot.right().saturating_sub(hw + 1);
        let y0 = plot.y;

        let bpm = self.hr.filter(|v| *v > 0.0);
        // Systolic thump: a sharp flare at each beat, decaying over ~40% of the
        // cycle. heart_phase already advances at the real rate upstream.
        let ph = self.heart_phase.rem_euclid(1.0);
        let beat = if bpm.is_some() {
            (1.0 - ph / 0.4).max(0.0).powf(1.6)
        } else {
            0.0
        };
        let rest = mix(theme.heart_color(), pal.bg, 0.45);
        let color = if bpm.is_some() {
            mix(rest, pal.fg, beat)
        } else {
            pal.dim
        };

        for (ry, line) in HEART.iter().enumerate() {
            let y = y0 + ry as u16;
            if y >= plot.bottom() {
                break;
            }
            for (rx, ch) in line.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let x = x0 + rx as u16;
                if x < plot.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char('█');
                        cell.fg = color;
                    }
                }
            }
        }

        let label = match bpm {
            Some(v) => format!("{} BPM", v.round() as i64),
            None => "-- BPM".to_string(),
        };
        let lw = label.chars().count() as u16;
        let lx = x0 + hw.saturating_sub(lw) / 2;
        let ly = y0 + hh;
        if ly < plot.bottom() {
            crate::boot::put_str(buf, plot, lx, ly, &label, color, pal.bg);
        }
    }
}
