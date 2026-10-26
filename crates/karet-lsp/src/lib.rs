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
//! pushed diagnostics, work-done progress ([`LspClient::work_progress`]) and
//! show/log messages ([`LspClient::messages`]), and answers the few
//! server→client requests a headless client must not leave hanging
//! (`workspace/configuration` from the consumer's [`ConnectOptions::settings`],
//! `client/registerCapability`, `window/workDoneProgress/create`,
//! `workspace/inlayHint/refresh`, `workspace/semanticTokens/refresh`,
//! `workspace/diagnostic/refresh` -- the last three also surfaced through
//! [`LspClient::refreshes`]). The server requests that must wait on the
//! editor, `workspace/applyEdit` and `window/showMessageRequest`, are handed
//! out through [`LspClient::apply_edit_requests`] and
//! [`LspClient::message_requests`] instead. File watchers a server registers
//! are kept, and [`LspClient::did_change_watched_files`] filters a consumer's
//! file changes through them.
//!
//! Three protocol choices are deliberate and documented here once:
//!
//! - **Positions cross this API in UTF-16.** The client offers the LSP-default
//!   `utf-16` as its only position encoding and stays faithful to it; a server's
//!   `positionEncoding` reply is recorded in [`Capabilities`] but not yet acted
//!   on. Every [`LineCol`] and [`Range`] passed to or returned from this crate
//!   counts columns in UTF-16 code units. karet is internally UTF-32; the conversions live on
//!   `karet_text::TextBuffer` (`line_col_to_utf16` / `utf16_to_line_col`) and are
//!   applied by the consumer that owns the text (karet-session), not here.
//! - **Document sync is the consumer's choice of two.** [`LspClient::did_change`]
//!   sends the whole document, which every server accepts;
//!   [`LspClient::did_change_incremental`] sends only the edited ranges, and is
//!   refused without sending for a server that did not ask for incremental sync.
//!   The consumer owns the text, so it decides which one an edit can use.
//! - **Snippets are advertised** (`completionItem.snippetSupport: true`). A
//!   snippet completion maps to the text it inserts before anyone edits it
//!   (placeholders filled in, the first choice chosen), with its stops beside it
//!   in [`karet_core::CompletionItem::tabstops`] for a consumer that runs a
//!   snippet session; one that does not inserts the text as is. An item's
//!   documentation, detail and additional edits may be deferred to
//!   [`LspClient::resolve_completion`].
//!
//! Transport, lifecycle, document sync, diagnostics (pushed, and pulled through
//! [`LspClient::pull_diagnostics`]), completion, navigation,
//! symbols, document highlights, inlay hints, semantic tokens (whole and delta,
//! see [`LspClient::semantic_tokens`]), rename, signature help, code actions (with
//! `codeAction/resolve` and `workspace/executeCommand`), document/range
//! formatting, on-type formatting, linked editing ranges and will-save edits
//! are implemented as typed, non-panicking operations.

pub use apply_edit::ApplyEditRequest;
pub use code_action::CodeActionTrigger;
pub use config::ConnectOptions;
pub use editing::LinkedEditingRanges;
pub use editing::SaveReason;
pub use highlight::DocumentHighlight;
pub use highlight::HighlightKind;
/// `Content-Length` message framing (the LSP base protocol).
///
/// Re-exported from [`karet_jsonrpc::framing::content_length`], which is where
/// the implementation now lives; the `karet_lsp::codec` path is kept because it
/// is this crate's published surface.
pub use karet_jsonrpc::framing::content_length as codec;
pub use launch::ExitReport;
pub use launch::LaunchCause;
pub use launch::LaunchFailure;
pub use pull::PulledDiagnostics;
pub use semantic_tokens::SemanticTokens;
pub use watch::FileChange;
pub use watch::FileChangeKind;
pub use window::MessageLevel;
pub use window::MessageRequest;
pub use window::ProgressToken;
pub use window::ServerMessage;
pub use window::ServerMessageKind;
pub use window::WorkProgress;
pub use window::WorkProgressKind;

mod apply_edit;
mod capability;
mod code_action;
mod codelens;
mod completion;
mod config;
mod conn;
mod convert;
mod editing;
mod gate;
mod handshake;
mod hierarchy;
mod highlight;
mod inlay;
mod launch;
mod navigation;
mod pull;
mod selection;
mod selector;
mod semantic_tokens;
mod snippet;
mod uri;
mod watch;
mod window;

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;

use karet_core::Capabilities;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::PrepareRename;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::SignatureHelp;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use karet_core::WorkspaceSymbol;
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
    /// The language server could not be launched, with what is known about why.
    ///
    /// Replaces the `Spawn` variant this enum used to carry, which nothing
    /// constructed any more. Deprecating it rather than removing it would have
    /// left a downstream `match e { LspError::Spawn => ..., _ => ... }`
    /// compiling with a warning on the pattern alone while its missing-binary
    /// arm went dead, sending every launch failure to `_`; the removal makes
    /// that a compile error instead, which is the only way the arm gets moved.
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
    /// The server did not advertise this capability, so no request was issued.
    ///
    /// Deliberately distinct from [`LspError::Server`]: a server that does not
    /// implement `textDocument/inlayHint` is not failing, it is answering a
    /// question nobody should have asked it. Collapsing the two is what made a
    /// missing feature read as a broken server, and left the difference
    /// invisible in a `warn` log nobody sees.
    #[error("the language server does not support {method}")]
    Unsupported {
        /// The request that was not issued.
        method: &'static str,
    },
}

/// The indentation a formatting request states on behalf of the buffer.
///
/// LSP puts this in the *request*: `textDocument/formatting` carries a
/// `FormattingOptions`, and a server that honours it (clangd with no
/// `.clang-format`, jdtls, lua-language-server, omnisharp) reindents the whole
/// file to whatever the client said. So the client's real settings have to
/// cross this API — a constant here silently overrides the user's
/// `editor.tabSize` / `editor.insertSpaces` on every save.
///
/// Deliberately this crate's own type rather than `lsp_types::FormattingOptions`:
/// `lsp_types` is an internal dependency and appears nowhere in the published
/// surface, and keeping it that way is what lets the dependency move without a
/// breaking release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indentation {
    /// Columns one indentation level occupies (`editor.tabSize`).
    pub tab_size: u32,
    /// Whether indentation is written as spaces rather than tab characters
    /// (`editor.insertSpaces`).
    pub insert_spaces: bool,
}

impl Default for Indentation {
    /// Four spaces: the LSP specification's own example values, and what this
    /// client sent unconditionally before callers could say otherwise.
    ///
    /// A fallback for a caller that has no editor settings to resolve — not a
    /// value any caller that *does* have them should be reaching for.
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
        }
    }
}

/// How long a failed handshake waits for the child to exit before reporting.
///
/// Only spent on a launch that already failed, and only to learn whether the
/// process died -- a server that is still running is a protocol failure, and
/// must not be waited on.
const CHILD_EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// How long a failed launch waits for the stderr drain to reach EOF.
///
/// The child's exit closes the write end, so the drain finishes on its own and
/// this is normally not spent at all. It bounds the one case where it cannot:
/// a surviving grandchild still holding that end open.
const STDERR_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// How to launch a language server.
///
/// Constructed through [`LspSpec::new`] rather than as a struct literal, so a
/// server that later needs another knob -- as Astro needed
/// `initialization_options` -- can be given one without breaking every consumer
/// that builds a spec. The fields stay public and assignable.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct LspSpec {
    /// The server executable.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Language identifiers this server handles (e.g. `"rust"`).
    pub languages: Vec<String>,
    /// Server-specific `initializationOptions`, sent verbatim with
    /// `initialize`.
    ///
    /// Some servers cannot start without one. Astro's, for instance, refuses
    /// the handshake unless it is told where a TypeScript SDK lives, rather
    /// than looking for one itself — no argv can substitute, because the option
    /// is part of the protocol rather than the command line.
    pub initialization_options: Option<Value>,
    /// The settings `workspace/configuration` is answered from, keyed by
    /// section (see [`ConnectOptions::settings`]).
    pub settings: Option<Value>,
}

impl LspSpec {
    /// A spec launching `command` with `args`, serving `languages`.
    ///
    /// Server-specific `initializationOptions` are attached separately with
    /// [`LspSpec::with_initialization_options`]; most servers need none.
    #[must_use]
    pub fn new(command: impl Into<String>, args: Vec<String>, languages: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            languages,
            initialization_options: None,
            settings: None,
        }
    }

    /// Attach the `initializationOptions` sent verbatim with `initialize`.
    #[must_use]
    pub fn with_initialization_options(mut self, options: Option<Value>) -> Self {
        self.initialization_options = options;
        self
    }

    /// Attach the settings `workspace/configuration` is answered from.
    #[must_use]
    pub fn with_settings(mut self, settings: Option<Value>) -> Self {
        self.settings = settings;
        self
    }
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

/// A server's request that the client re-fetch something it answered before.
///
/// Delivered through [`LspClient::refreshes`]. The client has already answered
/// the request by the time a subscriber sees it; what remains is the
/// subscriber's half -- dropping what it cached and asking again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServerRefresh {
    /// `workspace/inlayHint/refresh`: every inlay hint this server has
    /// answered, for any document, may now be stale.
    ///
    /// Sent when something *outside* the requested document changed the
    /// answer -- editing a function's return type in one file changes the
    /// hints shown at its call sites in every other.
    InlayHints,
    /// `workspace/semanticTokens/refresh`: every document's semantic tokens
    /// from this server may now be stale -- a project-wide change (an index
    /// that finished loading, a dependency that resolved) reclassified names
    /// in files nobody edited.
    SemanticTokens,
    /// `workspace/diagnostic/refresh`: every document's pulled diagnostics
    /// from this server may now be stale -- a project-wide change (a
    /// dependency edit, a configuration reload) changed what it would report
    /// for files nobody edited, so each open document is pulled again.
    Diagnostics,
}

/// An async client for a single language server.
///
/// Dropping the client tears the connection down ungracefully (a spawned
/// process is killed); prefer [`LspClient::shutdown`] for the polite handshake.
pub struct LspClient {
    conn: conn::Connection,
    child: Option<tokio::process::Child>,
    /// What this server said it can do, and for which documents.
    ///
    /// Behind a lock, and *shared with the connection handler* rather than
    /// copied at the handshake, because `client/registerCapability` arrives on
    /// the handler and has to be visible here immediately. Never held across an
    /// `.await`: every read clones or answers a question outright.
    gate: std::sync::Arc<std::sync::RwLock<gate::Gate>>,
}

impl LspClient {
    /// Spawn and initialize the server described by `spec`, rooted at `root`.
    ///
    /// The child speaks LSP on its stdio; its stderr is drained to `tracing`
    /// debug logs. The `initialize` handshake completes before returning (see
    /// [`LspClient::connect`] for what is negotiated).
    ///
    /// # Errors
    /// Returns [`LspError::Launch`] if the process cannot start or dies during
    /// the handshake, or any other handshake error from
    /// [`LspClient::connect`].
    pub async fn spawn(spec: LspSpec, root: &Path) -> Result<Self, LspError> {
        let mut command = tokio::process::Command::new(&spec.command);
        command.args(&spec.args).current_dir(root);
        Self::spawn_command_configured(command, &spec.command, root, ConnectOptions::from(&spec))
            .await
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
        command: tokio::process::Command,
        display_name: &str,
        root: &Path,
    ) -> Result<Self, LspError> {
        Self::spawn_command_with(command, display_name, root, None).await
    }

    /// Spawn as [`Self::spawn_command`], sending `initialization_options` with
    /// the handshake.
    ///
    /// # Errors
    /// As [`Self::spawn_command`].
    pub async fn spawn_command_with(
        command: tokio::process::Command,
        display_name: &str,
        root: &Path,
        initialization_options: Option<Value>,
    ) -> Result<Self, LspError> {
        let options = ConnectOptions::default().with_initialization_options(initialization_options);
        Self::spawn_command_configured(command, display_name, root, options).await
    }

    /// Spawn as [`Self::spawn_command`], connecting with `options` (see
    /// [`Self::connect_configured`]).
    ///
    /// # Errors
    /// As [`Self::spawn_command`].
    pub async fn spawn_command_configured(
        mut command: tokio::process::Command,
        display_name: &str,
        root: &Path,
        options: ConnectOptions,
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
        let stderr_drain = child
            .stderr
            .take()
            .map(|stderr| drain_stderr(stderr, display_name.to_owned(), tail.clone()));
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(fail(launch::LaunchCause::NoStdio, None, tail.lines()));
        };
        match Self::connect_configured(stdout, stdin, root, options).await {
            Ok(mut client) => {
                client.child = Some(child);
                // Dropping the drain's handle detaches it rather than
                // cancelling it, so a healthy server keeps logging its stderr
                // and the success path neither waits nor leaks.
                drop(stderr_drain);
                Ok(client)
            },
            // A server that dies during the handshake reaches here as a bare
            // `Closed`, which says nothing. Its exit status and last words do.
            Err(error) => Err(Self::launch_failure(child, tail, stderr_drain, error, fail).await),
        }
    }

    /// Turn a handshake failure into a launch failure when the child is the
    /// reason, leaving a genuine protocol error alone.
    async fn launch_failure(
        mut child: tokio::process::Child,
        tail: launch::StderrTail,
        stderr_drain: Option<tokio::task::JoinHandle<()>>,
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
        // The server's last words only reach the tail once the drain has read
        // the pipe, so joining it is what makes the diagnosis reliable instead
        // of a race the reader wins about four times in five. The child's exit
        // closes the pipe, so the drain ends by itself; the bound only covers
        // a grandchild still holding the write end, and a handle dropped on
        // timeout detaches rather than blocking.
        if let Some(drain) = stderr_drain {
            let _ = tokio::time::timeout(STDERR_DRAIN_GRACE, drain).await;
        }
        let stderr = tail.lines();
        // `Exited` is permanent, so it has to mean the process really is gone:
        // an expired wait grace says the opposite. Stderr is evidence about
        // *what* the server is unhappy about, never that it died.
        let Some(exit) = exit else {
            return match error {
                // Alive and silent past the handshake deadline -- a large
                // workspace still indexing looks exactly like this, so it
                // keeps its retries.
                LspError::Timeout => fail(launch::LaunchCause::Timeout, None, stderr),
                // Alive with its stdio closed is a protocol failure rather
                // than a launch one, and is reported as it arrived.
                other => other,
            };
        };
        fail(launch::LaunchCause::Exited, Some(exit), stderr)
    }

    /// Connect over an arbitrary async I/O pair and perform the `initialize`
    /// handshake, rooted at `root`.
    ///
    /// This is the transport seam: [`LspClient::spawn`] passes child stdio here,
    /// tests pass the ends of a `tokio::io::duplex`, and embedders can pass any
    /// in-process or remote byte stream.
    ///
    /// The handshake advertises the `utf-16` position encoding, completion
    /// with snippet support, diagnostics with related information, inlay
    /// hints with `workspace/inlayHint/refresh` support, and dynamic
    /// registration for the gated requests whose registration the client
    /// honours -- each scoped to the documents its `documentSelector` covers
    /// (see [`Self::supports_for`]); it then sends `initialized`.
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
        Self::connect_with(read, write, root, None).await
    }

    /// Connect as [`Self::connect`], sending `initialization_options` with the
    /// handshake.
    ///
    /// Separate from [`Self::connect`] rather than a parameter on it, so the
    /// common case stays a three-argument call. Some servers cannot start
    /// without their options: Astro's refuses the handshake unless it is told
    /// where a TypeScript SDK lives.
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn connect_with<R, W>(
        read: R,
        write: W,
        root: &Path,
        initialization_options: Option<Value>,
    ) -> Result<Self, LspError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let options = ConnectOptions::default().with_initialization_options(initialization_options);
        Self::connect_configured(read, write, root, options).await
    }

    /// Connect as [`Self::connect`], with `options`: `initializationOptions`
    /// sent with the handshake, and the settings `workspace/configuration` is
    /// answered from -- held before `initialize` goes out, so a server that
    /// asks at once is answered from them, and pushed once with
    /// `workspace/didChangeConfiguration` after `initialized`.
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn connect_configured<R, W>(
        read: R,
        write: W,
        root: &Path,
        options: ConnectOptions,
    ) -> Result<Self, LspError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let mut params = handshake::initialize_params(root)?;
        params.initialization_options = options.initialization_options;
        let conn = conn::Connection::start(read, write);
        if let Some(settings) = &options.settings {
            conn.set_settings(settings.clone());
        }
        let result: Value = conn
            .request("initialize", handshake::to_wire(params)?)
            .await?;
        // Seeded into the gate the handler already owns. Registrations are kept
        // apart from the advertised set, so one that arrives between here and
        // the first request survives the seeding.
        let gate = conn.gate();
        let parsed = capability::parse(&result);
        tracing::debug!(
            features = parsed.len(),
            encoding = ?parsed.position_encoding,
            sync = ?parsed.text_sync,
            "language server advertised its capabilities"
        );
        if let Ok(mut live) = gate.write() {
            live.advertise(parsed);
        }
        conn.notify("initialized", lsp_types::InitializedParams {})?;
        if let Some(settings) = options.settings {
            conn.notify(
                "workspace/didChangeConfiguration",
                serde_json::json!({ "settings": settings }),
            )?;
        }
        Ok(Self {
            conn,
            child: None,
            gate,
        })
    }

    /// What this server said it can do.
    ///
    /// A snapshot: dynamic registration may change it afterwards, so a caller
    /// deciding about one request should ask again rather than cache this. A
    /// feature registered only for some documents shows here as supported;
    /// [`Self::supports_for`] answers for one document.
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        self.gate
            .read()
            .map(|gate| gate.capabilities())
            .unwrap_or_default()
    }

    /// Whether this server currently supports `feature` for at least one
    /// document.
    ///
    /// A feature registered dynamically with a `documentSelector` counts here
    /// even though a request for a document outside the selector is refused;
    /// ask [`Self::supports_for`] about a particular document.
    #[must_use]
    pub fn supports(&self, feature: ServerFeature) -> bool {
        self.gate.read().is_ok_and(|gate| gate.supports(feature))
    }

    /// Whether this server currently supports `feature` for the document at
    /// `doc`: advertised at the handshake, or dynamically registered with a
    /// `documentSelector` that covers it (or with none).
    ///
    /// A selector's `language` filter is matched against the `languageId` the
    /// document was opened with through [`Self::did_open`]; a document never
    /// opened on this client matches no language filter.
    ///
    /// # Open before asking
    ///
    /// Call [`Self::did_open`] for a document before asking about it or sending
    /// it a request. The protocol already requires that order, and it is the
    /// only way this client learns the document's language: no language is
    /// guessed from the path, so a language-scoped registration covers a
    /// document only once it is open.
    #[must_use]
    pub fn supports_for(&self, feature: ServerFeature, doc: &Path) -> bool {
        self.gate
            .read()
            .is_ok_and(|gate| gate.supports_for(feature, doc))
    }

    /// Whether this server currently supports `feature` for the document at
    /// `doc` were it open as `language`, open now or not.
    ///
    /// As [`Self::supports_for`], except that a selector's `language` filter
    /// is matched against `language` rather than the `languageId` the client
    /// recorded at [`Self::did_open`], which [`Self::did_close`] forgets. It
    /// answers for a document already closed: whether a registration still
    /// covers what it was last open as.
    #[must_use]
    pub fn supports_for_language(
        &self,
        feature: ServerFeature,
        doc: &Path,
        language: &str,
    ) -> bool {
        self.gate
            .read()
            .is_ok_and(|gate| gate.supports_as(feature, doc, Some(language)))
    }

    /// Refuse `method` when the server never said it could answer it for any
    /// document.
    ///
    /// The refusal is the point: issuing the request anyway produced a
    /// JSON-RPC error that looked exactly like a failure, so a server missing
    /// a feature was indistinguishable from a server that was broken.
    ///
    /// A poisoned lock refuses too. It can only be poisoned by a panic while
    /// capabilities were being written, and at that point what the server
    /// supports is genuinely unknown -- refusing is the safe reading.
    fn require(&self, feature: ServerFeature, method: &'static str) -> Result<(), LspError> {
        refuse_unless(self.supports(feature), feature, method)
    }

    /// Refuse `method` for `doc` when the server never said it could answer it
    /// for that document. As [`Self::require`], per document.
    fn require_for(
        &self,
        feature: ServerFeature,
        method: &'static str,
        doc: &Path,
    ) -> Result<(), LspError> {
        refuse_unless(self.supports_for(feature, doc), feature, method)
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

    /// Resolve once this server's connection is gone -- it exited, its stream
    /// lost framing, or a write failed.
    ///
    /// Meant for a `select!` arm beside whatever else a caller waits on. Without
    /// one, a server that dies while the editor is idle goes unnoticed until the
    /// next request happens to fail, which can be an arbitrarily long time: the
    /// badge keeps reading healthy and no restart is scheduled. Resolves
    /// immediately for a connection that is already closed.
    pub async fn closed(&self) {
        self.conn.closed().await;
    }

    // --- document sync (the seam the editing path drives) -----------------

    /// Notify the server that `doc` opened, with its `language_id`, `version` and
    /// full `text`.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or a full
    /// outbound queue, or [`LspError::Closed`] if the connection is gone.
    pub async fn did_open(
        &self,
        doc: &Path,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> Result<(), LspError> {
        // Recorded before sending, so a registration scoped by language covers
        // the document from the moment the server can know about it.
        if let Ok(mut gate) = self.gate.write() {
            gate.opened(doc, language_id);
        }
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
    /// Sync is **full-text**: the whole document travels on every change, which
    /// every server accepts regardless of the sync kind it prefers. Callers
    /// should therefore debounce rapid edits, and prefer
    /// [`Self::did_change_incremental`] where the server supports it.
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or a full
    /// outbound queue, or [`LspError::Closed`] if the connection is gone.
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

    /// Notify the server that `doc` changed by `edits`, reaching document
    /// `version` — incremental sync, for a server whose `textDocumentSync` is
    /// `Incremental`.
    ///
    /// The edits travel as one `didChange` and are applied **in order**, each
    /// against the text the previous one left, exactly as LSP applies a
    /// notification's `contentChanges`. Ranges are UTF-16, like every position
    /// crossing this crate. The caller owns the text and so owns the
    /// correctness of the deltas: they must take the server's copy of the
    /// document from the last version it was sent to `version`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without sending anything** when the
    /// server did not ask for incremental sync (send the whole text with
    /// [`Self::did_change`] instead), [`LspError::Protocol`] for an
    /// unconvertible path or a full outbound queue, or [`LspError::Closed`] if
    /// the connection is gone.
    pub async fn did_change_incremental(
        &self,
        doc: &Path,
        version: i32,
        edits: &[TextEdit],
    ) -> Result<(), LspError> {
        if self.capabilities().text_sync != karet_core::TextSyncKind::Incremental {
            return Err(LspError::Unsupported {
                method: "incremental textDocument/didChange",
            });
        }
        let params = lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier::new(
                uri::path_to_uri(doc)?,
                version,
            ),
            content_changes: edits
                .iter()
                .map(|edit| lsp_types::TextDocumentContentChangeEvent {
                    range: Some(convert::range_to_lsp(edit.range)),
                    range_length: None,
                    text: edit.new_text.clone(),
                })
                .collect(),
        };
        self.conn.notify("textDocument/didChange", params)
    }

    /// Notify the server that `doc` was saved (optionally including its text).
    ///
    /// # Errors
    /// Returns [`LspError::Protocol`] for an unconvertible path or a full
    /// outbound queue, or [`LspError::Closed`] if the connection is gone.
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
    /// Returns [`LspError::Protocol`] for an unconvertible path or a full
    /// outbound queue, or [`LspError::Closed`] if the connection is gone.
    pub async fn did_close(&self, doc: &Path) -> Result<(), LspError> {
        if let Ok(mut gate) = self.gate.write() {
            gate.closed(doc);
        }
        let params = lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
        };
        self.conn.notify("textDocument/didClose", params)
    }

    /// Request hover information at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn hover(&self, doc: &Path, pos: LineCol) -> Result<Option<Hover>, LspError> {
        self.require_for(ServerFeature::Hover, "textDocument/hover", doc)?;
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
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn document_symbols(&self, doc: &Path) -> Result<Vec<Symbol>, LspError> {
        self.require_for(
            ServerFeature::DocumentSymbol,
            "textDocument/documentSymbol",
            doc,
        )?;
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

    /// Search workspace symbols matching `query`, each with the file it is in.
    /// Ranges are in the server's UTF-16 columns, as everywhere in this crate.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for a request that does
    /// not encode or a result that does not decode, or [`LspError::Closed`] if
    /// the connection is gone.
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>, LspError> {
        self.require(ServerFeature::WorkspaceSymbol, "workspace/symbol")?;
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
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn implementations(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<Location>, LspError> {
        self.require_for(
            ServerFeature::Implementation,
            "textDocument/implementation",
            doc,
        )?;
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

    /// Resolve the definition location(s) of the symbol at `pos`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn definition(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        self.require_for(ServerFeature::Definition, "textDocument/definition", doc)?;
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::GotoDefinitionResponse> =
            self.conn.request("textDocument/definition", params).await?;
        Ok(convert::locations_from_lsp(response))
    }

    /// Rename the symbol at `pos` to `new_name`, returning the edits to apply,
    /// resource operations included.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode, a result that does not decode or
    /// an edit naming a URI that is not a file, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn rename(
        &self,
        doc: &Path,
        pos: LineCol,
        new_name: &str,
    ) -> Result<WorkspaceEdit, LspError> {
        self.require_for(ServerFeature::Rename, "textDocument/rename", doc)?;
        let params = lsp_types::RenameParams {
            text_document_position: text_document_position(doc, pos)?,
            new_name: new_name.to_owned(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<lsp_types::WorkspaceEdit> =
            self.conn.request("textDocument/rename", params).await?;
        response.map_or_else(
            || Ok(WorkspaceEdit::default()),
            convert::workspace_edit_from_lsp,
        )
    }

    /// Ask whether the symbol at `pos` can be renamed, and what to prefill the
    /// new name with. `Ok(None)` means the server says it cannot.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise `prepareProvider`, else [`LspError::Server`]
    /// (servers commonly explain a refusal this way), [`LspError::Timeout`],
    /// [`LspError::Protocol`] for an unconvertible path, a request that does
    /// not encode or a result that does not decode, or [`LspError::Closed`] if
    /// the connection is gone.
    pub async fn prepare_rename(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Option<PrepareRename>, LspError> {
        self.require_for(
            ServerFeature::PrepareRename,
            "textDocument/prepareRename",
            doc,
        )?;
        let response: Option<lsp_types::PrepareRenameResponse> = self
            .conn
            .request(
                "textDocument/prepareRename",
                text_document_position(doc, pos)?,
            )
            .await?;
        Ok(convert::prepare_rename_from_lsp(response))
    }

    /// Take the stream of the server's `workspace/applyEdit` requests.
    ///
    /// Returns the receiver once; every later call returns `None`, because each
    /// request has exactly one answerer. Until it is taken -- and after it is
    /// dropped -- every such request is answered `applied: false`, so a server
    /// never waits on a client that is not listening. Positions in the edits
    /// are UTF-16, as everywhere in this crate.
    #[must_use]
    pub fn apply_edit_requests(&self) -> Option<tokio::sync::mpsc::Receiver<ApplyEditRequest>> {
        self.conn.apply_edit_requests()
    }

    /// Request signature help at `pos` in `doc`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn signature_help(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Option<SignatureHelp>, LspError> {
        self.require_for(
            ServerFeature::SignatureHelp,
            "textDocument/signatureHelp",
            doc,
        )?;
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

    /// Request whole-document formatting edits for `doc`, indented as
    /// `indentation` says.
    ///
    /// `indentation` is the caller's resolved editor settings for this file,
    /// not a default: see [`Indentation`] for why the difference is visible in
    /// the result.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn formatting(
        &self,
        doc: &Path,
        indentation: Indentation,
    ) -> Result<Vec<TextEdit>, LspError> {
        self.require_for(ServerFeature::Formatting, "textDocument/formatting", doc)?;
        let params = lsp_types::DocumentFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            options: formatting_options(indentation),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<Vec<lsp_types::TextEdit>> =
            self.conn.request("textDocument/formatting", params).await?;
        Ok(convert::text_edits_from_lsp(response))
    }

    /// Whether the server currently offers `textDocument/formatting` for `doc`,
    /// from its handshake or a later registration covering it.
    ///
    /// Shorthand for [`Self::supports_for`] with [`ServerFeature::Formatting`]. A
    /// caller that has its own formatter to fall back on needs the answer
    /// before it decides, not after: [`Self::formatting`] would refuse, but a
    /// refusal is not the place to discover which formatter runs.
    #[must_use]
    pub fn supports_formatting(&self, doc: &Path) -> bool {
        self.supports_for(ServerFeature::Formatting, doc)
    }

    /// Request formatting edits for `range` in `doc`, indented as `indentation`
    /// says.
    ///
    /// Takes the same [`Indentation`] as [`Self::formatting`] for the same
    /// reason: a range format reindents the lines it rewrites.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn range_formatting(
        &self,
        doc: &Path,
        range: Range,
        indentation: Indentation,
    ) -> Result<Vec<TextEdit>, LspError> {
        self.require_for(
            ServerFeature::RangeFormatting,
            "textDocument/rangeFormatting",
            doc,
        )?;
        let params = lsp_types::DocumentRangeFormattingParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            range: convert::range_to_lsp(range),
            options: formatting_options(indentation),
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

    /// Subscribe to the server's refresh requests (see [`ServerRefresh`]).
    ///
    /// The request itself is answered by the client; a subscriber only has to
    /// drop what it cached. A slow subscriber loses only duplicates: every
    /// entry means the same thing.
    #[must_use]
    pub fn refreshes(&self) -> broadcast::Receiver<ServerRefresh> {
        self.conn.refreshes()
    }

    /// Subscribe to changes in what this server may be asked for which
    /// documents.
    ///
    /// Every `client/registerCapability` and `client/unregisterCapability`
    /// marks the receiver changed. A receiver starts with the current state
    /// seen, so a consumer that subscribes and then reads
    /// [`supports_for`](Self::supports_for) misses nothing: a registration
    /// that arrived earlier is in the gate, and one that arrives later wakes
    /// [`changed`](tokio::sync::watch::Receiver::changed).
    #[must_use]
    pub fn registrations(&self) -> tokio::sync::watch::Receiver<()> {
        self.conn.registrations()
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

/// `Ok` if `supported`, else the [`LspError::Unsupported`] refusal for `method`.
fn refuse_unless(
    supported: bool,
    feature: ServerFeature,
    method: &'static str,
) -> Result<(), LspError> {
    if supported {
        return Ok(());
    }
    tracing::debug!(
        method,
        ?feature,
        "refusing a request the server cannot answer"
    );
    Err(LspError::Unsupported { method })
}

/// Drain a child's stderr into the debug log and into `tail`.
///
/// The returned handle is the synchronization point a failed launch needs: the
/// tail holds the server's last words only once this task has read the pipe to
/// EOF.
fn drain_stderr(
    stderr: tokio::process::ChildStderr,
    command: String,
    tail: launch::StderrTail,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "karet_lsp::stderr", server = %command, "{line}");
            tail.push(line);
        }
    })
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

fn formatting_options(indentation: Indentation) -> lsp_types::FormattingOptions {
    lsp_types::FormattingOptions {
        tab_size: indentation.tab_size,
        insert_spaces: indentation.insert_spaces,
        ..lsp_types::FormattingOptions::default()
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
