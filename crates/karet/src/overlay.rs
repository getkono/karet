//! Centered modal overlays: quick-open (go to file) and the command palette.
//!
//! Both are a [`Picker`] over labeled items with an incremental subsequence filter.
//! (The richer `karet-fuzzy` ranking / `karet-widgets::Picker` widget is a future
//! home; this keeps the skeleton dependency-light.)

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::{self, Command};
use crate::keymap;

/// What an overlay key press resulted in.
pub enum OverlayEvent {
    /// The key was consumed; keep the overlay open.
    Consumed,
    /// Dismiss the overlay.
    Close,
    /// Open the chosen file.
    AcceptFile(PathBuf),
    /// Run the chosen command.
    AcceptCommand(Command),
}

/// An incremental picker over labeled items of type `T`.
pub struct Picker<T> {
    title: String,
    query: String,
    items: Vec<(String, T)>,
    filtered: Vec<usize>,
    selected: usize,
}

impl<T> Picker<T> {
    /// Build a picker titled `title` over `items` (label + value).
    fn new(title: impl Into<String>, items: Vec<(String, T)>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            title: title.into(),
            query: String::new(),
            items,
            filtered,
            selected: 0,
        }
    }

    /// The picker title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The visible (filtered) row labels, in order.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.items[i].0.as_str())
            .collect()
    }

    /// The visible (filtered) row values, in order.
    fn values(&self) -> Vec<&T> {
        self.filtered.iter().map(|&i| &self.items[i].1).collect()
    }

    /// The selected row index within the filtered list.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Recompute the filtered list for the current query.
    fn refilter(&mut self) {
        let needle = self.query.to_ascii_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| subsequence(&needle, &label.to_ascii_lowercase()))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
    }

    /// The currently-selected value, if any.
    fn accepted(&self) -> Option<&T> {
        self.filtered.get(self.selected).map(|&i| &self.items[i].1)
    }

    /// Handle a key; returns whether the picker was closed or accepted a row.
    fn handle_key(&mut self, key: KeyEvent) -> PickerEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return PickerEvent::Close,
            KeyCode::Enter => return PickerEvent::Accept,
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('p') if ctrl => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.move_down(),
            KeyCode::Char('n') if ctrl => self.move_down(),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                self.refilter();
            }
            _ => {}
        }
        PickerEvent::Consumed
    }

    /// Move the selection down, clamped to the filtered list.
    fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }
}

/// The internal outcome of a [`Picker`] key press.
enum PickerEvent {
    Consumed,
    Close,
    Accept,
}

/// A modal overlay.
pub enum Overlay {
    /// Quick-open: pick a file to open.
    QuickOpen(Picker<PathBuf>),
    /// Command palette: pick a command to run.
    CommandPalette(Picker<Command>),
}

impl Overlay {
    /// Build a quick-open overlay over `(display, path)` pairs.
    #[must_use]
    pub fn quick_open(files: Vec<(String, PathBuf)>) -> Self {
        Self::QuickOpen(Picker::new("Go to File", files))
    }

    /// Build the command palette.
    #[must_use]
    pub fn command_palette() -> Self {
        let items = command::palette()
            .into_iter()
            .map(|cmd| (cmd.label().to_string(), cmd))
            .collect();
        Self::CommandPalette(Picker::new("Command Palette", items))
    }

    /// The overlay title.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::QuickOpen(p) => p.title(),
            Self::CommandPalette(p) => p.title(),
        }
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        match self {
            Self::QuickOpen(p) => p.query(),
            Self::CommandPalette(p) => p.query(),
        }
    }

    /// The visible row labels.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        match self {
            Self::QuickOpen(p) => p.rows(),
            Self::CommandPalette(p) => p.rows(),
        }
    }

    /// The per-row right-aligned hints (key chords), aligned with [`rows`](Self::rows).
    /// Quick-open rows have no hint.
    #[must_use]
    pub fn row_hints(&self) -> Vec<Option<String>> {
        match self {
            Self::QuickOpen(p) => p.rows().iter().map(|_| None).collect(),
            Self::CommandPalette(p) => p
                .values()
                .into_iter()
                .map(|cmd| keymap::hint_for(*cmd))
                .collect(),
        }
    }

    /// The selected row index.
    #[must_use]
    pub fn selected(&self) -> usize {
        match self {
            Self::QuickOpen(p) => p.selected(),
            Self::CommandPalette(p) => p.selected(),
        }
    }

    /// Handle a key, producing an [`OverlayEvent`] the app acts on.
    pub fn handle_key(&mut self, key: KeyEvent) -> OverlayEvent {
        match self {
            Self::QuickOpen(p) => match p.handle_key(key) {
                PickerEvent::Consumed => OverlayEvent::Consumed,
                PickerEvent::Close => OverlayEvent::Close,
                PickerEvent::Accept => p
                    .accepted()
                    .cloned()
                    .map_or(OverlayEvent::Close, OverlayEvent::AcceptFile),
            },
            Self::CommandPalette(p) => match p.handle_key(key) {
                PickerEvent::Consumed => OverlayEvent::Consumed,
                PickerEvent::Close => OverlayEvent::Close,
                PickerEvent::Accept => p
                    .accepted()
                    .copied()
                    .map_or(OverlayEvent::Close, OverlayEvent::AcceptCommand),
            },
        }
    }
}

/// Whether `needle` is a subsequence of `hay` (both already lowercased).
fn subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn subsequence_matches_in_order() {
        assert!(subsequence("ap", "app.rs"));
        assert!(subsequence("ars", "app.rs"));
        assert!(!subsequence("rsa", "app.rs"));
    }

    #[test]
    fn typing_filters_and_enter_accepts() {
        let files = vec![
            ("app.rs".to_string(), PathBuf::from("/x/app.rs")),
            ("main.rs".to_string(), PathBuf::from("/x/main.rs")),
        ];
        let mut overlay = Overlay::quick_open(files);
        // Type "ma" -> only main.rs remains.
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Char('m'))),
            OverlayEvent::Consumed
        ));
        let _ = overlay.handle_key(key(KeyCode::Char('a')));
        assert_eq!(overlay.rows(), vec!["main.rs"]);
        match overlay.handle_key(key(KeyCode::Enter)) {
            OverlayEvent::AcceptFile(p) => assert_eq!(p, PathBuf::from("/x/main.rs")),
            _ => unreachable!("enter accepts the single match"),
        }
    }

    #[test]
    fn esc_closes() {
        let mut overlay = Overlay::command_palette();
        assert!(matches!(
            overlay.handle_key(key(KeyCode::Esc)),
            OverlayEvent::Close
        ));
    }

    #[test]
    fn palette_accepts_a_command() {
        let mut overlay = Overlay::command_palette();
        // "quit" filters to the Quit command.
        for c in "quit".chars() {
            let _ = overlay.handle_key(key(KeyCode::Char(c)));
        }
        match overlay.handle_key(key(KeyCode::Enter)) {
            OverlayEvent::AcceptCommand(cmd) => assert_eq!(cmd, Command::Quit),
            _ => unreachable!("enter accepts the filtered command"),
        }
    }

    #[test]
    fn palette_rows_have_aligned_hints() {
        let overlay = Overlay::command_palette();
        assert_eq!(overlay.rows().len(), overlay.row_hints().len());
        // The Quit row carries its Ctrl+Q hint.
        let quit = overlay
            .rows()
            .iter()
            .position(|r| *r == Command::Quit.label())
            .expect("quit row present");
        assert_eq!(overlay.row_hints()[quit].as_deref(), Some("Ctrl+Q"));
    }
}
