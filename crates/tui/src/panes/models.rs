//! Models pane: the scanned model registry. Each entry shows name/id,
//! byte size, sha256, discovery time, and its OPAQUE meta JSON rendered
//! verbatim (pretty-printed, never interpreted). Scrollable.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use t1dm_core::Model;

use crate::widgets::ScrollState;

use super::help::{scroll_key, scroll_mouse};
use super::notes::fmt_dt;
use super::{Action, Ctx, PaneView};

const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct ModelsPane {
    scroll: ScrollState,
    models: Vec<Model>,
    last_fetch: Option<Instant>,
    err: Option<String>,
}

impl ModelsPane {
    fn refresh(&mut self, ctx: &Ctx) {
        let due = self
            .last_fetch
            .map(|t| t.elapsed() >= REFRESH)
            .unwrap_or(true);
        if !due {
            return;
        }
        self.last_fetch = Some(Instant::now());
        match ctx.store.list_models() {
            Ok(m) => {
                self.models = m;
                self.err = None;
            }
            Err(e) => self.err = Some(e.to_string()),
        }
    }
}

impl PaneView for ModelsPane {
    fn title(&self) -> &str {
        "Models"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.refresh(ctx);
        let pal = ctx.theme.palette();

        let block = Block::default()
            .title(Span::styled(
                format!(" Models · {} ", self.models.len()),
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
        if self.models.is_empty() {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "no models discovered",
                        Style::default().fg(pal.dim),
                    )),
                    Line::from(Span::styled(
                        format!("drop model files into {}", ctx.store.models_dir().display()),
                        Style::default().fg(pal.dim),
                    )),
                ]),
                inner,
            );
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        for m in &self.models {
            lines.push(Line::from(vec![
                Span::styled("▣ ", Style::default().fg(pal.accent)),
                Span::styled(
                    m.name.clone(),
                    Style::default()
                        .fg(pal.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  ({})", m.id), Style::default().fg(pal.dim)),
            ]));
            lines.push(kv("size", &human_bytes(m.bytes), pal));
            lines.push(kv(
                "format",
                if m.ext.is_empty() { "—" } else { &m.ext },
                pal,
            ));
            lines.push(kv("sha256", &m.sha256, pal));
            lines.push(kv("path", &m.path, pal));
            lines.push(kv("found", &fmt_dt(m.discovered_at, 0), pal));
            lines.push(kv(
                "download",
                "available (GET /v1/models/{id}/download)",
                pal,
            ));

            lines.push(Line::from(Span::styled(
                "  meta:",
                Style::default().fg(pal.secondary),
            )));
            let meta_txt =
                serde_json::to_string_pretty(&m.meta).unwrap_or_else(|_| m.meta.to_string());
            for row in meta_txt.lines() {
                lines.push(Line::from(Span::styled(
                    format!("    {row}"),
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

fn kv(k: &str, v: &str, pal: &crate::theme::Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {k:<9}"), Style::default().fg(pal.dim)),
        Span::styled(v.to_string(), Style::default().fg(pal.fg)),
    ])
}

/// Human-readable byte size.
pub(crate) fn human_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
