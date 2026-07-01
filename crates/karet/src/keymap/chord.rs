//! The atomic keybinding representation: a [`KeyChord`] — a key code plus its
//! modifier state — and the matching logic that maps a live terminal event onto it.
//!
//! A chord is stored in a *canonical* form so plain equality captures how terminals
//! encode keys under the kitty protocol:
//!
//! - a letter with `Ctrl`/`Alt` folds to lower case and keeps `Shift` as a distinct
//!   flag (so `Ctrl+Shift+P` stays distinguishable from `Ctrl+P`);
//! - a bare letter carries `Shift` in its character case (`g` vs `G`), so the
//!   `Shift` flag is dropped;
//! - `BackTab` normalizes to `Tab` + `Shift`.
//!
//! [`KeyChord::from_event`] canonicalizes a live event; the binding table is
//! authored in the same canonical form (guarded by a test), so a binding matches a
//! key press iff their chords are equal. Storing chords as comparable values — not a
//! bespoke `match` — is what lets a *sequence* of them ([multi-key bindings]) be
//! compared the same way.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

/// Modifier state for a [`KeyChord`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    /// Control held.
    pub ctrl: bool,
    /// Alt/Option held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
}

/// One key press: a key code plus its modifiers — the atom of a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// The key code.
    pub code: KeyCode,
    /// The active modifiers.
    pub mods: Mods,
}

/// Build a [`KeyChord`] for the binding table. Author in canonical form: a
/// `Ctrl`/`Alt` letter lower-cased, a bare letter with `shift = false`.
#[must_use]
pub const fn chord(ctrl: bool, shift: bool, alt: bool, code: KeyCode) -> KeyChord {
    KeyChord {
        code,
        mods: Mods { ctrl, alt, shift },
    }
}

impl KeyChord {
    /// The canonical form of this chord: a `Ctrl`/`Alt` letter is lower-cased and a
    /// bare letter drops its `Shift` flag (the case already carries it).
    #[must_use]
    pub fn canonical(self) -> Self {
        let mut c = self;
        if let KeyCode::Char(ch) = c.code {
            if c.mods.ctrl || c.mods.alt {
                c.code = KeyCode::Char(ch.to_ascii_lowercase());
            } else {
                c.mods.shift = false;
            }
        }
        c
    }

    /// The canonical chord for a live terminal key event.
    #[must_use]
    pub fn from_event(ev: KeyEvent) -> Self {
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);
        // Some terminals report Shift+Tab as `BackTab`; fold it back to Tab + Shift.
        let (code, shift) = match ev.code {
            KeyCode::BackTab => (KeyCode::Tab, true),
            other => (other, ev.modifiers.contains(KeyModifiers::SHIFT)),
        };
        KeyChord {
            code,
            mods: Mods { ctrl, alt, shift },
        }
        .canonical()
    }

    /// Whether this chord matches a live key event.
    #[must_use]
    pub fn matches(self, ev: KeyEvent) -> bool {
        self.canonical() == Self::from_event(ev)
    }

    /// Render this chord for humans in the requested [`ChordStyle`].
    #[must_use]
    pub fn display(self, style: ChordStyle) -> String {
        let mut s = String::new();
        match style {
            ChordStyle::Verbose => {
                if self.mods.ctrl {
                    s.push_str("Ctrl+");
                }
                if self.mods.alt {
                    s.push_str("Alt+");
                }
                if self.mods.shift {
                    s.push_str("Shift+");
                }
            },
            ChordStyle::Caret => {
                if self.mods.ctrl {
                    s.push('^');
                }
                if self.mods.alt {
                    s.push('⌥');
                }
                if self.mods.shift {
                    s.push('⇧');
                }
            },
        }
        s.push_str(&format_code(self.code));
        s
    }
}

/// How a [`KeyChord`] is rendered for humans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordStyle {
    /// Verbose, plus-separated: `"Ctrl+Shift+P"`. Command palette, welcome, help.
    Verbose,
    /// Compact caret/symbol notation: `"^P"`, `"⇧^P"`, `"⌥1"`. The tight status bar.
    Caret,
}

/// Format a single key code for display (shared by every [`ChordStyle`]).
fn format_code(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_ascii_uppercase().to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PgUp".to_string(),
        KeyCode::PageDown => "PgDn".to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_letter_is_case_insensitive_and_keeps_shift() {
        let ctrl_p = chord(true, false, false, KeyCode::Char('p'));
        assert!(ctrl_p.matches(ev(KeyCode::Char('p'), KeyModifiers::CONTROL)));
        assert!(ctrl_p.matches(ev(KeyCode::Char('P'), KeyModifiers::CONTROL)));
        // Ctrl+Shift+P is a distinct chord.
        assert!(!ctrl_p.matches(ev(
            KeyCode::Char('P'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        let ctrl_shift_p = chord(true, true, false, KeyCode::Char('p'));
        assert!(ctrl_shift_p.matches(ev(
            KeyCode::Char('P'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn bare_letter_case_carries_shift() {
        // `S` matches a shifted `s` regardless of how the terminal reports Shift.
        let cap_s = chord(false, false, false, KeyCode::Char('S'));
        assert!(cap_s.matches(ev(KeyCode::Char('S'), KeyModifiers::SHIFT)));
        assert!(cap_s.matches(ev(KeyCode::Char('S'), KeyModifiers::NONE)));
        // A bare letter never matches a Ctrl/Alt chord.
        assert!(!cap_s.matches(ev(KeyCode::Char('s'), KeyModifiers::CONTROL)));
        let low_s = chord(false, false, false, KeyCode::Char('s'));
        assert!(!low_s.matches(ev(KeyCode::Char('S'), KeyModifiers::SHIFT)));
    }

    #[test]
    fn backtab_normalizes_to_shift_tab() {
        let shift_tab = chord(false, true, false, KeyCode::Tab);
        assert!(shift_tab.matches(ev(KeyCode::BackTab, KeyModifiers::NONE)));
        assert!(shift_tab.matches(ev(KeyCode::Tab, KeyModifiers::SHIFT)));
    }

    #[test]
    fn non_char_keys_respect_shift_flag() {
        let shift_down = chord(false, true, false, KeyCode::Down);
        assert!(shift_down.matches(ev(KeyCode::Down, KeyModifiers::SHIFT)));
        assert!(!shift_down.matches(ev(KeyCode::Down, KeyModifiers::NONE)));
    }

    #[test]
    fn display_renders_each_style() {
        let ctrl_shift_p = chord(true, true, false, KeyCode::Char('p'));
        assert_eq!(ctrl_shift_p.display(ChordStyle::Verbose), "Ctrl+Shift+P");
        assert_eq!(ctrl_shift_p.display(ChordStyle::Caret), "^⇧P");
        let ctrl_p = chord(true, false, false, KeyCode::Char('p'));
        assert_eq!(ctrl_p.display(ChordStyle::Caret), "^P");
        let tab = chord(false, false, false, KeyCode::Tab);
        assert_eq!(tab.display(ChordStyle::Verbose), "Tab");
    }
}
