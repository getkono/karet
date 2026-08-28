//! The vocabulary spoken between the session actor and its server tasks.
//!
//! [`ServerCmd`] travels down to one per-language task; [`LspUpdate`] travels
//! back. Both are plain data on purpose: the task owns the `LspClient` and the
//! protocol, while position conversion (LSP UTF-16 ↔ buffer UTF-32) and event
//! emission happen on the actor, where the buffer lives.

use std::path::PathBuf;

use karet_core::CompletionItem;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;

use crate::api::DocumentId;
use crate::api::LanguageServerId;
use crate::api::LanguageServerRuntimeState;
use crate::api::RequestId;

/// A command for one per-language server task.
pub(crate) enum ServerCmd {
    /// Forward `textDocument/didOpen`.
    DidOpen {
        /// The document path.
        path: PathBuf,
        /// LSP `languageId` for this document.
        language: String,
        /// The document version.
        version: i32,
        /// The full text.
        text: String,
    },
    /// Forward `textDocument/didChange` (full text, debounced).
    DidChange {
        /// The document path.
        path: PathBuf,
        /// The document version.
        version: i32,
        /// The full text after the change.
        text: String,
    },
    /// Forward `textDocument/didClose`.
    DidClose {
        /// The document path.
        path: PathBuf,
    },
    /// Forward `textDocument/didSave`.
    DidSave {
        /// Saved document path.
        path: PathBuf,
        /// Current full text, supplied for servers that request it.
        text: String,
    },
    /// Request completions; always answered with an [`LspUpdate::Completions`].
    Completion {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
        /// The position, already converted to UTF-16 columns.
        position: LineCol,
    },
    /// Request the document's structural symbols.
    DocumentSymbols {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
    },
    /// Request hover information.
    Hover {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
    },
    /// Request definition locations.
    Definition {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
    },
    WorkspaceSymbols {
        request: RequestId,
        query: String,
    },
    Rename {
        request: RequestId,
        path: PathBuf,
        position: LineCol,
        new_name: String,
    },
    Formatting {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
    },
}

/// A result flowing from a server task back to the session actor.
pub(crate) enum LspUpdate {
    /// A server-pushed status line (jdtls `language/status`-style), for the
    /// status bar while a heavyweight server imports/indexes.
    ServerStatus {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The language the server serves.
        server: String,
        /// The human-readable status message.
        message: String,
    },
    /// Completion items answering a [`ServerCmd::Completion`] (ranges still in
    /// UTF-16 columns; the session converts them against the buffer).
    Completions {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped items.
        items: Vec<CompletionItem>,
    },
    /// Document symbols answering a [`ServerCmd::DocumentSymbols`] request. Ranges
    /// remain in UTF-16 until the session adopts the update.
    Symbols {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped symbol tree.
        symbols: Vec<Symbol>,
    },
    /// Hover response in UTF-16 coordinates.
    Hover {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        hover: Option<Hover>,
    },
    /// Definition response in UTF-16 coordinates.
    Definitions {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        locations: Vec<Location>,
    },
    WorkspaceSymbols {
        generation: u64,
        request: RequestId,
        symbols: Vec<Symbol>,
    },
    WorkspaceEdit {
        generation: u64,
        request: RequestId,
        edit: WorkspaceEdit,
    },
    Formatting {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        edits: Vec<TextEdit>,
    },
    /// A complete server diagnostic layer for one file.
    Diagnostics {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// Provider/root identity whose diagnostic layer is replaced.
        server: String,
        /// File whose LSP diagnostic layer is replaced.
        path: PathBuf,
        /// LSP document version, when the server supplied it.
        version: Option<i32>,
        /// Diagnostics in UTF-16 coordinates.
        diagnostics: Vec<Diagnostic>,
    },
    /// The server binary could not be started (reported once per language).
    SpawnFailed {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The language the server was for.
        language: String,
        /// The executable that failed to start.
        command: String,
    },
    /// A launch preflight failed with a specific diagnosis (reported once per
    /// generation); the server is not spawned.
    PreflightFailed {
        /// The manager generation the preflight ran under.
        generation: u64,
        /// The human-readable diagnosis (what is missing and how to fix it).
        message: String,
    },
    /// A running server's connection closed (reported once per language).
    ServerDied {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The language whose server died.
        language: String,
    },
    /// A built-in provider karet can install is locally absent. No network
    /// operation was attempted.
    InstallRequired {
        /// The manager generation that observed the missing installation.
        generation: u64,
        /// Missing managed provider.
        server: LanguageServerId,
        /// The language whose document wanted it — the key its per-language
        /// enable flag is stored under.
        language: String,
    },
    /// A built-in provider is locally absent and karet cannot install it.
    ///
    /// Separate from [`LspUpdate::InstallRequired`] because the two ask
    /// completely different things of the user: one offers a download, the
    /// other explains a toolchain they have to set up themselves. Offering the
    /// download for these providers produced a prompt whose install always
    /// failed.
    ManualInstallRequired {
        /// The manager generation that observed the missing installation.
        generation: u64,
        /// The provider the user must supply.
        server: LanguageServerId,
        /// The executable karet looked for.
        command: String,
        /// Why karet will not install it, from
        /// [`manual_install_reason`](crate::lsp_registry::manual_install_reason).
        reason: String,
    },
    /// A provider/root connection changed lifecycle state.
    RuntimeState {
        generation: u64,
        server: LanguageServerId,
        root: PathBuf,
        state: LanguageServerRuntimeState,
        error: Option<String>,
    },
}
