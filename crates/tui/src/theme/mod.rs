//! Theme system. Themes are declarative and layout-constant: each owns a
//! full semantic palette, animation-style hooks, a boot sequence, an ASCII
//! logo, and a glyph set. Adding a fourth theme is one `ThemeKind` variant
//! plus one module.

pub mod hellokitty;
pub mod tron;
pub mod umbrella;

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::anim;

/// Which committed theme is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Tron,
    Umbrella,
    HelloKitty,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 3] = [ThemeKind::Tron, ThemeKind::Umbrella, ThemeKind::HelloKitty];

    /// Config string identifier.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeKind::Tron => "tron",
            ThemeKind::Umbrella => "umbrella",
            ThemeKind::HelloKitty => "hellokitty",
        }
    }

    /// Parse from the config `[ui].theme` value; defaults to Tron.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "umbrella" | "umbrella_corp" | "umbrella-corp" => ThemeKind::Umbrella,
            "hellokitty" | "hello_kitty" | "hello-kitty" => ThemeKind::HelloKitty,
            _ => ThemeKind::Tron,
        }
    }

    /// Cycle to the next theme (TUI `t` key).
    pub fn next(self) -> Self {
        match self {
            ThemeKind::Tron => ThemeKind::Umbrella,
            ThemeKind::Umbrella => ThemeKind::HelloKitty,
            ThemeKind::HelloKitty => ThemeKind::Tron,
        }
    }
}

/// The full semantic palette. Field names are stable so every widget reads
/// the same slots across all themes.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub fg: Color,
    pub dim: Color,
    pub grid: Color,
    pub hypo: Color,
    pub hyper: Color,
    /// Three shaded fan tiers, innermost (tightest quantiles) first.
    pub fan: [Color; 3],
}

/// How a theme swaps one pane for another. Layout is identical across themes;
/// only the *motion* differs. The renderer maps this to a transition effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionStyle {
    /// Crossfade — the neutral default.
    Fade,
    /// Dissolve into flickering noise, then resolve (Tron).
    Derezz,
    /// Cold wireframe re-assembly (Umbrella).
    Wireframe,
    /// Bouncy slide with overshoot (Hello Kitty).
    Bounce,
}

/// A theme's glyph set for chart marks, markers, and boot flourishes.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub live_point: char,
    pub note_marker: char,
    pub photo_marker: char,
    pub spark: [char; 8],
}

impl Default for Glyphs {
    fn default() -> Self {
        Glyphs {
            live_point: '●',
            note_marker: '◆',
            photo_marker: '▣',
            spark: ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
        }
    }
}

/// A theme: palette + animation style + boot sequence + identity.
pub trait Theme: Send + Sync {
    fn kind(&self) -> ThemeKind;
    fn name(&self) -> &'static str;
    fn palette(&self) -> &Palette;
    fn glyphs(&self) -> &Glyphs;

    /// Colour of the beating-heart HR indicator. Defaults to the hyper slot;
    /// Tron overrides it to its lime primary.
    fn heart_color(&self) -> Color {
        self.palette().hyper
    }

    /// Multi-line ASCII logo used on the boot screen and Help pane.
    fn logo(&self) -> &'static [&'static str];

    /// The boot tagline shown as the sequence completes.
    fn boot_tagline(&self) -> &'static str;

    /// A COMPACT one-line icon shown in the header in place of the theme name.
    /// Keep it narrow (<= ~8 display columns) and single-width-safe.
    fn badge(&self) -> &'static str {
        "◈"
    }

    /// Easing style for metric bars / transitions; input and output in 0..=1.
    /// The default is a smoothstep; themes may override for character.
    fn ease(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Boot-sequence duration.
    fn boot_duration(&self) -> Duration {
        Duration::from_millis(5000)
    }

    /// Render one boot frame at `elapsed` into `area`. The default centers
    /// the logo and reveals the tagline in the final second; themes override
    /// to add their signature flourish.
    fn render_boot(&self, elapsed: Duration, area: Rect, buf: &mut Buffer) {
        crate::boot::default_boot_frame(self, elapsed, area, buf);
    }

    // -- Animation STYLE hooks -------------------------------------------------
    // Shared vocabulary, per-theme character. Each returns a pure scalar (or a
    // colour) the widgets scale their draw by; all default to a tasteful neutral
    // so a fourth theme need override only what it wants to distinguish.

    /// How panes transition in this theme.
    fn transition(&self) -> TransitionStyle {
        TransitionStyle::Fade
    }

    /// Brightness 0..=1 of the live BG point for a wrapping pulse `phase`.
    /// Tron glows, Umbrella blinks mechanically, Hello Kitty breathes.
    fn point_pulse(&self, phase: f32) -> f32 {
        anim::sine01(phase)
    }

    /// Shimmer intensity 0..=1 of fan `tier` (0 = innermost) at `phase`.
    fn fan_shimmer(&self, tier: usize, phase: f32) -> f32 {
        anim::shimmer(tier, phase)
    }

    /// Alert-flash intensity 0..=1 at a wrapping `phase`.
    fn alert_flash(&self, phase: f32) -> f32 {
        anim::pulse(phase)
    }

    /// Optional decorative overlay colour for cell `(x, y)` at `elapsed` — the
    /// theme's signature surface texture (Tron scanlines, Umbrella hazard
    /// stripes, Hello Kitty sparkle). `None` leaves the cell untouched, so this
    /// never alters layout, only ornament.
    fn texture(&self, x: u16, y: u16, elapsed: Duration) -> Option<Color> {
        let _ = (x, y, elapsed);
        None
    }

    /// Render this theme's signature animation into `area`, tiling VERTICALLY to
    /// fill the height. Called each frame for the right-hand panel (Full layout).
    /// `elapsed` is monotonic since app start. Default: no-op.
    fn render_animation(&self, area: Rect, elapsed: Duration, buf: &mut Buffer) {
        let _ = (area, elapsed, buf);
    }

    /// Whether this theme paints a right-panel animation (keeps the demand-driven
    /// loop serving frames while the right panel is visible). Default false.
    fn has_animation(&self) -> bool {
        false
    }
}

/// Construct the theme implementation for a kind.
pub fn theme_for(kind: ThemeKind) -> Box<dyn Theme> {
    match kind {
        ThemeKind::Tron => Box::new(tron::Tron::default()),
        ThemeKind::Umbrella => Box::new(umbrella::Umbrella::default()),
        ThemeKind::HelloKitty => Box::new(hellokitty::HelloKitty::default()),
    }
}
