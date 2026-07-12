//! Shared scroll state for every list/log pane (j/k, g/G, Ctrl-d/u, wheel).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ScrollbarState;

/// Lines a single mouse-wheel notch advances the viewport.
const WHEEL_LINES: usize = 3;

/// Tracks a vertical scroll offset over `total` items shown in `viewport`
/// rows. All movement helpers keep the offset in range.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollState {
    pub offset: usize,
    pub total: usize,
    pub viewport: usize,
}

impl ScrollState {
    pub fn new() -> Self {
        ScrollState::default()
    }

    /// Update the known content length and viewport height, re-clamping.
    pub fn set_bounds(&mut self, total: usize, viewport: usize) {
        self.total = total;
        self.viewport = viewport;
        self.clamp();
    }

    fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.viewport.max(1))
    }

    fn clamp(&mut self) {
        let m = self.max_offset();
        if self.offset > m {
            self.offset = m;
        }
    }

    pub fn down(&mut self, n: usize) {
        self.offset = (self.offset + n).min(self.max_offset());
    }

    pub fn up(&mut self, n: usize) {
        self.offset = self.offset.saturating_sub(n);
    }

    pub fn page_down(&mut self) {
        self.down(self.viewport.max(1));
    }

    pub fn page_up(&mut self) {
        self.up(self.viewport.max(1));
    }

    pub fn top(&mut self) {
        self.offset = 0;
    }

    pub fn bottom(&mut self) {
        self.offset = self.max_offset();
    }

    /// Whether the offset is pinned to the bottom (useful for logs that should
    /// auto-follow new lines only while the user has not scrolled up).
    pub fn at_bottom(&self) -> bool {
        self.offset >= self.max_offset()
    }

    /// The half-open range of item indices currently visible.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let end = (self.offset + self.viewport).min(self.total);
        self.offset..end
    }

    /// The slice of `items` currently in view.
    pub fn visible<'a, T>(&self, items: &'a [T]) -> &'a [T] {
        let r = self.visible_range();
        items.get(r).unwrap_or(&[])
    }

    /// Scroll one wheel notch up.
    pub fn wheel_up(&mut self) {
        self.up(WHEEL_LINES);
    }

    /// Scroll one wheel notch down.
    pub fn wheel_down(&mut self) {
        self.down(WHEEL_LINES);
    }

    /// Keep item `index` within the viewport, scrolling the minimum needed.
    pub fn ensure_visible(&mut self, index: usize) {
        if index < self.offset {
            self.offset = index;
        } else if self.viewport > 0 && index >= self.offset + self.viewport {
            self.offset = index + 1 - self.viewport;
        }
        self.clamp();
    }

    /// Interpret a standard scroll key (j/k, g/G, Ctrl-d/u, Page/Home/End,
    /// arrows). Returns true when the offset actually moved, so a pane can
    /// decide whether it consumed the key.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        let before = self.offset;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.viewport.max(1) / 2).max(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.down(1),
            KeyCode::Char('k') | KeyCode::Up => self.up(1),
            KeyCode::Char('d') if ctrl => self.down(half),
            KeyCode::Char('u') if ctrl => self.up(half),
            KeyCode::PageDown => self.page_down(),
            KeyCode::PageUp => self.page_up(),
            KeyCode::Char('g') | KeyCode::Home => self.top(),
            KeyCode::Char('G') | KeyCode::End => self.bottom(),
            _ => return false,
        }
        self.offset != before
    }

    /// A ratatui [`ScrollbarState`] tracking this viewport, for panes that
    /// want to draw a scrollbar alongside their content.
    pub fn scrollbar_state(&self) -> ScrollbarState {
        ScrollbarState::new(self.total)
            .viewport_content_length(self.viewport)
            .position(self.offset)
    }
}
