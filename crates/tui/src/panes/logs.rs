//! Logs pane. Reads a process-wide, bounded in-memory ring of log records and
//! renders it scrollable, level-coloured, with a tail-follow toggle.
//!
//! Nothing in the TUI crate depends on `tracing`, so the wiring seam is a
//! dependency-free [`LogSink`] (an [`std::io::Write`]). The root binary can
//! forward `tracing` output here with, e.g.
//! `tracing_subscriber::fmt().with_writer(|| tui::panes::logs::LogSink)`; the
//! ring then fills with the same formatted lines the pane displays.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::theme::Palette;
use crate::widgets::ScrollState;

use super::help::{scroll_key, scroll_mouse};
use super::{Action, Ctx, PaneView};

/// Maximum records retained; oldest are evicted first.
const RING_CAP: usize = 4000;

/// Severity of a log record, parsed leniently from a formatted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn from_line(line: &str) -> LogLevel {
        // `tracing_subscriber::fmt` emits the level as an uppercase token.
        if line.contains("ERROR") {
            LogLevel::Error
        } else if line.contains("WARN") {
            LogLevel::Warn
        } else if line.contains("DEBUG") {
            LogLevel::Debug
        } else if line.contains("TRACE") {
            LogLevel::Trace
        } else {
            LogLevel::Info
        }
    }

    fn color(self, pal: &Palette) -> Color {
        match self {
            LogLevel::Error => pal.hyper,
            LogLevel::Warn => pal.accent,
            LogLevel::Info => pal.fg,
            LogLevel::Debug => pal.secondary,
            LogLevel::Trace => pal.dim,
        }
    }
}

/// One retained log line, with its parsed level and capture time.
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub ts: i64,
    pub level: LogLevel,
    pub line: String,
}

fn ring() -> &'static Mutex<VecDeque<LogRecord>> {
    static R: OnceLock<Mutex<VecDeque<LogRecord>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(VecDeque::with_capacity(256)))
}

/// Append one formatted log line to the ring (level auto-detected).
pub fn push_line(line: impl Into<String>) {
    let line = line.into();
    let level = LogLevel::from_line(&line);
    let mut r = ring().lock().unwrap_or_else(|e| e.into_inner());
    if r.len() >= RING_CAP {
        r.pop_front();
    }
    r.push_back(LogRecord {
        ts: store::now_ms(),
        level,
        line,
    });
}

/// Snapshot the ring oldest-first for rendering.
pub fn snapshot() -> Vec<LogRecord> {
    let r = ring().lock().unwrap_or_else(|e| e.into_inner());
    r.iter().cloned().collect()
}

/// A dependency-free `tracing`/log sink. Each write is split into lines and
/// pushed to the ring; hand it to a `MakeWriter` in the root binary.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogSink;

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            if !line.trim().is_empty() {
                push_line(line.to_string());
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub struct LogsPane {
    scroll: ScrollState,
    /// Tail-follow: stick to the newest record until the user scrolls up.
    follow: bool,
    seeded: bool,
}

impl PaneView for LogsPane {
    fn title(&self) -> &str {
        "Logs"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        let pal = ctx.theme.palette();

        if !self.seeded {
            self.seeded = true;
            self.follow = true;
            if snapshot().is_empty() {
                push_line(format!(
                    "INFO tui::logs ring ready (capacity {RING_CAP}); tracing records appear here once the sink is wired"
                ));
            }
        }

        let records = snapshot();
        let title = format!(
            " Logs · {} records{} ",
            records.len(),
            if self.follow { " · tail" } else { "" }
        );
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines: Vec<Line> = records
            .iter()
            .map(|rec| {
                Line::from(Span::styled(
                    rec.line.clone(),
                    Style::default().fg(rec.level.color(pal)),
                ))
            })
            .collect();

        self.scroll.set_bounds(lines.len(), inner.height as usize);
        if self.follow {
            self.scroll.bottom();
        }

        if lines.is_empty() {
            let hint = Paragraph::new(Line::from(Span::styled(
                "no log records yet",
                Style::default().fg(pal.dim),
            )));
            frame.render_widget(hint, inner);
        } else {
            let para = Paragraph::new(lines).scroll((self.scroll.offset as u16, 0));
            frame.render_widget(para, inner);
        }
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Action {
        if let KeyCode::Char('f') = key.code {
            self.follow = !self.follow;
            return Action::Redraw;
        }
        if scroll_key(&mut self.scroll, key) {
            // Any manual movement other than a jump-to-bottom drops tail-follow.
            self.follow = matches!(key.code, KeyCode::Char('G'));
            Action::Redraw
        } else {
            Action::None
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        if scroll_mouse(&mut self.scroll, mouse) {
            self.follow = false;
            Action::Redraw
        } else {
            Action::None
        }
    }
}
