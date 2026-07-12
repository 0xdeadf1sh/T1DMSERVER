//! Responsive layout. The whole tree reflows by terminal width into three
//! modes; content never scrolls horizontally.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Width-driven layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// > 120 cols: header | left | main | right | footer.
    Full,
    /// 60–120 cols: header | main | footer (panels toggle in).
    Compact,
    /// < 60 cols / Termux vertical: condensed switcher | single column.
    Mobile,
}

impl LayoutMode {
    pub fn for_width(width: u16) -> LayoutMode {
        if width > 120 {
            LayoutMode::Full
        } else if width >= 60 {
            LayoutMode::Compact
        } else {
            LayoutMode::Mobile
        }
    }
}

/// Computed regions for a frame. `left`/`right` are `None` outside Full mode.
#[derive(Debug, Clone, Copy)]
pub struct AppLayout {
    pub mode: LayoutMode,
    pub header: Rect,
    pub footer: Rect,
    pub main: Rect,
    pub left: Option<Rect>,
    pub right: Option<Rect>,
}

impl AppLayout {
    /// Split `area` into header/body/footer, then body into panels per mode.
    pub fn compute(area: Rect) -> AppLayout {
        let mode = LayoutMode::for_width(area.width);
        let header_h = 1;
        let footer_h = if matches!(mode, LayoutMode::Mobile) {
            2
        } else {
            1
        };

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(header_h),
                Constraint::Min(1),
                Constraint::Length(footer_h),
            ])
            .split(area);
        let header = rows[0];
        let body = rows[1];
        let footer = rows[2];

        let (main, left, right) = match mode {
            LayoutMode::Full => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(28),
                        Constraint::Min(20),
                        Constraint::Length(28),
                    ])
                    .split(body);
                (cols[1], Some(cols[0]), Some(cols[2]))
            }
            LayoutMode::Compact | LayoutMode::Mobile => (body, None, None),
        };

        AppLayout {
            mode,
            header,
            footer,
            main,
            left,
            right,
        }
    }
}
