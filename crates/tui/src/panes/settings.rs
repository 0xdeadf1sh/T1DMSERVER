//! Settings pane. Live-adjustable settings (theme, BG unit) apply immediately
//! through the app's action channel; file-managed settings (data_dir, fps,
//! show_boot) are shown read-only with a restart-to-apply notice. `b` (or
//! activating the row) backs up the database into `backups/`.
//!
//! `Ctx` intentionally carries no `Config`, so fps/show_boot cannot be read
//! back here; they are labelled as config-file managed.

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{Action, Ctx, PaneView};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Theme,
    Unit,
    DataDir,
    Fps,
    ShowBoot,
    Backup,
}

const ROWS: [Row; 6] = [
    Row::Theme,
    Row::Unit,
    Row::DataDir,
    Row::Fps,
    Row::ShowBoot,
    Row::Backup,
];

#[derive(Default)]
pub struct SettingsPane {
    sel: usize,
    status: Option<String>,
}

impl SettingsPane {
    fn backup_now(&mut self, ctx: &Ctx) -> Action {
        let dir = ctx.store.data_dir().to_path_buf();
        let src = dir.join("t1dm.db");
        let backups = dir.join("backups");
        if let Err(e) = std::fs::create_dir_all(&backups) {
            self.status = Some(format!("backup failed: {e}"));
            return Action::Redraw;
        }
        let stamp = store::now_ms();
        let dst = backups.join(format!("t1dm-{stamp}.db"));
        match std::fs::copy(&src, &dst) {
            Ok(bytes) => {
                // Best-effort sidecar copy so a WAL snapshot travels with it.
                for suffix in ["-wal", "-shm"] {
                    let side = dir.join(format!("t1dm.db{suffix}"));
                    if side.exists() {
                        let _ =
                            std::fs::copy(&side, backups.join(format!("t1dm-{stamp}.db{suffix}")));
                    }
                }
                self.status = Some(format!(
                    "backed up {} to {}",
                    super::models::human_bytes(bytes as i64),
                    dst.display()
                ));
            }
            Err(e) => self.status = Some(format!("backup failed: {e}")),
        }
        Action::Redraw
    }

    fn activate(&mut self, ctx: &Ctx) -> Action {
        match ROWS[self.sel] {
            Row::Theme => Action::CycleTheme,
            Row::Unit => Action::ToggleUnit,
            Row::Backup => self.backup_now(ctx),
            Row::DataDir | Row::Fps | Row::ShowBoot => {
                self.status =
                    Some("this setting is edited in config.toml — restart to apply".into());
                Action::Redraw
            }
        }
    }
}

impl PaneView for SettingsPane {
    fn title(&self) -> &str {
        "Settings"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        let pal = ctx.theme.palette();
        let block = Block::default()
            .title(Span::styled(
                " Settings ",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let data_dir = ctx.store.data_dir().display().to_string();
        let entries: [(Row, &str, String, &str); 6] = [
            (
                Row::Theme,
                "Theme",
                ctx.theme.name().to_string(),
                "[t] cycle",
            ),
            (
                Row::Unit,
                "BG unit",
                ctx.unit.label().to_string(),
                "[u] toggle",
            ),
            (Row::DataDir, "Data dir", data_dir, "restart to apply"),
            (
                Row::Fps,
                "FPS cap",
                "config.toml".to_string(),
                "restart to apply",
            ),
            (
                Row::ShowBoot,
                "Boot anim",
                "config.toml".to_string(),
                "restart to apply",
            ),
            (
                Row::Backup,
                "Backup now",
                "→ data/backups/".to_string(),
                "[b] / Enter",
            ),
        ];

        let mut lines: Vec<Line> = Vec::new();
        for (i, (_row, label, value, hint)) in entries.iter().enumerate() {
            let selected = i == self.sel;
            let marker = if selected { "▶ " } else { "  " };
            let label_style = if selected {
                Style::default()
                    .fg(pal.bg)
                    .bg(pal.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(pal.secondary)
            };
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(pal.accent)),
                Span::styled(format!("{label:<12}"), label_style),
                Span::styled(format!(" {value}"), Style::default().fg(pal.fg)),
                Span::styled(format!("   {hint}"), Style::default().fg(pal.dim)),
            ]));
        }
        lines.push(Line::from(""));
        if let Some(status) = &self.status {
            lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(pal.secondary),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "j/k select · Enter activate · b backup",
                Style::default().fg(pal.dim),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.sel = (self.sel + 1) % ROWS.len();
                Action::Redraw
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sel = (self.sel + ROWS.len() - 1) % ROWS.len();
                Action::Redraw
            }
            KeyCode::Char('g') => {
                self.sel = 0;
                Action::Redraw
            }
            KeyCode::Char('G') => {
                self.sel = ROWS.len() - 1;
                Action::Redraw
            }
            KeyCode::Char('b') => self.backup_now(ctx),
            KeyCode::Enter | KeyCode::Char(' ') => self.activate(ctx),
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent, _ctx: &mut Ctx) -> Action {
        use ratatui::crossterm::event::MouseEventKind;
        // Wheel moves the selection.
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.sel = (self.sel + 1) % ROWS.len();
                Action::Redraw
            }
            MouseEventKind::ScrollUp => {
                self.sel = (self.sel + ROWS.len() - 1) % ROWS.len();
                Action::Redraw
            }
            _ => Action::None,
        }
    }
}
