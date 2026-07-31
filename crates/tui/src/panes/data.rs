//! Data pane: the wide sample table on the 5-minute grid, newest-first,
//! scrollable. Columns adapt to the available width; BG cells are tinted by
//! range. Values are shown in the active display unit for BG.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use t1dm_core::{SampleRow, Series};

use crate::widgets::ScrollState;

use super::help::{scroll_key, scroll_mouse};
use super::fmt_dt;
use super::{Action, Ctx, PaneView};

const REFRESH: Duration = Duration::from_secs(2);
/// How far back the table loads (30 days ≈ 8640 grid rows).
const WINDOW_MS: i64 = 30 * 86_400_000;
/// Width below which the table drops to a condensed column set.
const WIDE_MIN: u16 = 74;

#[derive(Default)]
pub struct DataPane {
    scroll: ScrollState,
    rows: Vec<SampleRow>,
    /// Per-bucket reconstructed carb-appearance / bolus / basal, keyed by grid
    /// ts. Carbs/bolus/basal are no longer sample columns; the store folds them
    /// from the stored meal/dose curve events (display-only; the §4-#6 basal
    /// XOR is applied inside `reconstruct_channels`).
    carb: HashMap<i64, f64>,
    bolus: HashMap<i64, f64>,
    basal: HashMap<i64, f64>,
    last_fetch: Option<Instant>,
    err: Option<String>,
}

impl DataPane {
    fn refresh(&mut self, ctx: &Ctx) {
        let due = self
            .last_fetch
            .map(|t| t.elapsed() >= REFRESH)
            .unwrap_or(true);
        if !due {
            return;
        }
        self.last_fetch = Some(Instant::now());
        let from = ctx.now_ms - WINDOW_MS;
        match ctx.store.get_samples(
            &Series::ALL,
            Some(from),
            Some(ctx.now_ms),
            Some(20_000),
            None,
        ) {
            Ok(mut rows) => {
                rows.reverse(); // newest-first
                self.rows = rows;
                self.err = None;
            }
            Err(e) => self.err = Some(e.to_string()),
        }

        // Carbs/bolus/basal are no longer sample columns — the store folds them
        // back from the stored meal/dose curve events onto the same 5-minute
        // grid the rows sit on (display-only; not part of any dosing guarantee).
        // Key each channel's (ts, value) points by grid ts for per-row lookup; a
        // reconstruction error degrades to empty channels (dim dots) rather than
        // blanking the sample table.
        let channels = ctx
            .store
            .reconstruct_channels(from, ctx.now_ms)
            .unwrap_or_default();
        self.carb = channels.carb.into_iter().collect();
        self.bolus = channels.bolus.into_iter().collect();
        self.basal = channels.basal.into_iter().collect();
    }
}

impl PaneView for DataPane {
    fn title(&self) -> &str {
        "Data"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.refresh(ctx);
        let pal = ctx.theme.palette();

        let block = Block::default()
            .title(Span::styled(
                format!(" Data · {} rows · {} ", self.rows.len(), ctx.unit.label()),
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if let Some(err) = &self.err {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!("store error: {err}"),
                    Style::default().fg(pal.hyper),
                ))),
                inner,
            );
            return;
        }
        if inner.height < 2 {
            return;
        }
        if self.rows.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "no samples — generate synthetic data in the Developer pane",
                    Style::default().fg(pal.dim),
                ))),
                inner,
            );
            return;
        }

        let wide = inner.width >= WIDE_MIN;
        let header = if wide {
            format!(
                "{:<16}{:>7}{:>7}{:>7}{:>7}{:>6}{:>7}{:>5}{:>5}{:>5}",
                "time", "BG", "carbs", "bolus", "basal", "HR", "steps", "slp", "exc", "mood"
            )
        } else {
            format!(
                "{:<16}{:>7}{:>7}{:>7}{:>6}",
                "time", "BG", "carbs", "ins", "HR"
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                header,
                Style::default().fg(pal.dim).add_modifier(Modifier::BOLD),
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );

        let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        self.scroll
            .set_bounds(self.rows.len(), body.height as usize);

        let end = (self.scroll.offset + body.height as usize).min(self.rows.len());
        let mut lines: Vec<Line> = Vec::with_capacity(body.height as usize);
        for row in &self.rows[self.scroll.offset..end] {
            let time = fmt_dt(row.ts, row.tz_offset);
            let bg_txt = row
                .bg
                .map(|v| ctx.unit.format(v))
                .unwrap_or_else(|| "·".to_string());
            let bg_style = match row.bg {
                Some(v) if v < 70.0 => Style::default().fg(pal.hypo),
                Some(v) if v > 180.0 => Style::default().fg(pal.hyper),
                Some(_) => Style::default().fg(pal.primary),
                None => Style::default().fg(pal.dim),
            };
            let mut spans = vec![
                Span::styled(format!("{time:<16}"), Style::default().fg(pal.fg)),
                Span::styled(format!("{bg_txt:>7}"), bg_style),
            ];
            let carb = chan_at(&self.carb, row.ts);
            if wide {
                spans.push(cell(carb, 7, 0, pal.fg, pal.dim));
                spans.push(cell(chan_at(&self.bolus, row.ts), 7, 1, pal.secondary, pal.dim));
                spans.push(cell(chan_at(&self.basal, row.ts), 7, 2, pal.secondary, pal.dim));
                spans.push(cell(row.hr, 6, 0, pal.accent, pal.dim));
                spans.push(cell(row.steps, 7, 0, pal.fg, pal.dim));
                spans.push(cell(row.sleep, 5, 0, pal.fg, pal.dim));
                spans.push(cell(row.exercise, 5, 0, pal.fg, pal.dim));
                spans.push(cell(row.mood.map(|m| m as f64), 5, 0, pal.fg, pal.dim));
            } else {
                spans.push(cell(carb, 7, 0, pal.fg, pal.dim));
                spans.push(cell(
                    total_insulin_at(&self.bolus, &self.basal, row.ts),
                    7,
                    1,
                    pal.secondary,
                    pal.dim,
                ));
                spans.push(cell(row.hr, 6, 0, pal.accent, pal.dim));
            }
            lines.push(Line::from(spans));
        }

        frame.render_widget(Paragraph::new(lines), body);
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Action {
        if scroll_key(&mut self.scroll, key) {
            Action::Redraw
        } else {
            Action::None
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        if scroll_mouse(&mut self.scroll, mouse) {
            Action::Redraw
        } else {
            Action::None
        }
    }

    fn invalidate(&mut self) {
        self.last_fetch = None;
    }
}

/// A right-aligned numeric cell, dimmed to a middle dot when absent.
fn cell(
    v: Option<f64>,
    w: usize,
    dec: usize,
    present: ratatui::style::Color,
    absent: ratatui::style::Color,
) -> Span<'static> {
    match v {
        Some(x) => Span::styled(format!("{x:>w$.dec$}"), Style::default().fg(present)),
        None => Span::styled(format!("{:>w$}", "·"), Style::default().fg(absent)),
    }
}

/// The reconstructed channel value for the grid bucket at `ts`, or `None` when
/// the bucket carries no appearance/action (absent, or a non-positive value —
/// rendered as a dot, as the dashboard strips also skip `v <= 0`).
fn chan_at(ch: &HashMap<i64, f64>, ts: i64) -> Option<f64> {
    ch.get(&ts).copied().filter(|&v| v > 0.0)
}

/// Combined bolus+basal action for the condensed `ins` column; `None` when
/// neither channel contributes in this bucket.
fn total_insulin_at(bolus: &HashMap<i64, f64>, basal: &HashMap<i64, f64>, ts: i64) -> Option<f64> {
    let sum = chan_at(bolus, ts).unwrap_or(0.0) + chan_at(basal, ts).unwrap_or(0.0);
    (sum > 0.0).then_some(sum)
}
