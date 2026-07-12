//! Boot sequence engine. Each theme owns its ~5s launch animation; any key
//! skips it, and `[ui].show_boot=false` disables it entirely. The engine
//! here drives timing; the theme paints each frame.

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Frame;

use crate::theme::Theme;

/// Live boot animation state.
pub struct BootSequence {
    start: Instant,
    duration: Duration,
    skipped: bool,
}

impl BootSequence {
    pub fn new(theme: &dyn Theme) -> Self {
        BootSequence {
            start: Instant::now(),
            duration: theme.boot_duration(),
            skipped: false,
        }
    }

    /// Elapsed since launch.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// True once the sequence has finished or been skipped.
    pub fn done(&self) -> bool {
        self.skipped || self.start.elapsed() >= self.duration
    }

    /// Any key skips the sequence.
    pub fn skip(&mut self) {
        self.skipped = true;
    }

    /// Paint the current boot frame via the active theme.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &dyn Theme) {
        theme.render_boot(self.elapsed(), area, frame.buffer_mut());
    }
}

// ---------------------------------------------------------------------------
// Shared paint helpers for theme boot sequences. All are bounds-checked so a
// tiny or resized terminal can never index the buffer out of range.
// ---------------------------------------------------------------------------

/// Write a single cell only if it lies inside `area`.
pub(crate) fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, ch: char, fg: Color, bg: Color) {
    if x >= area.left() && x < area.right() && y >= area.top() && y < area.bottom() {
        buf[(x, y)].set_char(ch).set_fg(fg).set_bg(bg);
    }
}

/// Write a string left-to-right from `(x, y)`, clipping at the area edge.
pub(crate) fn put_str(buf: &mut Buffer, area: Rect, x: u16, y: u16, s: &str, fg: Color, bg: Color) {
    for (i, ch) in s.chars().enumerate() {
        let cx = x.saturating_add(i as u16);
        if cx >= area.right() {
            break;
        }
        put(buf, area, cx, y, ch, fg, bg);
    }
}

/// Flood `area` with a solid background wash.
pub(crate) fn fill_bg(area: Rect, buf: &mut Buffer, color: Color) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_char(' ').set_fg(color).set_bg(color);
        }
    }
}

/// Top-left origin that centres a block of ASCII-art lines within `area`,
/// biased slightly above centre to leave room for a tagline below.
pub(crate) fn logo_origin(area: Rect, art: &[&str]) -> (u16, u16) {
    let h = art.len() as u16;
    let w = art.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let x = area.left() + area.width.saturating_sub(w) / 2;
    let y = area.top() + area.height.saturating_sub(h + 2) / 2;
    (x, y)
}

/// A cheap deterministic hash over two coordinates and a salt, used to drive
/// pseudo-random flourishes (grid materialisation, sparkle rain) without any
/// per-frame allocation or RNG state.
pub(crate) fn hash2(x: u32, y: u32, salt: u32) -> u32 {
    let mut h =
        x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Default boot frame: clears to the theme background, centers the ASCII
/// logo, and reveals the tagline in the final second. Themes may override
/// [`Theme::render_boot`] for signature flourishes.
pub fn default_boot_frame<T: Theme + ?Sized>(
    theme: &T,
    elapsed: Duration,
    area: Rect,
    buf: &mut Buffer,
) {
    let pal = theme.palette();
    // Background wash.
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_char(' ').set_bg(pal.bg);
        }
    }

    let logo = theme.logo();
    let logo_h = logo.len() as u16;
    let start_y = area.top() + area.height.saturating_sub(logo_h + 2) / 2;

    let reveal = {
        let total = theme.boot_duration().as_secs_f32().max(0.001);
        (elapsed.as_secs_f32() / total).clamp(0.0, 1.0)
    };

    for (i, line) in logo.iter().enumerate() {
        let y = start_y + i as u16;
        if y >= area.bottom() {
            break;
        }
        let w = line.chars().count() as u16;
        let x = area.left() + area.width.saturating_sub(w) / 2;
        // Reveal logo lines progressively over the first ~80% of the sequence.
        let line_reveal = (reveal / 0.8 * logo.len() as f32) as usize;
        let style = if i <= line_reveal {
            Style::default().fg(pal.primary).bg(pal.bg)
        } else {
            Style::default().fg(pal.dim).bg(pal.bg)
        };
        buf.set_string(x, y, line, style);
    }

    if reveal > 0.8 {
        let tag = theme.boot_tagline();
        let ty = (start_y + logo_h + 1).min(area.bottom().saturating_sub(1));
        let tx = area.left() + area.width.saturating_sub(tag.chars().count() as u16) / 2;
        buf.set_string(tx, ty, tag, Style::default().fg(pal.secondary).bg(pal.bg));
    }
}
