//! `karet-lsp` — an async Language Server Protocol client for karet.
//!
//! Headless: connects to language servers over stdio and turns their responses
//! into neutral `karet-core` models (`Diagnostic`, `Symbol`, `CompletionItem`,
//! `Hover`, `InlayHint`, …). Usable from a CLI or
//! a non-ratatui UI. (The ratatui completion/hover popups live in `karet-widgets`,
//! which renders these models, so this crate stays free of UI dependencies.)
//!
//! The transport is the shared `karet-jsonrpc` correlation actor over
//! `Content-Length` framing, on generic async I/O: [`LspClient::spawn`] wraps a
//! child process's stdio, and [`LspClient::connect`] accepts any
//! `AsyncRead`/`AsyncWrite` pair — the seam the in-memory (`tokio::io::duplex`)
//! tests and embedders use. A reader task correlates responses by id, broadcasts
//! pushed diagnostics, and answers the few server→client requests a headless
//! client must not leave hanging (`workspace/configuration`,
//! `client/registerCapability`, `window/workDoneProgress/create`).
//!
//! Three protocol choices are deliberate and documented here once:
//!
//! - **Positions cross this API in UTF-16.** The client negotiates the LSP-default
//!   `utf-16` position encoding and stays faithful to it: every [`LineCol`] and
//!   [`Range`] passed to or returned from this crate counts columns in UTF-16 code
//!   units. karet is internally UTF-32; the conversions live on
//!   `karet_text::TextBuffer` (`line_col_to_utf16` / `utf16_to_line_col`) and are
//!   applied by the consumer that owns the text (karet-session), not here.
//! - **Document sync is full-text.** [`LspClient::did_change`] sends the whole
//!   document on every change — the simplest correct v1; incremental sync is a
//!   possible later optimization behind the same method.
//! - **Snippets are not advertised** (`completionItem.snippetSupport: false`), so
//!   servers send plain-text completions; snippet syntax that leaks through anyway
//!   is degraded to plain text at the completion mapping.
//!
//! Transport, lifecycle, document sync, diagnostics, completion, navigation,
//! symbols, inlay hints, rename, signature help, code actions, and document/range
//! formatting are implemented as typed, non-panicking operations.

/// `Content-Length` message framing (the LSP base protocol).
///
/// Re-exported from [`karet_jsonrpc::framing::content_length`], which is where
/// the implementation now lives; the `karet_lsp::codec` path is kept because it
/// is this crate's published surface.
pub use karet_jsonrpc::framing::content_length as codec;
pub use launch::ExitReport;
pub use launch::LaunchCause;
pub use launch::LaunchFailure;

mod conn;
mod convert;
mod launch;
mod snippet;
mod uri;

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;

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
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::sync::broadcast;

/// Errors produced by the LSP client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LspError {
    /// The language server process could not be spawned.
    #[deprecated(
        since = "0.6.0",
        note = "superseded by LspError::Launch, which carries why the launch failed"
    )]
    #[error("failed to spawn language server")]
    Spawn,
    /// The language server could not be launched, with what is known about why.
    #[error("{0}")]
    Launch(Box<LaunchFailure>),
    /// The server responded with an error.
    #[error("language server error: {0}")]
    Server(String),
    /// A request timed out.
    #[error("request timed out")]
    Timeout,
    /// A message could not be encoded, decoded, or otherwise violated the
    /// protocol (bad framing, malformed JSON, an invalid URI).
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The connection to the server closed (process exit or stream EOF).
    #[error("connection to the language server closed")]
    Closed,
}

/// How long a failed handshake waits for the child to exit before reporting.
///
/// Only spent on a launch that already failed, and only to learn whether the
/// process died -- a server that is still running is a protocol failure, and
/// must not be waited on.
const CHILD_EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

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

/// One complete diagnostic publication from a language server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedDiagnostics {
    /// File whose diagnostic layer is replaced by this publication.
    pub path: PathBuf,
    /// Document version supplied by the server, when available.
    pub version: Option<i32>,
    /// Complete diagnostics for this server and file.
    pub diagnostics: Vec<Diagnostic>,
}

/// A server-initiated notification, delivered undecoded.
///
/// The escape hatch for methods the typed surface does not model — a consumer
/// subscribes via [`LspClient::raw_notifications`] and decodes the methods it
/// recognizes (jdtls `language/status`, rust-analyzer `experimental/*`, …).
/// Notifications the client also handles itself (diagnostics) still fan out
/// here, so a subscriber sees the complete stream.
#[derive(Clone, Debug)]
pub struct RawNotification {
    /// The JSON-RPC method name.
    pub method: String,
    /// The notification parameters, verbatim.
    pub params: serde_json::Value,
}

/// An async client for a single language server.
///
/// Dropping the client tears the connection down ungracefully (a spawned
/// process is killed); prefer [`LspClient::shutdown`] for the polite handshake.
pub struct LspClient {
    conn: conn::Connection,
    child: Option<tokio::process::Child>,
}

impl LspClient {
    /// Spawn and initialize the server described by `spec`, rooted at `root`.
    ///
    /// The child speaks LSP on its stdio; its stderr is drained to `tracing`
    /// debug logs. The `initialize` handshake completes before returning (see
    /// [`LspClient::connect`] for what is negotiated).
    ///
    /// # Errors
    /// Returns [`LspError::Spawn`] if the process cannot start, or any handshake
    /// error from [`LspClient::connect`].
    pub async fn spawn(spec: LspSpec, root: &Path) -> Result<Self, LspError> {
        let mut command = tokio::process::Command::new(&spec.command);
        command.args(&spec.args).current_dir(root);
        Self::spawn_command(command, &spec.command, root).await
    }

    /// Spawn and initialize a server through a caller-prepared command.
    ///
    /// This is the process-ownership seam used by hosts that wrap a language
    /// server in a crash-safe supervisor. The command's stdin/stdout become the
    /// LSP transport and its stderr is drained to tracing. karet itself prepares
    /// a hidden supervisor command here; simple embedders can continue using
    /// [`Self::spawn`].
    ///
    /// # Errors
    /// Returns [`LspError::Launch`] if the prepared process cannot start, does
    /// not expose piped standard I/O, or dies during the handshake — carrying
    /// the argv, the exit status and the tail of the server's stderr. Other
    /// initialization errors come from [`Self::connect`].
    pub async fn spawn_command(
        mut command: tokio::process::Command,
        display_name: &str,
        root: &Path,
    ) -> Result<Self, LspError> {
        // Recovered from the prepared command rather than taken as a parameter,
        // so the reported argv is what was actually run. `display_name` stays
        // the caller's friendlier label for the executable.
        let args = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let fail = |cause, exit, stderr: Vec<String>| {
            let failure = LaunchFailure::new(display_name, args.clone(), cause)
                .with_exit(exit)
                .with_stderr(stderr);
            tracing::warn!(error = %failure, "language server launch failed");
            LspError::Launch(Box::new(failure))
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                let cause = match error.kind() {
                    std::io::ErrorKind::NotFound => launch::LaunchCause::NotFound,
                    std::io::ErrorKind::PermissionDenied => launch::LaunchCause::PermissionDenied,
                    _ => launch::LaunchCause::Io,
                };
                fail(cause, None, vec![error.to_string()])
            })?;
        let tail = launch::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            let command = display_name.to_owned();
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "karet_lsp::stderr", server = %command, "{line}");
                    tail.push(line);
                }
            });
        }
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(fail(launch::LaunchCause::NoStdio, None, tail.lines()));
        };
        match Self::connect(stdout, stdin, root).await {
            Ok(mut client) => {
                client.child = Some(child);
                Ok(client)
            },
            // A server that dies during the handshake reaches here as a bare
            // `Closed`, which says nothing. Its exit status and last words do.
            Err(error) => Err(Self::launch_failure(child, tail, error, fail).await),
        }
    }

    /// Turn a handshake failure into a launch failure when the child is the
    /// reason, leaving a genuine protocol error alone.
    async fn launch_failure(
        mut child: tokio::process::Child,
        tail: launch::StderrTail,
        error: LspError,
        fail: impl Fn(launch::LaunchCause, Option<ExitReport>, Vec<String>) -> LspError,
    ) -> LspError {
        if !matches!(error, LspError::Closed | LspError::Timeout) {
            return error;
        }
        // Bounded: a server that closed its stdio but is still running is a
        // protocol failure, not a launch one, and must not stall the report.
        let exit = tokio::time::timeout(CHILD_EXIT_GRACE, child.wait())
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(ExitReport::from_status);
        let stderr = tail.lines();
        if exit.is_none() && stderr.is_empty() {
            return error;
        }
        fail(launch::LaunchCause::Exited, exit, stderr)
    }

    /// Connect over an arbitrary async I/O pair and perform the `initialize`
    /// handshake, rooted at `root`.
    ///
    /// This is the transport seam: [`LspClient::spawn`] passes child stdio here,
    /// tests pass the ends of a `tokio::io::duplex`, and embedders can pass any
    /// in-process or remote byte stream.
    ///
    /// The handshake advertises the `utf-16` position encoding, completion
    /// without snippet support, and diagnostics with related information; it
    /// then sends `initialized`.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] when `root` cannot form a `file://` URI,
    /// or [`LspError::Server`] / [`LspError::Timeout`] / [`LspError::Closed`]
    /// when the `initialize` request fails.
    pub async fn connect<R, W>(read: R, write: W, root: &Path) -> Result<Self, LspError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let params = initialize_params(root)?;
        let conn = conn::Connection::start(read, write);
        let _server_capabilities: Value = conn.request("initialize", params).await?;
        conn.notify("initialized", lsp_types::InitializedParams {})?;
        Ok(Self { conn, child: None })
    }

    /// Shut the server down (`shutdown` request + `exit` notification) and await
    /// the process; a process that overstays the shutdown deadline is killed.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] (or [`LspError::Timeout`] /
    /// [`LspError::Closed`]) if the shutdown handshake fails; cleanup still runs.
    pub async fn shutdown(mut self) -> Result<(), LspError> {
        let outcome: Result<Value, LspError> = self
            .conn
            .request_with("shutdown", Value::Null, conn::SHUTDOWN_TIMEOUT)
            .await;
        let _ = self.conn.notify("exit", Value::Null);
        // Drain the queue so the `exit` notification actually reaches the wire
        // before the connection tasks stop.
        self.conn.close().await;
        if let Some(mut child) = self.child.take() {
            match tokio::time::timeout(conn::SHUTDOWN_TIMEOUT, child.wait()).await {
                Ok(_) => {},
                Err(_elapsed) => {
                    let _ = child.kill().await;
                },
            }
        }
        outcome.map(|_| ())
    }

    // --- document sync (the seam the editing path drives) -----------------

    /// Notify the server that `doc` opened, with its `language_id`, `version` and
    /// full `text`.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn did_open(
        &self,
        doc: &Path,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem::new(
                uri::path_to_uri(doc)?,
                language_id.to_owned(),
                version,
                text.to_owned(),
            ),
        };
        self.conn.notify("textDocument/didOpen", params)
    }

    /// Notify the server that `doc` changed, replacing its content with `text`
    /// at document `version`.
    ///
    /// Sync is **full-text** (see the crate docs): the whole document travels on
    /// every change, which every server accepts regardless of the sync kind it
    /// prefers. Callers should therefore debounce rapid edits.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn did_change(&self, doc: &Path, version: i32, text: &str) -> Result<(), LspError> {
        let params = lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier::new(
                uri::path_to_uri(doc)?,
                version,
            ),
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        };
        self.conn.notify("textDocument/didChange", params)
    }

    /// Notify the server that `doc` was saved (optionally including its text).
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn did_save(&self, doc: &Path, text: Option<&str>) -> Result<(), LspError> {
        let params = lsp_types::DidSaveTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            text: text.map(ToOwned::to_owned),
        };
        self.conn.notify("textDocument/didSave", params)
    }

    /// Notify the server that `doc` was closed.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn did_close(&self, doc: &Path) -> Result<(), LspError> {
        let params = lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
        };
        self.conn.notify("textDocument/didClose", params)
    }

    /// Request completions at `pos` in `doc` (`pos.col` in UTF-16 units, per
    /// the crate docs).
    ///
    /// The response is flattened to a plain list: a `CompletionList`'s
    /// `isIncomplete` flag is deliberately dropped because this contract
    /// returns `Vec<CompletionItem>`. Consumers compensate by **re-requesting
    /// on trigger characters** (and on any prefix the server might narrow
    /// differently) instead of tracking incompleteness. Snippet-format insert
    /// text is degraded to plain text — this client does not advertise
    /// `snippetSupport`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn completion(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<CompletionItem>, LspError> {
        let params = lsp_types::CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
                position: convert::position_to_lsp(pos),
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: None,
        };
        let response: Option<lsp_types::CompletionResponse> =
            self.conn.request("textDocument/completion", params).await?;
        Ok(convert::completions_from_lsp(response))
    }

    /// Request hover information at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn hover(&self, doc: &Path, pos: LineCol) -> Result<Option<Hover>, LspError> {
        let params = lsp_types::HoverParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<lsp_types::Hover> =
            self.conn.request("textDocument/hover", params).await?;
        Ok(convert::hover_from_lsp(response))
    }

    /// Request the document symbols of `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn document_symbols(&self, doc: &Path) -> Result<Vec<Symbol>, LspError> {
        let params = lsp_types::DocumentSymbolParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::DocumentSymbolResponse> = self
            .conn
            .request("textDocument/documentSymbol", params)
            .await?;
        Ok(convert::document_symbols_from_lsp(response))
    }

    /// Search workspace symbols matching `query`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<Symbol>, LspError> {
        let params = lsp_types::WorkspaceSymbolParams {
            query: query.to_owned(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::WorkspaceSymbolResponse> =
            self.conn.request("workspace/symbol", params).await?;
        Ok(convert::workspace_symbols_from_lsp(response))
    }

    /// Resolve the implementations of the contract at `pos`.
    ///
    /// Enrichment, not a prerequisite: a caller that already matched implementations
    /// structurally uses this to confirm and extend them, and loses precision rather than
    /// function when no server is running.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn implementations(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<Location>, LspError> {
        let params = lsp_types::request::GotoImplementationParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::request::GotoImplementationResponse> = self
            .conn
            .request("textDocument/implementation", params)
            .await?;
        Ok(convert::locations_from_lsp(response))
    }

    /// Resolve the supertypes of the type at `pos` — what it derives from or implements.
    ///
    /// Two round trips, as the protocol requires: `prepare` establishes the item, and
    /// `supertypes` walks upward from it. A server that declines the first returns no
    /// supertypes rather than an error, since not supporting the request is not a failure.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn supertypes(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        let prepare = lsp_types::TypeHierarchyPrepareParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let items: Option<Vec<lsp_types::TypeHierarchyItem>> = self
            .conn
            .request("textDocument/prepareTypeHierarchy", prepare)
            .await?;
        let Some(item) = items.unwrap_or_default().into_iter().next() else {
            return Ok(Vec::new());
        };
        let params = lsp_types::TypeHierarchySupertypesParams {
            item,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<lsp_types::TypeHierarchyItem>> = self
            .conn
            .request("typeHierarchy/supertypes", params)
            .await?;
        Ok(convert::type_hierarchy_locations(response))
    }

    /// Resolve the definition location(s) of the symbol at `pos`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn definition(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::GotoDefinitionResponse> =
            self.conn.request("textDocument/definition", params).await?;
        Ok(convert::locations_from_lsp(response))
    }

    /// Request inlay hints within `range`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn inlay_hints(&self, doc: &Path, range: Range) -> Result<Vec<InlayHint>, LspError> {
        let params = lsp_types::InlayHintParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            range: convert::range_to_lsp(range),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<Vec<lsp_types::InlayHint>> =
            self.conn.request("textDocument/inlayHint", params).await?;
        Ok(convert::inlay_hints_from_lsp(response))
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
        let params = lsp_types::RenameParams {
            text_document_position: text_document_position(doc, pos)?,
            new_name: new_name.to_owned(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<lsp_types::WorkspaceEdit> =
            self.conn.request("textDocument/rename", params).await?;
        Ok(response.map_or_else(WorkspaceEdit::default, convert::workspace_edit_from_lsp))
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
        let params = lsp_types::SignatureHelpParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            context: None,
        };
        let response: Option<lsp_types::SignatureHelp> = self
            .conn
            .request("textDocument/signatureHelp", params)
            .await?;
        Ok(convert::signature_help_from_lsp(response))
    }

    /// Request code actions available for `range` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn code_action(&self, doc: &Path, range: Range) -> Result<Vec<CodeAction>, LspError> {
        let params = lsp_types::CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            range: convert::range_to_lsp(range),
            context: lsp_types::CodeActionContext {
                diagnostics: Vec::new(),
                only: None,
                trigger_kind: Some(lsp_types::CodeActionTriggerKind::INVOKED),
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::CodeActionResponse> =
            self.conn.request("textDocument/codeAction", params).await?;
        Ok(convert::code_actions_from_lsp(response))
    }

    /// Request whole-document formatting edits for `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Server`] or [`LspError::Timeout`].
    pub async fn formatting(&self, doc: &Path) -> Result<Vec<TextEdit>, LspError> {
        let params = lsp_types::DocumentFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            options: formatting_options(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<Vec<lsp_types::TextEdit>> =
            self.conn.request("textDocument/formatting", params).await?;
        Ok(convert::text_edits_from_lsp(response))
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
        let params = lsp_types::DocumentRangeFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            range: convert::range_to_lsp(range),
            options: formatting_options(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<Vec<lsp_types::TextEdit>> = self
            .conn
            .request("textDocument/rangeFormatting", params)
            .await?;
        Ok(convert::text_edits_from_lsp(response))
    }

    /// Subscribe to server-pushed diagnostics.
    ///
    /// Ranges are in UTF-16 columns, per the crate-level position-encoding note.
    #[must_use]
    pub fn diagnostics(&self) -> broadcast::Receiver<PublishedDiagnostics> {
        self.conn.diagnostics()
    }

    /// Subscribe to every server-initiated notification, undecoded (see
    /// [`RawNotification`]). Slow subscribers drop the oldest entries.
    #[must_use]
    pub fn raw_notifications(&self) -> broadcast::Receiver<RawNotification> {
        self.conn.raw_notifications()
    }

    /// Issue an arbitrary request and await its typed result.
    ///
    /// The escape hatch for server-specific extensions the typed surface does
    /// not model (jdtls `java/classFileContents`, clangd
    /// `textDocument/switchSourceHeader`, …). `method` goes on the wire
    /// verbatim; the standard request timeout applies.
    ///
    /// # Errors
    ///
    /// [`LspError::Closed`] when the connection is gone, [`LspError::Timeout`]
    /// when the server does not answer in time, [`LspError::Server`] when it
    /// answers with an error, and [`LspError::Protocol`] when the result does
    /// not decode as `T`.
    pub async fn custom_request<P, T>(&self, method: &str, params: P) -> Result<T, LspError>
    where
        P: serde::Serialize,
        T: serde::de::DeserializeOwned,
    {
        self.conn.request(method, params).await
    }

    /// Send an arbitrary notification (fire-and-forget), `method` verbatim.
    ///
    /// # Errors
    ///
    /// [`LspError::Closed`] when the connection is gone, or
    /// [`LspError::Protocol`] when `params` fail to encode or the outbound
    /// queue is full.
    pub fn custom_notify<P: serde::Serialize>(
        &self,
        method: &str,
        params: P,
    ) -> Result<(), LspError> {
        self.conn.notify(method, params)
    }
}

fn text_document_position(
    doc: &Path,
    position: LineCol,
) -> Result<lsp_types::TextDocumentPositionParams, LspError> {
    Ok(lsp_types::TextDocumentPositionParams {
        text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
        position: convert::position_to_lsp(position),
    })
}

fn formatting_options() -> lsp_types::FormattingOptions {
    lsp_types::FormattingOptions {
        tab_size: 4,
        insert_spaces: true,
        ..lsp_types::FormattingOptions::default()
    }
}

/// Build the `initialize` params advertising what this client actually does.
fn initialize_params(root: &Path) -> Result<lsp_types::InitializeParams, LspError> {
    let root_uri = uri::path_to_uri(root)?;
    let folder_name = root.file_name().map_or_else(
        || "workspace".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let capabilities = lsp_types::ClientCapabilities {
        general: Some(lsp_types::GeneralClientCapabilities {
            position_encodings: Some(vec![lsp_types::PositionEncodingKind::UTF16]),
            ..lsp_types::GeneralClientCapabilities::default()
        }),
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            completion: Some(lsp_types::CompletionClientCapabilities {
                completion_item: Some(lsp_types::CompletionItemCapability {
                    // Snippets degrade to plain text (see the crate docs).
                    snippet_support: Some(false),
                    deprecated_support: Some(true),
                    ..lsp_types::CompletionItemCapability::default()
                }),
                ..lsp_types::CompletionClientCapabilities::default()
            }),
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                ..lsp_types::PublishDiagnosticsClientCapabilities::default()
            }),
            ..lsp_types::TextDocumentClientCapabilities::default()
        }),
        ..lsp_types::ClientCapabilities::default()
    };
    // `root_uri` is deprecated in favour of `workspace_folders`, but older
    // servers read it exclusively, so we deliberately send both.
    #[allow(deprecated)]
    Ok(lsp_types::InitializeParams {
        process_id: Some(std::process::id()),
        root_uri: Some(root_uri.clone()),
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: root_uri,
            name: folder_name,
        }]),
        capabilities,
        ..lsp_types::InitializeParams::default()
    })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
