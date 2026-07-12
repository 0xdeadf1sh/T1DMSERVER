//! UMBRELLA CORP theme — oxblood/steel/hazard, a spinning parasol on boot and
//! blood that weeps down the dark. Mechanical blink, cold shimmer.

use std::f32::consts::{PI, TAU};
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{Glyphs, Palette, Theme, ThemeKind, TransitionStyle};
use crate::{anim, boot};

pub struct Umbrella {
    palette: Palette,
    glyphs: Glyphs,
}

impl Default for Umbrella {
    fn default() -> Self {
        Umbrella {
            palette: Palette {
                bg: Color::Rgb(0x0B, 0x0B, 0x0D),
                surface: Color::Rgb(0x14, 0x14, 0x16),
                primary: Color::Rgb(0xC8, 0x10, 0x2E),
                secondary: Color::Rgb(0x8A, 0x9B, 0xA8),
                accent: Color::Rgb(0xF2, 0xC2, 0x00),
                fg: Color::Rgb(0xD8, 0xD8, 0xDC),
                dim: Color::Rgb(0x78, 0x62, 0x64),
                grid: Color::Rgb(0x2A, 0x14, 0x16),
                hypo: Color::Rgb(0x39, 0xC0, 0xED),
                hyper: Color::Rgb(0xFF, 0x1E, 0x2D),
                fan: [
                    Color::Rgb(0xFF, 0x1E, 0x2D),
                    Color::Rgb(0xC8, 0x10, 0x2E),
                    Color::Rgb(0x8A, 0x9B, 0xA8),
                ],
            },
            glyphs: Glyphs {
                live_point: '⬢',
                note_marker: '▲',
                photo_marker: '▦',
                spark: ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
            },
        }
    }
}

/// Clean umbrella emblem — a domed canopy with a scalloped rim, pole, and a
/// crooked handle. Every line is padded to the same width so per-line centering
/// keeps the icon rigid wherever the logo is rendered.
const LOGO: &[&str] = &[
    r"         .-~~~-.         ",
    r"       .'       '.       ",
    r"      /___________\      ",
    r"      \/\/\/\/\/\/\/      ",
    r"            |            ",
    r"            |            ",
    r"           _/            ",
    r"  U M B R E L L A   C O R P  ",
];

/// Cold boot diagnostics that scroll before the parasol spins up — the
/// facility's twin obsessions, glucose telemetry and viral containment, read
/// out in a single deadpan ledger. Far more lines than fit at once, so the
/// boot marches them upward.
const DIAG: &[&str] = &[
    ">> UMBRELLA BIOSYS  v4.11",
    ">> RED QUEEN KERNEL ......... ONLINE",
    ">> HIVE CORE TEMP ........... NOMINAL",
    ">> BIOHAZARD SEALS .......... LOCKED",
    ">> T-VIRUS CONTAINMENT ...... OK",
    ">> G-VIRUS CRYOSTASIS ....... SEALED",
    ">> CGM SENSOR ARRAY ......... SYNCED",
    ">> GLUCOSE TELEMETRY ........ LINKED",
    ">> BASAL SOLENOID ........... ARMED",
    ">> BOLUS ACTUATOR ........... PRIMED",
    ">> INSULIN RESERVOIR ........ 98%",
    ">> ISLET-CELL ASSAY ......... STABLE",
    ">> PANCREAS SIMULATOR ....... ONLINE",
    ">> HbA1c REGISTER ........... 6.4%",
    ">> HYPO ALARM MATRIX ........ ARMED",
    ">> HYPER ALARM MATRIX ....... ARMED",
    ">> KETONE SCRUBBERS ......... GREEN",
    ">> GLUCAGON VIALS ........... STOCKED",
    ">> LANCET STERILIZER ........ OK",
    ">> INFUSION-SET PRESSURE .... NOMINAL",
    ">> SUBCUTANEOUS PROBE ....... LINKED",
    ">> BLOOD-SAMPLE CRYOBANK .... SEALED",
    ">> PLASMA CENTRIFUGE ........ SPUN",
    ">> ANTIVIRAL SYNTHESIS ...... RUNNING",
    ">> T-CELL CULTURE ........... VIABLE",
    ">> PROGENITOR TISSUE ........ FROZEN",
    ">> HIVE MAINFRAME LINK ...... UP",
    ">> ARKLAY UPLINK ............ SECURE",
    ">> RACCOON GRID ............. ONLINE",
    ">> WHITE UMBRELLA AUTH ...... GRANTED",
    ">> RED QUEEN HEURISTICS ..... LEARNED",
    ">> LASER CORRIDOR ........... IDLE",
    ">> DECON SHOWERS ............ CHARGED",
    ">> AIRLOCK B-7 .............. SEALED",
    ">> QUARANTINE WARD .......... CLEAR",
    ">> BIO-SAMPLE MANIFEST ...... SIGNED",
    ">> NANOMACHINE SWARM ........ DORMANT",
    ">> METABOLIC LEDGER ......... BALANCED",
    ">> CARB-COUNT ORACLE ........ CALIBRATED",
    ">> DEXTROSE DRIP ............ STEADY",
    ">> ADRENAL RELAY ............ NOMINAL",
    ">> CORTISOL BASELINE ........ LOGGED",
    ">> A-VIRUS FIREWALL ......... HOLDING",
    ">> LEECH SUBJECT LOG ........ ARCHIVED",
    ">> TYRANT VITALS ............ FLATLINE",
    ">> NEMESIS PARASITE ......... CAGED",
    ">> CRIMSON HEADS ............ CONTAINED",
    ">> LICKER MOTION GRID ....... QUIET",
    ">> SPENCER ESTATE FEED ...... LIVE",
    ">> UNDERGROUND LAB PWR ...... 100%",
    ">> COOLANT LOOP ............. FLOWING",
    ">> CENTRIFUGAL BASAL PUMP ... TUNED",
    ">> CONTINUOUS DEXCOM ........ STREAMING",
    ">> SENSOR GLUCOSE MEAN ...... 112 mg/dL",
    ">> TIME-IN-RANGE ............ 78%",
    ">> GLYCEMIC STD DEV ......... 41 mg/dL",
    ">> INSULIN-ON-BOARD ......... 1.2 U",
    ">> REDUNDANT AI: RED QUEEN .. VOTING",
    ">> ALL BIO-SEALS ............ CONFIRMED",
    ">> UMBRELLA CORP CONTAINMENT  ONLINE",
];

/// Pick the single-cell line glyph that best traces a stroke pointing along
/// `theta` (screen space: +x right, +y down). Buckets the direction into the
/// four ASCII stroke glyphs with π/8 hysteresis at the boundaries.
fn dir_glyph(theta: f32) -> char {
    let m = theta.rem_euclid(PI);
    match ((m + PI / 8.0) / (PI / 4.0)).floor() as i32 % 4 {
        0 => '-',
        1 => '\\',
        2 => '|',
        _ => '/',
    }
}

impl Theme for Umbrella {
    fn kind(&self) -> ThemeKind {
        ThemeKind::Umbrella
    }
    fn name(&self) -> &'static str {
        "UMBRELLA CORP"
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
        "CONTAINMENT ONLINE"
    }

    /// A tiny umbrella for the header.
    fn badge(&self) -> &'static str {
        "☂"
    }

    /// Mechanical, near-linear ease.
    fn ease(&self, t: f32) -> f32 {
        t.clamp(0.0, 1.0)
    }

    fn transition(&self) -> TransitionStyle {
        TransitionStyle::Wireframe
    }

    /// A hard mechanical blink — on 60% of the cycle, off the rest.
    fn point_pulse(&self, phase: f32) -> f32 {
        anim::blink(phase, 0.6)
    }

    /// Cold and restrained: tiers barely shimmer.
    fn fan_shimmer(&self, tier: usize, phase: f32) -> f32 {
        0.5 + 0.15 * anim::sine01(phase + tier as f32 * 0.33)
    }

    /// A stern square-wave alarm flash.
    fn alert_flash(&self, phase: f32) -> f32 {
        anim::blink(phase, 0.5)
    }

    /// BLOOD. Roughly a third of the columns weep a slow rivulet: a bright
    /// droplet head that descends over time, trailing an oxblood streak that
    /// fades upward toward where it fell from. Each column runs at its own
    /// hashed cadence, so the whole surface drips out of step — alive, never a
    /// static band.
    fn texture(&self, x: u16, y: u16, elapsed: Duration) -> Option<Color> {
        let seed = boot::hash2(x as u32, 0, 0x0081_002E);
        if !seed.is_multiple_of(3) {
            return None;
        }
        let period = 16 + (seed >> 4) % 14; // 16..=29 rows
        let speed = 0.5 + (seed % 6) as f32 * 0.35; // 0.5..=2.25 rows/s
        let phase0 = (seed >> 8) % period;
        let head = (elapsed.as_secs_f32() * speed + phase0 as f32).rem_euclid(period as f32);
        // Distance of this cell *above* the droplet head (the trail lags upward).
        let above = (head - y as f32).rem_euclid(period as f32);
        let trail = 4.0;
        if above > trail {
            return None;
        }
        if above < 1.0 {
            Some(self.palette.hyper)
        } else {
            let f = (1.0 - above / trail) * 0.7;
            Some(anim::blend(self.palette.bg, self.palette.primary, f))
        }
    }

    /// Boot: red facility diagnostics scroll, then an actual umbrella spins up
    /// at centre — a domed canopy of eight ribs sweeping around a hazard hub
    /// above its crooked handle — while "UMBRELLA CORP" and "CONTAINMENT
    /// ONLINE" lock in. Any key skips it.
    fn render_boot(&self, elapsed: Duration, area: Rect, buf: &mut Buffer) {
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);
        if area.width < 22 || area.height < 13 {
            boot::default_boot_frame(self, elapsed, area, buf);
            return;
        }

        let dur = self.boot_duration().as_secs_f32().max(0.001);
        let p = (elapsed.as_secs_f32() / dur).clamp(0.0, 1.0);
        let t = elapsed.as_secs_f32();
        let ems = elapsed.as_millis() as u32;

        // Phase 1 — the ~60-line diagnostic ledger types itself out live: a
        // running count of characters "typed so far" (keyed to `elapsed`, so
        // the reveal is deterministic) marches through the ledger. Completed
        // lines are drawn whole, the line under the cursor is drawn partially
        // with a blinking caret, lines not yet reached stay dark. The window
        // rides down the ledger so freshly-typed lines march down the screen,
        // then scroll — the whole log finishing before the parasol takes over.
        if p < 0.42 {
            let alpha = 1.0 - ((p - 0.34) / 0.08).clamp(0.0, 1.0);
            let view = (area.height.saturating_sub(3)).max(1) as usize;

            // Count every glyph plus one for each line break, then set a typing
            // rate that empties the ledger by t = 0.30·dur — comfortably inside
            // phase 1 and ahead of the fade at p ≈ 0.34.
            let total_chars: usize = DIAG.iter().map(|l| l.chars().count() + 1).sum();
            let cps = total_chars as f32 / (0.30 * dur).max(0.001);
            let typed = (t * cps) as usize;

            // Walk the ledger, accruing line lengths, to find the fully-typed
            // prefix (`active` = first not-yet-complete line) and how far the
            // cursor has crept into that active line (`partial`).
            let mut acc = 0usize;
            let mut active = DIAG.len();
            let mut partial = 0usize;
            for (i, line) in DIAG.iter().enumerate() {
                let len = line.chars().count() + 1;
                if typed >= acc + len {
                    acc += len;
                } else {
                    active = i;
                    partial = typed.saturating_sub(acc);
                    break;
                }
            }

            // Anchor the freshest line at the viewport's foot; early on, when
            // the log is short, `first` pins to 0 and the lines fill downward.
            let last_line = active.min(DIAG.len().saturating_sub(1));
            let first = (last_line + 1).saturating_sub(view);

            let ok_words = [
                "OK",
                "ONLINE",
                "NOMINAL",
                "LOCKED",
                "LINKED",
                "SEALED",
                "ARMED",
                "PRIMED",
                "STABLE",
                "SYNCED",
                "GREEN",
                "SECURE",
                "GRANTED",
                "CLEAR",
                "CONFIRMED",
                "LIVE",
                "STEADY",
                "TUNED",
                "CALIBRATED",
                "STREAMING",
                "VIABLE",
                "BALANCED",
                "LEARNED",
                "STOCKED",
                "HOLDING",
                "SIGNED",
                "%",
            ];

            let mut caret: Option<(u16, u16)> = None;
            for r in 0..view {
                let idx = first + r;
                if idx > last_line || idx >= DIAG.len() {
                    break;
                }
                let line = DIAG[idx];
                let y = area.top() + 1 + r as u16;
                let show = if idx < active {
                    line.chars().count()
                } else {
                    partial
                };
                let ok = ok_words.iter().any(|w| line.contains(w));
                let base = if ok { pal.accent } else { pal.primary };
                let col = anim::blend(pal.bg, base, alpha.max(0.15));
                if show > 0 {
                    let shown: String = line.chars().take(show).collect();
                    boot::put_str(buf, area, area.left() + 2, y, &shown, col, pal.bg);
                }
                if idx == active {
                    caret = Some((area.left() + 2 + show as u16, y));
                }
            }
            if let Some((cx, cy)) = caret {
                if (ems / 220).is_multiple_of(2) {
                    boot::put(buf, area, cx, cy, '_', pal.accent, pal.bg);
                }
            }
        }

        // Umbrella geometry — an ellipse (canopy top-view), 2:1 to correct for
        // cell aspect. Biased above centre to leave room for the handle below.
        let cx = area.left() as f32 + area.width as f32 / 2.0;
        let cy = area.top() as f32 + area.height as f32 * 0.46;
        let rx = ((area.width as f32 / 2.0) - 4.0).clamp(6.0, 13.0);
        let ry = (rx * 0.5).min(area.height as f32 * 0.26).max(3.0);
        let cxi = cx.round() as u16;

        // Phase 2 — the parasol fades in and spins.
        let up = ((p - 0.28) / 0.16).clamp(0.0, 1.0);
        if up > 0.0 {
            let rot = t * 2.4;

            // Scalloped canopy rim.
            let steps = 72;
            for s in 0..steps {
                let th = s as f32 / steps as f32 * TAU;
                let px = cx + th.cos() * rx;
                let py = cy + th.sin() * ry;
                let panel = ((th / (PI / 4.0)).floor() as i32).rem_euclid(2) == 0;
                let base = if panel { pal.primary } else { pal.secondary };
                let g = dir_glyph(th + PI / 2.0);
                boot::put(
                    buf,
                    area,
                    px.round() as u16,
                    py.round() as u16,
                    g,
                    anim::blend(pal.bg, base, up),
                    pal.bg,
                );
            }

            // Eight ribs sweeping from hub to rim; the leading rib burns bright.
            let ribs = 8;
            for k in 0..ribs {
                let th = rot + k as f32 / ribs as f32 * TAU;
                let g = dir_glyph(th);
                let base = if k == 0 {
                    pal.hyper
                } else if k % 2 == 0 {
                    pal.primary
                } else {
                    pal.secondary
                };
                let col = anim::blend(pal.bg, base, up);
                let mut r = 0.18;
                while r <= 1.0 {
                    let px = cx + th.cos() * rx * r;
                    let py = cy + th.sin() * ry * r;
                    boot::put(
                        buf,
                        area,
                        px.round() as u16,
                        py.round() as u16,
                        g,
                        col,
                        pal.bg,
                    );
                    r += 0.16;
                }
            }

            // Hazard hub and crooked handle.
            boot::put(buf, area, cxi, cy.round() as u16, '◉', pal.accent, pal.bg);
            let base_y = (cy + ry).round() as u16;
            let steel = anim::blend(pal.bg, pal.secondary, up);
            for h in 1..=3u16 {
                boot::put(buf, area, cxi, base_y + h, '|', steel, pal.bg);
            }
            boot::put(
                buf,
                area,
                cxi.saturating_sub(1),
                base_y + 4,
                '_',
                steel,
                pal.bg,
            );
            boot::put(buf, area, cxi, base_y + 4, '/', steel, pal.bg);
        }

        // Phase 3 — the wordmark rises above the canopy.
        if p > 0.44 {
            let tp = ((p - 0.44) / 0.20).clamp(0.0, 1.0);
            let title = "UMBRELLA CORP";
            let tn = title.chars().count() as u16;
            let ty = (cy - ry - 2.0).round() as u16;
            let tx = area.left() + area.width.saturating_sub(tn) / 2;
            boot::put_str(
                buf,
                area,
                tx,
                ty,
                title,
                anim::blend(pal.dim, pal.primary, tp),
                pal.bg,
            );
        }

        // Phase 4 — tagline locks in with a mechanical blink, then holds.
        if p > 0.66 {
            let tag = self.boot_tagline();
            let tn = tag.chars().count() as u16;
            let ty = ((cy + ry).round() as u16 + 6).min(area.bottom().saturating_sub(1));
            let tx = area.left() + area.width.saturating_sub(tn) / 2;
            let solid = p > 0.90 || (ems / 220).is_multiple_of(2);
            let col = if solid { pal.primary } else { pal.dim };
            boot::put_str(buf, area, tx, ty, tag, col, pal.bg);
        }
    }

    fn has_animation(&self) -> bool {
        true
    }

    /// A rotating 3D DNA double helix filling the right panel. Two backbones
    /// share one wrapping phase π apart; each row's x = centre + amp·sin(y·k+t)
    /// and its z = cos(...) drives depth — the front strand burns bright red,
    /// the back cools to steel and sinks toward the dark. Where the strands
    /// swing near one another a faint base-pair rung bridges them. `t` grows
    /// with `elapsed`, so the ladder visibly spins.
    fn render_animation(&self, area: Rect, elapsed: Duration, buf: &mut Buffer) {
        if area.width < 6 || area.height < 3 {
            return;
        }
        let pal = &self.palette;
        boot::fill_bg(area, buf, pal.bg);

        let center = area.left() as f32 + area.width as f32 / 2.0;
        let amp = (area.width as f32 / 2.0 - 2.0).max(2.0);
        let k = 0.5_f32; // radians of twist per row
        let t = elapsed.as_secs_f32() * 1.8; // spin rate
        let rung = anim::blend(pal.bg, pal.secondary, 0.30);
        let left = area.left() as i32;
        let right = area.right() as i32;

        for row in 0..area.height {
            let y = area.top() + row;
            let phase = row as f32 * k + t;
            let x1 = center + amp * phase.sin();
            let z1 = phase.cos();
            let x2 = center + amp * (phase + PI).sin();
            let z2 = (phase + PI).cos();

            // Base-pair rung where the two backbones swing near/across.
            let (lo, hi) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
            if hi - lo < amp * 1.25 {
                let a = lo.round() as i32 + 1;
                let b = hi.round() as i32;
                for xi in a..b {
                    if xi >= left && xi < right {
                        boot::put(buf, area, xi as u16, y, '-', rung, pal.bg);
                    }
                }
            }

            // Draw back strand first so the nearer node wins a shared cell.
            let mut strands = [(x1, z1), (x2, z2)];
            if strands[0].1 > strands[1].1 {
                strands.swap(0, 1);
            }
            for (x, z) in strands {
                let xi = x.round() as i32;
                if xi < left || xi >= right {
                    continue;
                }
                let (glyph, col) = helix_node(pal, z);
                boot::put(buf, area, xi as u16, y, glyph, col, pal.bg);
            }
        }
    }
}

/// Map a backbone node's depth `z` (−1 back .. +1 front) to a glyph and colour:
/// the front burns oxblood→bright, the mid holds oxblood, the back cools to
/// steel and dims toward the void.
fn helix_node(pal: &Palette, z: f32) -> (char, Color) {
    if z > 0.35 {
        let f = ((z - 0.35) / 0.65).clamp(0.0, 1.0);
        ('O', anim::blend(pal.primary, pal.hyper, f))
    } else if z > -0.35 {
        ('o', pal.primary)
    } else {
        let f = ((-z - 0.35) / 0.65).clamp(0.0, 1.0);
        ('.', anim::blend(pal.secondary, pal.bg, f * 0.6))
    }
}
