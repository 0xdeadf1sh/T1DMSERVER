//! WINDOWS XP theme — a bright "Luna/Bliss" world: a pale sky field with dark
//! ink text, Luna royal-blue chrome, and a Start-button green. Its boot flies
//! the four-colour Windows flag on a ripple above the iconic sliding loading
//! pill, and its right panel rolls the green Bliss hills beneath drifting
//! clouds. A light theme, so every glyph is dark-on-light.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{Glyphs, Palette, Theme, ThemeKind};
use crate::{anim, boot};

pub struct WinXp {
    palette: Palette,
    glyphs: Glyphs,
}

impl Default for WinXp {
    fn default() -> Self {
        WinXp {
            // A pale Luna sky. `fg` is a deep ink slate so text stays crisp on
            // the bright field (~12:1); `surface` is the classic XP control
            // beige (#ECE9D8) for window chrome. Blues, green, amber below are
            // deepened just enough to read against the light bg.
            palette: Palette {
                bg: Color::Rgb(0xEA, 0xF2, 0xFC),
                surface: Color::Rgb(0xEC, 0xE9, 0xD8),
                primary: Color::Rgb(0x2A, 0x5B, 0xDA),
                secondary: Color::Rgb(0x3C, 0x8F, 0x3C),
                accent: Color::Rgb(0xC7, 0x7A, 0x00),
                fg: Color::Rgb(0x16, 0x28, 0x3C),
                dim: Color::Rgb(0x5E, 0x70, 0x88),
                grid: Color::Rgb(0xC3, 0xD6, 0xEC),
                hypo: Color::Rgb(0x6D, 0x2C, 0xC9),
                hyper: Color::Rgb(0xD3, 0x32, 0x32),
                fan: [
                    Color::Rgb(0x4F, 0x86, 0xC6),
                    Color::Rgb(0x86, 0xB0, 0xDE),
                    Color::Rgb(0x7C, 0xA6, 0x5C),
                ],
            },
            glyphs: Glyphs {
                live_point: '■',
                photo_marker: '▦',
                spark: ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
            },
        }
    }
}

/// The four-pane Windows flag (2×2 blocks) over the wordmark. Left/right panes
/// and top/bottom halves map to the flag's four quadrants; the boot sequence
/// colours them, the Help pane draws them in the primary blue. Byte length ==
/// column count (pure ASCII blocks aside), so column maths stay simple.
const LOGO: &[&str] = &[
    " ████ ████",
    " ████ ████",
    " ████ ████",
    " ████ ████",
    "WINDOWS XP",
];

/// The genuine Microsoft four-colour logo: red, green / blue, yellow.
const FLAG_RED: Color = Color::Rgb(0xF6, 0x53, 0x14);
const FLAG_GREEN: Color = Color::Rgb(0x7C, 0xBB, 0x00);
const FLAG_BLUE: Color = Color::Rgb(0x00, 0xA1, 0xF1);
const FLAG_YELLOW: Color = Color::Rgb(0xFF, 0xBB, 0x00);

/// Quadrant colour for a flag cell at logo line `row` (0..4) and column index
/// `col` within the " ████ ████" line: left pane is cols 1..=4, right 6..=9.
fn flag_color(row: usize, col: usize) -> Color {
    let top = row < 2;
    let left = col <= 4;
    match (top, left) {
        (true, true) => FLAG_RED,
        (true, false) => FLAG_GREEN,
        (false, true) => FLAG_BLUE,
        (false, false) => FLAG_YELLOW,
    }
}

impl Theme for WinXp {
    fn kind(&self) -> ThemeKind {
        ThemeKind::WinXp
    }
    fn name(&self) -> &'static str {
        "WINDOWS XP"
    }
    fn palette(&self) -> &Palette {
        &self.palette
    }
    fn glyphs(&self) -> &Glyphs {
        &self.glyphs
    }
    fn logo(&self) -> &'static [&'static str] {
        LOGO
    }
    fn boot_tagline(&self) -> &'static str {
        "welcome"
    }
    /// A tiny four-pane window for the header.
    fn badge(&self) -> &'static str {
        "⊞ XP"
    }
    /// Smooth Luna settle.
    fn ease(&self, t: f32) -> f32 {
        anim::ease_out_cubic(t.clamp(0.0, 1.0))
    }

    /// A calm Luna glow on the live point.
    fn point_pulse(&self, phase: f32) -> f32 {
        0.55 + 0.45 * anim::sine01(phase)
    }

    /// Boot: the four-colour flag ripples in on a wave, the iconic loading pill
    /// marches its three blue segments beneath it, and "welcome" fades up over
    /// a thin divider — all on the pale sky field, dark-on-light.
    fn render_boot(&self, elapsed: Duration, area: Rect, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 16 || area.height < 12 {
            boot::default_boot_frame(self, elapsed, area, buf);
            return;
        }

        let dur = self.boot_duration().as_secs_f32().max(0.001);
        let p = (elapsed.as_secs_f32() / dur).clamp(0.0, 1.0);
        let t = elapsed.as_secs_f32();

        // Centre the 5-line logo block, biased up to leave room for the pill.
        let (lx, ly) = boot::logo_origin(area, LOGO);
        let ly = ly.saturating_sub(2);

        // Phase 1 — the flag flutters in, panes in their true colours. The wave
        // is broad and loose at first, then settles as the flag "unfurls".
        let appear = (p / 0.30).clamp(0.0, 1.0);
        let wave_amp = 0.6 + 1.4 * (1.0 - p);
        for (i, line) in LOGO.iter().enumerate().take(4) {
            for (j, ch) in line.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let col = flag_color(i, j);
                let wave = wave_amp * (j as f32 * 0.55 + t * 3.2).sin();
                let y = (ly as i32 + i as i32 + wave.round() as i32).max(0) as u16;
                let x = lx + j as u16;
                boot::put(buf, area, x, y, ch, anim::blend(pal.bg, col, appear), pal.bg);
            }
        }

        // The wordmark drops in under the flag once it is mostly unfurled.
        if p > 0.50 {
            let wp = self.ease(((p - 0.50) / 0.30).clamp(0.0, 1.0));
            let word = LOGO[4];
            let wx = area.left() + area.width.saturating_sub(word.chars().count() as u16) / 2;
            let wy = ly + 5;
            boot::put_str(buf, area, wx, wy, word, anim::blend(pal.bg, pal.primary, wp), pal.bg);
        }

        // Phase 2 — the loading pill: a rounded Luna-blue capsule with three
        // blue segments sliding left→right on a loop, exactly as XP boots.
        if p > 0.35 {
            let pill_w = 22u16.min(area.width.saturating_sub(4));
            let pill_x = area.left() + area.width.saturating_sub(pill_w) / 2;
            let pill_y = (ly + 7).min(area.bottom().saturating_sub(3));
            draw_pill(buf, area, pill_x, pill_y, pill_w, pal, elapsed);
        }

        // Phase 3 — "welcome" resolves over a thin divider, XP-login style.
        if p > 0.80 {
            let tp = self.ease(((p - 0.80) / 0.20).clamp(0.0, 1.0));
            let tag = self.boot_tagline();
            let ty = (ly + 10).min(area.bottom().saturating_sub(1));
            let rule_w = 18u16.min(area.width.saturating_sub(2));
            let rule_x = area.left() + area.width.saturating_sub(rule_w) / 2;
            let rule_col = anim::blend(pal.bg, pal.grid, tp);
            for k in 0..rule_w {
                boot::put(buf, area, rule_x + k, ty.saturating_sub(1), '─', rule_col, pal.bg);
            }
            let tx = area.left() + area.width.saturating_sub(tag.chars().count() as u16) / 2;
            boot::put_str(buf, area, tx, ty, tag, anim::blend(pal.bg, pal.primary, tp), pal.bg);
        }
    }

    fn has_animation(&self) -> bool {
        true
    }

    /// The BLISS motif, tiled down the right panel: a blue sky with white clouds
    /// drifting rightward at staggered per-lane speeds over two rolling green
    /// hill layers that scroll at different rates for parallax depth.
    fn render_animation(&self, area: Rect, elapsed: Duration, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 8 || area.height < 6 {
            return;
        }

        let sky_top = Color::Rgb(0x6F, 0xA8, 0xE6);
        let sky_bot = Color::Rgb(0xCF, 0xE6, 0xFB);
        let hill_back = Color::Rgb(0x6E, 0x9B, 0x4E);
        let hill_front = Color::Rgb(0x8C, 0xBF, 0x5C);
        let cloud = Color::Rgb(0xFF, 0xFF, 0xFF);
        let cloud_soft = Color::Rgb(0xE6, 0xEF, 0xFA);

        let ms = elapsed.as_millis() as u32;
        let lane_h: u16 = 10; // 6 sky rows + 4 hill rows
        let sky_h: u16 = 6;

        // A hill column height (in rows, fractional) for a layer: a slow sine of
        // the scrolling column, in [0, hill_h].
        let hill_h = (lane_h - sky_h) as f32;
        let contour = |x: u16, scroll: u32, freq: f32, phase: f32, amp: f32| -> f32 {
            let xf = (x as f32 + scroll as f32) * freq + phase;
            (0.5 + 0.5 * xf.sin()) * amp
        };

        let mut ly = area.top();
        let mut lane = 0u32;
        while ly < area.bottom() && lane < 256 {
            // Sky: a soft vertical gradient, deep at the top of the lane.
            for r in 0..sky_h {
                let y = ly + r;
                if y >= area.bottom() {
                    break;
                }
                let g = r as f32 / (sky_h.max(1) - 1).max(1) as f32;
                let col = anim::blend(sky_top, sky_bot, g);
                for x in area.left()..area.right() {
                    boot::put(buf, area, x, y, ' ', col, col);
                }
            }

            // Clouds: a couple of drifting puffs per lane, wrapping across width.
            for k in 0..2u32 {
                let seed = boot::hash2(lane, k, 0x8D);
                let cw = 4 + (seed % 3) as u16; // puff width
                let cy = ly + 1 + ((seed >> 4) % (sky_h.saturating_sub(2)).max(1) as u32) as u16;
                let speed = 1 + (seed >> 8) % 3;
                let span = area.width as u32 + cw as u32;
                let cx0 = ((seed >> 2) + ms / 90 * speed) % span;
                let cxl = area.left() as i32 + cx0 as i32 - cw as i32;
                for c in 0..cw {
                    let x = cxl + c as i32;
                    if x < area.left() as i32 || x >= area.right() as i32 {
                        continue;
                    }
                    let x = x as u16;
                    // Soft, rounded ends; a solid, brighter middle.
                    let edge = c == 0 || c == cw - 1;
                    let (ch, col) = if edge {
                        (if c == 0 { '▗' } else { '▖' }, cloud_soft)
                    } else {
                        ('▄', cloud)
                    };
                    boot::put(buf, area, x, cy.saturating_sub(1), ch, col, sky_bot);
                    boot::put(buf, area, x, cy, '█', cloud, sky_bot);
                }
            }

            // Hills: back layer first (darker, slower), then the brighter front.
            let hill_top = ly + sky_h;
            for x in area.left()..area.right() {
                let hb = contour(x, ms / 130, 0.11, 0.0, hill_h);
                let hf = contour(x, ms / 80, 0.15, 1.7, hill_h * 0.85) + hill_h * 0.15;
                paint_hill_col(buf, area, x, hill_top, lane_h - sky_h, hb, hill_back, sky_bot, self);
                paint_hill_col(buf, area, x, hill_top, lane_h - sky_h, hf, hill_front, sky_bot, self);
            }

            ly = ly.saturating_add(lane_h);
            lane += 1;
        }
    }
}

/// Paint one column of a hill layer: solid fill from the lane's bottom up to a
/// fractional height `h` (in rows), the topmost partial cell drawn with a
/// bottom-anchored block from the theme's spark ramp so the silhouette rolls
/// smoothly. Cells above the hill are left as-is (sky or a farther hill).
#[allow(clippy::too_many_arguments)]
fn paint_hill_col(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    hill_top: u16,
    hill_rows: u16,
    h: f32,
    color: Color,
    sky: Color,
    theme: &WinXp,
) {
    let h = h.clamp(0.0, hill_rows as f32);
    let full = h.floor() as u16;
    let frac = h - full as f32;
    let bottom = hill_top + hill_rows; // exclusive
    // Solid rows: the bottom `full` rows of the hill band.
    for r in 0..full {
        let y = bottom.saturating_sub(1 + r);
        if y < hill_top {
            break;
        }
        boot::put(buf, area, x, y, '█', color, color);
    }
    // Partial crown one row above the solid fill.
    if frac > 0.05 && full < hill_rows {
        let y = bottom.saturating_sub(1 + full);
        if y >= hill_top {
            let idx = anim::spark_index(frac, 0.0, 1.0).min(theme.glyphs.spark.len() - 1);
            let ch = theme.glyphs.spark[idx];
            boot::put(buf, area, x, y, ch, color, sky);
        }
    }
}

/// Draw the XP loading pill at `(x, y)` of width `w` (>= 6): a rounded capsule
/// outlined in the Luna primary, with three blue segments sliding rightward and
/// wrapping, the way the real boot bar animates.
fn draw_pill(buf: &mut Buffer, area: Rect, x: u16, y: u16, w: u16, pal: &Palette, elapsed: Duration) {
    if w < 6 {
        return;
    }
    let right = x + w - 1;
    // Rounded border on a single interior row.
    boot::put(buf, area, x, y, '╭', pal.primary, pal.bg);
    boot::put(buf, area, right, y, '╮', pal.primary, pal.bg);
    boot::put(buf, area, x, y + 2, '╰', pal.primary, pal.bg);
    boot::put(buf, area, right, y + 2, '╯', pal.primary, pal.bg);
    for cx in (x + 1)..right {
        boot::put(buf, area, cx, y, '─', pal.primary, pal.bg);
        boot::put(buf, area, cx, y + 2, '─', pal.primary, pal.bg);
        boot::put(buf, area, cx, y + 1, ' ', pal.bg, pal.bg);
    }
    boot::put(buf, area, x, y + 1, '│', pal.primary, pal.bg);
    boot::put(buf, area, right, y + 1, '│', pal.primary, pal.bg);

    // Three segments (2 cells each, 1-cell gap) march across the interior and
    // wrap, so the train slides in from the left as it exits the right.
    let inner_lo = x + 1;
    let inner_w = w.saturating_sub(2);
    if inner_w == 0 {
        return;
    }
    let train = 8i32; // 3 blocks * 2 + 2 gaps
    let period = inner_w as i32 + train;
    let head = (elapsed.as_millis() as i32 / 55) % period - train;
    let seg = Color::Rgb(0x3A, 0x74, 0xE0);
    let seg_hi = Color::Rgb(0x8F, 0xB8, 0xF4);
    for s in 0..3i32 {
        let base = head + s * 3;
        for d in 0..2i32 {
            let off = base + d;
            if off >= 0 && off < inner_w as i32 {
                let cx = inner_lo + off as u16;
                let col = if d == 0 { seg } else { seg_hi };
                boot::put(buf, area, cx, y + 1, '█', col, pal.bg);
            }
        }
    }
}
