//! `karet-core` — the shared vocabulary for the karet TUI editor toolkit.
//!
//! This crate is intentionally tiny and dependency-light (no ratatui, no async
//! runtime): it defines the coordinate types and neutral data models that let the
//! other `karet-*` libraries interoperate, and that keep rendering widgets
//! decoupled from the engines that produce data. Producers (`karet-lsp`,
//! `karet-vcs`, `karet-dap`, …) *emit* these models; widgets (`karet-editor`,
//! `karet-widgets`) *render* them; the backend (`karet-session`) and the
//! application connect the two.
//!
//! With the optional **`serde`** feature every value type derives
//! `Serialize`/`Deserialize`, so the same models double as the wire vocabulary for
//! a future client-server split.
//!
//! # Modules
//! - [`blame`] — neutral current-buffer attribution models.
//! - [`geometry`] — `Point`, `Size`, `Rect`, offsets, and the [`clamp`] helper.
//! - [`coord`] — text coordinates: `BytePos`, `CharPos`, `LineCol`, `Range`, `Span`.
//! - [`model`] — neutral models: diagnostics, decorations, symbols, completion, hover, ….
//! - [`graph`] — a neutral directed-graph model ([`GraphView`]) for visualizations.
//! - [`edit`] — neutral edit/selection types (`TextEdit`, `Change`, `Selection`, `CursorState`).
//! - [`provider`] — interop traits such as [`SymbolProvider`].
//! - [`token`] — semantic [`TokenId`] / [`ThemeRole`] vocabulary shared by syntax & theme.

pub mod blame;
pub mod coord;
pub mod edit;
pub mod error;
pub mod geometry;
pub mod graph;
pub mod model;
pub mod notify;
pub mod provider;
pub mod token;

pub use blame::BlameAttribution;
pub use blame::BlameCommit;
pub use coord::BytePos;
pub use coord::CharPos;
pub use coord::LineCol;
pub use coord::PositionEncoding;
pub use coord::Range;
pub use coord::Span;
pub use edit::Change;
pub use edit::CursorState;
pub use edit::Selection;
pub use edit::TextEdit;
pub use edit::WorkspaceEdit;
pub use error::CoreError;
pub use geometry::Offset;
pub use geometry::Point;
pub use geometry::Rect;
pub use geometry::Size;
pub use geometry::clamp;
pub use graph::GraphEdge;
pub use graph::GraphEdgeKind;
pub use graph::GraphNode;
pub use graph::GraphNodeKind;
pub use graph::GraphView;
pub use model::CodeAction;
pub use model::CodeLens;
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
pub use model::UnderlineStyle;
pub use notify::Notification;
pub use notify::NotificationId;
pub use notify::NotificationKind;
pub use notify::severity_role;
pub use provider::DecorationSource;
pub use provider::DiagnosticSource;
pub use provider::SymbolProvider;
pub use token::StandardToken;
pub use token::ThemeRole;
pub use token::TokenId;
