//! Dashboard pane — the default view. A tall braille BG chart (actual line
//! past + future) with the shaded quantile fan and note/photo markers sits at
//! the top; the taller time-aligned insulin/carbs/HR strips and a circadian
//! bio-time gauge are stacked beneath it and revealed by scrolling down. All
//! data is pulled live through the [`Ctx`] store handle each frame.
//!
//! Keymap: Left/Right pan, `j`/`k` zoom, Up/Down and PageUp/PageDown scroll
//! the stacked content vertically, `=` reset, `0` snap-to-now, `f` follow-lock,
//! `i` cycle the insulin series; mouse drag pans and the wheel zooms. The
//! [`PaneView`] surface and [`InsulinDisplay`] are locked here.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Frame;

use t1dm_core::{snap_grid, BgUnit, SampleRow, Series, GRID_MS, TOD_BINS};

use super::{Action, Ctx, PaneView};
use crate::boot::{put, put_str};
use crate::layout::LayoutMode;
use crate::theme::Palette;
use crate::widgets::chart::{plot_area, Proj, HYPER_MGDL, HYPO_MGDL, Y_GUTTER};
use crate::widgets::{BgChart, BioTimeGauge, FanWidget, ScrollState, StripWidget};

const DEFAULT_ZOOM_MS: i64 = 5 * 60 * 1000;
const MIN_ZOOM_MS: i64 = 60_000;
const MAX_ZOOM_MS: i64 = 4 * 3_600_000;
/// Columns panned per arrow press / drag column.
const PAN_COLS: i64 = 8;

/// Rows per insulin/carb/HR strip (label + several plot rows).
const STRIP_H: u16 = 6;
/// The single time-axis row painted directly beneath the BG chart.
const AXIS_H: u16 = 1;
/// A horizontal divider row inserted between stacked sections.
const SEP_H: u16 = 1;
/// Rows for the bio-time section — enough to seat a large round analog dial
/// (the freed HR strip gives it the room).
const BIO_H: u16 = 16;
/// Degraded bio-time height for short terminals where the full height would
/// swallow the fold; the clock widget draws a smaller dial into whatever it is
/// given.
const BIO_H_MIN: u16 = 11;
/// Clamp on the BG chart's natural height so that, on very tall terminals,
/// the strips begin to peek above the fold without any scrolling.
const MIN_CHART_H: u16 = 6;
const MAX_CHART_H: u16 = 36;

/// Which insulin series the strip renders (dashboard `i` key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InsulinDisplay {
    /// bolus + basal (display-derived).
    #[default]
    Total,
    Bolus,
    Basal,
}

impl InsulinDisplay {
    pub fn next(self) -> Self {
        match self {
            InsulinDisplay::Total => InsulinDisplay::Bolus,
            InsulinDisplay::Bolus => InsulinDisplay::Basal,
            InsulinDisplay::Basal => InsulinDisplay::Total,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            InsulinDisplay::Total => "total",
            InsulinDisplay::Bolus => "bolus",
            InsulinDisplay::Basal => "basal",
        }
    }
    fn value(self, r: &SampleRow) -> Option<f64> {
        match self {
            InsulinDisplay::Total => r.total_insulin(),
            InsulinDisplay::Bolus => r.bolus,
            InsulinDisplay::Basal => r.basal,
        }
    }
}

pub struct DashboardPane {
    /// Live auto-follow lock (`f`).
    pub follow: bool,
    /// Horizontal zoom in milliseconds-per-column (set by j/k).
    pub zoom_ms: i64,
    /// Pan offset from "now" in ms (Left/Right arrows).
    pub pan_ms: i64,
    pub insulin: InsulinDisplay,
    /// Live-point pulse phase, advanced by `tick`.
    pub pulse: f32,
    /// Mouse-drag anchor column, while a left drag is in flight.
    drag_anchor: Option<u16>,
    /// Vertical scroll over the stacked chart/strips/gauge content.
    scroll: ScrollState,
    /// Beat phase 0..1 for the BG chart's heart, advanced at the live bpm.
    heart_phase: f32,
    /// Most recent heart rate (bpm) in view, set on render, read on tick.
    hr_bpm: Option<f64>,
}

impl Default for DashboardPane {
    fn default() -> Self {
        DashboardPane {
            follow: true,
            zoom_ms: DEFAULT_ZOOM_MS,
            pan_ms: 0,
            insulin: InsulinDisplay::Total,
            pulse: 0.0,
            drag_anchor: None,
            scroll: ScrollState::new(),
            heart_phase: 0.0,
            hr_bpm: None,
        }
    }
}

impl DashboardPane {
    fn zoom_in(&mut self) {
        self.zoom_ms = (self.zoom_ms * 3 / 4).max(MIN_ZOOM_MS);
    }

    fn zoom_out(&mut self) {
        self.zoom_ms = (self.zoom_ms * 4 / 3).min(MAX_ZOOM_MS);
    }

    fn pan_step(&self) -> i64 {
        self.zoom_ms * PAN_COLS
    }

    fn draw_title(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        unit: BgUnit,
        t_right: i64,
        tz: i32,
        pal: &Palette,
    ) {
        let follow = if self.follow { "LIVE" } else { "PAUSE" };
        let s = format!(
            " BG {}  [{}]  zoom {}/col  ins:{}  \u{2192}{}",
            unit.label(),
            follow,
            fmt_zoom(self.zoom_ms),
            self.insulin.label(),
            fmt_clock(t_right, tz),
        );
        let s = truncate(&s, area.width as usize);
        buf.set_string(
            area.x,
            area.y,
            &s,
            Style::default().fg(pal.primary).bg(pal.bg),
        );
    }
}

impl PaneView for DashboardPane {
    fn title(&self) -> &str {
        "Dashboard"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        if self.follow {
            self.pan_ms = 0;
        }
        let theme = ctx.theme;
        let pal = theme.palette();

        if area.width < Y_GUTTER + 8 || area.height < 5 {
            let buf = frame.buffer_mut();
            buf.set_string(
                area.x,
                area.y,
                truncate("dashboard: enlarge terminal", area.width as usize),
                Style::default().fg(pal.dim).bg(pal.bg),
            );
            return;
        }

        // --- visible time window -------------------------------------------
        let plot_w = area.width.saturating_sub(Y_GUTTER).max(1) as i64;
        let ms_per_col = self.zoom_ms.max(1);
        let span = ms_per_col * plot_w;
        let now = snap_grid(ctx.now_ms);
        let future = span / 4;
        let t_right = now + future + self.pan_ms;
        let t_left = t_right - span;

        // --- pull samples ---------------------------------------------------
        let n_pts = (span / GRID_MS + 4).clamp(1, 40_000) as usize;
        let samples = ctx
            .store
            .get_samples(&Series::ALL, Some(t_left), Some(t_right), Some(n_pts), None)
            .unwrap_or_default();

        let bg_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| r.bg.map(|v| (r.ts, v)))
            .collect();
        let ins_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| self.insulin.value(r).map(|v| (r.ts, v)))
            .collect();
        let carb_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| r.carbs.map(|v| (r.ts, v)))
            .collect();
        let hr_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| r.hr.map(|v| (r.ts, v)))
            .collect();
        // Latest in-view heart rate drives the beating-heart indicator.
        let hr_bpm = hr_pts.last().map(|&(_, v)| v).filter(|v| *v > 0.0);
        self.hr_bpm = hr_bpm;
        let tz = samples.last().map(|r| r.tz_offset).unwrap_or(0);

        // --- pull the latest forecast --------------------------------------
        let pred = ctx.store.get_prediction_latest().ok().flatten();
        let mut median_pts: Vec<(i64, f64)> = Vec::new();
        let mut fan_pts: Vec<(i64, [f64; 7])> = Vec::new();
        let mut tod = vec![0.0f64; TOD_BINS];
        let mut conf = 0.0f64;
        if let Some(p) = &pred {
            let base = p.made_at;
            for (i, &m) in p.line.iter().enumerate() {
                median_pts.push((base + i as i64 * GRID_MS, m));
            }
            for i in 0..p.line.len() {
                let mut q = [0.0f64; 7];
                let mut ok = true;
                for (k, slot) in q.iter_mut().enumerate() {
                    match p.fan.get(k).and_then(|row| row.get(i)) {
                        Some(&v) => *slot = v,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    fan_pts.push((base + i as i64 * GRID_MS, q));
                }
            }
            if p.tod.len() == TOD_BINS {
                tod = p.tod.clone();
            }
            conf = p.tod_conf;
        }

        // --- value axis: always straddle the 70/180 thresholds -------------
        let mut lo = HYPO_MGDL;
        let mut hi = HYPER_MGDL;
        for &(_, v) in bg_pts.iter().chain(median_pts.iter()) {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        for &(_, q) in &fan_pts {
            lo = lo.min(q[0]);
            hi = hi.max(q[6]);
        }
        lo = (lo - 10.0).max(40.0);
        hi = (hi + 10.0).min(400.0);
        if hi - lo < 40.0 {
            hi = (lo + 40.0).min(400.0);
            lo = (hi - 40.0).max(40.0);
        }

        // --- markers --------------------------------------------------------
        let notes: Vec<i64> = ctx
            .store
            .get_notes(Some(t_left), Some(t_right))
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.ts)
            .collect();
        let photos: Vec<i64> = ctx
            .store
            .get_photos(Some(t_left), Some(t_right))
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.ts)
            .collect();

        // --- stacked virtual layout ----------------------------------------
        // The BG chart occupies the top and takes its natural (large) size; the
        // taller strips and the bio-time gauge live beneath it. The whole
        // column is rendered into an off-screen buffer of the full stacked
        // height, then the scrolled slice is blitted into the visible viewport.
        let want_strips = !matches!(ctx.layout, LayoutMode::Mobile);
        let viewport_h = area.height - 1; // guard above ensures height >= 5
        let chart_h = viewport_h.clamp(MIN_CHART_H, MAX_CHART_H);
        let n_strips: u16 = if want_strips { 3 } else { 0 };
        let strips_total = n_strips * STRIP_H;
        let bio_h = if viewport_h < 10 { BIO_H_MIN } else { BIO_H };
        // One divider rule seats above each strip and above the clock, so the
        // stacked sections read as distinct panels rather than bleeding together.
        let n_sep = n_strips + 1;
        let sep_total = n_sep * SEP_H;
        let total_h = chart_h + AXIS_H + sep_total + strips_total + bio_h;

        // Evenly spaced, wall-clock-rounded time ticks shared by the chart's
        // vertical back-grid and the axis row painted beneath it.
        let ticks = time_ticks(t_left, t_right, tz, plot_w);

        self.scroll
            .set_bounds(total_h as usize, viewport_h as usize);
        let offset = self.scroll.offset as u16;

        // Virtual buffer spanning the full stacked content, pre-filled with the
        // theme background so hidden cells copy cleanly into the viewport.
        let content_top = area.y + 1;
        let virt_area = Rect::new(area.x, 0, area.width, total_h);
        let mut virt = Buffer::empty(virt_area);
        for y in 0..total_h {
            for x in area.x..area.right() {
                if let Some(cell) = virt.cell_mut((x, y)) {
                    cell.set_bg(pal.bg);
                }
            }
        }

        let chart_region = Rect::new(area.x, 0, area.width, chart_h);
        FanWidget::new()
            .window(t_left, t_right)
            .y_bounds(lo, hi)
            .points(fan_pts)
            .render(chart_region, &mut virt, theme);
        BgChart::new()
            .window(t_left, t_right)
            .y_bounds(lo, hi)
            .unit(ctx.unit)
            .now(now)
            .pulse(self.pulse)
            .bg(bg_pts)
            .median(median_pts)
            .notes(notes)
            .photos(photos)
            .ticks(ticks.clone())
            .hr(hr_bpm)
            .heart_phase(self.heart_phase)
            .render(chart_region, &mut virt, theme);

        // Time axis: one row of ticks + hh:mm labels directly under the chart,
        // column-aligned with the chart's vertical gridlines.
        let axis_rect = Rect::new(area.x, chart_h, area.width, AXIS_H);
        let axis_plot = plot_area(axis_rect);
        let axis_proj = Proj {
            t_left,
            t_right,
            y_min: lo,
            y_max: hi,
        };
        for &ts in &ticks {
            if let Some(col) = axis_proj.col(axis_plot, ts) {
                put(&mut virt, axis_rect, col, chart_h, '┴', pal.grid, pal.bg);
                let label = fmt_clock(ts, tz);
                put_str(
                    &mut virt,
                    axis_rect,
                    col + 1,
                    chart_h,
                    &label,
                    pal.dim,
                    pal.bg,
                );
            }
        }

        // A titled divider that names the section beneath it, painted bright in
        // the section's own colour so the strips read as labelled panels rather
        // than an anonymous stack. Bounds-checked so a resized buffer can never
        // overrun, and clipped to the single SEP_H row.
        let draw_sep = |virt: &mut Buffer, y: u16, title: &str, color: Color| {
            let rect = Rect::new(area.x, y, area.width, SEP_H);
            for x in area.x..area.right() {
                put(virt, rect, x, y, '─', color, pal.bg);
            }
            let head = format!("\u{2500}\u{2500} {title} ");
            put_str(&mut *virt, rect, area.x, y, &head, color, pal.bg);
        };

        // "INSULIN · TOTAL" (never a bare "total"); the mode is a qualifier.
        let ins_title = format!("INSULIN \u{00B7} {}", self.insulin.label().to_uppercase());

        let mut y = chart_h + AXIS_H;
        if n_strips == 3 {
            draw_sep(&mut virt, y, &ins_title, pal.secondary);
            y += SEP_H;
            StripWidget::new()
                .window(t_left, t_right)
                .label("ins")
                .color(pal.secondary)
                .data(ins_pts)
                .ticks(ticks.clone())
                .render(Rect::new(area.x, y, area.width, STRIP_H), &mut virt, theme);
            y += STRIP_H;
            draw_sep(&mut virt, y, "CARBS", pal.accent);
            y += SEP_H;
            StripWidget::new()
                .window(t_left, t_right)
                .label("carb")
                .color(pal.accent)
                .data(carb_pts)
                .ticks(ticks.clone())
                .render(Rect::new(area.x, y, area.width, STRIP_H), &mut virt, theme);
            y += STRIP_H;
            draw_sep(&mut virt, y, "HEART-RATE", pal.hyper);
            y += SEP_H;
            StripWidget::new()
                .window(t_left, t_right)
                .label("bpm")
                .color(pal.hyper)
                .data(hr_pts)
                .ticks(ticks.clone())
                .render(Rect::new(area.x, y, area.width, STRIP_H), &mut virt, theme);
            y += STRIP_H;
        }

        draw_sep(&mut virt, y, "BIO-TIME", pal.secondary);
        y += SEP_H;
        let local_min = (now + tz as i64 * 60_000)
            .div_euclid(60_000)
            .rem_euclid(1440);
        BioTimeGauge::new()
            .tod(tod)
            .conf(conf)
            .current_bin(current_bin(now, tz))
            .now_local((local_min / 60) as u8, (local_min % 60) as u8)
            .render(Rect::new(area.x, y, area.width, bio_h), &mut virt, theme);

        // --- draw -----------------------------------------------------------
        let buf = frame.buffer_mut();
        self.draw_title(
            buf,
            Rect::new(area.x, area.y, area.width, 1),
            ctx.unit,
            t_right,
            tz,
            pal,
        );

        for r in 0..viewport_h {
            let sy = offset + r;
            if sy >= total_h {
                break;
            }
            for x in area.x..area.right() {
                if let Some(src) = virt.cell((x, sy)) {
                    let src = src.clone();
                    if let Some(dst) = buf.cell_mut((x, content_top + r)) {
                        *dst = src;
                    }
                }
            }
        }

        // Scroll affordances: a caret at the right edge whenever content is
        // clipped above or below the viewport.
        if area.width > 0 {
            let edge_x = area.right() - 1;
            if offset > 0 {
                if let Some(cell) = buf.cell_mut((edge_x, content_top)) {
                    cell.set_char('\u{25B2}'); // ▲
                    cell.fg = pal.accent;
                    cell.bg = pal.bg;
                }
            }
            if !self.scroll.at_bottom() {
                if let Some(cell) = buf.cell_mut((edge_x, area.bottom() - 1)) {
                    cell.set_char('\u{25BC}'); // ▼
                    cell.fg = pal.accent;
                    cell.bg = pal.bg;
                }
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Action {
        match key.code {
            KeyCode::Char('f') => {
                self.follow = !self.follow;
                Action::Redraw
            }
            KeyCode::Char('i') => {
                self.insulin = self.insulin.next();
                Action::Redraw
            }
            KeyCode::Char('=') => {
                self.zoom_ms = DEFAULT_ZOOM_MS;
                self.pan_ms = 0;
                Action::Redraw
            }
            KeyCode::Char('0') => {
                self.pan_ms = 0;
                self.follow = true;
                Action::Redraw
            }
            KeyCode::Char('j') => {
                self.zoom_in();
                Action::Redraw
            }
            KeyCode::Char('k') => {
                self.zoom_out();
                Action::Redraw
            }
            KeyCode::Left => {
                self.follow = false;
                self.pan_ms -= self.pan_step();
                Action::Redraw
            }
            KeyCode::Right => {
                self.follow = false;
                self.pan_ms += self.pan_step();
                Action::Redraw
            }
            KeyCode::Down => {
                self.scroll.down(1);
                Action::Redraw
            }
            KeyCode::Up => {
                self.scroll.up(1);
                Action::Redraw
            }
            KeyCode::PageDown => {
                self.scroll.page_down();
                Action::Redraw
            }
            KeyCode::PageUp => {
                self.scroll.page_up();
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.zoom_in();
                Action::Redraw
            }
            MouseEventKind::ScrollDown => {
                self.zoom_out();
                Action::Redraw
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag_anchor = Some(mouse.column);
                Action::None
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let anchor = self.drag_anchor.unwrap_or(mouse.column);
                let dcols = anchor as i64 - mouse.column as i64;
                self.drag_anchor = Some(mouse.column);
                if dcols != 0 {
                    self.follow = false;
                    self.pan_ms += dcols * self.zoom_ms;
                    Action::Redraw
                } else {
                    Action::None
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_anchor = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) {
        // Delta-time scaled so the pulse looks identical at any fps.
        self.pulse = (self.pulse + dt.as_secs_f32()) % 1.0;
        // Advance the heart at the live rate (bpm/60 beats per second) so the
        // on-screen beat matches the heart rate the server is receiving.
        if let Some(bpm) = self.hr_bpm {
            let bps = (bpm as f32 / 60.0).max(0.0);
            self.heart_phase = (self.heart_phase + dt.as_secs_f32() * bps) % 1.0;
            ctx.request_redraw();
        }
        if self.follow {
            ctx.request_redraw();
        }
    }
}

/// The 2-hour circadian bin (0..12) for `now_ms` at timezone `tz` (minutes).
fn current_bin(now_ms: i64, tz_offset: i32) -> usize {
    let local = now_ms + tz_offset as i64 * 60_000;
    let hours = local.div_euclid(3_600_000).rem_euclid(24);
    (hours / 2) as usize
}

/// Human-readable zoom, e.g. `5m`, `1.5h`, `30s`.
fn fmt_zoom(ms: i64) -> String {
    let mins = ms as f64 / 60_000.0;
    if mins >= 60.0 {
        let h = mins / 60.0;
        if h.fract().abs() < 0.05 {
            format!("{}h", h.round() as i64)
        } else {
            format!("{h:.1}h")
        }
    } else if mins >= 1.0 {
        if mins.fract().abs() < 0.05 {
            format!("{}m", mins.round() as i64)
        } else {
            format!("{mins:.1}m")
        }
    } else {
        format!("{}s", (ms / 1000).max(1))
    }
}

/// Evenly spaced time ticks across `[t_left, t_right]`, snapped to a round
/// wall-clock interval (so labels read `12:30`, `13:00`, …) and spaced widely
/// enough that their `hh:mm` labels never collide across `plot_w` columns.
fn time_ticks(t_left: i64, t_right: i64, tz_offset: i32, plot_w: i64) -> Vec<i64> {
    let span = (t_right - t_left).max(1);
    // Room for one `HH:MM` label (~6 cols) per tick; cap at six.
    let max_ticks = (plot_w / 12).clamp(2, 6);
    const STEPS_MIN: [i64; 11] = [1, 2, 5, 10, 15, 30, 60, 120, 240, 480, 720];
    let step_ms = STEPS_MIN
        .iter()
        .map(|m| m * 60_000)
        .find(|&s| span / s <= max_ticks)
        .unwrap_or(1440 * 60_000);

    let tz_ms = tz_offset as i64 * 60_000;
    let local_left = t_left + tz_ms;
    let local_right = t_right + tz_ms;
    let mut local = if local_left.rem_euclid(step_ms) == 0 {
        local_left
    } else {
        (local_left.div_euclid(step_ms) + 1) * step_ms
    };
    let mut out = Vec::new();
    while local <= local_right && out.len() < 16 {
        out.push(local - tz_ms);
        local += step_ms;
    }
    out
}

/// Local wall-clock `HH:MM` for an epoch-ms timestamp at `tz` (minutes).
fn fmt_clock(ts: i64, tz_offset: i32) -> String {
    let local = ts + tz_offset as i64 * 60_000;
    let mins = local.div_euclid(60_000).rem_euclid(1440);
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}
