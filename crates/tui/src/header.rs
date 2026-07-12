//! Header bar: clickable pane tabs (with hit rects for the mouse), the theme
//! badge, the BG-unit indicator, and a live/connection dot. In mobile width the
//! full tab strip collapses to a condensed `‹ Pane n/N ›` switcher whose arrows
//! are themselves clickable.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::layout::LayoutMode;
use crate::panes::{Ctx, Pane};

/// Draw the header and return each tab's clickable rectangle for hit-testing.
pub fn render(frame: &mut Frame, area: Rect, ctx: &Ctx, current: Pane) -> Vec<(Pane, Rect)> {
    match ctx.layout {
        LayoutMode::Mobile => render_switcher(frame, area, ctx, current),
        LayoutMode::Full | LayoutMode::Compact => render_tabs(frame, area, ctx, current),
    }
}

/// Full tab strip: one clickable rect per pane, then a right-aligned status.
fn render_tabs(frame: &mut Frame, area: Rect, ctx: &Ctx, current: Pane) -> Vec<(Pane, Rect)> {
    let pal = ctx.theme.palette();
    let mut spans: Vec<Span> = Vec::new();
    let mut hits: Vec<(Pane, Rect)> = Vec::new();

    let mut x = area.left();
    for pane in Pane::HEADER {
        let label = format!(" {} ", pane.title());
        let w = label.chars().count() as u16;
        let style = if pane == current {
            Style::default()
                .fg(pal.bg)
                .bg(pal.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(pal.secondary).bg(pal.surface)
        };
        if x + w <= area.right() {
            hits.push((pane, Rect::new(x, area.top(), w, 1)));
        }
        spans.push(Span::styled(label, style));
        x = x.saturating_add(w);
    }

    let left = Paragraph::new(Line::from(spans)).style(Style::default().bg(pal.surface));
    frame.render_widget(left, area);

    render_status(frame, area, ctx, true);
    hits
}

/// Condensed switcher for narrow terminals: `‹ Title n/N ›` with clickable
/// arrows that map onto the previous/next pane in the cycle.
fn render_switcher(frame: &mut Frame, area: Rect, ctx: &Ctx, current: Pane) -> Vec<(Pane, Rect)> {
    let pal = ctx.theme.palette();
    let mut hits: Vec<(Pane, Rect)> = Vec::new();

    let cycle = Pane::CYCLE;
    let position = cycle.iter().position(|&p| p == current);
    let counter = match position {
        Some(i) => format!("{}/{}", i + 1, cycle.len()),
        // Help is an overlay outside the cycle; label it plainly.
        None => "•".to_string(),
    };

    let arrow_style = Style::default().fg(pal.secondary).bg(pal.surface);
    let center_style = Style::default()
        .fg(pal.bg)
        .bg(pal.primary)
        .add_modifier(Modifier::BOLD);

    let larrow = "‹ ";
    let center = format!(" {} {} ", current.title(), counter);
    let rarrow = " ›";

    let mut x = area.left();
    let mut spans: Vec<Span> = Vec::new();

    let lw = larrow.chars().count() as u16;
    if x + lw <= area.right() {
        hits.push((current.prev(), Rect::new(x, area.top(), lw, 1)));
    }
    spans.push(Span::styled(larrow, arrow_style));
    x = x.saturating_add(lw);

    let cw = center.chars().count() as u16;
    spans.push(Span::styled(center, center_style));
    x = x.saturating_add(cw);

    let rw = rarrow.chars().count() as u16;
    if x + rw <= area.right() {
        hits.push((current.next(), Rect::new(x, area.top(), rw, 1)));
    }
    spans.push(Span::styled(rarrow, arrow_style));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(pal.surface)),
        area,
    );

    render_status(frame, area, ctx, false);
    hits
}

/// Right-aligned status cluster: the theme badge, the unit label, and the
/// connection dot. `with_theme` drops the badge in mobile when space is tight.
fn render_status(frame: &mut Frame, area: Rect, ctx: &Ctx, with_theme: bool) {
    let pal = ctx.theme.palette();
    let dot = if ctx.connected { '●' } else { '○' };
    let dot_color = if ctx.connected { pal.primary } else { pal.dim };

    let prefix = if with_theme {
        format!("{} · {} ", ctx.theme.badge(), ctx.unit.label())
    } else {
        format!("{} ", ctx.unit.label())
    };
    let width = prefix.chars().count() as u16 + 1;
    if width >= area.width {
        return;
    }
    let rx = area.right().saturating_sub(width);
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(pal.fg).bg(pal.surface)),
        Span::styled(
            dot.to_string(),
            Style::default().fg(dot_color).bg(pal.surface),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), Rect::new(rx, area.top(), width, 1));
}
