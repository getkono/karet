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

mod conflict;
mod state;
mod text;
mod view;
mod visual;

#[cfg(test)]
mod tests;

pub use conflict::conflict_decorations;
use karet_core::BytePos;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::Severity;
use karet_core::ThemeRole;
use karet_core::TokenId;
use karet_syntax::HighlightSpan;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlock;
use karet_syntax::SemanticBlocks;
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
pub use text::word_bounds;
pub use view::Editor;
