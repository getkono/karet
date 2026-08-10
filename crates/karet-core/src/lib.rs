//! `karet-core` — the shared vocabulary for the karet TUI editor toolkit.
//!
//! This crate is intentionally tiny and dependency-light (no ratatui, no async
//! runtime): it defines the coordinate types and neutral data models that let the
//! other `karet-*` libraries interoperate, and that keep rendering widgets
//! decoupled from the engines that produce data. Producers (`karet-lsp`,
//! `karet-vcs`, …) *emit* these models; widgets (`karet-editor`,
//! `karet-widgets`) *render* them; the backend (`karet-session`) and the
//! application connect the two.
//!
//! With the optional **`serde`** feature every value type derives
//! `Serialize`/`Deserialize`, so the same models double as the wire vocabulary for
//! a future client-server split.
//!
//! Every type is exported flat from the crate root — `karet_core::LineCol`,
//! `karet_core::Diagnostic`, … — which is the one supported spelling.
//!
//! # Vocabulary map
//! - Blame: [`BlameAttribution`], [`BlameCommit`].
//! - Text coordinates: [`BytePos`], [`LineCol`], [`Span`], [`Range`], [`LineIndex`].
//! - Neutral models: [`Diagnostic`], [`Decoration`], [`Symbol`], [`CompletionItem`],
//!   [`Hover`], [`InlayHint`], [`SignatureHelp`], [`CodeAction`], ….
//! - Graph: [`GraphView`] and its nodes/edges, for visualizations.
//! - Edits & cursors: [`TextEdit`], [`Change`], [`AppliedEdit`], [`EditCause`],
//!   [`Selection`], [`CursorState`].
//! - Highlighting & folds: [`HighlightSpan`], [`Highlights`], [`FoldRegion`],
//!   [`FoldRegions`].
//! - Interop traits: [`SymbolProvider`].
//! - Theme vocabulary: [`TokenId`], [`StandardToken`], [`ThemeRole`], [`Emphasis`].
//! - Notifications: [`Notification`], [`NotificationKind`], [`severity_role`].

mod blame;
mod coord;
mod edit;
mod error;
mod graph;
mod highlight;
mod model;
mod notify;
mod provider;
mod token;
mod word;

pub use blame::BlameAttribution;
pub use blame::BlameCommit;
pub use coord::BytePos;
pub use coord::LineCol;
pub use coord::LineIndex;
pub use coord::Range;
pub use coord::Span;
pub use edit::AppliedEdit;
pub use edit::BytePoint;
pub use edit::Change;
pub use edit::CursorState;
pub use edit::EditCause;
pub use edit::Selection;
pub use edit::TextEdit;
pub use edit::WorkspaceEdit;
pub use error::CoreError;
pub use graph::GraphEdge;
pub use graph::GraphEdgeKind;
pub use graph::GraphNode;
pub use graph::GraphNodeKind;
pub use graph::GraphView;
pub use highlight::FoldRegion;
pub use highlight::FoldRegions;
pub use highlight::HighlightSpan;
pub use highlight::Highlights;
pub use model::CodeAction;
pub use model::CommandId;
pub use model::CompletionItem;
pub use model::CompletionKind;
pub use model::Decoration;
pub use model::DecorationKind;
pub use model::Diagnostic;
pub use model::DiagnosticTag;
pub use model::Hover;
pub use model::InlayHint;
pub use model::InlayHintKind;
pub use model::Location;
pub use model::Markup;
pub use model::MarkupKind;
pub use model::ParamInfo;
pub use model::RelatedInfo;
pub use model::Severity;
pub use model::Signature;
pub use model::SignatureHelp;
pub use model::Symbol;
pub use model::SymbolKind;
pub use notify::Notification;
pub use notify::NotificationId;
pub use notify::NotificationKind;
pub use notify::severity_role;
pub use provider::SymbolProvider;
pub use token::Emphasis;
pub use token::StandardToken;
pub use token::ThemeRole;
pub use token::TokenId;
pub use word::WordClass;
pub use word::is_word_char;
pub use word::word_class;
