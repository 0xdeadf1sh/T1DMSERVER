//! Sessions pane: live RW/RO tokens and client sessions, minting and
//! revocation, and the login QR rendered as Unicode half-blocks.
//!
//! Free-text input is impractical here because the app routes a handful of
//! single keys (`h`,`t`,`u`,`q`,`?`) to global actions before a pane sees
//! them — so a typed device label could never contain those letters. RO labels
//! are therefore chosen from a preset picker (navigated with j/k/Enter/Esc,
//! none of which are globally intercepted).

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use t1dm_core::{QrConfig, QrPayload, Session, Token, TokenKind};

use crate::theme::Palette;
use crate::widgets::qr::render_qr;

use super::fmt_dt;
use super::{Action, Ctx, PaneView};

const REFRESH: Duration = Duration::from_secs(2);

/// Device labels offered for RO tokens (see module note on why not free-text).
const LABELS: [&str; 7] = [
    "Phone", "Watch", "Tablet", "Laptop", "Partner", "Clinic", "Spare",
];

struct QrView {
    heading: String,
    endpoint: String,
    secret: String,
    lines: Vec<String>,
}

enum Overlay {
    None,
    LabelPick(usize),
    Qr(QrView),
}

pub struct SessionsPane {
    sel: usize,
    tokens: Vec<Token>,
    sessions: Vec<Session>,
    last_fetch: Option<Instant>,
    overlay: Overlay,
    status: Option<String>,
    addr: Option<String>,
    port: u16,
    err: Option<String>,
}

impl Default for SessionsPane {
    fn default() -> Self {
        SessionsPane {
            sel: 0,
            tokens: Vec::new(),
            sessions: Vec::new(),
            last_fetch: None,
            overlay: Overlay::None,
            status: None,
            addr: None,
            port: QrConfig::default().advertise_port,
            err: None,
        }
    }
}

impl SessionsPane {
    fn refresh(&mut self, ctx: &Ctx, force: bool) {
        let due = force
            || self
                .last_fetch
                .map(|t| t.elapsed() >= REFRESH)
                .unwrap_or(true);
        if !due {
            return;
        }
        self.last_fetch = Some(Instant::now());
        match ctx.store.list_tokens(false) {
            Ok(t) => {
                self.tokens = t;
                self.err = None;
            }
            Err(e) => self.err = Some(e.to_string()),
        }
        self.sessions = ctx.store.list_sessions().unwrap_or_default();
        if self.sel >= self.tokens.len() {
            self.sel = self.tokens.len().saturating_sub(1);
        }
        // The login QR must advertise the operator-configured address/port
        // (typically the Tailscale IP), not an auto-detected LAN address.
        self.addr = Some(ctx.qr_addr.to_string());
        self.port = ctx.qr_port;
    }

    fn show_qr(&mut self, token: &Token, secret: &str) {
        let addr = self
            .addr
            .clone()
            .unwrap_or_else(|| QrConfig::default().advertise_addr);
        let payload = QrPayload::new(secret, addr.clone(), self.port);
        let json = serde_json::to_string(&payload).unwrap_or_default();
        let lines = render_qr(&json);
        let heading = match (token.kind, &token.label) {
            (TokenKind::Rw, _) => "RW login".to_string(),
            (TokenKind::Ro, Some(l)) => format!("RO login · {l}"),
            (TokenKind::Ro, None) => "RO login".to_string(),
        };
        self.overlay = Overlay::Qr(QrView {
            heading,
            endpoint: format!("{addr}:{}", self.port),
            secret: secret.to_string(),
            lines,
        });
    }

    fn mint_rw(&mut self, ctx: &Ctx) -> Action {
        match ctx.store.mint_token(TokenKind::Rw, None) {
            Ok((token, secret)) => {
                self.refresh(ctx, true);
                self.show_qr(&token, &secret);
                self.status = Some("minted RW token (any prior RW revoked)".into());
            }
            Err(e) => self.status = Some(format!("mint failed: {e}")),
        }
        Action::Redraw
    }

    fn mint_ro(&mut self, ctx: &Ctx, label: &str) -> Action {
        match ctx.store.mint_token(TokenKind::Ro, Some(label.to_string())) {
            Ok((token, secret)) => {
                self.refresh(ctx, true);
                self.show_qr(&token, &secret);
                self.status = Some(format!("minted RO token “{label}”"));
            }
            Err(e) => self.status = Some(format!("mint failed: {e}")),
        }
        Action::Redraw
    }

    fn revoke_selected(&mut self, ctx: &Ctx) -> Action {
        let Some(token) = self.tokens.get(self.sel) else {
            self.status = Some("no token selected".into());
            return Action::Redraw;
        };
        let id = token.id;
        match ctx.store.revoke_token(id) {
            Ok(()) => self.status = Some(format!("revoked token #{id}")),
            Err(e) => self.status = Some(format!("revoke failed: {e}")),
        }
        self.refresh(ctx, true);
        Action::Redraw
    }
}

impl PaneView for SessionsPane {
    fn title(&self) -> &str {
        "Sessions"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.refresh(ctx, false);
        let pal = ctx.theme.palette();

        let block = Block::default()
            .title(Span::styled(
                " Sessions · tokens & clients ",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Split into tokens (top) and sessions (bottom); a hint/status row last.
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .split(inner);

        self.render_tokens(frame, rows[0], pal);
        self.render_sessions(frame, rows[1], pal, ctx.now_ms);

        let hint = if let Some(status) = &self.status {
            Span::styled(status.clone(), Style::default().fg(pal.secondary))
        } else {
            Span::styled(
                "m mint RW · o mint RO · x revoke · j/k select",
                Style::default().fg(pal.dim),
            )
        };
        frame.render_widget(Paragraph::new(Line::from(hint)), rows[2]);

        match &self.overlay {
            Overlay::LabelPick(sel) => render_label_pick(frame, area, pal, *sel),
            Overlay::Qr(view) => render_qr_overlay(frame, area, pal, view),
            Overlay::None => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        if let Overlay::Qr(_) = self.overlay {
            if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ')) {
                self.overlay = Overlay::None;
            }
            return Action::Redraw;
        }
        if let Overlay::LabelPick(mut sel) = self.overlay {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    sel = (sel + 1) % LABELS.len();
                    self.overlay = Overlay::LabelPick(sel);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    sel = (sel + LABELS.len() - 1) % LABELS.len();
                    self.overlay = Overlay::LabelPick(sel);
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let label = LABELS[sel];
                    self.overlay = Overlay::None;
                    return self.mint_ro(ctx, label);
                }
                KeyCode::Esc => self.overlay = Overlay::None,
                _ => {}
            }
            return Action::Redraw;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.tokens.is_empty() {
                    self.sel = (self.sel + 1) % self.tokens.len();
                }
                Action::Redraw
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.tokens.is_empty() {
                    self.sel = (self.sel + self.tokens.len() - 1) % self.tokens.len();
                }
                Action::Redraw
            }
            KeyCode::Char('g') => {
                self.sel = 0;
                Action::Redraw
            }
            KeyCode::Char('G') => {
                self.sel = self.tokens.len().saturating_sub(1);
                Action::Redraw
            }
            KeyCode::Char('m') => self.mint_rw(ctx),
            KeyCode::Char('o') => {
                self.overlay = Overlay::LabelPick(0);
                Action::Redraw
            }
            KeyCode::Char('x') | KeyCode::Char('d') => self.revoke_selected(ctx),
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        use ratatui::crossterm::event::MouseEventKind;
        if !matches!(self.overlay, Overlay::None) {
            return Action::None;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown if !self.tokens.is_empty() => {
                self.sel = (self.sel + 1) % self.tokens.len();
                Action::Redraw
            }
            MouseEventKind::ScrollUp if !self.tokens.is_empty() => {
                self.sel = (self.sel + self.tokens.len() - 1) % self.tokens.len();
                Action::Redraw
            }
            _ => Action::None,
        }
    }
}

impl SessionsPane {
    fn render_tokens(&self, frame: &mut Frame, area: Rect, pal: &Palette) {
        if area.height == 0 {
            return;
        }
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "TOKENS",
            Style::default()
                .fg(pal.primary)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(err) = &self.err {
            lines.push(Line::from(Span::styled(
                format!("store error: {err}"),
                Style::default().fg(pal.hyper),
            )));
        } else if self.tokens.is_empty() {
            lines.push(Line::from(Span::styled(
                "no live tokens — press m (RW) or o (RO) to mint",
                Style::default().fg(pal.dim),
            )));
        }
        let cap = (area.height as usize).saturating_sub(1);
        for (i, tok) in self.tokens.iter().take(cap).enumerate() {
            let selected = i == self.sel;
            let marker = if selected { "▶ " } else { "  " };
            let kind_col = if tok.kind == TokenKind::Rw {
                pal.accent
            } else {
                pal.secondary
            };
            let label = tok.label.clone().unwrap_or_else(|| "—".into());
            let name_style = if selected {
                Style::default()
                    .fg(pal.bg)
                    .bg(pal.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(pal.fg)
            };
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(pal.accent)),
                Span::styled(
                    format!("{:<3}", tok.kind.as_str().to_uppercase()),
                    Style::default().fg(kind_col).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" #{:<4} ", tok.id), Style::default().fg(pal.dim)),
                Span::styled(format!("{label:<10}"), name_style),
                Span::styled(
                    format!("  {}", fmt_dt(tok.created_at, 0)),
                    Style::default().fg(pal.dim),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_sessions(&self, frame: &mut Frame, area: Rect, pal: &Palette, now_ms: i64) {
        if area.height == 0 {
            return;
        }
        let kind_of: std::collections::HashMap<i64, TokenKind> =
            self.tokens.iter().map(|t| (t.id, t.kind)).collect();
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("CLIENTS · {}", self.sessions.len()),
            Style::default()
                .fg(pal.primary)
                .add_modifier(Modifier::BOLD),
        )));
        if self.sessions.is_empty() {
            lines.push(Line::from(Span::styled(
                "no client sessions yet",
                Style::default().fg(pal.dim),
            )));
        }
        let cap = (area.height as usize).saturating_sub(1);
        for sess in self.sessions.iter().take(cap) {
            let kind = kind_of
                .get(&sess.token_id)
                .map(|k| k.as_str().to_uppercase())
                .unwrap_or_else(|| "?".into());
            let device = if sess.device.is_empty() {
                sess.user_agent.clone()
            } else {
                sess.device.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{kind:<3} "), Style::default().fg(pal.secondary)),
                Span::styled(format!("{:<15} ", sess.ip), Style::default().fg(pal.fg)),
                Span::styled(format!("{device:<16}"), Style::default().fg(pal.fg)),
                Span::styled(
                    format!(" {} ago", fmt_rel(now_ms, sess.last_seen)),
                    Style::default().fg(pal.dim),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn render_label_pick(frame: &mut Frame, area: Rect, pal: &Palette, sel: usize) {
    let w = 30u16.min(area.width.saturating_sub(2)).max(16);
    let h = (LABELS.len() as u16 + 4)
        .min(area.height.saturating_sub(2))
        .max(6);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect::new(x, y, w, h);

    frame.render_widget(Clear, modal);
    let block = Block::default()
        .title(Span::styled(
            " mint RO · label ",
            Style::default()
                .fg(pal.primary)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(pal.accent))
        .style(Style::default().bg(pal.surface));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let mut lines: Vec<Line> = Vec::new();
    for (i, label) in LABELS.iter().enumerate() {
        let selected = i == sel;
        let style = if selected {
            Style::default()
                .fg(pal.bg)
                .bg(pal.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(pal.fg)
        };
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "▶ " } else { "  " },
                Style::default().fg(pal.accent),
            ),
            Span::styled(format!("{label:<12}"), style),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter select · Esc cancel",
        Style::default().fg(pal.dim),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_qr_overlay(frame: &mut Frame, area: Rect, pal: &Palette, view: &QrView) {
    let qr_w = view
        .lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let w = (qr_w + 4).clamp(24, area.width.saturating_sub(2).max(24));
    let h = (view.lines.len() as u16 + 6)
        .min(area.height.saturating_sub(2))
        .max(8);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let modal = Rect::new(x, y, w, h);

    frame.render_widget(Clear, modal);
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", view.heading),
            Style::default()
                .fg(pal.primary)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(pal.secondary))
        .style(Style::default().bg(pal.surface));
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    // Dark modules on a light field for scanner contrast.
    let qr_style = Style::default()
        .fg(Color::Rgb(18, 18, 18))
        .bg(Color::Rgb(236, 236, 236));
    let mut lines: Vec<Line> = Vec::new();
    for row in &view.lines {
        lines.push(Line::from(Span::styled(row.clone(), qr_style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("endpoint ", Style::default().fg(pal.dim)),
        Span::styled(view.endpoint.clone(), Style::default().fg(pal.fg)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("secret   ", Style::default().fg(pal.dim)),
        Span::styled(view.secret.clone(), Style::default().fg(pal.secondary)),
    ]));
    lines.push(Line::from(Span::styled(
        "shown once — Enter/Esc to dismiss",
        Style::default().fg(pal.hyper),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn fmt_rel(now_ms: i64, then_ms: i64) -> String {
    let d = (now_ms - then_ms).max(0) / 1000;
    if d < 60 {
        format!("{d}s")
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86_400 {
        format!("{}h", d / 3600)
    } else {
        format!("{}d", d / 86_400)
    }
}
