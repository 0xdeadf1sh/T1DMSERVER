//! Global keymap. Keys handled here are pane-independent; everything else is
//! offered to the visible pane. No collisions with pane-local keys.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::panes::{Action, Pane};

/// Translate a key into a global [`Action`], or `None` to defer to the pane.
///
/// Global bindings: Tab/BackTab cycle panes; `h`/`?` toggle Help; `t` cycle
/// theme; `u` toggle unit; `q`/Ctrl-C quit.
pub fn global_action(key: KeyEvent, _current: Pane) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char('c') = key.code {
            return Some(Action::Quit);
        }
    }
    match key.code {
        KeyCode::Tab => Some(Action::NextPane),
        KeyCode::BackTab => Some(Action::PrevPane),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('h') | KeyCode::Char('?') => Some(Action::ToggleHelp),
        KeyCode::Char('t') => Some(Action::CycleTheme),
        KeyCode::Char('u') => Some(Action::ToggleUnit),
        _ => None,
    }
}
