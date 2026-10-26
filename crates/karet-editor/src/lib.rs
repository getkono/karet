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
pub use diagnostic::severity_role;
pub use diagnostic::severity_sign;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::HighlightSpan;
use karet_core::Highlights;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::SemanticBlock;
use karet_core::SemanticBlocks;
use karet_core::Severity;
use karet_core::ThemeRole;
use karet_core::TokenId;
use karet_core::VirtualLine;
use karet_core::VirtualText;
use karet_core::VirtualTextPlacement;
use karet_text::TextBuffer;
use karet_theme::Rgba;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;
pub use state::EditorState;
pub use state::Fold;
pub use state::GutterHit;
pub use state::GutterLane;
pub use state::Motion;
pub use state::resolve_folds;
pub use text::class_run_at;
pub use text::cluster_bounds;
pub use text::identifier_at;
pub use text::line_span;
pub use text::next_word_boundary;
pub use text::previous_word_boundary;
pub use text::selection_text;
pub use text::word_bounds;
pub use view::Editor;
pub use view::LineNumbers;
pub use view::RenderWhitespace;
use view::gutter_width;
pub use visual::WrappingIndent;
pub use visual::cluster_widths;
