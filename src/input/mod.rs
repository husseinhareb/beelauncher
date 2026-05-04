//! Keyboard input processing: xkbcommon state → typed characters and actions.

use smithay_client_toolkit::seat::keyboard::{KeyEvent, Keysym, Modifiers};

/// High-level actions emitted by the input handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Printable character appended to query.
    Char(char),
    /// Backspace pressed.
    Backspace,
    /// Move selection down.
    SelectNext,
    /// Move selection up.
    SelectPrev,
    /// Confirm (Enter).
    Confirm,
    /// Cancel (Escape / Ctrl-c).
    Cancel,
    /// Ctrl-w: delete last word.
    DeleteWord,
    /// Ctrl-u: clear the query.
    ClearLine,
}

/// Convert a smithay KeyEvent into an optional Action.
pub fn translate(event: &KeyEvent, mods: &Modifiers) -> Option<Action> {
    let ctrl = mods.ctrl;

    match event.keysym {
        Keysym::Return | Keysym::KP_Enter => Some(Action::Confirm),
        Keysym::Escape => Some(Action::Cancel),
        Keysym::BackSpace => Some(Action::Backspace),
        Keysym::Up | Keysym::KP_Up => Some(Action::SelectPrev),
        Keysym::Down | Keysym::KP_Down => Some(Action::SelectNext),

        Keysym::Tab => Some(if mods.shift {
            Action::SelectPrev
        } else {
            Action::SelectNext
        }),

        Keysym::n if ctrl => Some(Action::SelectNext),
        Keysym::p if ctrl => Some(Action::SelectPrev),
        Keysym::w if ctrl => Some(Action::DeleteWord),
        Keysym::u if ctrl => Some(Action::ClearLine),
        Keysym::c if ctrl => Some(Action::Cancel),

        _ => {
            // Fall through to the UTF-8 character if it's printable.
            event.utf8.as_deref().and_then(printable_char).map(Action::Char)
        }
    }
}

fn printable_char(s: &str) -> Option<char> {
    let ch = s.chars().next()?;
    // Filter control characters; keep everything else (including Unicode).
    if ch.is_control() { None } else { Some(ch) }
}
