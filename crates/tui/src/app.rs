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
use crate::{AppEvent, ClientProbe};

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

/// How recently a session must have been seen to count as live. Session rows
/// are never deleted — each `(token, ip, device)` triple persists across
/// reconnects and DHCP leases — so a row's own liveness can only be read off
/// `last_seen`. The phone touches its session on every authenticated request
/// and pushes on the 5-minute sample grid; two missed cycles is the tightest
/// window that cannot flap between two healthy pushes. A client whose only
/// traffic is the open stream never bumps `last_seen` at all, so it is counted
/// through [`App::clients`] instead.
const SESSION_LIVE_MS: i64 = 10 * 60 * 1000;

/// Minimum wall-clock gap between store-event cache invalidations. Bulk writes
/// broadcast one hub event per stored row, so a re-mirror arrives as thousands
/// of back-to-back wake-ups; invalidating on each would clear the panes' fetch
/// caches faster than their own throttles can refill them, defeating those
/// throttles during exactly the burst they exist for. The debounce keeps a
/// leading edge — an isolated meal or dose still repaints on the next frame —
/// and folds the remainder of a burst into one trailing refetch.
const INVALIDATE_COALESCE: Duration = Duration::from_millis(250);

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
    /// Number of clients attached to the WS stream, probed from the server.
    clients: ClientProbe,
    /// A store event arrived inside the [`INVALIDATE_COALESCE`] window and its
    /// invalidation is still owed.
    pending_invalidate: bool,
    /// When the pane caches were last invalidated.
    last_invalidate: Option<Instant>,
}

impl App {
    pub fn new(store: Store, cfg: Config, clients: ClientProbe) -> Self {
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
            clients,
            pending_invalidate: false,
            last_invalidate: None,
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

    /// Refresh the cached panel statistics from the phone-pushed blocks (#6).
    /// The server no longer computes statistics; it serves the most recent
    /// per-window block the phone stored, verbatim. A window with no pushed
    /// block, or a block that fails to parse, degrades to an all-zero
    /// `Stats::empty`.
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
                .get_stats_block(window.as_str())
                .ok()
                .flatten()
                .and_then(|json| serde_json::from_str::<Stats>(&json).ok())
                .unwrap_or_else(|| Stats::empty(window));
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
        // Before the visible pane is taken out of the map, so a settling
        // invalidation reaches it too.
        let settling = self.settle_invalidation();
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
        let animating = ctx.dirty
            || settling
            || (matches!(self.mode(), LayoutMode::Full) && self.theme.has_animation());
        self.panes.insert(pane_id, pane);
        animating
    }

    /// Apply a data event pushed from the server side. Every kind that mutates
    /// a record a pane serves out of a throttled cache — samples, predictions,
    /// photos, and the aggregate [`AppEvent::StoreChanged`] — drops that cache.
    /// Alerts are the exception: they reach the footer directly and no
    /// pane reads them back from the store.
    pub fn apply_event(&mut self, ev: AppEvent) {
        self.connected = true;
        match ev {
            AppEvent::Alert(a) => self.footer.alert_ticker = Some(a.kind),
            AppEvent::Sample(row) => {
                self.footer.last_sample_ms = Some(row.ts);
                // The phone stamps its live offset on every row precisely so
                // the appliance can render local time; the footer clocks track
                // the most recent one and keep the previous until then.
                self.footer.tz_offset_min = row.tz_offset;
                self.note_store_change();
            }
            AppEvent::Prediction(_) | AppEvent::Photo(_) | AppEvent::StoreChanged => {
                self.note_store_change()
            }
        }
    }

    /// Record a store write, invalidating the cached reads at most once per
    /// [`INVALIDATE_COALESCE`]. An event inside the window only marks the
    /// invalidation as owed; [`App::settle_invalidation`] pays it on the first
    /// frame after the window closes.
    fn note_store_change(&mut self) {
        if self.invalidate_due() {
            self.invalidate_caches();
        } else {
            self.pending_invalidate = true;
        }
    }

    fn invalidate_due(&self) -> bool {
        self.last_invalidate
            .is_none_or(|t| t.elapsed() >= INVALIDATE_COALESCE)
    }

    /// Drop the app-level and per-pane fetch caches so the next frame re-reads.
    fn invalidate_caches(&mut self) {
        self.last_invalidate = Some(Instant::now());
        self.pending_invalidate = false;
        self.last_stats_refresh = None;
        for pane in self.panes.values_mut() {
            pane.invalidate();
        }
    }

    /// Pay an owed invalidation once its coalescing window has closed. Returns
    /// true while one is still outstanding, which keeps the demand-driven loop
    /// serving frames long enough for the trailing refetch to land.
    fn settle_invalidation(&mut self) -> bool {
        if !self.pending_invalidate {
            return false;
        }
        if !self.invalidate_due() {
            return true;
        }
        self.invalidate_caches();
        false
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

        let (rw, ro) = self.session_counts(now_ms);
        // A read-only viewer may send nothing but the stream it holds open, and
        // nothing on that socket touches `last_seen` after the upgrade, so the
        // session sweep cannot see it at all. Credit every attached stream the
        // sweep did not account for to the read-only tally — a writer pushes on
        // the 5-minute grid and always refreshes its own row — otherwise the
        // footer contradicts itself, reading "connected" beside "RO 0".
        let attached = (self.clients)() as u32;
        let ro = ro + attached.saturating_sub(rw + ro);
        self.footer.rw_sessions = rw;
        self.footer.ro_sessions = ro;
        self.connected = rw + ro > 0;
    }

    /// Count sessions seen within [`SESSION_LIVE_MS`] by the access class of
    /// their token. Only live tokens are mapped, so a session left behind by a
    /// revoked token stops being tallied — matching the WS liveness sweep,
    /// which drops such a stream within its own cut-off.
    fn session_counts(&self, now_ms: i64) -> (u32, u32) {
        let mut kind_by_id: HashMap<i64, TokenKind> = HashMap::new();
        for t in self.store.list_tokens(false).unwrap_or_default() {
            kind_by_id.insert(t.id, t.kind);
        }
        let mut rw = 0u32;
        let mut ro = 0u32;
        for s in self.store.list_sessions().unwrap_or_default() {
            if now_ms.saturating_sub(s.last_seen) >= SESSION_LIVE_MS {
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use t1dm_core::{Alert, Photo, PredictionEvent, SampleRow};

    /// A store rooted in a fresh temp dir, wiped on drop.
    struct TempStore {
        store: Store,
        dir: PathBuf,
    }

    impl TempStore {
        fn new() -> TempStore {
            static CTR: AtomicU64 = AtomicU64::new(0);
            let n = CTR.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("t1dm-tui-test-{}-{}", std::process::id(), n));
            let _ = std::fs::remove_dir_all(&dir);
            let store = Store::open_at(&dir).expect("open store");
            TempStore { store, dir }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// An app over a throwaway store, with the client probe pinned to
    /// `clients`. The store outlives the app, so the tuple must be held whole.
    fn test_app(clients: usize) -> (App, TempStore) {
        let ts = TempStore::new();
        let probe: ClientProbe = Arc::new(move || clients);
        let app = App::new(ts.store.clone(), Config::default(), probe);
        (app, ts)
    }

    /// Apply one event against a clean slate and report whether it dropped the
    /// cached reads.
    fn invalidates(app: &mut App, ev: AppEvent) -> bool {
        app.last_invalidate = None;
        app.pending_invalidate = false;
        app.last_stats_refresh = Some(Instant::now());
        app.apply_event(ev);
        app.last_stats_refresh.is_none()
    }

    #[test]
    fn store_events_coalesce_into_one_invalidation() {
        let (mut app, _ts) = test_app(0);
        // A pane that neither animates nor requests redraws, in a layout with
        // no gutter animation, so `tick` reports only the pending invalidation.
        app.current = Pane::Models;
        app.last_area = Rect::new(0, 0, 80, 24);

        // Leading edge: the first write of a burst repaints at once.
        app.last_stats_refresh = Some(Instant::now());
        app.apply_event(AppEvent::StoreChanged);
        assert!(app.last_stats_refresh.is_none());
        assert!(!app.pending_invalidate);

        // A bulk re-mirror broadcasts one event per row; the cache must survive
        // all of them and be dropped once, afterwards.
        app.last_stats_refresh = Some(Instant::now());
        for _ in 0..2000 {
            app.apply_event(AppEvent::StoreChanged);
        }
        assert!(
            app.last_stats_refresh.is_some(),
            "burst re-invalidated per event"
        );
        assert!(app.pending_invalidate);
        assert!(
            app.tick(Duration::from_millis(16)),
            "loop must stay awake for the trailing refetch"
        );
        assert!(app.last_stats_refresh.is_some());

        std::thread::sleep(INVALIDATE_COALESCE);
        assert!(!app.tick(Duration::from_millis(16)));
        assert!(app.last_stats_refresh.is_none());
        assert!(!app.pending_invalidate);
        assert!(!app.tick(Duration::from_millis(16)));
    }

    #[test]
    fn every_cached_event_kind_drops_the_caches() {
        let (mut app, _ts) = test_app(0);

        assert!(invalidates(&mut app, AppEvent::Sample(SampleRow::default())));
        assert!(invalidates(
            &mut app,
            AppEvent::Prediction(PredictionEvent::default())
        ));
        assert!(invalidates(&mut app, AppEvent::Photo(Photo::default())));
        assert!(invalidates(&mut app, AppEvent::StoreChanged));
        // Alerts reach the footer directly; no pane reads them back.
        assert!(!invalidates(&mut app, AppEvent::Alert(Alert::default())));

        // The footer indicators the typed variants carry still land.
        app.apply_event(AppEvent::Sample(SampleRow {
            ts: 1_700_000_000_000,
            ..Default::default()
        }));
        assert_eq!(app.footer.last_sample_ms, Some(1_700_000_000_000));
        app.apply_event(AppEvent::Alert(Alert {
            kind: "hypo".to_string(),
            ..Default::default()
        }));
        assert_eq!(app.footer.alert_ticker.as_deref(), Some("hypo"));
    }

    #[test]
    fn revoked_tokens_drop_out_of_the_session_counts() {
        let (app, ts) = test_app(0);
        let now = app.now_ms();

        let (ro, _) = ts.store.mint_token(TokenKind::Ro, None).unwrap();
        ts.store
            .upsert_session(ro.id, "10.0.0.9", "viewer", "laptop")
            .unwrap();
        let (old_rw, _) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
        ts.store
            .upsert_session(old_rw.id, "10.0.0.2", "phone", "phone")
            .unwrap();
        // Minting a second RW revokes the first (`one_rw`); the stale session
        // row survives and must stop being tallied.
        let (new_rw, _) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
        ts.store
            .upsert_session(new_rw.id, "10.0.0.3", "phone", "phone")
            .unwrap();
        assert_eq!(app.session_counts(now), (1, 1));

        ts.store.revoke_token(ro.id).unwrap();
        assert_eq!(app.session_counts(now), (1, 0));
    }

    #[test]
    fn an_open_stream_reads_as_connected() {
        let (mut app, _ts) = test_app(1);
        let now = app.now_ms();
        app.refresh_footer(now);
        // The counts and the indicator must never contradict each other: a
        // stream-only viewer bumps no `last_seen`, so the sweep misses it and
        // the attached-client probe is the only evidence it is alive.
        assert_eq!(
            (app.footer.rw_sessions, app.footer.ro_sessions),
            (0, 1),
            "a client whose only traffic is the stream never bumps last_seen"
        );
        assert!(app.connected);

        let (mut idle, _ts2) = test_app(0);
        let now = idle.now_ms();
        idle.refresh_footer(now);
        assert_eq!((idle.footer.rw_sessions, idle.footer.ro_sessions), (0, 0));
        assert!(!idle.connected);
    }

    #[test]
    fn a_streaming_writer_is_not_counted_twice() {
        // The phone holds a stream open AND touches its session on every push,
        // so the one attached client is already in the RW tally.
        let (mut app, ts) = test_app(1);
        let (rw, _) = ts.store.mint_token(TokenKind::Rw, None).unwrap();
        ts.store
            .upsert_session(rw.id, "10.0.0.2", "phone", "phone")
            .unwrap();
        let now = app.now_ms();
        app.refresh_footer(now);
        assert_eq!((app.footer.rw_sessions, app.footer.ro_sessions), (1, 0));

        // A second stream beside it is a viewer the sweep cannot see.
        let (mut two, ts2) = test_app(2);
        let (rw2, _) = ts2.store.mint_token(TokenKind::Rw, None).unwrap();
        ts2.store
            .upsert_session(rw2.id, "10.0.0.2", "phone", "phone")
            .unwrap();
        let now = two.now_ms();
        two.refresh_footer(now);
        assert_eq!((two.footer.rw_sessions, two.footer.ro_sessions), (1, 1));
    }

    #[test]
    fn the_footer_clocks_follow_the_phones_offset() {
        let (mut app, _ts) = test_app(0);
        assert_eq!(app.footer.tz_offset_min, 0);
        app.apply_event(AppEvent::Sample(SampleRow {
            ts: 1_735_689_600_000,
            tz_offset: -300,
            ..Default::default()
        }));
        assert_eq!(app.footer.tz_offset_min, -300);
        // A later row in a new zone moves the clocks with it.
        app.apply_event(AppEvent::Sample(SampleRow {
            ts: 1_735_693_200_000,
            tz_offset: 345,
            ..Default::default()
        }));
        assert_eq!(app.footer.tz_offset_min, 345);
        // An event carrying no offset leaves the last one standing.
        app.apply_event(AppEvent::StoreChanged);
        assert_eq!(app.footer.tz_offset_min, 345);
    }
}
