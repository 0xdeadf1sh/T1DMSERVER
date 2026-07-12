//! TRON LEGACY theme — lime/cyan on near-black, glow + scanlines + derezz.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{Glyphs, Palette, Theme, ThemeKind, TransitionStyle};
use crate::{anim, boot};

/// A neon light-cycle in side profile, read left-to-right: rear wheel, glowing
/// chassis, front wheel, and a nose wedge marking the direction of travel. Its
/// light-wall trail streams out behind (to the left) and fades into the grid.
const CYCLE: &[char] = &['◉', '═', '◉', '▸'];
/// Cells of fading light-wall drawn behind each cycle.
const TRAIL: i32 = 9;
/// Rows per lane: the cycle rides the top row, a dashed track sits beneath.
const LANE_H: u16 = 2;
/// Per-lane cruising speeds (cells/sec); staggered so the field reads as a race.
const SPEEDS: [f32; 5] = [11.0, 15.0, 8.0, 13.0, 9.5];
/// Per-lane phase offsets (cells) so no two cycles start abreast.
const OFFS: [f32; 5] = [0.0, 7.0, 15.0, 3.0, 20.0];

pub struct Tron {
    palette: Palette,
    glyphs: Glyphs,
}

impl Default for Tron {
    fn default() -> Self {
        Tron {
            palette: Palette {
                bg: Color::Rgb(0x05, 0x07, 0x0A),
                surface: Color::Rgb(0x0D, 0x14, 0x20),
                primary: Color::Rgb(0x9E, 0xFF, 0x00),
                secondary: Color::Rgb(0x00, 0xE5, 0xFF),
                accent: Color::Rgb(0x1B, 0x6F, 0xEB),
                fg: Color::Rgb(0xD6, 0xFF, 0xF6),
                dim: Color::Rgb(0x55, 0x6E, 0x86),
                grid: Color::Rgb(0x12, 0x30, 0x3A),
                hypo: Color::Rgb(0xFF, 0x2D, 0x55),
                hyper: Color::Rgb(0xFF, 0xB3, 0x00),
                fan: [
                    Color::Rgb(0x9E, 0xFF, 0x00),
                    Color::Rgb(0x4F, 0xC2, 0x8C),
                    Color::Rgb(0x00, 0xE5, 0xFF),
                ],
            },
            glyphs: Glyphs {
                live_point: '◉',
                note_marker: '◆',
                photo_marker: '▣',
                spark: ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
            },
        }
    }
}

// A neon light-cycle in side profile: sleek enclosed pod, cockpit canopy with a
// forward-trailing cowl, two clearly round wheels bridged by a glowing chassis,
// and a plain-lettered wordmark below. Every line is padded to an identical 41
// columns so the boot's per-line centring keeps the silhouette aligned; the
// trailing spaces are load-bearing — do not trim them.
const LOGO: &[&str] = &[
    r"              ______                     ",
    r"            _/  ()  \_____________       ",
    r"       ____/                      \____  ",
    r"      /    ___                  ___    \ ",
    r"     |    /   \                /   \    |",
    r"     |   | (O) |==============| (O) |   |",
    r"      \   \___/                \___/   / ",
    r"       \_______/            \_______/    ",
    r"          T R O N   L E G A C Y          ",
];

impl Theme for Tron {
    fn kind(&self) -> ThemeKind {
        ThemeKind::Tron
    }
    fn name(&self) -> &'static str {
        "TRON LEGACY"
    }
    fn palette(&self) -> &Palette {
        &self.palette
    }
    fn glyphs(&self) -> &Glyphs {
        &self.glyphs
    }
    /// The heart beats in Tron's lime, not the amber hyper hue.
    fn heart_color(&self) -> Color {
        self.palette.primary
    }
    fn logo(&self) -> &'static [&'static str] {
        LOGO
    }
    fn boot_tagline(&self) -> &'static str {
        "SYSTEM ONLINE"
    }
    /// A tiny two-wheeler for the header: twin glowing wheels bridged by a
    /// heavy neon chassis.
    fn badge(&self) -> &'static str {
        "◉━◉"
    }
    /// Sharper ease — a crisp derezz feel.
    fn ease(&self, t: f32) -> f32 {
        anim::ease_in_quad(t)
    }

    fn transition(&self) -> TransitionStyle {
        TransitionStyle::Derezz
    }

    /// A steady glow that never fully dims — the point always burns.
    fn point_pulse(&self, phase: f32) -> f32 {
        0.62 + 0.38 * anim::sine01(phase)
    }

    /// Crisp, quick tier shimmer with a cyan-scanline cadence.
    fn fan_shimmer(&self, tier: usize, phase: f32) -> f32 {
        anim::sine01(phase * 1.6 + tier as f32 * 0.28)
    }

    /// Hard, fast alert flash.
    fn alert_flash(&self, phase: f32) -> f32 {
        anim::blink(phase, 0.5)
    }

    /// A living neon backdrop: parallel light-cycle sweeps ride a diagonal
    /// across the grid, their leading edge glowing in alternating lime and
    /// cyan bands, over CRT scanlines that creep steadily upward.
    fn texture(&self, x: u16, y: u16, elapsed: Duration) -> Option<Color> {
        let pal = &self.palette;
        let ms = elapsed.as_millis() as i64;

        // Neon sweep: a family of diagonals scrolling right-to-down. Each band
        // is two cells thick — a bright leading edge trailed by a softer glow —
        // so the light bleeds rather than snaps.
        let sweep = ms / 45;
        let diag = (x as i64 + (y as i64) * 2 - sweep).rem_euclid(52);
        if diag < 2 {
            let cyan = (x as i64 + y as i64 - sweep / 2).rem_euclid(80) < 40;
            let neon = if cyan { pal.secondary } else { pal.primary };
            let t = if diag == 0 { 0.55 } else { 0.26 };
            return Some(anim::blend(pal.grid, neon, t));
        }

        // Scanlines beneath: every third row, creeping upward over time.
        let scan = (ms / 90) as u16;
        if (y.wrapping_add(scan)).is_multiple_of(3) {
            return Some(pal.grid);
        }
        None
    }

    fn has_animation(&self) -> bool {
        true
    }

    /// The right-panel signature: a stack of horizontal lanes, each a neon
    /// light-cycle speeding rightward and dragging a fading light-wall behind
    /// it, wrapping around. Lanes run at staggered speeds and phases so the
    /// field reads as a race; the panel tiles vertically to any height.
    fn render_animation(&self, area: Rect, elapsed: Duration, buf: &mut Buffer) {
        let pal = &self.palette;
        if area.width == 0 || area.height == 0 {
            return;
        }
        boot::fill_bg(area, buf, pal.bg);

        let secs = elapsed.as_secs_f32();
        let w = area.width as i32;
        let sprite_len = CYCLE.len() as i32;
        // A cycle travels from fully off the left (trail hidden) to fully off
        // the right, then wraps — so `span` covers the whole entry-to-exit run.
        let span = (w + TRAIL + sprite_len) as f32;
        let lanes = area.height / LANE_H;

        for lane in 0..lanes {
            let ay = area.top() + lane * LANE_H;
            let neon = match lane % 3 {
                0 => pal.primary,
                1 => pal.secondary,
                _ => pal.accent,
            };
            let sp = SPEEDS[(lane as usize) % SPEEDS.len()];
            let off = OFFS[(lane as usize) % OFFS.len()];
            // Rear-wheel column, in lane-local space; may sit off either edge.
            let rear = (secs * sp + off).rem_euclid(span).floor() as i32 - TRAIL;

            // Dashed lane track beneath the cycle, drawn first so nothing on the
            // rider row can be clobbered.
            let ty = ay + 1;
            if ty < area.bottom() {
                for lx in (0..w).step_by(2) {
                    boot::put(
                        buf,
                        area,
                        area.left() + lx as u16,
                        ty,
                        '·',
                        pal.grid,
                        pal.bg,
                    );
                }
            }

            // Light-wall: heaviest just behind the rear wheel, fading to grid.
            for d in 1..=TRAIL {
                let lx = rear - d;
                if lx < 0 || lx >= w {
                    continue;
                }
                let frac = 1.0 - (d - 1) as f32 / TRAIL as f32;
                let col = anim::blend(pal.grid, neon, frac * 0.85);
                let ch = if (d as f32) < TRAIL as f32 * 0.6 {
                    '━'
                } else {
                    '─'
                };
                boot::put(buf, area, area.left() + lx as u16, ay, ch, col, pal.bg);
            }

            // The cycle itself; the nose blooms white-hot as the leading edge.
            for (k, &ch) in CYCLE.iter().enumerate() {
                let lx = rear + k as i32;
                if lx < 0 || lx >= w {
                    continue;
                }
                let col = if k + 1 == CYCLE.len() {
                    anim::scale(neon, 1.4)
                } else {
                    neon
                };
                boot::put(buf, area, area.left() + lx as u16, ay, ch, col, pal.bg);
            }
        }
    }

    /// Boot: a power grid materialises, a light-cycle trail draws the logo, and
    /// "SYSTEM ONLINE" derezzes into place.
    fn render_boot(&self, elapsed: Duration, area: Rect, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 10 || area.height < 7 {
            boot::default_boot_frame(self, elapsed, area, buf);
            return;
        }

        let dur = self.boot_duration().as_secs_f32().max(0.001);
        let p = (elapsed.as_secs_f32() / dur).clamp(0.0, 1.0);
        let ems = elapsed.as_millis() as u32;

        // Phase 1 — grid materialises, then fades as the logo takes over.
        if p < 0.64 {
            let appear = (p / 0.32).clamp(0.0, 1.0);
            let fade = 1.0 - ((p - 0.32) / 0.32).clamp(0.0, 1.0);
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    let vline = x % 6 == 0;
                    let hline = y % 3 == 0;
                    if !vline && !hline {
                        continue;
                    }
                    let thr = (boot::hash2(x as u32, y as u32, 1) % 100) as f32 / 100.0;
                    if thr > appear {
                        continue;
                    }
                    let ch = if vline && hline {
                        '+'
                    } else if vline {
                        '│'
                    } else {
                        '─'
                    };
                    let col = anim::blend(pal.grid, pal.secondary, 0.30 * fade);
                    boot::put(buf, area, x, y, ch, col, pal.bg);
                }
            }
        }

        // Phase 2 — light-cycle trail draws the logo left to right.
        let (_, oy) = boot::logo_origin(area, LOGO);
        let draw_p = ((p - 0.28) / 0.50).clamp(0.0, 1.0);
        for (i, line) in LOGO.iter().enumerate() {
            let ly = oy + i as u16;
            let n = line.chars().count();
            if n == 0 {
                continue;
            }
            let lx = area.left() + area.width.saturating_sub(n as u16) / 2;
            let show = (draw_p * n as f32).ceil() as usize;
            for (j, ch) in line.chars().enumerate() {
                if j >= show {
                    break;
                }
                if ch == ' ' {
                    continue;
                }
                let head = j + 1 == show && draw_p < 1.0;
                let col = if head { pal.secondary } else { pal.primary };
                boot::put(buf, area, lx + j as u16, ly, ch, col, pal.bg);
            }
        }

        // Phase 3 — the tagline derezzes in, char by char out of noise.
        if p > 0.70 {
            let tp = ((p - 0.70) / 0.30).clamp(0.0, 1.0);
            let tag = self.boot_tagline();
            let tn = tag.chars().count() as u16;
            let ty = (oy + LOGO.len() as u16 + 1).min(area.bottom().saturating_sub(1));
            let tx = area.left() + area.width.saturating_sub(tn) / 2;
            let noise = ['/', '\\', '|', '_', '·', '='];
            for (k, ch) in tag.chars().enumerate() {
                let x = tx + k as u16;
                let hv = (boot::hash2(k as u32, 9, 0) % 100) as f32 / 100.0;
                if hv < tp {
                    boot::put(buf, area, x, ty, ch, pal.fg, pal.bg);
                } else {
                    let idx = (boot::hash2(k as u32, ty as u32, ems / 60) as usize) % noise.len();
                    boot::put(buf, area, x, ty, noise[idx], pal.dim, pal.bg);
                }
            }
        }
    }
}
