//! HELLO KITTY theme — a bright, light-pink world: bouncy easing, a bow-topped
//! kitty logo, and a drifting glitter of pastel hearts and sparkles.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{Glyphs, Palette, Theme, ThemeKind, TransitionStyle};
use crate::{anim, boot};

pub struct HelloKitty {
    palette: Palette,
    glyphs: Glyphs,
}

impl Default for HelloKitty {
    fn default() -> Self {
        HelloKitty {
            // A soft, light cotton-candy field. `fg` is a deep berry so text
            // stays crisply legible on the bright background (contrast ~10:1);
            // the pinks, blue, gold and mint below are all deepened just enough
            // to read against the light bg rather than wash out.
            palette: Palette {
                bg: Color::Rgb(0xFF, 0xE1, 0xEE),
                surface: Color::Rgb(0xFB, 0xCF, 0xE4),
                primary: Color::Rgb(0xE5, 0x38, 0x8F),
                secondary: Color::Rgb(0x20, 0x68, 0xB0),
                accent: Color::Rgb(0xA0, 0x5C, 0x0E),
                fg: Color::Rgb(0x5C, 0x1A, 0x3B),
                dim: Color::Rgb(0x9A, 0x5E, 0x78),
                grid: Color::Rgb(0xEE, 0xC0, 0xD6),
                hypo: Color::Rgb(0x6A, 0x4B, 0xD8),
                hyper: Color::Rgb(0xD8, 0x36, 0x46),
                fan: [
                    Color::Rgb(0xFF, 0x9F, 0xC9),
                    Color::Rgb(0xF2, 0x6F, 0xB0),
                    Color::Rgb(0x53, 0xC1, 0x9E),
                ],
            },
            glyphs: Glyphs {
                live_point: '❀',
                photo_marker: '❁',
                spark: ['⠁', '⠃', '⠇', '⠏', '⠟', '⠿', '⡿', '⣿'],
            },
        }
    }
}

/// A bow-topped kitty face. Every line is padded to the same display width and
/// is left/right symmetric, so it stacks cleanly whether centred per-line (boot)
/// or left-aligned (Help pane).
const LOGO: &[&str] = &[
    r"    ◣◈◢    ",
    r"  /\   /\  ",
    r" /  \_/  \ ",
    r"|  ●   ●  |",
    r"| =  ▾  = |",
    r" \  \_/  / ",
    r"  \_____/  ",
    r"HELLO KITTY",
];

/// A ribbon bow that assembles from the centre out before the logo drops in.
const BOW: &[&str] = &[
    r"  __       __  ",
    r" /  \_____/  \ ",
    r"(     ( ◈ )    )",
    r" \  _/     \_  /",
    r"  \/       \/  ",
];

/// Nyan Cat sprite: a grey kitty head atop a sprinkled pop-tart body, 8 columns
/// wide, its six rows aligned to the six rainbow bands so the tart sits flush
/// against the trail. Two leg frames alternate to bounce the paws. Pure ASCII,
/// so byte indexing equals column indexing.
const NYAN_A: [&str; 6] = [
    r"  /\_/\ ",
    r" ( o.o )",
    r"[======]",
    r"[.#.#.#]",
    r"[======]",
    r" ^    ^ ",
];
const NYAN_B: [&str; 6] = [
    r"  /\_/\ ",
    r" ( o.o )",
    r"[======]",
    r"[.#.#.#]",
    r"[======]",
    r"  v  v  ",
];

/// The six trailing rainbow bands, red through violet.
const RAINBOW: [Color; 6] = [
    Color::Rgb(0xFF, 0x3B, 0x30),
    Color::Rgb(0xFF, 0x95, 0x00),
    Color::Rgb(0xFF, 0xCC, 0x00),
    Color::Rgb(0x34, 0xC7, 0x59),
    Color::Rgb(0x0A, 0x84, 0xFF),
    Color::Rgb(0xAF, 0x52, 0xDE),
];

impl Theme for HelloKitty {
    fn kind(&self) -> ThemeKind {
        ThemeKind::HelloKitty
    }
    fn name(&self) -> &'static str {
        "HELLO KITTY"
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
        "(=^.w.^=) purr-fect"
    }
    /// A tiny whiskered kitty for the header.
    fn badge(&self) -> &'static str {
        "=^..^="
    }
    /// Bouncy overshoot ease (back-out).
    fn ease(&self, t: f32) -> f32 {
        anim::ease_out_back(t)
    }

    fn transition(&self) -> TransitionStyle {
        TransitionStyle::Bounce
    }

    /// A soft, bouncy breath.
    fn point_pulse(&self, phase: f32) -> f32 {
        0.55 + 0.45 * anim::sine01(phase)
    }

    /// Quick, playful tier twinkle.
    fn fan_shimmer(&self, tier: usize, phase: f32) -> f32 {
        anim::sine01(phase * 2.0 + tier as f32 * 0.4)
    }

    /// A gentle swelling flash.
    fn alert_flash(&self, phase: f32) -> f32 {
        anim::sine01(phase)
    }

    /// The theme's signature glitter: two drifting layers — cool sparkles
    /// streaming up-and-right, warm hearts wafting up-and-left — each cell
    /// twinkling in and out on its own phase so the whole field shimmers and
    /// flows rather than sitting still. Sparse enough to stay ambient beneath
    /// overlaid text.
    fn texture(&self, x: u16, y: u16, elapsed: Duration) -> Option<Color> {
        let pal = &self.palette;
        let ms = elapsed.as_millis() as u32;
        let drift = ms / 130;

        // Per-cell twinkle window: each cell lights for part of a 12-tick cycle,
        // offset by a spatial hash so neighbours pulse out of step.
        let tw = (ms / 95 + boot::hash2(x as u32, y as u32, 7) % 12) % 12;

        // Sparkles drift up and to the right (cool pastels).
        let s = boot::hash2(x as u32 + drift / 4, y as u32 + drift, 0xA5);
        if s.is_multiple_of(34) && tw < 7 {
            return Some(match (s >> 5) % 3 {
                0 => pal.secondary,
                1 => pal.accent,
                _ => pal.fan[2],
            });
        }

        // Hearts waft up and to the left, a touch slower (warm pinks).
        let h = boot::hash2(x as u32 + (drift * 3) / 2, y as u32 + drift / 2, 0x5A);
        if h.is_multiple_of(44) && tw >= 4 {
            return Some(match (h >> 5) % 2 {
                0 => pal.primary,
                _ => pal.fan[0],
            });
        }

        None
    }

    /// Boot: sparkle rain falls, a ribbon bow draws itself, then the kitty logo
    /// bounces in and "(=^.w.^=) purr-fect" pops with a sparkle burst — all on
    /// the bright pink field with dark-on-light glyphs.
    fn render_boot(&self, elapsed: Duration, area: Rect, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 14 || area.height < 8 {
            boot::default_boot_frame(self, elapsed, area, buf);
            return;
        }

        let dur = self.boot_duration().as_secs_f32().max(0.001);
        let p = (elapsed.as_secs_f32() / dur).clamp(0.0, 1.0);
        let ems = elapsed.as_millis() as u32;

        // Phase 1 — sparkle rain, thinning as the logo assembles.
        let rain = 1.0 - ((p - 0.55) / 0.30).clamp(0.0, 1.0);
        if rain > 0.02 {
            let sp = ['✦', '✧', '⋆', '·', '♥', '❀'];
            let span = area.height as u32;
            let count = (area.width as u32 * area.height as u32) / 12;
            for n in 0..count {
                let seed = boot::hash2(n, 0, 7);
                let x = area.left() + (seed % area.width as u32) as u16;
                let speed = 2 + (seed >> 7) % 6;
                let y = area.top() + (((seed >> 13) + (ems / 90) * speed) % span) as u16;
                let base = match (seed >> 3) % 4 {
                    0 => pal.primary,
                    1 => pal.secondary,
                    2 => pal.accent,
                    _ => pal.fan[2],
                };
                let ch = sp[((seed >> 5) as usize + (ems / 160) as usize) % sp.len()];
                // Fade the glitter up from the light background as it appears.
                boot::put(buf, area, x, y, ch, anim::blend(pal.bg, base, rain), pal.bg);
            }
        }

        // Vertical layout: optional bow stacked above the logo, both centred.
        let logo_h = LOGO.len() as u16;
        let bow_h = BOW.len() as u16;
        let show_bow = area.height >= bow_h + logo_h + 4;
        let block_h = if show_bow { bow_h + 1 + logo_h } else { logo_h };
        let top = area.top() + area.height.saturating_sub(block_h + 2) / 2;
        let (bow_y, logo_y) = if show_bow {
            (top, top + bow_h + 1)
        } else {
            (top, top)
        };

        // Phase 2 — the ribbon bow draws outward from its knot with a bounce.
        if show_bow {
            let ap = ((p - 0.30) / 0.40).clamp(0.0, 1.0);
            let boww = BOW.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
            let bcx = area.left() as i32 + area.width as i32 / 2;
            let reveal = anim::ease_out_back(ap).clamp(0.0, 1.0) * (boww as f32 / 2.0 + 1.0);
            for (i, line) in BOW.iter().enumerate() {
                let ly = bow_y + i as u16;
                let n = line.chars().count();
                let lx = area.left() + area.width.saturating_sub(n as u16) / 2;
                for (j, ch) in line.chars().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    let x = lx + j as u16;
                    let dist = (x as i32 - bcx).unsigned_abs() as f32;
                    if dist <= reveal {
                        let col = if (x as i32 - bcx).abs() <= 1 {
                            pal.accent
                        } else {
                            pal.primary
                        };
                        boot::put(buf, area, x, ly, ch, col, pal.bg);
                    }
                }
            }
        }

        // Phase 3 — the kitty logo bounces up into place.
        if p > 0.45 {
            let lp = ((p - 0.45) / 0.40).clamp(0.0, 1.0);
            let a = self.ease(lp).clamp(0.0, 1.0);
            let drop = ((1.0 - a) * 3.0) as u16;
            let col = anim::blend(pal.bg, pal.primary, a);
            for (i, line) in LOGO.iter().enumerate() {
                let ly = logo_y + i as u16 + drop;
                let n = line.chars().count();
                let lx = area.left() + area.width.saturating_sub(n as u16) / 2;
                for (j, ch) in line.chars().enumerate() {
                    if ch == ' ' {
                        continue;
                    }
                    boot::put(buf, area, lx + j as u16, ly, ch, col, pal.bg);
                }
            }
        }

        // Phase 4 — tagline pops with a small sparkle burst around it.
        if p > 0.82 {
            let tp = ((p - 0.82) / 0.18).clamp(0.0, 1.0);
            let tag = self.boot_tagline();
            let tn = tag.chars().count() as u16;
            let ty = (logo_y + logo_h + 1).min(area.bottom().saturating_sub(1));
            let tx = area.left() + area.width.saturating_sub(tn) / 2;
            let col = anim::blend(pal.bg, pal.primary, self.ease(tp).clamp(0.0, 1.0));
            boot::put_str(buf, area, tx, ty, tag, col, pal.bg);

            let burst = ['✦', '✧', '⋆', '♥'];
            let bn = (tp * 10.0) as u32;
            for k in 0..bn {
                let seed = boot::hash2(k, ty as u32, 3);
                let dx = (seed % (tn as u32 + 8)) as i32 - 4;
                let dy = ((seed >> 8) % 3) as i32 - 1;
                let x = (tx as i32 + dx).clamp(area.left() as i32, area.right() as i32 - 1) as u16;
                let y = (ty as i32 + dy).clamp(area.top() as i32, area.bottom() as i32 - 1) as u16;
                let ch = burst[(seed as usize >> 4) % burst.len()];
                boot::put(buf, area, x, y, ch, pal.secondary, pal.bg);
            }
        }
    }

    fn has_animation(&self) -> bool {
        true
    }

    /// The classic NYAN CAT, tiled down the right panel: a grey kitty on a
    /// sprinkled pop-tart flies rightward, streaming a six-band rainbow that
    /// scrolls beneath twinkling stars. Each lane runs at a staggered phase so
    /// the stack shimmers out of lockstep; the paws bounce frame to frame.
    fn render_animation(&self, area: Rect, elapsed: Duration, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 10 || area.height < 6 {
            return;
        }

        // Cat colours: a grey tabby, a toasted crust, a candy-pink tart.
        let grey = Color::Rgb(0x8C, 0x8C, 0x8C);
        let toast = Color::Rgb(0xC9, 0x6A, 0x2A);
        let tart = Color::Rgb(0xF7, 0x9E, 0xC4);
        let sprinkle = [Color::Rgb(0xFF, 0xFF, 0xFF), pal.secondary, pal.fan[2]];
        let dark = pal.fg;

        let ms = elapsed.as_millis() as u32;
        let scroll = ms / 70; // rainbow seam march
        let star_scroll = ms / 110; // starfield drift (leftward)

        let cat_w: u16 = 8;
        let cat_left = area.right().saturating_sub(cat_w + 1).max(area.left());

        // A twinkling star for a background cell, or None for empty sky.
        let star_at = |x: u16, y: u16| -> Option<(char, Color)> {
            let h = boot::hash2(x as u32 + star_scroll, y as u32, 0x33);
            if h.is_multiple_of(11) {
                let (ch, col) = match (h >> 4) % 3 {
                    0 => ('✦', Color::Rgb(0xFF, 0xFF, 0xFF)),
                    1 => ('·', pal.secondary),
                    _ => ('+', pal.accent),
                };
                Some((ch, col))
            } else {
                None
            }
        };

        let lane_h: u16 = 7; // 6 rainbow bands + 1 star-gap row
        let mut ly = area.top();
        let mut lane = 0u32;
        while ly < area.bottom() && lane < 256 {
            let legs_up = (ms / 150 + lane).is_multiple_of(2);
            let sprite = if legs_up { &NYAN_A } else { &NYAN_B };

            for r in 0..6u16 {
                let y = ly + r;
                if y >= area.bottom() {
                    break;
                }
                for x in area.left()..area.right() {
                    if x >= cat_left && x < cat_left + cat_w {
                        // Cat sprite cell (spaces fall through to the sky).
                        let ch = sprite[r as usize].as_bytes()[(x - cat_left) as usize] as char;
                        if ch == ' ' {
                            if let Some((sc, col)) = star_at(x, y) {
                                boot::put(buf, area, x, y, sc, col, pal.bg);
                            }
                            continue;
                        }
                        let (fg, bg) = match r {
                            0 | 1 => match ch {
                                'o' | '.' => (dark, pal.bg),
                                _ => (grey, pal.bg),
                            },
                            2 | 4 => (toast, tart),
                            _ => match ch {
                                '#' | '.' => (sprinkle[x as usize % sprinkle.len()], tart),
                                _ => (toast, tart),
                            },
                        };
                        boot::put(buf, area, x, y, ch, fg, bg);
                    } else if x < cat_left {
                        // Rainbow trail with a scrolling seam for a sense of speed.
                        let band = RAINBOW[r as usize];
                        let seam = (x as u32).wrapping_add(scroll).is_multiple_of(4);
                        let bg = if seam {
                            anim::blend(band, dark, 0.22)
                        } else {
                            band
                        };
                        boot::put(buf, area, x, y, ' ', bg, bg);
                    } else if let Some((sc, col)) = star_at(x, y) {
                        boot::put(buf, area, x, y, sc, col, pal.bg);
                    }
                }
            }

            // The star-gap row between lanes.
            let gy = ly + 6;
            if gy < area.bottom() {
                for x in area.left()..area.right() {
                    if let Some((sc, col)) = star_at(x, gy) {
                        boot::put(buf, area, x, gy, sc, col, pal.bg);
                    }
                }
            }

            ly = ly.saturating_add(lane_h);
            lane += 1;
        }
    }
}
