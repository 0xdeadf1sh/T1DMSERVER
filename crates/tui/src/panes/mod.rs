//! Pane contract. Every pane implements [`PaneView`]; the app owns one boxed
//! instance per [`Pane`] and routes input/render/tick to the visible one.
//! Downstream agents each own a single `panes/*.rs` file.

pub mod dashboard;
pub mod data;
pub mod developer;
pub mod device;
pub mod help;
pub mod logs;
pub mod models;
pub mod notes;
pub mod sessions;
pub mod settings;

use std::time::Duration;

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::Frame;

use store::Store;
use t1dm_core::BgUnit;

use crate::layout::LayoutMode;
use crate::theme::Theme;

/// The header-ordered set of panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Dashboard,
    Data,
    Models,
    Notes,
    Sessions,
    Device,
    Developer,
    Logs,
    Settings,
    Help,
}

impl Pane {
    /// Panes in header order. `Help` is an overlay and is intentionally last;
    /// it is not part of the Tab cycle set returned by [`Pane::cycle`].
    pub const HEADER: [Pane; 10] = [
        Pane::Dashboard,
        Pane::Data,
        Pane::Models,
        Pane::Notes,
        Pane::Sessions,
        Pane::Device,
        Pane::Developer,
        Pane::Logs,
        Pane::Settings,
        Pane::Help,
    ];

    /// The Tab-cycle set (everything except the Help overlay).
    pub const CYCLE: [Pane; 9] = [
        Pane::Dashboard,
        Pane::Data,
        Pane::Models,
        Pane::Notes,
        Pane::Sessions,
        Pane::Device,
        Pane::Developer,
        Pane::Logs,
        Pane::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Dashboard => "Dashboard",
            Pane::Data => "Data",
            Pane::Models => "Models",
            Pane::Notes => "Notes",
            Pane::Sessions => "Sessions",
            Pane::Device => "Device",
            Pane::Developer => "Developer",
            Pane::Logs => "Logs",
            Pane::Settings => "Settings",
            Pane::Help => "Help",
        }
    }

    /// Next pane in the Tab cycle.
    pub fn next(self) -> Pane {
        let set = Pane::CYCLE;
        let idx = set.iter().position(|&p| p == self).unwrap_or(0);
        set[(idx + 1) % set.len()]
    }

    /// Previous pane in the Tab cycle (BackTab).
    pub fn prev(self) -> Pane {
        let set = Pane::CYCLE;
        let idx = set.iter().position(|&p| p == self).unwrap_or(0);
        set[(idx + set.len() - 1) % set.len()]
    }
}

/// Render/update context handed to panes each frame. Cheap references plus
/// copyable display state; panes mutate `dirty` to request another frame.
pub struct Ctx<'a> {
    pub store: &'a Store,
    pub theme: &'a dyn Theme,
    pub unit: BgUnit,
    pub layout: LayoutMode,
    /// Pi-side wall clock in epoch ms for this frame.
    pub now_ms: i64,
    /// Whether at least one client is currently connected.
    pub connected: bool,
    /// Delta time since the previous frame (for animation).
    pub dt: Duration,
    /// Set by a pane to request an additional redraw (animation in flight).
    pub dirty: bool,
    /// Advertised host for login QR payloads (from the `[qr]` config).
    pub qr_addr: &'a str,
    /// Advertised port for login QR payloads (from the `[qr]` config).
    pub qr_port: u16,
}

impl Ctx<'_> {
    /// Convenience: mark that another frame is needed.
    pub fn request_redraw(&mut self) {
        self.dirty = true;
    }
}

/// Result of an input event: what the app should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing; no redraw required.
    None,
    /// Redraw, but no state change.
    Redraw,
    /// Switch to a specific pane.
    Switch(Pane),
    /// Move to the next pane in the cycle.
    NextPane,
    /// Move to the previous pane in the cycle.
    PrevPane,
    /// Toggle the Help overlay (returns to previous pane).
    ToggleHelp,
    /// Cycle the active theme.
    CycleTheme,
    /// Toggle the BG display unit.
    ToggleUnit,
    /// Quit the application.
    Quit,
    /// Pane-defined action, interpreted by the app (e.g. "mint-rw").
    Custom(String),
}

/// A visible pane. `title` and `render` are required; input and animation
/// hooks default to no-ops so stub panes stay tiny.
pub trait PaneView {
    /// Pane display title (usually [`Pane::title`]).
    fn title(&self) -> &str;

    /// Draw the pane into `area`.
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx);

    /// Handle a key event.
    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx) -> Action {
        Action::None
    }

    /// Handle a mouse event.
    fn on_mouse(&mut self, _mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        Action::None
    }

    /// Advance any live animation by `dt`. Only called for the visible pane.
    fn tick(&mut self, _dt: Duration, _ctx: &mut Ctx) {}
}

/// Construct the boxed pane implementation for a [`Pane`].
pub fn make_pane(pane: Pane) -> Box<dyn PaneView> {
    match pane {
        Pane::Dashboard => Box::new(dashboard::DashboardPane::default()),
        Pane::Data => Box::new(data::DataPane::default()),
        Pane::Models => Box::new(models::ModelsPane::default()),
        Pane::Notes => Box::new(notes::NotesPane::default()),
        Pane::Sessions => Box::new(sessions::SessionsPane::default()),
        Pane::Device => Box::new(device::DevicePane::default()),
        Pane::Developer => Box::new(developer::DeveloperPane::default()),
        Pane::Logs => Box::new(logs::LogsPane::default()),
        Pane::Settings => Box::new(settings::SettingsPane::default()),
        Pane::Help => Box::new(help::HelpPane::default()),
    }
}
