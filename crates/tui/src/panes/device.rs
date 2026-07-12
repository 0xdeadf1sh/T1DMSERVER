//! Device pane: detailed host/server health — CPU (global + per-core), memory,
//! the data-directory filesystem, uptime, CPU temperature, the primary LAN
//! address, live session/token counts, and store footprint. Data refreshes on
//! render, throttled so frequent redraws don't hammer the probes.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use sysinfo::{Disks, System};

use crate::theme::Palette;
use crate::widgets::ScrollState;

use super::help::{scroll_key, scroll_mouse};
use super::models::human_bytes;
use super::{Action, Ctx, PaneView};

const REFRESH: Duration = Duration::from_millis(1000);

struct DiskInfo {
    mount: String,
    avail: u64,
    total: u64,
}

pub struct DevicePane {
    sys: System,
    scroll: ScrollState,
    last: Option<Instant>,
    disks: Vec<DiskInfo>,
    tokens_rw: usize,
    tokens_ro: usize,
    sessions: usize,
    models: usize,
    ip: Option<String>,
    db_bytes: u64,
}

impl Default for DevicePane {
    fn default() -> Self {
        DevicePane {
            sys: System::new(),
            scroll: ScrollState::new(),
            last: None,
            disks: Vec::new(),
            tokens_rw: 0,
            tokens_ro: 0,
            sessions: 0,
            models: 0,
            ip: None,
            db_bytes: 0,
        }
    }
}

impl DevicePane {
    fn refresh(&mut self, ctx: &Ctx) {
        let due = self.last.map(|t| t.elapsed() >= REFRESH).unwrap_or(true);
        if !due {
            return;
        }
        self.last = Some(Instant::now());

        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();

        let data_dir = ctx.store.data_dir().to_path_buf();
        self.disks = disk_for(&data_dir);
        self.db_bytes = std::fs::metadata(data_dir.join("t1dm.db"))
            .map(|m| m.len())
            .unwrap_or(0);

        if let Ok(tokens) = ctx.store.list_tokens(false) {
            self.tokens_rw = tokens
                .iter()
                .filter(|t| t.kind == t1dm_core::TokenKind::Rw)
                .count();
            self.tokens_ro = tokens.len() - self.tokens_rw;
        }
        self.sessions = ctx.store.list_sessions().map(|s| s.len()).unwrap_or(0);
        self.models = ctx.store.list_models().map(|m| m.len()).unwrap_or(0);

        if self.ip.is_none() {
            self.ip = local_ip();
        }
    }
}

impl PaneView for DevicePane {
    fn title(&self) -> &str {
        "Device"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &mut Ctx) {
        self.refresh(ctx);
        let pal = ctx.theme.palette();

        let block = Block::default()
            .title(Span::styled(
                " Device · host & server ",
                Style::default()
                    .fg(pal.primary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(pal.dim))
            .style(Style::default().bg(pal.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let bar_w = (inner.width as usize).saturating_sub(24).clamp(8, 40);
        let mut lines: Vec<Line> = Vec::new();

        // Host identity.
        lines.push(section("HOST", pal));
        lines.push(kv(
            "machine",
            &format!(
                "{} · {}",
                System::host_name().unwrap_or_else(|| "unknown".into()),
                System::name().unwrap_or_else(|| "?".into())
            ),
            pal,
        ));
        lines.push(kv(
            "kernel",
            &System::kernel_version().unwrap_or_else(|| "?".into()),
            pal,
        ));
        lines.push(kv("uptime", &fmt_uptime(System::uptime()), pal));
        let temp = cpu_temp_c()
            .map(|t| format!("{t:.0} °C"))
            .unwrap_or_else(|| "n/a".into());
        lines.push(kv("cpu temp", &temp, pal));
        lines.push(Line::from(""));

        // CPU.
        lines.push(section("CPU", pal));
        let global = self.sys.global_cpu_usage();
        lines.push(gauge(
            "all cores",
            global as f64 / 100.0,
            bar_w,
            pal,
            &format!("{global:>5.1}%"),
        ));
        for (i, cpu) in self.sys.cpus().iter().enumerate() {
            let u = cpu.cpu_usage();
            lines.push(gauge(
                &format!("core {i}"),
                u as f64 / 100.0,
                bar_w,
                pal,
                &format!("{u:>5.1}%"),
            ));
        }
        lines.push(Line::from(""));

        // Memory.
        lines.push(section("MEMORY", pal));
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let frac = if total > 0 {
            used as f64 / total as f64
        } else {
            0.0
        };
        lines.push(gauge(
            "ram",
            frac,
            bar_w,
            pal,
            &format!(
                "{} / {}",
                human_bytes(used as i64),
                human_bytes(total as i64)
            ),
        ));
        let sw_total = self.sys.total_swap();
        if sw_total > 0 {
            let sw_used = self.sys.used_swap();
            lines.push(gauge(
                "swap",
                sw_used as f64 / sw_total as f64,
                bar_w,
                pal,
                &format!(
                    "{} / {}",
                    human_bytes(sw_used as i64),
                    human_bytes(sw_total as i64)
                ),
            ));
        }
        lines.push(Line::from(""));

        // Storage.
        lines.push(section("STORAGE", pal));
        for d in &self.disks {
            let used_bytes = d.total.saturating_sub(d.avail);
            let used_frac = if d.total > 0 {
                used_bytes as f64 / d.total as f64
            } else {
                0.0
            };
            lines.push(gauge(
                &d.mount,
                used_frac,
                bar_w,
                pal,
                &format!(
                    "{} / {}",
                    human_bytes(used_bytes as i64),
                    human_bytes(d.total as i64)
                ),
            ));
        }
        lines.push(kv("database", &human_bytes(self.db_bytes as i64), pal));
        lines.push(Line::from(""));

        // Server.
        lines.push(section("SERVER", pal));
        let dot = if ctx.connected {
            "● connected"
        } else {
            "○ idle"
        };
        let dot_style = Style::default().fg(if ctx.connected { pal.primary } else { pal.dim });
        lines.push(Line::from(vec![
            Span::styled("  link      ", Style::default().fg(pal.dim)),
            Span::styled(dot.to_string(), dot_style),
        ]));
        lines.push(kv(
            "lan addr",
            self.ip.as_deref().unwrap_or("unavailable"),
            pal,
        ));
        lines.push(kv(
            "sessions",
            &format!(
                "{} active · RW {} · RO {} tokens",
                self.sessions, self.tokens_rw, self.tokens_ro
            ),
            pal,
        ));
        lines.push(kv("models", &self.models.to_string(), pal));

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

fn section(title: &str, pal: &Palette) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(pal.primary)
            .add_modifier(Modifier::BOLD),
    ))
}

fn kv(k: &str, v: &str, pal: &Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {k:<9} "), Style::default().fg(pal.dim)),
        Span::styled(v.to_string(), Style::default().fg(pal.fg)),
    ])
}

/// A labelled fraction gauge: `label [████░░░]  value`.
fn gauge(label: &str, frac: f64, width: usize, pal: &Palette, value: &str) -> Line<'static> {
    let frac = frac.clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let color = if frac >= 0.9 {
        pal.hyper
    } else if frac >= 0.75 {
        pal.accent
    } else {
        pal.primary
    };
    let bar_full: String = "█".repeat(filled);
    let bar_empty: String = "░".repeat(width - filled);
    Line::from(vec![
        Span::styled(format!("  {label:<9} "), Style::default().fg(pal.dim)),
        Span::styled(bar_full, Style::default().fg(color)),
        Span::styled(bar_empty, Style::default().fg(pal.grid)),
        Span::styled(format!("  {value}"), Style::default().fg(pal.fg)),
    ])
}

/// The filesystem carrying `path`: the mount point with the longest matching
/// prefix. `used` holds *available* bytes (converted at render time).
fn disk_for(path: &std::path::Path) -> Vec<DiskInfo> {
    let disks = Disks::new_with_refreshed_list();
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut best: Option<DiskInfo> = None;
    let mut best_len = 0usize;
    for d in disks.list() {
        let mp = d.mount_point();
        if canon.starts_with(mp) {
            let len = mp.as_os_str().len();
            if len >= best_len {
                best_len = len;
                best = Some(DiskInfo {
                    mount: mp.display().to_string(),
                    avail: d.available_space(),
                    total: d.total_space(),
                });
            }
        }
    }
    best.into_iter().collect()
}

/// CPU temperature in °C from the Linux thermal zone, if present.
fn cpu_temp_c() -> Option<f32> {
    let raw = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    let milli: f32 = raw.trim().parse().ok()?;
    Some(milli / 1000.0)
}

/// Best-effort primary outbound IPv4 (UDP connect picks the route; no packet
/// is sent). Returns `None` when no route is available.
fn local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip().to_string())
}

fn fmt_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", secs % 60)
    }
}
