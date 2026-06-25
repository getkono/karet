//! `karet-lsp` — an async Language Server Protocol client for karet.
//!
//! Headless: connects to language servers (stdio/TCP) and turns their responses
//! into neutral `karet-core` models (`Diagnostic`, `Symbol`, `CompletionItem`,
//! `Hover`, `InlayHint`, …), implementing [`SymbolProvider`]. Usable from a CLI or
//! a non-ratatui UI. (The ratatui completion/hover popups live in `karet-widgets`,
//! which renders these models, so this crate stays free of UI dependencies.)
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! async-lsp transport and feature logic are filled in separately.

use karet_core::{
    CompletionItem, Diagnostic, Hover, InlayHint, LineCol, Location, Range, Symbol, SymbolProvider,
    WorkspaceEdit,
};
use std::path::{Path, PathBuf};
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
