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
//! - [`geometry`] — `Point`, `Size`, `Rect`, offsets, and the [`clamp`] helper.
//! - [`coord`] — text coordinates: `BytePos`, `CharPos`, `LineCol`, `Range`, `Span`.
//! - [`model`] — neutral models: diagnostics, decorations, symbols, completion, hover, ….
//! - [`edit`] — neutral edit/selection types (`TextEdit`, `Change`, `Selection`, `CursorState`).
//! - [`provider`] — interop traits such as [`SymbolProvider`].
//! - [`token`] — semantic [`TokenId`] / [`ThemeRole`] vocabulary shared by syntax & theme.

pub mod coord;
pub mod edit;
pub mod error;
pub mod geometry;
pub mod model;
pub mod provider;
pub mod token;

pub use coord::{BytePos, CharPos, LineCol, PositionEncoding, Range, Span};
pub use edit::{Change, CursorState, Selection, TextEdit, WorkspaceEdit};
pub use error::CoreError;
pub use geometry::{Offset, Point, Rect, Size, clamp};
pub use model::{
    CodeAction, CodeLens, CommandId, CompletionItem, CompletionKind, Decoration, DecorationKind,
    Diagnostic, DiagnosticTag, Hover, InlayHint, InlayHintKind, Location, Markup, MarkupKind,
    ParamInfo, RelatedInfo, Severity, Signature, SignatureHelp, Symbol, SymbolKind, UnderlineStyle,
};
pub use provider::{DecorationSource, DiagnosticSource, SymbolProvider};
pub use token::{StandardToken, ThemeRole, TokenId};
