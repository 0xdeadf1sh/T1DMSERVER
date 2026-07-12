//! Notes pane: a reverse-chronological timeline of pinned notes, wrapped and
//! scrollable.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use t1dm_core::Note;

use crate::widgets::ScrollState;

use super::help::{scroll_key, scroll_mouse};
use super::{Action, Ctx, PaneView};

const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct NotesPane {
    scroll: ScrollState,
    notes: Vec<Note>,
    last_fetch: Option<Instant>,
    err: Option<String>,
}

impl NotesPane {
    fn refresh(&mut self, ctx: &Ctx) {
        let due = self
            .last_fetch
            .map(|t| t.elapsed() >= REFRESH)
            .unwrap_or(true);
        if !due {
            return;
        }
        self.last_fetch = Some(Instant::now());
        match ctx.store.get_notes(None, None) {
            Ok(n) => {
                self.notes = n;
                self.err = None;
            }
            Err(e) => self.err = Some(e.to_string()),
        }
    }
}

impl PaneView for NotesPane {
    fn title(&self) -> &str {
        "Notes"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.refresh(ctx);
        let pal = ctx.theme.palette();

        let block = Block::default()
            .title(Span::styled(
                format!(" Notes · {} ", self.notes.len()),
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
            let p = Paragraph::new(Line::from(Span::styled(
                format!("store error: {err}"),
                Style::default().fg(pal.hyper),
            )));
            frame.render_widget(p, inner);
            return;
        }
        if self.notes.is_empty() {
            let p = Paragraph::new(Line::from(Span::styled(
                "no notes yet — generate synthetic data in the Developer pane",
                Style::default().fg(pal.dim),
            )));
            frame.render_widget(p, inner);
            return;
        }

        let glyph = ctx.theme.glyphs().note_marker;
        let wrap_w = (inner.width as usize).saturating_sub(2).max(8);
        let mut lines: Vec<Line> = Vec::new();
        for note in &self.notes {
            let stamp = fmt_dt(note.ts, note.tz_offset);
            lines.push(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(pal.accent)),
                Span::styled(stamp, Style::default().fg(pal.secondary)),
            ]));
            for wrapped in textwrap::wrap(&note.text, wrap_w) {
                lines.push(Line::from(Span::styled(
                    format!("   {wrapped}"),
                    Style::default().fg(pal.fg),
                )));
            }
            lines.push(Line::from(""));
        }

        self.scroll.set_bounds(lines.len(), inner.height as usize);
        let para = Paragraph::new(lines).scroll((self.scroll.offset as u16, 0));
        frame.render_widget(para, inner);
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
}

/// Format an epoch-ms instant, applying a tz offset in minutes east of UTC, as
/// `YYYY-MM-DD HH:MM`.
pub(crate) fn fmt_dt(ms: i64, tz_off_min: i32) -> String {
    use time::{OffsetDateTime, UtcOffset};
    let secs = ms.div_euclid(1000);
    let mut dt = OffsetDateTime::from_unix_timestamp(secs).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    if let Ok(off) = UtcOffset::from_whole_seconds(tz_off_min * 60) {
        dt = dt.to_offset(off);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute()
    )
}
