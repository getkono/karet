//! Centered modal overlays: quick-open (go to file) and the command palette.
//!
//! Both are a [`Picker`] over labeled items with an incremental subsequence filter.
//! (The richer `karet-fuzzy` ranking / `karet-widgets::Picker` widget is a future
//! home; this keeps the skeleton dependency-light.)

use std::path::PathBuf;

use crate::command::Command;
use crate::command::{self};
use crate::keymap;

/// The outcome of accepting the highlighted overlay row.
pub enum OverlayEvent {
    /// Nothing was highlighted; dismiss the overlay.
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

    /// Move the selection up.
    fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down, clamped to the filtered list.
    fn select_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    /// Append a character to the query and refilter.
    fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Remove the last query character and refilter.
    fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }
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
                .map(|cmd| keymap::hint_for(*cmd, keymap::ChordStyle::Verbose))
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

    /// Move the selection up.
    pub fn select_up(&mut self) {
        match self {
            Self::QuickOpen(p) => p.select_up(),
            Self::CommandPalette(p) => p.select_up(),
        }
    }

    /// Move the selection down.
    pub fn select_down(&mut self) {
        match self {
            Self::QuickOpen(p) => p.select_down(),
            Self::CommandPalette(p) => p.select_down(),
        }
    }

    /// Append a character to the query.
    pub fn push_char(&mut self, c: char) {
        match self {
            Self::QuickOpen(p) => p.push_char(c),
            Self::CommandPalette(p) => p.push_char(c),
        }
    }

    /// Remove the last query character.
    pub fn pop_char(&mut self) {
        match self {
            Self::QuickOpen(p) => p.pop_char(),
            Self::CommandPalette(p) => p.pop_char(),
        }
    }

    /// The outcome of accepting the highlighted row (open a file / run a command),
    /// or [`OverlayEvent::Close`] when nothing is highlighted.
    #[must_use]
    pub fn accept(&self) -> OverlayEvent {
        match self {
            Self::QuickOpen(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptFile),
            Self::CommandPalette(p) => p
                .accepted()
                .copied()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptCommand),
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

    #[test]
    fn subsequence_matches_in_order() {
        assert!(subsequence("ap", "app.rs"));
        assert!(subsequence("ars", "app.rs"));
        assert!(!subsequence("rsa", "app.rs"));
    }

    #[test]
    fn typing_filters_and_accept_opens() {
        let files = vec![
            ("app.rs".to_string(), PathBuf::from("/x/app.rs")),
            ("main.rs".to_string(), PathBuf::from("/x/main.rs")),
        ];
        let mut overlay = Overlay::quick_open(files);
        // Type "ma" -> only main.rs remains.
        overlay.push_char('m');
        overlay.push_char('a');
        assert_eq!(overlay.rows(), vec!["main.rs"]);
        match overlay.accept() {
            OverlayEvent::AcceptFile(p) => assert_eq!(p, PathBuf::from("/x/main.rs")),
            _ => unreachable!("accept opens the single match"),
        }
    }

    #[test]
    fn palette_accepts_a_command() {
        let mut overlay = Overlay::command_palette();
        // "quit" filters to the Quit command.
        for c in "quit".chars() {
            overlay.push_char(c);
        }
        match overlay.accept() {
            OverlayEvent::AcceptCommand(cmd) => assert_eq!(cmd, Command::Quit),
            _ => unreachable!("accept runs the filtered command"),
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
