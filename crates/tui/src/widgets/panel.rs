//! Side-panel statistics tiles. In Full-width layout the two 28-column gutters
//! flank the main pane; this widget fills them with auxiliary glycemic
//! statistics — one titled, bordered box per panel, each holding one or more
//! labelled windows rendered as a stack of value tiles with a time-in-range
//! bar. All figures come straight from [`t1dm_core::Stats`]; nothing here
//! computes, so the caller is free to feed it a cached, interval-refreshed
//! block rather than recomputing per frame.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use t1dm_core::{BgUnit, Stats};

use crate::boot::{put, put_str};
use crate::theme::Palette;

/// One labelled statistics window to stack inside a panel.
pub struct Section<'a> {
    /// Short window caption, e.g. `"24 HOURS"`.
    pub label: &'a str,
    pub stats: &'a Stats,
}

/// A single laid-out content row. The stacked sections are flattened into one
/// tall column of these, then a scroll-offset window of it is painted. Keeping
/// the model explicit lets both the renderer and [`content_height`] agree on
/// exactly how many rows a set of sections occupies.
enum Line<'a> {
    /// Vertical breathing room between windows.
    Blank,
    /// Window caption plus a dim, right-aligned sample count.
    Caption { label: &'a str, count: String },
    /// Horizontal rule under a caption.
    Rule,
    /// Time-in-range spectrum bar for a window.
    TirBar(&'a Stats),
    /// A label/value statistic tile.
    Tile {
        label: &'a str,
        value: String,
        col: Color,
    },
    /// A dim free-text note (e.g. the empty-window placeholder).
    Text(&'a str),
}

/// Number of content rows a section stack occupies once flattened. Mirrors
/// [`build_lines`] exactly so callers can clamp a scroll offset without a
/// palette or unit in hand.
pub fn content_height(sections: &[Section]) -> u16 {
    let mut h = 0usize;
    for (i, sec) in sections.iter().enumerate() {
        if i > 0 {
            h += 1; // Blank
        }
        h += 2; // Caption + Rule
        h += if sec.stats.n_samples == 0 {
            1
        } else {
            window_rows(sec.stats)
        };
    }
    h as u16
}

/// Content rows one populated window occupies: the spectrum bar, its range
/// caveat, the eight always-present tiles, and whichever of the four
/// treatment/heart-rate tiles the phone actually populated.
fn window_rows(s: &Stats) -> usize {
    10 + treatment_tiles(s).len()
}

/// The treatment/heart-rate tiles that carry a real measurement. The phone
/// cannot compute any of them from what it pushes — carbs, bolus and basal were
/// demoted off the sample row to curve events, and `mean_hr`/`bg_hr_corr` have
/// no home in its stats block yet, so all four arrive as a hard 0.0. A zero is
/// therefore indistinguishable from "none logged", and rendering it as a
/// measured quantity would assert a week of 1400 g and 300 U was a week of
/// nothing. The phone's own stats screen suppresses exactly these rows.
fn treatment_tiles(s: &Stats) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if s.mean_daily_carbs > 0.0 {
        out.push(("Carbs/d", format!("{:.0} g", s.mean_daily_carbs)));
    }
    if s.tdd > 0.0 {
        out.push(("TDD", format!("{:.1} U", s.tdd)));
    }
    if s.bolus_basal_ratio > 0.0 {
        out.push(("Bolus:Basal", format!("{:.2}", s.bolus_basal_ratio)));
    }
    if s.mean_hr > 0.0 {
        out.push(("HR", format!("{:.0}", s.mean_hr)));
    }
    out
}

/// Flatten the section stack into the ordered column of [`Line`]s.
fn build_lines<'a>(sections: &'a [Section], pal: &Palette, unit: BgUnit) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::Blank);
        }
        lines.push(Line::Caption {
            label: sec.label,
            count: human_count(sec.stats.n_samples),
        });
        lines.push(Line::Rule);

        if sec.stats.n_samples == 0 {
            lines.push(Line::Text("no samples in window"));
            continue;
        }

        lines.push(Line::TirBar(sec.stats));
        // The three band fractions are the phone's, computed against the user's
        // own configurable target range — which nothing on the wire carries.
        // They must not read as the chart's fixed 70/180 rails on the same
        // screen, so the labels and this line say whose range they are.
        lines.push(Line::Text("(tgt) = phone target range"));
        let s = sec.stats;
        let mut rows: Vec<(&str, String, Color)> = vec![
            ("TIR (tgt)", pct(s.tir), pal.primary),
            ("Below (tgt)", pct(s.time_below), pal.hypo),
            ("Above (tgt)", pct(s.time_above), pal.hyper),
            ("Mean BG", unit.format(s.mean_bg), pal.fg),
            ("GMI", format!("{:.1}%", s.gmi), pal.fg),
            ("CV", format!("{:.1}%", s.cv), pal.fg),
            ("SD", unit.format_spread(s.sd), pal.fg),
            (
                "Hypo/Hyper",
                format!("{} / {}", s.hypo_events.count, s.hyper_events.count),
                pal.fg,
            ),
        ];
        rows.extend(
            treatment_tiles(s)
                .into_iter()
                .map(|(label, value)| (label, value, pal.fg)),
        );
        for (label, value, col) in rows {
            lines.push(Line::Tile { label, value, col });
        }
    }
    lines
}

/// Paint one flattened [`Line`] at row `y` inside `inner`.
fn draw_line(
    buf: &mut ratatui::buffer::Buffer,
    inner: Rect,
    y: u16,
    line: &Line,
    pal: &Palette,
    bg: Color,
) {
    match line {
        Line::Blank => {}
        Line::Caption { label, count } => {
            put_str(buf, inner, inner.left(), y, label, pal.accent, bg);
            let nx = inner.right().saturating_sub(count.chars().count() as u16);
            put_str(buf, inner, nx, y, count, pal.dim, bg);
        }
        Line::Rule => rule(buf, inner, y, pal.grid, bg),
        Line::TirBar(s) => tir_bar(buf, inner, y, s, pal, bg),
        Line::Tile { label, value, col } => tile(buf, inner, y, label, value, *col, pal),
        Line::Text(t) => put_str(buf, inner, inner.left(), y, t, pal.dim, bg),
    }
}

/// Render a bordered, titled panel of stat tiles into `area`, scrolled down by
/// `scroll` rows. The sections are flattened into one tall column; the visible
/// slice is painted and `▲`/`▼` affordances appear on the right border whenever
/// content is clipped off the top or bottom. `scroll` is clamped to the last
/// full page, so an over-large offset simply pins to the end.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    pal: &Palette,
    unit: BgUnit,
    title: &str,
    sections: &[Section],
    scroll: u16,
) {
    if area.width < 6 || area.height < 3 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(pal.grid).bg(pal.surface))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(pal.secondary)
                .bg(pal.surface)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(pal.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let bg = pal.surface;
    let lines = build_lines(sections, pal, unit);
    let visible = inner.height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    let scroll = (scroll as usize).min(max_scroll);

    let buf = frame.buffer_mut();
    for (row, line) in lines.iter().enumerate().skip(scroll).take(visible) {
        let y = inner.top() + (row - scroll) as u16;
        draw_line(buf, inner, y, line, pal, bg);
    }

    // Scroll affordances on the right border when content is clipped.
    let arrow_x = area.right().saturating_sub(1);
    if scroll > 0 {
        put(
            buf,
            area,
            arrow_x,
            area.top() + 1,
            '▲',
            pal.accent,
            pal.surface,
        );
    }
    if scroll < max_scroll {
        put(
            buf,
            area,
            arrow_x,
            area.bottom().saturating_sub(2),
            '▼',
            pal.accent,
            pal.surface,
        );
    }
}

/// A label/value tile: label flush-left dim, value flush-right in `vcol`.
fn tile(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    label: &str,
    value: &str,
    vcol: Color,
    pal: &Palette,
) {
    put_str(buf, area, area.left(), y, label, pal.dim, pal.surface);
    let vw = value.chars().count() as u16;
    let vx = area.right().saturating_sub(vw);
    // Never let the value overwrite the label on a very tight panel.
    let lw = label.chars().count() as u16;
    if vx > area.left() + lw {
        put_str(buf, area, vx, y, value, vcol, pal.surface);
    }
}

/// A horizontal rule spanning the inner width.
fn rule(buf: &mut ratatui::buffer::Buffer, area: Rect, y: u16, col: Color, bg: Color) {
    for x in area.left()..area.right() {
        put(buf, area, x, y, '─', col, bg);
    }
}

/// The time-in-range spectrum: a solid bar split below | in-range | above,
/// coloured from the hypo / primary / hyper palette slots.
fn tir_bar(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    y: u16,
    s: &Stats,
    pal: &Palette,
    bg: Color,
) {
    let w = area.width;
    if w == 0 {
        return;
    }
    let t1 = s.time_below;
    let t2 = s.time_below + s.tir;
    for i in 0..w {
        let frac = (i as f64 + 0.5) / w as f64;
        let col = if frac < t1 {
            pal.hypo
        } else if frac < t2 {
            pal.primary
        } else {
            pal.hyper
        };
        put(buf, area, area.left() + i, y, '█', col, bg);
    }
}

/// `82.3%` for a 0..=1 fraction.
fn pct(frac: f64) -> String {
    format!("{:.1}%", frac * 100.0)
}

/// Compact sample count: `288`, `1.9k`, `12k`.
fn human_count(n: u32) -> String {
    if n < 1000 {
        format!("{n}")
    } else if n < 100_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        format!("{}k", n / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use t1dm_core::StatsWindow;

    use crate::theme::{theme_for, ThemeKind};

    fn tiles<'a>(sections: &'a [Section], pal: &Palette) -> Vec<(&'a str, String)> {
        build_lines(sections, pal, BgUnit::Mgdl)
            .into_iter()
            .filter_map(|l| match l {
                Line::Tile { label, value, .. } => Some((label, value)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tiles_the_phone_cannot_populate_are_suppressed() {
        let theme = theme_for(ThemeKind::Tron);
        let pal = theme.palette();

        // What the phone actually pushes: carbs/bolus/basal are demoted off the
        // sample row and `mean_hr` is hard-coded 0.0, so all four arrive zero
        // however much was logged. None of them may render as a measurement.
        let mut s = Stats::empty(StatsWindow::D7);
        s.n_samples = 2016;
        s.tir = 0.714;
        let sections = [Section {
            label: "7 DAYS",
            stats: &s,
        }];
        let labels: Vec<&str> = tiles(&sections, pal).iter().map(|t| t.0).collect();
        assert!(!labels.contains(&"Carbs/d"));
        assert!(!labels.contains(&"TDD"));
        assert!(!labels.contains(&"Bolus:Basal"));
        assert!(!labels.contains(&"HR"));
        assert_eq!(labels.len(), 8);

        // A block that does carry them renders them verbatim.
        let mut full = s;
        full.mean_daily_carbs = 200.0;
        full.tdd = 42.5;
        full.bolus_basal_ratio = 1.25;
        full.mean_hr = 68.4;
        let sections = [Section {
            label: "7 DAYS",
            stats: &full,
        }];
        let rows = tiles(&sections, pal);
        assert_eq!(rows.len(), 12);
        assert_eq!(rows[8], ("Carbs/d", "200 g".to_string()));
        assert_eq!(rows[9], ("TDD", "42.5 U".to_string()));
        assert_eq!(rows[10], ("Bolus:Basal", "1.25".to_string()));
        assert_eq!(rows[11], ("HR", "68".to_string()));
    }

    #[test]
    fn the_range_tiles_name_whose_range_they_are() {
        let theme = theme_for(ThemeKind::Tron);
        let pal = theme.palette();
        let mut s = Stats::empty(StatsWindow::D7);
        s.n_samples = 288;
        s.tir = 0.5;
        let sections = [Section {
            label: "7 DAYS",
            stats: &s,
        }];
        let labels: Vec<&str> = tiles(&sections, pal).iter().map(|t| t.0).collect();
        // The chart's rails are a fixed 70/180; these fractions are not.
        assert_eq!(&labels[..3], &["TIR (tgt)", "Below (tgt)", "Above (tgt)"]);
        assert!(build_lines(&sections, pal, BgUnit::Mgdl)
            .iter()
            .any(|l| matches!(l, Line::Text(t) if t.contains("target range"))));
    }

    #[test]
    fn content_height_tracks_the_rows_actually_built() {
        let theme = theme_for(ThemeKind::Tron);
        let pal = theme.palette();
        let empty = Stats::empty(StatsWindow::D7);
        let mut lean = Stats::empty(StatsWindow::D30);
        lean.n_samples = 2016;
        let mut full = lean;
        full.mean_daily_carbs = 200.0;
        full.tdd = 42.5;
        full.bolus_basal_ratio = 1.25;
        full.mean_hr = 68.4;

        let sections = [
            Section {
                label: "7 DAYS",
                stats: &empty,
            },
            Section {
                label: "30 DAYS",
                stats: &lean,
            },
            Section {
                label: "90 DAYS",
                stats: &full,
            },
        ];
        assert_eq!(
            content_height(&sections) as usize,
            build_lines(&sections, pal, BgUnit::Mgdl).len()
        );
    }
}
