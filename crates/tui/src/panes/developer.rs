//! Developer pane: seed the store with a realistic synthetic dataset over a
//! chosen range (so the dashboard renders before any client exists), and the
//! destructive teardown behind a type-to-confirm `WIPE` modal.

use std::thread::JoinHandle;
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use store::{FakeOpts, FakeRange};

use super::{Action, Ctx, PaneView};

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Selectable synthetic ranges, in days.
const RANGES: [(&str, i64); 4] = [
    ("Last 24 hours", 1),
    ("Last 7 days", 7),
    ("Last 30 days", 30),
    ("Last 90 days", 90),
];

enum Mode {
    Menu,
    /// Teardown confirmation; holds the text typed so far.
    Confirm(String),
}

pub struct DeveloperPane {
    sel: usize,
    with_predictions: bool,
    mode: Mode,
    status: Option<String>,
    /// Background generation job (kept off the render thread so the UI stays
    /// responsive); reads use a separate pool from the writer.
    job: Option<JoinHandle<std::result::Result<usize, String>>>,
    job_label: String,
    phase: f32,
}

impl Default for DeveloperPane {
    fn default() -> Self {
        DeveloperPane {
            sel: 0,
            with_predictions: true,
            mode: Mode::Menu,
            status: None,
            job: None,
            job_label: String::new(),
            phase: 0.0,
        }
    }
}

impl DeveloperPane {
    fn generate(&mut self, ctx: &Ctx) -> Action {
        if self.job.is_some() {
            self.status = Some("a generation is already running".into());
            return Action::Redraw;
        }
        let (label, days) = RANGES[self.sel];
        let range = FakeRange::last_days(days);
        let opts = FakeOpts {
            // A fresh seed per run so repeated generations differ.
            seed: ctx.now_ms as u64 ^ 0x9E37_79B9_7F4A_7C15,
            with_predictions: self.with_predictions,
            ..FakeOpts::default()
        };
        let store = ctx.store.clone();
        self.job_label = label.to_string();
        self.status = None;
        self.job = Some(std::thread::spawn(move || {
            store.generate_fake(range, opts).map_err(|e| e.to_string())
        }));
        Action::Redraw
    }

    /// Reap a finished background generation, if any.
    fn poll_job(&mut self) {
        if self.job.as_ref().map(|h| h.is_finished()).unwrap_or(false) {
            if let Some(handle) = self.job.take() {
                let label = std::mem::take(&mut self.job_label);
                self.status = Some(match handle.join() {
                    Ok(Ok(n)) => format!("generated {n} grid rows over “{label}”"),
                    Ok(Err(e)) => format!("generation failed: {e}"),
                    Err(_) => "generation thread panicked".to_string(),
                });
            }
        }
    }

    fn render_menu(&self, frame: &mut Frame, inner: Rect, ctx: &Ctx) {
        let pal = ctx.theme.palette();
        let mut lines: Vec<Line> = vec![
            Line::from(Span::styled(
                "Synthetic dataset",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "diurnal BG + meal excursions + matching carbs/insulin, a widening",
                Style::default().fg(pal.dim),
            )),
            Line::from(Span::styled(
                "prediction fan — written straight into the store.",
                Style::default().fg(pal.dim),
            )),
            Line::from(""),
        ];

        for (i, (label, days)) in RANGES.iter().enumerate() {
            let selected = i == self.sel;
            let marker = if selected { "▶ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(pal.bg)
                    .bg(pal.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(pal.fg)
            };
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(pal.accent)),
                Span::styled(format!("{label:<16}"), style),
                Span::styled(
                    format!("  (~{} rows)", days * 288),
                    Style::default().fg(pal.dim),
                ),
            ]));
        }
        lines.push(Line::from(""));

        let flag = |on: bool| if on { "on" } else { "off" };
        lines.push(Line::from(vec![
            Span::styled("  predictions ", Style::default().fg(pal.dim)),
            Span::styled(
                format!("[{}]", flag(self.with_predictions)),
                Style::default().fg(pal.secondary),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "j/k select · Enter generate · p predictions · X teardown",
            Style::default().fg(pal.dim),
        )));
        lines.push(Line::from(""));
        if self.job.is_some() {
            let spin = SPINNER[(self.phase as usize) % SPINNER.len()];
            lines.push(Line::from(Span::styled(
                format!("{spin} generating “{}” …", self.job_label),
                Style::default().fg(pal.accent).add_modifier(Modifier::BOLD),
            )));
        } else if let Some(status) = &self.status {
            lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(pal.secondary),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, ctx: &Ctx, typed: &str) {
        let pal = ctx.theme.palette();
        let w = 52.min(area.width.saturating_sub(2)).max(20);
        let h = 9.min(area.height.saturating_sub(2)).max(6);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let modal = Rect::new(x, y, w, h);

        frame.render_widget(Clear, modal);
        let block = Block::default()
            .title(Span::styled(
                " TEARDOWN ",
                Style::default()
                    .fg(pal.bg)
                    .bg(pal.hyper)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.hyper))
            .style(Style::default().bg(pal.surface));
        let inner = block.inner(modal);
        frame.render_widget(block, modal);

        let ok = typed == "WIPE";
        let entry_style = if ok {
            Style::default()
                .fg(pal.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(pal.hyper)
        };
        let lines = vec![
            Line::from(Span::styled(
                "Drop & recreate all tables and clear photos/.",
                Style::default().fg(pal.fg),
            )),
            Line::from(Span::styled(
                "Models are left untouched. This cannot be undone.",
                Style::default().fg(pal.dim),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("type WIPE: ", Style::default().fg(pal.fg)),
                Span::styled(format!("{typed}▏"), entry_style),
            ]),
            Line::from(Span::styled(
                if ok {
                    "Enter to confirm · Esc to cancel"
                } else {
                    "Esc to cancel"
                },
                Style::default().fg(pal.dim),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

impl PaneView for DeveloperPane {
    fn title(&self) -> &str {
        "Developer"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.poll_job();
        let pal = ctx.theme.palette();
        let block = Block::default()
            .title(Span::styled(
                " Developer ",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.render_menu(frame, inner, ctx);
        if let Mode::Confirm(typed) = &self.mode {
            self.render_confirm(frame, area, ctx, typed);
        }
    }

    fn tick(&mut self, dt: Duration, ctx: &mut Ctx) {
        // While a background generation runs, keep the spinner animating and
        // the loop awake so completion is noticed promptly.
        if self.job.is_some() {
            self.phase += dt.as_secs_f32() * 12.0;
            ctx.request_redraw();
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        // Modal captures all keys while open.
        if let Mode::Confirm(typed) = &mut self.mode {
            match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Menu;
                    self.status = Some("teardown cancelled".into());
                }
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Enter => {
                    if typed == "WIPE" {
                        let outcome = match ctx.store.teardown() {
                            Ok(()) => "store wiped — tables recreated, photos cleared".to_string(),
                            Err(e) => format!("teardown failed: {e}"),
                        };
                        self.mode = Mode::Menu;
                        self.status = Some(outcome);
                    }
                }
                KeyCode::Char(c) if c.is_ascii_alphabetic() && typed.len() < 8 => {
                    typed.push(c.to_ascii_uppercase());
                }
                _ => {}
            }
            return Action::Redraw;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.sel = (self.sel + 1) % RANGES.len();
                Action::Redraw
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sel = (self.sel + RANGES.len() - 1) % RANGES.len();
                Action::Redraw
            }
            KeyCode::Char('g') => {
                self.sel = 0;
                Action::Redraw
            }
            KeyCode::Char('G') => {
                self.sel = RANGES.len() - 1;
                Action::Redraw
            }
            KeyCode::Char('p') => {
                self.with_predictions = !self.with_predictions;
                Action::Redraw
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.generate(ctx),
            KeyCode::Char('X') => {
                self.mode = Mode::Confirm(String::new());
                self.status = None;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        if matches!(self.mode, Mode::Confirm(_)) {
            return Action::None;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.sel = (self.sel + 1) % RANGES.len();
                Action::Redraw
            }
            MouseEventKind::ScrollUp => {
                self.sel = (self.sel + RANGES.len() - 1) % RANGES.len();
                Action::Redraw
            }
            _ => Action::None,
        }
    }
}
