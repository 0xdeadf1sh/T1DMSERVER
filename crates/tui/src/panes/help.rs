//! Help overlay: the theme logo plus a grouped keymap reference. Toggled with
//! `h`/`?` (global), and scrollable when the terminal is short.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::widgets::ScrollState;

use super::{Action, Ctx, PaneView};

#[derive(Default)]
pub struct HelpPane {
    scroll: ScrollState,
}

impl PaneView for HelpPane {
    fn title(&self) -> &str {
        "Help"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        let pal = ctx.theme.palette();
        let block = Block::default()
            .title(Span::styled(
                " Help · keymap ",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let key = |k: &str, d: &str| -> Line<'static> {
            Line::from(vec![
                Span::styled(format!("  {k:<12}"), Style::default().fg(pal.secondary)),
                Span::styled(d.to_string(), Style::default().fg(pal.fg)),
            ])
        };
        let head = |t: &str| -> Line<'static> {
            Line::from(Span::styled(
                t.to_string(),
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
        };
        let blank = || Line::from("");

        let mut lines: Vec<Line> = Vec::new();
        for row in ctx.theme.logo() {
            lines.push(Line::from(Span::styled(
                (*row).to_string(),
                Style::default().fg(pal.accent),
            )));
        }
        lines.push(Line::from(Span::styled(
            ctx.theme.boot_tagline().to_string(),
            Style::default().fg(pal.dim).add_modifier(Modifier::ITALIC),
        )));
        lines.push(blank());

        lines.push(head("Global"));
        lines.push(key("Tab / ⇧Tab", "cycle panes forward / back"));
        lines.push(key("click", "header tab switches pane"));
        lines.push(key("h / ?", "toggle this help (returns to prior pane)"));
        lines.push(key("t", "cycle theme"));
        lines.push(key("u", "toggle BG unit (mg/dL ⇄ mmol/L)"));
        lines.push(key("q / Ctrl-C", "quit"));
        lines.push(blank());

        lines.push(head("Lists & logs"));
        lines.push(key("j / k", "down / up one line"));
        lines.push(key("g / G", "jump to top / bottom"));
        lines.push(key("Ctrl-d/u", "half-page down / up"));
        lines.push(key("PgDn/PgUp", "page down / up"));
        lines.push(key("wheel", "scroll under the cursor"));
        lines.push(blank());

        lines.push(head("Dashboard"));
        lines.push(key("← / →", "pan through time"));
        lines.push(key("j / k", "zoom in / out"));
        lines.push(key("=", "reset zoom & pan"));
        lines.push(key("0", "snap to now (re-enables follow)"));
        lines.push(key("f", "toggle live-follow lock"));
        lines.push(key("i", "cycle insulin (total / bolus / basal)"));
        lines.push(key("drag / wheel", "pan / zoom with the mouse"));
        lines.push(blank());

        lines.push(head("Sessions"));
        lines.push(key("m", "mint RW token (revokes prior RW)"));
        lines.push(key("o", "mint RO token (type a device label)"));
        lines.push(key("x", "revoke the selected token"));
        lines.push(key("j / k", "move the token selection"));
        lines.push(blank());

        lines.push(head("Developer"));
        lines.push(key("j / k", "choose a synthetic range"));
        lines.push(key("Enter", "generate synthetic data"));
        lines.push(key("p / n", "toggle predictions / notes"));
        lines.push(key("X", "teardown store (type WIPE to confirm)"));
        lines.push(blank());

        lines.push(head("Settings"));
        lines.push(key("j / k", "move the setting selection"));
        lines.push(key("Enter", "activate the selected setting"));
        lines.push(key("b", "back up the database now"));

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

/// Shared list-scroll key handling; returns true when the event moved the view.
pub(crate) fn scroll_key(s: &mut ScrollState, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let half = (s.viewport / 2).max(1);
    match key.code {
        KeyCode::Char('d') if ctrl => s.down(half),
        KeyCode::Char('u') if ctrl => s.up(half),
        KeyCode::Char('j') | KeyCode::Down => s.down(1),
        KeyCode::Char('k') | KeyCode::Up => s.up(1),
        KeyCode::Char('g') => s.top(),
        KeyCode::Char('G') => s.bottom(),
        KeyCode::PageDown => s.page_down(),
        KeyCode::PageUp => s.page_up(),
        _ => return false,
    }
    true
}

/// Shared mouse-wheel scroll handling; returns true when it moved the view.
pub(crate) fn scroll_mouse(s: &mut ScrollState, mouse: MouseEvent) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollDown => s.down(3),
        MouseEventKind::ScrollUp => s.up(3),
        _ => return false,
    }
    true
}
