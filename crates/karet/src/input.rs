//! The keymap engine (merged from `karet-input`): map key chords to actions,
//! with modal modes, scoping and host rebinding. Generic over the action type `A`
//! (the app binds `A = karet_session::Command`).

/// Errors from the input layer.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// A keymap configuration file was malformed.
    #[error("invalid keymap configuration")]
    Config,
}

/// A backend-agnostic key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A printable character.
    Char(char),
    /// The Enter/Return key.
    Enter,
    /// The Escape key.
    Esc,
    /// The Tab key.
    Tab,
    /// The Backspace key.
    Backspace,
    /// The Up arrow.
    Up,
    /// The Down arrow.
    Down,
    /// The Left arrow.
    Left,
    /// The Right arrow.
    Right,
    /// A function key (`F1` = `F(1)`).
    F(u8),
}

/// Keyboard modifier state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Control held.
    pub ctrl: bool,
    /// Alt/Option held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
    /// Super/Cmd/Win held.
    pub sup: bool,
}

/// A single key event: a code plus its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// The key code.
    pub code: KeyCode,
    /// The active modifiers.
    pub mods: Modifiers,
}

/// An editing mode (modal editing); use only `Normal` for a non-modal setup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Normal/command mode.
    #[default]
    Normal,
    /// Insert/text-entry mode.
    Insert,
    /// Visual/selection mode.
    Visual,
    /// Command-line mode.
    Command,
}

/// The result of resolving a (possibly partial) key sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution<A> {
    /// A complete chord resolved to this action.
    Action(A),
    /// A prefix of one or more bindings; await more keys.
    Pending,
    /// No binding matches.
    None,
}

/// A keymap from key chords (scoped by [`Mode`]) to actions of type `A`.
#[derive(Clone, Debug)]
pub struct Keymap<A> {
    bindings: Vec<(Mode, Vec<KeyEvent>, A)>,
}

impl<A> Default for Keymap<A> {
    fn default() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }
}

impl<A> Keymap<A> {
    /// Create an empty keymap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the chord `keys` (in `mode`) to `action`.
    pub fn bind(&mut self, mode: Mode, keys: &[KeyEvent], action: A) {
        self.bindings.push((mode, keys.to_vec(), action));
    }

    /// Resolve a pending key sequence against the map.
    pub fn resolve(&self, mode: Mode, pending: &[KeyEvent]) -> Resolution<A> {
        let _ = (&self.bindings, mode, pending);
        todo!()
    }
}

impl From<crossterm::event::KeyEvent> for KeyEvent {
    fn from(ev: crossterm::event::KeyEvent) -> Self {
        let _ = ev;
        todo!()
    }
}

impl<A: serde::de::DeserializeOwned> Keymap<A> {
    /// Load a keymap from a TOML configuration string.
    pub fn from_toml(s: &str) -> Result<Self, InputError> {
        let _ = s;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_binding() {
        assert_eq!(Mode::default(), Mode::Normal);
        assert_eq!(KeyCode::Char('a'), KeyCode::Char('a'));
        let mut km: Keymap<u8> = Keymap::new();
        km.bind(
            Mode::Normal,
            &[KeyEvent {
                code: KeyCode::Esc,
                mods: Modifiers::default(),
            }],
            1,
        );
        assert_eq!(
            InputError::Config.to_string(),
            "invalid keymap configuration"
        );
    }
}
