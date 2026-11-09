//! `karet-editor` — the composable code-editor widget for karet.
//!
//! Combines the text engines (`karet-text`, `karet-theme`) into a ratatui editor
//! widget. By design it depends on **none** of the feature producers
//! (`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`): diagnostics, git
//! markers, breakpoints, inlay hints and code lenses arrive as `karet-core`
//! decorations supplied by the application from the backend's event stream.
//! Nor does it depend on the syntax engine: highlight spans, fold regions and
//! semantic blocks are the neutral `karet-core` shapes `karet-syntax` computes,
//! so a consumer of the widget never builds tree-sitter.

mod bracket;
mod conflict;
mod diagnostic;
pub mod editing;
mod hint;
mod state;
mod text;
mod view;
mod visual;

#[cfg(test)]
mod tests;

pub use bracket::BracketPair;
pub use bracket::bracket_decorations;
pub use bracket::bracket_pair_at;
pub use bracket::enclosing_pair;
pub use bracket::matching_pair;
pub use conflict::conflict_decorations;
pub use diagnostic::InlineDiagnostics;
pub use diagnostic::diagnostic_messages;
pub use diagnostic::diagnostic_signs;
pub use diagnostic::severity_sign;
use karet_core::LineCol;
use karet_core::Range;
use karet_text::TextBuffer;
use karet_text::words;
pub use karet_text::words::cluster_bounds;
pub use state::EditorState;
pub use state::Fold;
pub use state::GutterHit;
pub use state::GutterLane;
pub use state::Motion;
pub use state::resolve_folds;
pub use view::Editor;
pub use view::LineNumbers;
pub use view::RenderWhitespace;
pub use visual::WrappingIndent;
pub use visual::cluster_widths;

/// Deprecated path of [`karet_text::words::word_bounds`].
#[deprecated(note = "use `karet_text::words::word_bounds`")]
#[must_use]
pub fn word_bounds(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    words::word_bounds(buffer, pos)
}

/// Deprecated path of [`karet_text::words::class_run_at`].
#[deprecated(note = "use `karet_text::words::class_run_at`")]
#[must_use]
pub fn class_run_at(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    words::class_run_at(buffer, pos)
}

/// Deprecated path of [`karet_text::words::identifier_at`].
#[deprecated(note = "use `karet_text::words::identifier_at`")]
#[must_use]
pub fn identifier_at(buffer: &TextBuffer, pos: LineCol) -> Option<(LineCol, LineCol)> {
    words::identifier_at(buffer, pos)
}

/// Deprecated path of [`karet_text::words::previous_word_boundary`].
#[deprecated(note = "use `karet_text::words::previous_word_boundary`")]
#[must_use]
pub fn previous_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    words::previous_word_boundary(buffer, pos)
}

/// Deprecated path of [`karet_text::words::next_word_boundary`].
#[deprecated(note = "use `karet_text::words::next_word_boundary`")]
#[must_use]
pub fn next_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    words::next_word_boundary(buffer, pos)
}

/// Deprecated path of [`karet_text::words::line_span`].
#[deprecated(note = "use `karet_text::words::line_span`")]
#[must_use]
pub fn line_span(buffer: &TextBuffer, line: u32) -> (LineCol, LineCol) {
    words::line_span(buffer, line)
}

/// The text within `range`, sliced from `source` (the buffer's full text) using
/// byte offsets derived from `buffer`. `None` if the range cannot be resolved.
///
/// Deprecated: `source` must equal the buffer's text for the answer to mean
/// anything, and [`TextBuffer::slice_range`] reads the range off the buffer
/// itself without that precondition.
#[deprecated(note = "use `karet_text::TextBuffer::slice_range`")]
#[must_use]
pub fn selection_text(buffer: &TextBuffer, source: &str, range: Range) -> Option<String> {
    let start = buffer.line_col_to_byte(range.start).ok()?.0;
    let end = buffer.line_col_to_byte(range.end).ok()?.0;
    source.get(start..end).map(str::to_string)
}
