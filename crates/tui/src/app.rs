//! The application state machine. Owns the panes, active theme, display
//! settings, and footer telemetry, and interprets [`Action`]s. The terminal
//! event loop lives in `lib.rs::run`; this type is loop-agnostic.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::Frame;
use sysinfo::{Components, Disks, System};

use store::Store;
use t1dm_core::{BgUnit, Config, Stats, StatsWindow, TokenKind};

use crate::footer::{self, FooterInfo};
use crate::header;
use crate::layout::{AppLayout, LayoutMode};
use crate::panes::{make_pane, Action, Ctx, Pane, PaneView};
use crate::theme::{theme_for, Theme, ThemeKind};
use crate::widgets::panel::{self, Section};
use crate::AppEvent;

/// Minimum wall-clock gap between heavy footer refreshes (sysinfo + store).
/// The cheap clock field is refreshed on every drawn frame; the costly system
/// probes are throttled to this cadence so a 60 fps animation cannot hammer
/// the store or the kernel.
const FOOTER_REFRESH: Duration = Duration::from_millis(1000);

/// Minimum wall-clock gap between recomputes of the side-panel statistics.
/// `store.stats()` is a rayon-parallel fold over the whole window, far too
/// heavy for a per-frame call; the panels tolerate stale figures for a few
/// seconds, so the cached block is refreshed on this cadence instead.
const STATS_REFRESH: Duration = Duration::from_millis(4000);

pub struct App {
    store: Store,
    cfg: Config,
    theme_kind: ThemeKind,
    theme: Box<dyn Theme>,
    unit: BgUnit,
    current: Pane,
    /// Pane to return to when the Help overlay closes.
    help_return: Option<Pane>,
    panes: HashMap<Pane, Box<dyn PaneView>>,
    footer: FooterInfo,
    /// Clickable header tab rectangles from the last frame.
    tab_hits: Vec<(Pane, Rect)>,
    connected: bool,
    /// Full terminal rect from the last frame; input handlers reuse it so the
    /// layout mode a pane sees matches what was actually drawn.
    last_area: Rect,
    /// Persistent system probe (reused so CPU-usage deltas are meaningful).
    sys: System,
    /// When the heavy footer telemetry was last refreshed.
    last_footer_refresh: Option<Instant>,
    /// Monotonic clock from construction; drives the right-panel theme animation.
    start: Instant,
    /// Vertical scroll offset (in rows) of the left statistics panel, advanced by
    /// the mouse wheel while the cursor is over that panel and clamped so it can
    /// never run past the end of the content.
    panel_scroll: u16,
    /// Cached side-panel statistics, indexed to match [`StatsWindow::ALL`]
    /// (7d, 30d, 90d). Refreshed on the [`STATS_REFRESH`] cadence.
    stats_cache: Option<[Stats; 3]>,
    /// When the panel statistics were last recomputed.
    last_stats_refresh: Option<Instant>,
}

impl App {
    pub fn new(store: Store, cfg: Config) -> Self {
        let theme_kind = ThemeKind::from_str_or_default(&cfg.ui.theme);
        let theme = theme_for(theme_kind);
        let unit = cfg.ui.bg_unit;

        let mut panes: HashMap<Pane, Box<dyn PaneView>> = HashMap::new();
        for p in Pane::HEADER {
            panes.insert(p, make_pane(p));
        }

        // Establish the CPU baseline and populate the per-core list up front so
        // the first refresh a second later reports a real delta.
        let mut sys = System::new();
        sys.refresh_cpu_usage();

        App {
            store,
            cfg,
            theme_kind,
            theme,
            unit,
            current: Pane::Dashboard,
            help_return: None,
            panes,
            footer: FooterInfo::default(),
            tab_hits: Vec::new(),
            connected: false,
            last_area: Rect::new(0, 0, 100, 40),
            sys,
            last_footer_refresh: None,
            start: Instant::now(),
            panel_scroll: 0,
            stats_cache: None,
            last_stats_refresh: None,
        }
    }

    /// The active theme.
    pub fn theme_ref(&self) -> &dyn Theme {
        self.theme.as_ref()
    }

    /// Mutable access to the footer telemetry the loop refreshes each frame.
    pub fn footer_mut(&mut self) -> &mut FooterInfo {
        &mut self.footer
    }

    /// Update connection state (driven by the WS client count).
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Render a full frame: background, header, pane, footer.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.last_area = area;
        let layout = AppLayout::compute(area);

        let now_ms = self.now_ms();
        self.refresh_footer(now_ms);
        // Panels only exist in Full mode; only pay for the heavy stats then.
        if layout.left.is_some() || layout.right.is_some() {
            self.refresh_stats();
        }

        let bg_color = self.theme.palette().bg;
        frame.render_widget(Block::default().style(Style::default().bg(bg_color)), area);

        let pane_id = self.current;
        let mut pane = self
            .panes
            .remove(&pane_id)
            .unwrap_or_else(|| make_pane(pane_id));

        let mut ctx = Ctx {
            store: &self.store,
            theme: self.theme.as_ref(),
            unit: self.unit,
            layout: layout.mode,
            now_ms,
            connected: self.connected,
            dt: Duration::ZERO,
            dirty: false,
            qr_addr: self.cfg.qr.advertise_addr.as_str(),
            qr_port: self.cfg.qr.advertise_port,
        };

        self.tab_hits = header::render(frame, layout.header, &ctx, pane_id);
        pane.render(frame, layout.main, &mut ctx);
        footer::render(frame, layout.footer, &ctx, &self.footer);

        self.draw_panels(frame, &layout);

        // Right gutter: the active theme's signature animation, tiling to fill
        // the panel height. The old full-frame ambient texture behind the graph
        // has been retired.
        if let Some(right) = layout.right {
            if self.theme.has_animation() {
                self.theme
                    .render_animation(right, self.start.elapsed(), frame.buffer_mut());
            }
        }

        self.panes.insert(pane_id, pane);
    }

    /// Fill the Full-mode left gutter with all three cached statistics windows
    /// (7d, 30d, 90d), stacked and vertically scrollable. The right gutter is
    /// reserved for the theme animation and carries no statistics.
    fn draw_panels(&self, frame: &mut Frame, layout: &AppLayout) {
        let Some(stats) = self.stats_cache.as_ref() else {
            return;
        };
        let pal = self.theme.palette();
        if let Some(left) = layout.left {
            let sections = stat_sections(stats);
            panel::render(
                frame,
                left,
                pal,
                self.unit,
                "STATISTICS",
                &sections,
                self.panel_scroll,
            );
        }
    }

    /// Recompute the cached panel statistics if the refresh interval has
    /// elapsed (or nothing is cached yet). Each window is a heavy parallel fold,
    /// hence the throttle.
    fn refresh_stats(&mut self) {
        let due = self
            .last_stats_refresh
            .is_none_or(|t| t.elapsed() >= STATS_REFRESH);
        if !due && self.stats_cache.is_some() {
            return;
        }
        self.last_stats_refresh = Some(Instant::now());
        let mut out = [Stats::empty(StatsWindow::D7); 3];
        for (slot, &window) in out.iter_mut().zip(StatsWindow::ALL.iter()) {
            *slot = self
                .store
                .stats(window)
                .unwrap_or_else(|_| Stats::empty(window));
        }
        self.stats_cache = Some(out);
    }

    /// The layout mode implied by the last drawn frame, so panes see the same
    /// breakpoint during input handling as during rendering.
    fn mode(&self) -> LayoutMode {
        LayoutMode::for_width(self.last_area.width)
    }

    /// Handle a key event; returns true when the app should quit.
    pub fn on_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> bool {
        if let Some(action) = crate::input::global_action(key, self.current) {
            return self.apply(action);
        }
        let pane_id = self.current;
        let mut pane = self
            .panes
            .remove(&pane_id)
            .unwrap_or_else(|| make_pane(pane_id));
        let mut ctx = Ctx {
            store: &self.store,
            theme: self.theme.as_ref(),
            unit: self.unit,
            layout: self.mode(),
            now_ms: self.now_ms(),
            connected: self.connected,
            dt: Duration::ZERO,
            dirty: false,
            qr_addr: self.cfg.qr.advertise_addr.as_str(),
            qr_port: self.cfg.qr.advertise_port,
        };
        let action = pane.on_key(key, &mut ctx);
        self.panes.insert(pane_id, pane);
        self.apply(action)
    }

    /// Handle a mouse event (header tab hit-testing + pane routing).
    pub fn on_mouse(&mut self, mouse: MouseEvent) -> bool {
        // Mouse wheel over the left statistics panel scrolls its stacked windows.
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let layout = AppLayout::compute(self.last_area);
            if let Some(left) = layout.left {
                if hit(left, mouse.column, mouse.row) {
                    if let Some(stats) = self.stats_cache.as_ref() {
                        let sections = stat_sections(stats);
                        let visible = left.height.saturating_sub(2);
                        let max_scroll = panel::content_height(&sections).saturating_sub(visible);
                        self.panel_scroll = match mouse.kind {
                            MouseEventKind::ScrollUp => self.panel_scroll.saturating_sub(1),
                            _ => (self.panel_scroll + 1).min(max_scroll),
                        };
                    }
                    return false;
                }
            }
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            for (pane, rect) in &self.tab_hits {
                if hit(*rect, mouse.column, mouse.row) {
                    let target = *pane;
                    return self.apply(Action::Switch(target));
                }
            }
        }
        let pane_id = self.current;
        let mut pane = self
            .panes
            .remove(&pane_id)
            .unwrap_or_else(|| make_pane(pane_id));
        let mut ctx = Ctx {
            store: &self.store,
            theme: self.theme.as_ref(),
            unit: self.unit,
            layout: self.mode(),
            now_ms: self.now_ms(),
            connected: self.connected,
            dt: Duration::ZERO,
            dirty: false,
            qr_addr: self.cfg.qr.advertise_addr.as_str(),
            qr_port: self.cfg.qr.advertise_port,
        };
        let action = pane.on_mouse(mouse, &mut ctx);
        self.panes.insert(pane_id, pane);
        self.apply(action)
    }

    /// Advance the visible pane's animation; returns true if still animating.
    pub fn tick(&mut self, dt: Duration) -> bool {
        let pane_id = self.current;
        let mut pane = self
            .panes
            .remove(&pane_id)
            .unwrap_or_else(|| make_pane(pane_id));
        let mut ctx = Ctx {
            store: &self.store,
            theme: self.theme.as_ref(),
            unit: self.unit,
            layout: self.mode(),
            now_ms: self.now_ms(),
            connected: self.connected,
            dt,
            dirty: false,
            qr_addr: self.cfg.qr.advertise_addr.as_str(),
            qr_port: self.cfg.qr.advertise_port,
        };
        pane.tick(dt, &mut ctx);
        // The right-panel theme animation runs continuously whenever that panel
        // is on screen (Full layout), so it keeps the demand-driven loop serving
        // frames (the fps cap still governs the rate); otherwise only a live pane
        // animation does.
        let animating =
            ctx.dirty || (matches!(self.mode(), LayoutMode::Full) && self.theme.has_animation());
        self.panes.insert(pane_id, pane);
        animating
    }

    /// Apply a data event pushed from the server side.
    pub fn apply_event(&mut self, ev: AppEvent) {
        self.connected = true;
        match ev {
            AppEvent::Sample(row) => self.footer.last_sample_ms = Some(row.ts),
            AppEvent::Alert(a) => self.footer.alert_ticker = Some(a.kind),
            AppEvent::Prediction(_) | AppEvent::Note(_) | AppEvent::Photo(_) => {}
        }
    }

    /// Refresh footer telemetry. The wall clock is updated every frame; the
    /// costly system/store probes are throttled to [`FOOTER_REFRESH`].
    fn refresh_footer(&mut self, now_ms: i64) {
        self.footer.pi_time_ms = now_ms;

        let due = self
            .last_footer_refresh
            .is_none_or(|t| t.elapsed() >= FOOTER_REFRESH);
        if !due {
            return;
        }
        self.last_footer_refresh = Some(Instant::now());

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();

        self.footer.cpu_pct = self.sys.global_cpu_usage();
        self.footer.per_core = self.sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        self.footer.ram_used_mb = self.sys.used_memory() / (1024 * 1024);
        self.footer.ram_total_mb = self.sys.total_memory() / (1024 * 1024);
        self.footer.uptime_secs = System::uptime();
        self.footer.cpu_temp_c = read_cpu_temp();

        let (used_gb, total_gb) = storage_for(&self.cfg.storage.data_dir);
        self.footer.storage_used_gb = used_gb;
        self.footer.storage_total_gb = total_gb;

        self.footer.server_ip = format!(
            "{}:{}",
            self.cfg.qr.advertise_addr, self.cfg.qr.advertise_port
        );

        let (rw, ro) = self.session_counts();
        self.footer.rw_sessions = rw;
        self.footer.ro_sessions = ro;
        // A live session implies at least one connected client.
        if rw + ro > 0 {
            self.connected = true;
        }
    }

    /// Count persisted sessions by the access class of their token. This is the
    /// best proxy the TUI has for "connected RW/RO" without the WS hub.
    fn session_counts(&self) -> (u32, u32) {
        let mut kind_by_id: HashMap<i64, TokenKind> = HashMap::new();
        for t in self.store.list_tokens(true).unwrap_or_default() {
            kind_by_id.insert(t.id, t.kind);
        }
        let mut rw = 0u32;
        let mut ro = 0u32;
        for s in self.store.list_sessions().unwrap_or_default() {
            match kind_by_id.get(&s.token_id) {
                Some(TokenKind::Rw) => rw += 1,
                Some(TokenKind::Ro) => ro += 1,
                None => {}
            }
        }
        (rw, ro)
    }

    fn switch(&mut self, target: Pane) {
        if target != Pane::Help {
            self.help_return = None;
        }
        self.current = target;
    }

    /// Interpret an [`Action`]; returns true when it requests a quit.
    fn apply(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => return true,
            Action::NextPane => self.switch(self.current.next()),
            Action::PrevPane => self.switch(self.current.prev()),
            Action::Switch(p) => self.switch(p),
            Action::ToggleHelp => {
                if self.current == Pane::Help {
                    if let Some(prev) = self.help_return.take() {
                        self.current = prev;
                    } else {
                        self.current = Pane::Dashboard;
                    }
                } else {
                    self.help_return = Some(self.current);
                    self.current = Pane::Help;
                }
            }
            Action::CycleTheme => {
                self.theme_kind = self.theme_kind.next();
                self.theme = theme_for(self.theme_kind);
                self.cfg.ui.theme = self.theme_kind.as_str().to_string();
                self.panel_scroll = 0;
            }
            Action::ToggleUnit => self.unit = self.unit.toggled(),
            Action::Redraw | Action::None | Action::Custom(_) => {}
        }
        false
    }
}

fn hit(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.left() && col < rect.right() && row >= rect.top() && row < rect.bottom()
}

/// Build the three labelled statistics windows (7d, 30d, 90d) that stack in the
/// left panel, borrowing from the cached block.
fn stat_sections(stats: &[Stats; 3]) -> [Section<'_>; 3] {
    [
        Section {
            label: "7 DAYS",
            stats: &stats[0],
        },
        Section {
            label: "30 DAYS",
            stats: &stats[1],
        },
        Section {
            label: "90 DAYS",
            stats: &stats[2],
        },
    ]
}

/// CPU temperature in °C. Prefers the Pi's thermal zone; on hosts without it
/// (typical x86 dev box) falls back to sysinfo's component sensors, and to
/// `None` when nothing exposes a temperature.
fn read_cpu_temp() -> Option<f32> {
    if let Ok(raw) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(milli) = raw.trim().parse::<f64>() {
            return Some((milli / 1000.0) as f32);
        }
    }

    let components = Components::new_with_refreshed_list();
    let mut fallback: Option<f32> = None;
    for c in &components {
        let Some(t) = c.temperature() else { continue };
        let label = c.label().to_ascii_lowercase();
        if label.contains("cpu")
            || label.contains("package")
            || label.contains("tctl")
            || label.contains("coretemp")
        {
            return Some(t);
        }
        fallback = Some(fallback.map_or(t, |b| b.max(t)));
    }
    fallback
}

/// Used/total gigabytes for the filesystem backing `data_dir`. Chooses the
/// mounted disk whose mount point is the longest prefix of the target path.
fn storage_for(data_dir: &str) -> (f32, f32) {
    let target =
        std::fs::canonicalize(data_dir).unwrap_or_else(|_| std::path::PathBuf::from(data_dir));

    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64, u64)> = None;
    for d in &disks {
        let mp = d.mount_point();
        if target.starts_with(mp) {
            let len = mp.as_os_str().len();
            if best.is_none_or(|(l, _, _)| len > l) {
                best = Some((len, d.total_space(), d.available_space()));
            }
        }
    }

    let (_, total, avail) = best
        .or_else(|| {
            disks
                .iter()
                .next()
                .map(|d| (0usize, d.total_space(), d.available_space()))
        })
        .unwrap_or((0, 0, 0));

    const GB: f32 = 1024.0 * 1024.0 * 1024.0;
    (total.saturating_sub(avail) as f32 / GB, total as f32 / GB)
}
