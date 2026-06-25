//! `karet-editor` — the composable code-editor widget for karet.
//!
//! Combines the text engines (`karet-text`, `karet-syntax`, `karet-theme`) into a
//! ratatui editor widget. By design it depends on **none** of the feature
//! producers (`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`): diagnostics,
//! git markers, breakpoints, inlay hints and code lenses arrive as `karet-core`
//! decorations supplied by the application from the backend's event stream.
//!
//! This is the implementation *skeleton*: the [`Editor`] builder (the data joint)
//! and [`EditorState`] are defined; the painting/input logic is filled in
//! separately.

use karet_core::{Decoration, Diagnostic, InlayHint, LineCol, Range};
use karet_syntax::{FoldRegions, Highlights};
use karet_text::TextBuffer;
use karet_theme::Theme;

/// The persistent, per-view editor state (scroll, viewport, fold state).
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    /// The first visible buffer line (top of the viewport).
    pub scroll_line: u32,
}

impl EditorState {
    /// Create a fresh editor state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll so that `pos` is within the viewport.
    pub fn scroll_to(&mut self, pos: LineCol) {
        let _ = pos;
        todo!()
    }

    /// The currently-visible line range.
    #[must_use]
    pub fn viewport(&self) -> Range {
        todo!()
    }
}

/// The editor widget: a builder over the buffer and the (borrowed) data layers
/// the application supplies. Render it as a ratatui [`StatefulWidget`] with an
/// [`EditorState`].
///
/// [`StatefulWidget`]: ratatui::widgets::StatefulWidget
pub struct Editor<'a> {
    buffer: &'a TextBuffer,
    highlights: Option<&'a Highlights>,
    theme: Option<&'a Theme>,
    decorations: &'a [Decoration],
    diagnostics: &'a [Diagnostic],
    inlay_hints: &'a [InlayHint],
    folds: Option<&'a FoldRegions>,
}

impl<'a> Editor<'a> {
    /// Start building an editor over `buffer`.
    #[must_use]
    pub fn new(buffer: &'a TextBuffer) -> Self {
        Self {
            buffer,
            highlights: None,
            theme: None,
            decorations: &[],
            diagnostics: &[],
            inlay_hints: &[],
            folds: None,
        }
    }

    /// Supply syntax highlight spans.
    #[must_use]
    pub fn highlights(mut self, highlights: &'a Highlights) -> Self {
        self.highlights = Some(highlights);
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Supply decorations (VCS markers, breakpoints, search highlights, …).
    #[must_use]
    pub fn decorations(mut self, decorations: &'a [Decoration]) -> Self {
        self.decorations = decorations;
        self
    }

    /// Supply diagnostics (from LSP, spell-check, …).
    #[must_use]
    pub fn diagnostics(mut self, diagnostics: &'a [Diagnostic]) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// Supply inlay hints.
    #[must_use]
    pub fn inlay_hints(mut self, inlay_hints: &'a [InlayHint]) -> Self {
        self.inlay_hints = inlay_hints;
        self
    }

    /// Supply fold regions.
    #[must_use]
    pub fn folds(mut self, folds: &'a FoldRegions) -> Self {
        self.folds = Some(folds);
        self
    }
}

impl ratatui::widgets::StatefulWidget for Editor<'_> {
    type State = EditorState;

    fn render(
        self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        state: &mut EditorState,
    ) {
        let _ = (
            self.buffer,
            self.highlights,
            self.theme,
            self.decorations,
            self.diagnostics,
            self.inlay_hints,
            self.folds,
            area,
            buf,
            state,
        );
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_builder_collects_layers() {
        let buffer = TextBuffer::from_text("fn main() {}");
        let _editor = Editor::new(&buffer).diagnostics(&[]).decorations(&[]);
        assert_eq!(EditorState::new().scroll_line, 0);
    }
}
