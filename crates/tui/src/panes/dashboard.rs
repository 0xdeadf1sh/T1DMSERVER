//! Dashboard pane — the default view. A tall braille BG chart (actual line
//! past + future) with the shaded quantile fan and photo markers sits at
//! the top; the taller time-aligned insulin/carbs/HR strips and a circadian
//! bio-time gauge are stacked beneath it and revealed by scrolling down. All
//! data is pulled live through the [`Ctx`] store handle each frame.
//!
//! Keymap: Left/Right pan, `j`/`k` zoom, Up/Down and PageUp/PageDown scroll
//! the stacked content vertically, `=` reset, `0` snap-to-now, `f` follow-lock,
//! `i` cycle the insulin series; mouse drag pans and the wheel zooms. The
//! [`PaneView`] surface and [`InsulinDisplay`] are locked here.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Frame;

use store::ReconstructedChannels;
use t1dm_core::{snap_grid, BgUnit, PredictionEvent, Series, GRID_MS, TOD_BINS};

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

/// Minimum wall-clock gap between store fetches backing the dashboard.
/// The BG/insulin/carb/HR series and the forecast are pulled and — for the
/// strips — reconstructed from the stored curve events, which is far too heavy
/// to repeat on every animation frame (the live-point pulse and heartbeat
/// redraw at the fps ceiling). A change to the visible window refetches at
/// once; otherwise the cached bundle is reused until it ages past this, exactly
/// as the footer/stats probes are throttled. Data is on a 5-minute cadence, so
/// a sub-second staleness is invisible.
const FETCH_REFRESH: Duration = Duration::from_millis(1000);

/// Cached result of one dashboard store fetch, reused across animation frames
/// until the visible window changes or [`FETCH_REFRESH`] elapses. Holds the raw
/// series and reconstructed channels; the cheap per-frame derivations (insulin
/// selection, forecast fan expansion) run off this each frame.
struct FetchCache {
    /// The `(t_left, t_right)` window this bundle was fetched for.
    key: (i64, i64),
    /// When the fetch ran, for the [`FETCH_REFRESH`] age check.
    at: Instant,
    bg_pts: Vec<(i64, f64)>,
    hr_pts: Vec<(i64, f64)>,
    hr_bpm: Option<f64>,
    tz: i32,
    channels: ReconstructedChannels,
    pred: Option<PredictionEvent>,
    photos: Vec<i64>,
}

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
    /// Select this display's insulin channel from the reconstructed curves.
    /// `Total` is the element-wise bolus+basal sum; the §4-#6 basal XOR is
    /// already applied inside `reconstruct_channels`, so bolus and basal never
    /// double-count a scheduled-plus-injected background.
    fn select(self, ch: &ReconstructedChannels) -> Vec<(i64, f64)> {
        match self {
            InsulinDisplay::Total => sum_by_ts(&ch.bolus, &ch.basal),
            InsulinDisplay::Bolus => ch.bolus.clone(),
            InsulinDisplay::Basal => ch.basal.clone(),
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
    /// Throttled store-fetch cache; see [`FETCH_REFRESH`].
    cache: Option<FetchCache>,
    /// The phone's timezone offset, latched from the newest sample ever seen.
    /// A window holding no samples carries no offset of its own, and 0 is a
    /// real UTC offset rather than a sentinel — without this the whole pane
    /// would silently jump zones on a pan into a gap.
    last_tz: Option<i32>,
    /// The cold-start lookback for [`DashboardPane::last_tz`] has been tried.
    /// A store with no samples at all must not repeat it every fetch.
    tz_seeded: bool,
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
            cache: None,
            last_tz: None,
            tz_seeded: false,
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

    /// Pull every store-backed series the dashboard renders for the window
    /// `[t_left, t_right]` in one shot. Heavy (five queries plus curve
    /// reconstruction), so it is called only when [`FetchCache`] is stale.
    fn fetch(&mut self, ctx: &Ctx, t_left: i64, t_right: i64, n_pts: usize) -> FetchCache {
        let samples = ctx
            .store
            .get_samples(&Series::ALL, Some(t_left), Some(t_right), Some(n_pts), None)
            .unwrap_or_default();
        let bg_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| r.bg.map(|v| (r.ts, v)))
            .collect();
        let hr_pts: Vec<(i64, f64)> = samples
            .iter()
            .filter_map(|r| r.hr.map(|v| (r.ts, v)))
            .collect();
        // Latest in-view heart rate drives the beating-heart indicator.
        let hr_bpm = hr_pts.last().map(|&(_, v)| v).filter(|v| *v > 0.0);
        // The phone stamps its live offset on every row, so a populated window
        // latches it for the empty ones that follow: panning into a gap must
        // not silently re-render the axis, the title clock and the NOW dial in
        // UTC, which is indistinguishable from a genuine UTC user.
        let tz = match samples.last() {
            Some(r) => {
                self.last_tz = Some(r.tz_offset);
                r.tz_offset
            }
            None => match self.last_tz {
                Some(tz) => tz,
                None => self.seed_tz(ctx, t_right).unwrap_or(0),
            },
        };
        // Insulin and carb strips are reconstructed from the stored
        // meal/dose/basal events — each row resolved from its verbatim
        // custom_curve when present, else the ported gamma/Bateman params —
        // bucketized onto the grid with the §4-#6 basal XOR applied. Display-only.
        let channels = ctx
            .store
            .reconstruct_channels(t_left, t_right)
            .unwrap_or_default();
        let pred = ctx.store.get_prediction_latest().ok().flatten();
        let photos: Vec<i64> = ctx
            .store
            .get_photos(Some(t_left), Some(t_right))
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.ts)
            .collect();
        FetchCache {
            key: (t_left, t_right),
            at: Instant::now(),
            bg_pts,
            hr_pts,
            hr_bpm,
            tz,
            channels,
            pred,
            photos,
        }
    }

    /// Cold-start backstop for [`DashboardPane::last_tz`]: the offset stamped
    /// on the newest sample in the week before `t_right`. A first render onto
    /// an empty window — a fresh process panned into a gap, or one opened
    /// before the first sync — has nothing to latch from, and UTC is not a
    /// safe stand-in for "unknown". Runs at most once per pane.
    fn seed_tz(&mut self, ctx: &Ctx, t_right: i64) -> Option<i32> {
        if self.tz_seeded {
            return None;
        }
        self.tz_seeded = true;
        const LOOKBACK_MS: i64 = 7 * 24 * 3_600_000;
        // `get_samples` orders ascending and truncates from the oldest end, so
        // the bound is what puts the newest row within reach, not the limit.
        self.last_tz = ctx
            .store
            .get_samples(
                &Series::ALL,
                Some(t_right - LOOKBACK_MS),
                Some(t_right),
                Some(4096),
                None,
            )
            .unwrap_or_default()
            .last()
            .map(|r| r.tz_offset);
        self.last_tz
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

        // --- fetch (throttled) ---------------------------------------------
        // The BG/HR series, the reconstructed insulin/carb channels, the latest
        // forecast, and the photo markers are pulled here in one shot and
        // cached. A moved window (pan/zoom/resize or the 5-minute `now` tick)
        // refetches at once; otherwise the bundle is reused until it ages past
        // FETCH_REFRESH, so the pulse/heartbeat animation re-renders from the
        // cache each frame instead of re-hitting the store 60×/s.
        let n_pts = (span / GRID_MS + 4).clamp(1, 40_000) as usize;
        let key = (t_left, t_right);
        let stale = self
            .cache
            .as_ref()
            .is_none_or(|c| c.key != key || c.at.elapsed() >= FETCH_REFRESH);
        if stale {
            let bundle = self.fetch(ctx, t_left, t_right, n_pts);
            self.cache = Some(bundle);
        }
        let data = self.cache.as_ref().expect("cache populated above");

        let bg_pts: Vec<(i64, f64)> = data.bg_pts.clone();
        // The §4-#6 basal XOR is already applied inside reconstruct_channels, so
        // selecting/summing the cached channels here can't double-count. The
        // demoted `samples` scalars carry no carbs/bolus/basal — these curves
        // are the sole strip source. Display-only; not byte-exact.
        let ins_pts: Vec<(i64, f64)> = self.insulin.select(&data.channels);
        let carb_pts: Vec<(i64, f64)> = data.channels.carb.clone();
        let hr_pts: Vec<(i64, f64)> = data.hr_pts.clone();
        // Latest in-view heart rate drives the beating-heart indicator.
        let hr_bpm = data.hr_bpm;
        let tz = data.tz;
        let pred = data.pred.clone();
        let photos: Vec<i64> = data.photos.clone();
        self.hr_bpm = hr_bpm;
        let mut median_pts: Vec<(i64, f64)> = Vec::new();
        let mut fan_pts: Vec<(i64, [f64; 7])> = Vec::new();
        let mut tod = vec![0.0f64; TOD_BINS];
        let mut conf = 0.0f64;
        let mut bio_hour: Option<f64> = None;
        if let Some(p) = &pred {
            median_pts = median_points(p);
            fan_pts = fan_columns(p);
            // The prediction now carries a nested circadian belief (or `None`
            // when the model has no time head), not a zeroed 12-vector. The
            // phone's own `predicted_hour` — a probability-weighted mean
            // resultant over the hour circle — drives the dial; the
            // probabilities are kept only as the argmax fallback, so a model
            // whose bin count is not the gauge's 12 still reports its hour.
            if let Some(c) = &p.circadian {
                if !c.probs.is_empty() && c.probs.len() == c.n_bins as usize {
                    tod = c.probs.clone();
                }
                conf = c.resultant_r;
                bio_hour = Some(c.predicted_hour);
            }
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

        // Photo marker timestamps come from the cached bundle above.

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
            .unit(ctx.unit)
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
            unit: BgUnit::Mgdl,
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
        let mut gauge = BioTimeGauge::new()
            .tod(tod)
            .conf(conf)
            .current_bin(current_bin(now, tz))
            .now_local((local_min / 60) as u8, (local_min % 60) as u8);
        if let Some(h) = bio_hour {
            gauge = gauge.bio_hour(h);
        }
        gauge.render(Rect::new(area.x, y, area.width, bio_h), &mut virt, theme);

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

    fn invalidate(&mut self) {
        self.cache = None;
    }
}

/// The forecast timestamp of step `i`. Wire contract: `line[i]` and `fan[q][i]`
/// are the model's forecast for `made_at + (i + 1) * GRID_MS`, so step 0 lands
/// one grid step AFTER the cycle tick and never on it — `made_at` is the
/// measured anchor the curve grows out of, and the phone scores its own
/// accuracy against that same convention.
fn step_ts(made_at: i64, i: usize) -> i64 {
    made_at + (i as i64 + 1) * GRID_MS
}

/// The median line laid onto the grid.
fn median_points(p: &PredictionEvent) -> Vec<(i64, f64)> {
    p.line
        .iter()
        .enumerate()
        .map(|(i, &m)| (step_ts(p.made_at, i), m))
        .collect()
}

/// The quantile fan transposed to one 7-tuple per step. A step whose fan is
/// short of a quantile row is dropped whole rather than half-shaded.
fn fan_columns(p: &PredictionEvent) -> Vec<(i64, [f64; 7])> {
    let mut out = Vec::with_capacity(p.line.len());
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
            out.push((step_ts(p.made_at, i), q));
        }
    }
    out
}

/// The 2-hour circadian bin (0..12) for `now_ms` at timezone `tz` (minutes).
fn current_bin(now_ms: i64, tz_offset: i32) -> usize {
    let local = now_ms + tz_offset as i64 * 60_000;
    let hours = local.div_euclid(3_600_000).rem_euclid(24);
    (hours / 2) as usize
}

/// Element-wise sum of two grid-bucketed channels keyed by timestamp, used to
/// derive the `Total` insulin strip from the reconstructed bolus and basal
/// curves. Both arrive dense over the same window, so this is an aligned add;
/// the `BTreeMap` keeps the output ts-sorted and tolerates any length skew.
fn sum_by_ts(a: &[(i64, f64)], b: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut merged: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
    for &(ts, v) in a.iter().chain(b.iter()) {
        *merged.entry(ts).or_insert(0.0) += v;
    }
    merged.into_iter().collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 12:00:00Z, on the 5-minute grid.
    const MADE_AT: i64 = 1_735_732_800_000;

    fn pred(line: Vec<f64>) -> PredictionEvent {
        let h = line.len();
        PredictionEvent {
            made_at: MADE_AT,
            model_id: "m".into(),
            updated_at: MADE_AT,
            horizon_steps: h as i32,
            fan: (0..7)
                .map(|q| (0..h).map(|i| line[i] + q as f64).collect())
                .collect(),
            line,
            circadian: None,
        }
    }

    #[test]
    fn the_forecast_starts_one_bucket_after_the_cycle_tick() {
        let mut line = vec![120.0; 24];
        line[23] = 61.0;
        let p = pred(line);
        let med = median_points(&p);
        assert_eq!(med.len(), 24);
        // Step 0 is +5 min, never the anchor itself.
        assert_eq!(med[0].0, MADE_AT + GRID_MS);
        // The 61 mg/dL trough of a 24-step horizon is the 2 h point: 14:00,
        // which is what the phone reports for the same wire row.
        assert_eq!(med[23], (MADE_AT + 24 * GRID_MS, 61.0));
        assert_eq!(med[23].0 - MADE_AT, 2 * 3_600_000);

        // The fan columns must sit on the very same grid as the median.
        let fan = fan_columns(&p);
        assert_eq!(fan.len(), 24);
        for (a, b) in med.iter().zip(fan.iter()) {
            assert_eq!(a.0, b.0);
        }
    }

    #[test]
    fn a_short_fan_row_drops_that_step_whole() {
        let mut p = pred(vec![100.0, 110.0, 120.0]);
        p.fan[4].pop();
        let fan = fan_columns(&p);
        assert_eq!(fan.len(), 2);
        assert_eq!(fan[0].0, MADE_AT + GRID_MS);
    }
}
