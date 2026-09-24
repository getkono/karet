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
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use karet_lsp::Indentation;

use super::slot::SlotKey;
use super::slot::SlotToken;
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
    /// Request inlay hints for a range.
    InlayHints {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
        /// The range, already converted to UTF-16 columns.
        range: Range,
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
        /// The buffer's resolved `editor.tabSize` / `editor.insertSpaces`.
        ///
        /// Carried per request rather than read from the manager's settings:
        /// the setting is per *language*, resolved against the document's
        /// selector, and the server task serving the language has no document
        /// to resolve it from. A server that honours it reindents the whole
        /// file, so a default here is not a neutral choice.
        indentation: Indentation,
    },
}

/// A result flowing from a server task back to the session actor.
pub(crate) enum LspUpdate {
    /// A server-pushed status line (jdtls `language/status`-style), for the
    /// status bar while a heavyweight server imports/indexes.
    ServerStatus {
        /// The slot this report is about, and the incarnation making it.
        token: SlotToken,
        /// The slot the reporting task holds.
        key: SlotKey,
        /// The human-readable status message.
        message: String,
    },
    /// The server asked for every inlay hint it answered to be re-fetched
    /// (`workspace/inlayHint/refresh`).
    ///
    /// Fenced on the slot like the other reports a task makes about itself: a
    /// retired task's refresh describes a server nobody is asking any more.
    InlayHintsRefresh {
        /// The incarnation of the slot whose server asked.
        token: SlotToken,
        /// The slot whose server asked.
        key: SlotKey,
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
    /// Inlay hints answering a [`ServerCmd::InlayHints`] request. Positions
    /// remain in UTF-16 until the session adopts the update.
    InlayHints {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped hints.
        hints: Vec<InlayHint>,
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
    /// A request was refused because the server never offered `feature`.
    ///
    /// Sent only for requests a user asks for by hand, and always *before*
    /// the request's own empty answer, so the client can explain the empty
    /// answer rather than report it as nothing found.
    Unsupported {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The refused request.
        request: RequestId,
        /// The provider that was asked.
        server: LanguageServerId,
        /// What it does not offer.
        feature: ServerFeature,
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
        /// Whether a language server actually formatted this file.
        ///
        /// The question the session asks, rather than what the server claims it
        /// can do. Distinct from empty `edits`: a server that formatted and
        /// found nothing to change has done its job and this is `true`, while a
        /// server that never offered the method, was not reachable, or failed
        /// the request has formatted nothing — and the session is then free to
        /// use its own formatter instead.
        formatted: bool,
        edits: Vec<TextEdit>,
    },
    /// A complete server diagnostic layer for one file.
    Diagnostics {
        /// The incarnation of the slot that published these.
        token: SlotToken,
        /// Provider/root identity whose diagnostic layer is replaced.
        server: SlotKey,
        /// File whose LSP diagnostic layer is replaced.
        path: PathBuf,
        /// LSP document version, when the server supplied it.
        version: Option<i32>,
        /// Diagnostics in UTF-16 coordinates.
        diagnostics: Vec<Diagnostic>,
    },
    /// The server binary could not be started (reported once per language).
    SpawnFailed {
        /// The incarnation of the slot that failed to start.
        token: SlotToken,
        /// The slot that failed to start.
        ///
        /// Render `key.provider`, never the key: the key is
        /// `provider@/absolute/repository/root`, and printing it whole put a
        /// full path into the user's notification.
        key: SlotKey,
        /// The executable and arguments karet ran.
        command: String,
        /// The most specific one-line reason available, which for a server that
        /// ran at all is usually its own last line of stderr.
        reason: String,
        /// Whether retrying could ever help.
        permanent: bool,
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
        /// The incarnation of the slot that died.
        token: SlotToken,
        /// The slot whose server died.
        key: SlotKey,
    },
    /// A document-sync command never reached its server.
    ///
    /// Deliberately not a [`Self::RuntimeState`]: the server is running fine, it
    /// is merely behind, and publishing `Retrying` for it wrote a wrong state into
    /// the manager that nothing ever corrected -- the task had not transitioned, so
    /// it never re-reported `Running`, and the badge stayed a warning for the rest
    /// of the session while every request was answered normally.
    SyncFailed {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The provider whose copy of the document is now behind.
        server: LanguageServerId,
        /// What went wrong, phrased for the user.
        reason: String,
    },
    /// A provider stayed down past the grace period: drop what it published.
    ///
    /// Diagnostics used to be inserted and never removed, so a crashed server's
    /// squiggles outlived it. Sent by a task that is still live -- so still of the
    /// current generation -- and only after the grace window, so a reconnect inside
    /// it does not make every marker flicker off and back on.
    ///
    /// Scoped to what a live task can say about itself: a *retired* task's layer
    /// is cleared by the manager, synchronously, as part of retiring it.
    DiagnosticsCleared {
        /// The incarnation of the slot asking for the clear.
        token: SlotToken,
        /// The diagnostic layer to drop, keyed exactly as it was published.
        ///
        /// Being the slot's own key, this already scopes the clear to the one
        /// instance that died: a provider running at two repository roots keeps
        /// the markers published by the root that is still healthy.
        server: SlotKey,
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
        /// The incarnation of the slot reporting.
        token: SlotToken,
        /// The slot reporting about itself. Both halves the client needs --
        /// provider and root -- come from it, and it is also what the manager
        /// looks the slot up by, so a report cannot be filed against a different
        /// instance than the one that sent it.
        key: SlotKey,
        state: LanguageServerRuntimeState,
        error: Option<String>,
    },
}
