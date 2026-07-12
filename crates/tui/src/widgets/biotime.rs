//! Circadian bio-time gauge, rendered as ANALOG CLOCK dials on a braille
//! canvas. The model's 12-bin (2h) time-of-day distribution is reduced to its
//! peak bin and shown as a "BIO" clock whose hour hand points at the predicted
//! circadian phase; the prediction confidence is drawn both as a percentage
//! label and as a bright arc sweeping clockwise from 12 around the dial. When
//! the area affords it a second "NOW" dial shows the current local wall-clock
//! time with real minute precision, the two labels centred as a pair above.

use std::f64::consts::PI;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use t1dm_core::TOD_BINS;

use crate::theme::{Palette, Theme};

#[derive(Default)]
pub struct BioTimeGauge {
    tod: Vec<f64>,
    conf: f64,
    current_bin: usize,
    /// Real local wall-clock time for the NOW dial, if supplied.
    now_local: Option<(u8, u8)>,
}

impl BioTimeGauge {
    pub fn new() -> Self {
        BioTimeGauge::default()
    }

    pub fn tod(mut self, tod: Vec<f64>) -> Self {
        self.tod = tod;
        self
    }

    pub fn conf(mut self, conf: f64) -> Self {
        self.conf = conf.clamp(0.0, 1.0);
        self
    }

    /// The 2-hour bin (0..12) covering the current local time.
    pub fn current_bin(mut self, bin: usize) -> Self {
        self.current_bin = bin % TOD_BINS.max(1);
        self
    }

    /// The actual local wall-clock time, driving the NOW dial's hands with real
    /// minute precision (the BIO dial keeps its coarse 2h resolution).
    pub fn now_local(mut self, hour: u8, minute: u8) -> Self {
        self.now_local = Some((hour % 24, minute % 60));
        self
    }

    /// Argmax of the time-of-day distribution -> predicted 2h bin.
    fn peak_bin(&self) -> usize {
        let mut best = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for (i, &v) in self.tod.iter().enumerate() {
            if v > best_v {
                best_v = v;
                best = i;
            }
        }
        best % TOD_BINS.max(1)
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &dyn Theme) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        let pal = theme.palette();

        let bio_bin = self.peak_bin();
        // Centre of the 2h bin, in hours (1, 3, 5, ... 23).
        let bio_hour = (bio_bin * 2 + 1) % 24;
        // Prefer the real local time; fall back to the 2h bin's centre hour.
        let (now_hour, now_min) = match self.now_local {
            Some((h, m)) => (h as usize % 24, Some(m)),
            None => ((self.current_bin * 2 + 1) % 24, None),
        };
        let pct = (self.conf * 100.0).round() as i64;

        // A dial needs the header row plus at least one cell-row beneath it.
        if area.height < 2 {
            let head = truncate(
                &format!("BIO {bio_hour:02}:00  conf {pct}%"),
                area.width as usize,
            );
            buf.set_string(
                area.x,
                area.y,
                &head,
                Style::default().fg(pal.primary).bg(pal.bg),
            );
            return;
        }

        // Dial geometry. Braille dots are ~square in visual space, so a dial of
        // `h` cell-rows (4 dot-rows each) wants `2*h` cell-cols (2 dot-cols each)
        // to render circular. One header row is reserved at the top; the dials
        // then grow to consume whatever height remains, their width tracking at
        // `2*h`. A side-by-side pair must fit `2*(2h)+gap` within the width; when
        // the column is too narrow for that we fall back to a single larger dial.
        let gap: u16 = 3;
        let avail_h = (area.height - 1).max(1);
        // Tallest dial whose paired width (2 dials + gap) still fits the column.
        let two_h = (area.width.saturating_sub(gap) / 4).min(avail_h);
        // Tallest dial whose single width fits the column.
        let one_h = (area.width / 2).min(avail_h);
        let two = two_h >= 2;
        let h_cells = if two { two_h } else { one_h }.max(1);
        let w_cells = (2 * h_cells).min(area.width);

        let dial_y = area.y + 1;

        let bio_colors = DialColors {
            rim: pal.dim,
            arc: pal.accent,
            tick: pal.secondary,
            hour: pal.primary,
            minute: pal.fg,
            hub: pal.primary,
        };
        let now_colors = DialColors {
            rim: pal.grid,
            arc: pal.secondary,
            tick: pal.dim,
            hour: pal.fg,
            minute: pal.dim,
            hub: pal.secondary,
        };

        let now_hm = now_min.unwrap_or(0);

        if two {
            let group_w = 2 * w_cells + gap;
            let x0 = area.x + (area.width - group_w) / 2;
            let bio_x = x0;
            let now_x = x0 + w_cells + gap;

            // The two labels form one centred group over the pair of dials,
            // rather than each label perching over its own dial.
            let bio_label = format!("BIO {bio_hour:02}:00  conf {pct}%");
            let now_label = format!("NOW {now_hour:02}:{now_hm:02}");
            self.draw_header(buf, area, pal, &bio_label, &now_label);

            self.draw_dial(
                buf,
                Rect::new(bio_x, dial_y, w_cells, h_cells),
                (bio_hour, None),
                self.conf,
                &bio_colors,
                pal,
            );
            self.draw_dial(
                buf,
                Rect::new(now_x, dial_y, w_cells, h_cells),
                (now_hour, Some(now_hm)),
                1.0,
                &now_colors,
                pal,
            );
        } else {
            let x0 = area.x + (area.width.saturating_sub(w_cells)) / 2;
            let bio_label = format!("BIO {bio_hour:02}:00  conf {pct}%");
            let now_label = format!("NOW {now_hour:02}:{now_hm:02}");
            self.draw_header(buf, area, pal, &bio_label, &now_label);
            self.draw_dial(
                buf,
                Rect::new(x0, dial_y, w_cells, h_cells),
                (bio_hour, None),
                self.conf,
                &bio_colors,
                pal,
            );
        }
    }

    /// Paint the "BIO …" and "NOW …" labels as one horizontally centred group
    /// on the header row, BIO in the primary colour and NOW in the secondary,
    /// separated by two spaces. Falls back to a truncated single label when the
    /// combined group cannot fit the width.
    fn draw_header(&self, buf: &mut Buffer, area: Rect, pal: &Palette, bio: &str, now: &str) {
        let sep = "  ";
        let bio_w = bio.chars().count();
        let now_w = now.chars().count();
        let group = bio_w + sep.len() + now_w;
        if group <= area.width as usize {
            let gx = area.x + (area.width - group as u16) / 2;
            buf.set_string(gx, area.y, bio, Style::default().fg(pal.primary).bg(pal.bg));
            buf.set_string(
                gx + (bio_w + sep.len()) as u16,
                area.y,
                now,
                Style::default().fg(pal.secondary).bg(pal.bg),
            );
        } else {
            let head = truncate(bio, area.width as usize);
            let hx = area.x + (area.width.saturating_sub(head.chars().count() as u16)) / 2;
            buf.set_string(
                hx,
                area.y,
                &head,
                Style::default().fg(pal.primary).bg(pal.bg),
            );
        }
    }

    /// Raster one analog dial into `rect`: dim rim + a bright confidence arc
    /// (clockwise from 12, length ∝ `conf`), 12 hour ticks, an hour hand at
    /// `hour` (advanced fractionally when a minute is given), a minute hand at
    /// `minute` (pinned to 12 when `None`), and a centre hub.
    fn draw_dial(
        &self,
        buf: &mut Buffer,
        rect: Rect,
        hm: (usize, Option<u8>),
        conf: f64,
        c: &DialColors,
        pal: &Palette,
    ) {
        let (hour, minute) = hm;
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let mut canvas = Braille::new(rect);
        let w = canvas.dot_w as f64;
        let h = canvas.dot_h as f64;
        let cx = (w - 1.0) / 2.0;
        let cy = (h - 1.0) / 2.0;
        let radius = (w.min(h)) / 2.0 - 1.0;
        if radius < 1.0 {
            return;
        }

        // Angle `a` measured clockwise from 12 o'clock (top).
        let pos = |a: f64, r: f64| -> (f64, f64) { (cx + r * a.sin(), cy - r * a.cos()) };

        // Rim + confidence arc. One fine sweep; cells inside the confidence
        // sector take the bright colour, the rest the dim rim colour.
        let arc_end = conf.clamp(0.0, 1.0) * 2.0 * PI;
        let steps = ((2.0 * PI * radius * 2.0) as usize).max(24);
        for i in 0..=steps {
            let a = 2.0 * PI * i as f64 / steps as f64;
            let (x, y) = pos(a, radius);
            if a <= arc_end {
                canvas.plot(x, y, c.arc, 2);
            } else {
                canvas.plot(x, y, c.rim, 1);
            }
        }

        // Twelve hour ticks, biting inward from the rim.
        for k in 0..12 {
            let a = k as f64 / 12.0 * 2.0 * PI;
            for f in [radius - 1.4, radius - 0.7] {
                if f > 0.0 {
                    let (x, y) = pos(a, f);
                    canvas.plot(x, y, c.tick, 3);
                }
            }
        }

        // Hands. When a minute is supplied the hour hand advances fractionally
        // and the minute hand points at the real minute; otherwise the minute
        // hand is pinned to 12 (a 2h bin has no sub-hour phase to show).
        let frac = minute.map(|m| m as f64 / 60.0).unwrap_or(0.0);
        let hour_a = (((hour % 12) as f64 + frac) / 12.0) * 2.0 * PI;
        let minute_a = frac * 2.0 * PI;
        canvas.line(minute_a, 0.8 * radius, &pos, c.minute, 4);
        canvas.line(hour_a, 0.52 * radius, &pos, c.hour, 5);

        // Centre hub.
        canvas.plot(cx, cy, c.hub, 6);

        canvas.blit(buf, pal.bg);
    }
}

struct DialColors {
    rim: Color,
    arc: Color,
    tick: Color,
    hour: Color,
    minute: Color,
    hub: Color,
}

/// A braille dot canvas over a cell `Rect`. Each cell packs a 2x4 dot matrix
/// into one U+28xx glyph; per cell we keep the accumulated dot bits plus the
/// colour of the highest-priority stroke that touched it.
struct Braille {
    area: Rect,
    w_cells: usize,
    h_cells: usize,
    dot_w: usize,
    dot_h: usize,
    bits: Vec<u8>,
    color: Vec<Color>,
    prio: Vec<u8>,
    used: Vec<bool>,
}

/// Braille dot bit for cell-local (dx in 0..2, dy in 0..4).
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

impl Braille {
    fn new(area: Rect) -> Self {
        let w_cells = area.width as usize;
        let h_cells = area.height as usize;
        let n = w_cells * h_cells;
        Braille {
            area,
            w_cells,
            h_cells,
            dot_w: w_cells * 2,
            dot_h: h_cells * 4,
            bits: vec![0u8; n],
            color: vec![Color::Reset; n],
            prio: vec![0u8; n],
            used: vec![false; n],
        }
    }

    /// Set the dot nearest `(x, y)` (dot coordinates), colouring the owning cell
    /// with `color` when `prio` beats what's already there.
    fn plot(&mut self, x: f64, y: f64, color: Color, prio: u8) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let xi = x.round();
        let yi = y.round();
        if xi < 0.0 || yi < 0.0 {
            return;
        }
        let xi = xi as usize;
        let yi = yi as usize;
        if xi >= self.dot_w || yi >= self.dot_h {
            return;
        }
        let cx = xi / 2;
        let cy = yi / 4;
        let idx = cy * self.w_cells + cx;
        if idx >= self.bits.len() {
            return;
        }
        self.bits[idx] |= DOT[xi % 2][yi % 4];
        if !self.used[idx] || prio >= self.prio[idx] {
            self.color[idx] = color;
            self.prio[idx] = prio;
        }
        self.used[idx] = true;
    }

    /// Raster a straight hand from `(cx, cy)` outward at angle `a` to length
    /// `len`, using `pos` to convert (angle, radius) into dot coordinates.
    fn line<F: Fn(f64, f64) -> (f64, f64)>(
        &mut self,
        a: f64,
        len: f64,
        pos: &F,
        color: Color,
        prio: u8,
    ) {
        let n = (len * 2.0).ceil() as usize + 1;
        for i in 0..=n {
            let r = len * i as f64 / n as f64;
            let (x, y) = pos(a, r);
            self.plot(x, y, color, prio);
        }
    }

    fn blit(&self, buf: &mut Buffer, bg: Color) {
        for cy in 0..self.h_cells {
            for cx in 0..self.w_cells {
                let idx = cy * self.w_cells + cx;
                if !self.used[idx] || self.bits[idx] == 0 {
                    continue;
                }
                let ch = char::from_u32(0x2800 + self.bits[idx] as u32).unwrap_or('?');
                let px = self.area.x + cx as u16;
                let py = self.area.y + cy as u16;
                if px >= self.area.right() || py >= self.area.bottom() {
                    continue;
                }
                if let Some(cell) = buf.cell_mut((px, py)) {
                    cell.set_char(ch);
                    cell.fg = self.color[idx];
                    cell.bg = bg;
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}
