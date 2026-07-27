//! Footer bar: Pi health, session counts, clocks, request rate, and the
//! alert ticker. The app fills [`FooterInfo`] (sysinfo + store); this module
//! owns the rendering and reflows between a single wide line and a two-row
//! mobile stack.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::layout::LayoutMode;
use crate::panes::Ctx;
use crate::theme::Palette;

/// Live telemetry rendered in the footer. Populated each frame by the app.
#[derive(Debug, Clone, Default)]
pub struct FooterInfo {
    pub cpu_pct: f32,
    pub per_core: Vec<f32>,
    pub ram_used_mb: u64,
    pub ram_total_mb: u64,
    pub storage_used_gb: f32,
    pub storage_total_gb: f32,
    pub uptime_secs: u64,
    /// CPU temperature in °C, or `None` on hosts without a thermal zone.
    pub cpu_temp_c: Option<f32>,
    pub server_ip: String,
    pub rw_sessions: u32,
    pub ro_sessions: u32,
    /// Pi wall clock, epoch ms.
    pub pi_time_ms: i64,
    /// Timestamp of the last app-provided sample, epoch ms.
    pub last_sample_ms: Option<i64>,
    /// The phone's UTC offset in minutes, carried on every row it writes. Both
    /// clocks below render through it, as every other dated surface does, so
    /// the footer reads in the same zone as the panes above it.
    pub tz_offset_min: i32,
    pub requests_per_hour: u32,
    /// Most recent alert summary for the ticker.
    pub alert_ticker: Option<String>,
}

/// Render the footer line(s) into `area`.
pub fn render(frame: &mut Frame, area: Rect, ctx: &Ctx, info: &FooterInfo) {
    let pal = *ctx.theme.palette();
    let temp = info
        .cpu_temp_c
        .map(|t| format!("{t:.0}\u{00b0}C"))
        .unwrap_or_else(|| "--".to_string());
    let ip = if info.server_ip.is_empty() {
        "-"
    } else {
        info.server_ip.as_str()
    };

    match ctx.layout {
        LayoutMode::Mobile => render_mobile(frame, area, &pal, info, &temp, ip),
        LayoutMode::Full | LayoutMode::Compact => render_wide(frame, area, &pal, info, &temp, ip),
    }
}

/// Single-line footer: health, then sessions/clocks/rate, then the alert.
fn render_wide(
    frame: &mut Frame,
    area: Rect,
    pal: &Palette,
    info: &FooterInfo,
    temp: &str,
    ip: &str,
) {
    let health = format!(
        "CPU {:>3.0}% {} \u{00b7} RAM {}/{}MB \u{00b7} {:.1}/{:.1}GB \u{00b7} up {} \u{00b7} {} \u{00b7} {}",
        info.cpu_pct,
        per_core_str(&info.per_core),
        info.ram_used_mb,
        info.ram_total_mb,
        info.storage_used_gb,
        info.storage_total_gb,
        fmt_uptime(info.uptime_secs),
        temp,
        ip,
    );
    let mid = format!(
        "RW {} \u{00b7} RO {} \u{00b7} Pi {} \u{00b7} smp {} \u{00b7} {}/h",
        info.rw_sessions,
        info.ro_sessions,
        hms(info.pi_time_ms, info.tz_offset_min),
        info.last_sample_ms
            .map(|t| hms(t, info.tz_offset_min))
            .unwrap_or_else(|| "--:--:--".to_string()),
        info.requests_per_hour,
    );

    let mut spans = vec![
        Span::styled(health, Style::default().fg(pal.dim).bg(pal.surface)),
        Span::styled("  ", Style::default().bg(pal.surface)),
        Span::styled(mid, Style::default().fg(pal.secondary).bg(pal.surface)),
    ];
    if let Some(alert) = &info.alert_ticker {
        spans.push(Span::styled("  ", Style::default().bg(pal.surface)));
        spans.push(Span::styled(
            format!("\u{26a0} {alert}"),
            Style::default().fg(pal.hyper).bg(pal.surface),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(pal.surface)),
        area,
    );
}

/// Two-row footer for narrow terminals: condensed health above, then
/// sessions/clocks/rate with the alert ticker appended.
fn render_mobile(
    frame: &mut Frame,
    area: Rect,
    pal: &Palette,
    info: &FooterInfo,
    temp: &str,
    ip: &str,
) {
    let row0 = format!(
        "CPU {:>3.0}% \u{00b7} {}/{}MB \u{00b7} {} \u{00b7} {}",
        info.cpu_pct, info.ram_used_mb, info.ram_total_mb, temp, ip,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            row0,
            Style::default().fg(pal.dim).bg(pal.surface),
        )))
        .style(Style::default().bg(pal.surface)),
        Rect::new(area.left(), area.top(), area.width, 1),
    );

    if area.height >= 2 {
        let mut spans = vec![Span::styled(
            format!(
                "RW {} RO {} \u{00b7} Pi {} \u{00b7} smp {} \u{00b7} {}/h",
                info.rw_sessions,
                info.ro_sessions,
                hm(info.pi_time_ms, info.tz_offset_min),
                info.last_sample_ms
                    .map(|t| hm(t, info.tz_offset_min))
                    .unwrap_or_else(|| "--:--".to_string()),
                info.requests_per_hour,
            ),
            Style::default().fg(pal.secondary).bg(pal.surface),
        )];
        if let Some(alert) = &info.alert_ticker {
            spans.push(Span::styled(
                " \u{00b7} ",
                Style::default().fg(pal.dim).bg(pal.surface),
            ));
            spans.push(Span::styled(
                format!("\u{26a0} {alert}"),
                Style::default().fg(pal.hyper).bg(pal.surface),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(pal.surface)),
            Rect::new(area.left(), area.top() + 1, area.width, 1),
        );
    }
}

/// Compact per-core usage, e.g. `[ 8 15 10 20]`.
fn per_core_str(cores: &[f32]) -> String {
    if cores.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(cores.len() * 3 + 2);
    s.push('[');
    for (i, c) in cores.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{c:>2.0}"));
    }
    s.push(']');
    s
}

/// Human uptime: `2d3h`, `4h20m`, `45m`, or `30s`.
fn fmt_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

/// `HH:MM:SS` local wall clock for an epoch-ms instant at `tz_offset_min`
/// (24h wrap). The offset is the phone's, so the footer's clocks read in the
/// same zone as the dated surfaces above them.
fn hms(ms: i64, tz_offset_min: i32) -> String {
    if ms <= 0 {
        return "--:--:--".to_string();
    }
    let secs = (ms + tz_offset_min as i64 * 60_000).div_euclid(1000);
    let s = secs.rem_euclid(60);
    let m = secs.div_euclid(60).rem_euclid(60);
    let h = secs.div_euclid(3_600).rem_euclid(24);
    format!("{h:02}:{m:02}:{s:02}")
}

/// `HH:MM` local wall clock for the tight mobile row.
fn hm(ms: i64, tz_offset_min: i32) -> String {
    if ms <= 0 {
        return "--:--".to_string();
    }
    let secs = (ms + tz_offset_min as i64 * 60_000).div_euclid(1000);
    let m = secs.div_euclid(60).rem_euclid(60);
    let h = secs.div_euclid(3_600).rem_euclid(24);
    format!("{h:02}:{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clocks_render_in_the_phones_zone() {
        // 2025-01-01T00:00:00Z at UTC−5 is 2024-12-31 19:00 local, which is
        // what the Data pane and the phone itself both print for this row.
        let ts = 1_735_689_600_000;
        assert_eq!(hms(ts, -300), "19:00:00");
        assert_eq!(hm(ts, -300), "19:00");
        assert_eq!(hms(ts, 0), "00:00:00");
        // A half-hour zone (UTC+5:45, Kathmandu) must not round away.
        assert_eq!(hm(ts, 345), "05:45");
        // Wrapping forward past midnight.
        assert_eq!(hm(ts + 23 * 3_600_000, 120), "01:00");
    }

    #[test]
    fn the_no_data_sentinel_survives_an_offset() {
        assert_eq!(hms(0, -300), "--:--:--");
        assert_eq!(hm(0, 600), "--:--");
    }
}
