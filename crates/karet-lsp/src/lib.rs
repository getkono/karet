//! `karet-lsp` — an async Language Server Protocol client for karet.
//!
//! Headless: connects to language servers (stdio/TCP) and turns their responses
//! into neutral `karet-core` models (`Diagnostic`, `Symbol`, `CompletionItem`,
//! `Hover`, `InlayHint`, …), implementing [`SymbolProvider`]. Usable from a CLI or
//! a non-ratatui UI. (The ratatui completion/hover popups live in `karet-widgets`,
//! which renders these models, so this crate stays free of UI dependencies.)
//!
//! This is the implementation *skeleton*, but the **contract is complete**: every
//! method karet will ever call — lifecycle, the full document-sync set
//! (`did_open`/`did_change`/`did_save`/`did_close`), and every request — has its
//! final signature, so wiring the `async-lsp` transport later is pure body fill-in,
//! never an API change. Two seams are deliberately pinned now so nothing downstream
//! has to be retrofitted:
//!
//! - **`did_change` is driven by the same [`TextEdit`]s the editor applies**, so the
//!   session forwards an incremental change at its single apply site.
//! - **Position encoding is translated at the edge.** karet is internally UTF-32;
//!   LSP defaults to UTF-16. The conversions live on `karet_text::TextBuffer`
//!   (`line_col_to_utf16` / `utf16_to_line_col`); this client applies them when the
//!   negotiated encoding is not `utf-8`.

use std::path::Path;
use std::path::PathBuf;

use karet_core::CodeAction;
use karet_core::CompletionItem;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Range;
use karet_core::SignatureHelp;
use karet_core::Symbol;
use karet_core::SymbolProvider;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use tokio::sync::broadcast;

/// Errors produced by the LSP client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LspError {
    /// The language server process could not be spawned.
    #[error("failed to spawn language server")]
    Spawn,
    /// The server responded with an error.
    #[error("language server error: {0}")]
    Server(String),
    /// A request timed out.
    #[error("request timed out")]
    Timeout,
}

/// How to launch a language server.
#[derive(Clone, Debug)]
pub struct LspSpec {
    /// The server executable.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Language identifiers this server handles (e.g. `"rust"`).
    pub languages: Vec<String>,
}

/// An async client for a single language server.
pub struct LspClient {}

impl LspClient {
    /// Spawn and initialize the server described by `spec`, rooted at `root`.
    ///
    /// # Errors
    /// Returns [`LspError::Spawn`] if the process cannot start.
    pub async fn spawn(spec: LspSpec, root: &Path) -> Result<Self, LspError> {
        let _ = (spec, root);
        todo!()
    }

    /// Shut the server down (`shutdown` request + `exit` notification) and await the
    /// process.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] if the shutdown handshake fails.
    pub async fn shutdown(self) -> Result<(), LspError> {
        todo!()
    }

    // --- document sync (the seam the editing path drives) -----------------

    /// Notify the server that `doc` opened, with its `language_id`, `version` and
    /// full `text`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] if the notification cannot be sent.
    pub async fn did_open(
        &self,
        doc: &Path,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        let _ = (doc, language_id, version, text);
        todo!()
    }

    /// Notify the server of an incremental change to `doc`, derived from the same
    /// [`TextEdit`]s the editor just applied (translated to the negotiated encoding).
    ///
    /// # Errors
    /// Returns [`LspError::Server`] if the notification cannot be sent.
    pub async fn did_change(
        &self,
        doc: &Path,
        version: i32,
        edits: &[TextEdit],
    ) -> Result<(), LspError> {
        let _ = (doc, version, edits);
        todo!()
    }

    /// Notify the server that `doc` was saved (optionally including its text).
    ///
    /// # Errors
    /// Returns [`LspError::Server`] if the notification cannot be sent.
    pub async fn did_save(&self, doc: &Path, text: Option<&str>) -> Result<(), LspError> {
        let _ = (doc, text);
        todo!()
    }

    /// Notify the server that `doc` was closed.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] if the notification cannot be sent.
    pub async fn did_close(&self, doc: &Path) -> Result<(), LspError> {
        let _ = doc;
        todo!()
    }

    /// Request completions at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn completion(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<CompletionItem>, LspError> {
        let _ = (doc, pos);
        todo!()
    }

    /// Request hover information at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn hover(&self, doc: &Path, pos: LineCol) -> Result<Option<Hover>, LspError> {
        let _ = (doc, pos);
        todo!()
    }

    /// Request the document symbols of `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn document_symbols(&self, doc: &Path) -> Result<Vec<Symbol>, LspError> {
        let _ = doc;
        todo!()
    }

    /// Search workspace symbols matching `query`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<Symbol>, LspError> {
        let _ = query;
        todo!()
    }

    /// Resolve the definition location(s) of the symbol at `pos`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn definition(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        let _ = (doc, pos);
        todo!()
    }

    /// Request inlay hints within `range`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn inlay_hints(&self, doc: &Path, range: Range) -> Result<Vec<InlayHint>, LspError> {
        let _ = (doc, range);
        todo!()
    }

    /// Rename the symbol at `pos` to `new_name`, returning the edits to apply.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn rename(
        &self,
        doc: &Path,
        pos: LineCol,
        new_name: &str,
    ) -> Result<WorkspaceEdit, LspError> {
        let _ = (doc, pos, new_name);
        todo!()
    }

    /// Request signature help at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn signature_help(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Option<SignatureHelp>, LspError> {
        let _ = (doc, pos);
        todo!()
    }

    /// Request code actions available for `range` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn code_action(&self, doc: &Path, range: Range) -> Result<Vec<CodeAction>, LspError> {
        let _ = (doc, range);
        todo!()
    }

    /// Request whole-document formatting edits for `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn formatting(&self, doc: &Path) -> Result<Vec<TextEdit>, LspError> {
        let _ = doc;
        todo!()
    }

    /// Request formatting edits for `range` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn range_formatting(
        &self,
        doc: &Path,
        range: Range,
    ) -> Result<Vec<TextEdit>, LspError> {
        let _ = (doc, range);
        todo!()
    }

    /// Subscribe to server-pushed diagnostics, keyed by file path.
    #[must_use]
    pub fn diagnostics(&self) -> broadcast::Receiver<(PathBuf, Vec<Diagnostic>)> {
        todo!()
    }
}

/// A document's resolved symbols, cached so they can be borrowed as a
/// [`SymbolProvider`] by widgets that render an outline/breadcrumbs.
pub struct DocumentSymbols {
    symbols: Vec<Symbol>,
}

impl DocumentSymbols {
    /// Wrap a resolved symbol list.
    #[must_use]
    pub fn new(symbols: Vec<Symbol>) -> Self {
        Self { symbols }
    }
}

impl SymbolProvider for DocumentSymbols {
    fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_wraps_symbols() {
        let ds = DocumentSymbols::new(Vec::new());
        assert!(ds.symbols().is_empty());
    }

    #[test]
    fn error_displays() {
        assert_eq!(LspError::Timeout.to_string(), "request timed out");
    }
}
