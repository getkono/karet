//! `karet-lsp` — an async Language Server Protocol client for karet.
//!
//! Headless: connects to language servers over stdio and turns their responses
//! into neutral `karet-core` models (`Diagnostic`, `Symbol`, `CompletionItem`,
//! `Hover`, `InlayHint`, …), implementing [`SymbolProvider`]. Usable from a CLI or
//! a non-ratatui UI. (The ratatui completion/hover popups live in `karet-widgets`,
//! which renders these models, so this crate stays free of UI dependencies.)
//!
//! The transport is a hand-rolled `Content-Length`-framed JSON-RPC 2.0 codec over
//! generic async I/O: [`LspClient::spawn`] wraps a child process's stdio, and
//! [`LspClient::connect`] accepts any `AsyncRead`/`AsyncWrite` pair — the seam the
//! in-memory (`tokio::io::duplex`) tests and embedders use. A reader task
//! correlates responses by id, broadcasts pushed diagnostics, and answers the few
//! server→client requests a headless client must not leave hanging
//! (`workspace/configuration`, `client/registerCapability`,
//! `window/workDoneProgress/create`).
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
//! Transport, lifecycle, document sync, and completion are implemented. The
//! remaining typed request methods are being wired incrementally: the ones still
//! marked `todo!` in their bodies (`hover`, `definition`, `rename`, …) have final
//! signatures but panic if called.

mod codec;
mod conn;
mod convert;
mod jsonrpc;
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
use karet_core::SymbolProvider;
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
    #[error("failed to spawn language server")]
    Spawn,
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
        let mut child = tokio::process::Command::new(&spec.command)
            .args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                tracing::warn!(command = %spec.command, error = %e, "failed to spawn language server");
                LspError::Spawn
            })?;
        let stdin = child.stdin.take().ok_or(LspError::Spawn)?;
        let stdout = child.stdout.take().ok_or(LspError::Spawn)?;
        if let Some(stderr) = child.stderr.take() {
            let command = spec.command.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "karet_lsp::stderr", server = %command, "{line}");
                }
            });
        }
        let mut client = Self::connect(stdout, stdin, root).await?;
        client.child = Some(child);
        Ok(client)
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
    ///
    /// Ranges are in UTF-16 columns, per the crate-level position-encoding note.
    #[must_use]
    pub fn diagnostics(&self) -> broadcast::Receiver<(PathBuf, Vec<Diagnostic>)> {
        self.conn.diagnostics()
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
    use std::time::Duration;

    use karet_core::Severity;
    use serde_json::json;
    use tokio::io::DuplexStream;
    use tokio::io::ReadHalf;
    use tokio::io::WriteHalf;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// The scripted fake server side of an in-memory connection.
    struct FakeServer {
        reader: BufReader<ReadHalf<DuplexStream>>,
        writer: WriteHalf<DuplexStream>,
    }

    /// An in-memory wire: the client's `(read, write)` halves plus the fake
    /// server holding the other end.
    fn wire() -> (
        (ReadHalf<DuplexStream>, WriteHalf<DuplexStream>),
        FakeServer,
    ) {
        let (client_end, server_end) = tokio::io::duplex(1 << 20);
        let (client_read, client_write) = tokio::io::split(client_end);
        let (server_read, server_write) = tokio::io::split(server_end);
        (
            (client_read, client_write),
            FakeServer {
                reader: BufReader::new(server_read),
                writer: server_write,
            },
        )
    }

    impl FakeServer {
        /// Read one message, or `Null` on EOF/parse failure.
        async fn recv(&mut self) -> Value {
            match codec::read_frame(&mut self.reader).await {
                Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
                _ => Value::Null,
            }
        }

        async fn send(&mut self, message: &Value) {
            let bytes = serde_json::to_vec(message).unwrap_or_default();
            let _ = codec::write_frame(&mut self.writer, &bytes).await;
        }

        async fn respond(&mut self, id: &Value, result: Value) {
            self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
                .await;
        }

        /// Serve the `initialize`/`initialized` handshake, returning the
        /// `initialize` params for assertions.
        async fn handshake(&mut self) -> Value {
            let init = self.recv().await;
            assert_eq!(init["method"], "initialize");
            let id = init["id"].clone();
            self.respond(&id, json!({"capabilities": {}})).await;
            let initialized = self.recv().await;
            assert_eq!(initialized["method"], "initialized");
            init["params"].clone()
        }
    }

    #[tokio::test]
    async fn connect_negotiates_utf16_and_no_snippets() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            let params = server.handshake().await;
            assert_eq!(
                params["capabilities"]["general"]["positionEncodings"],
                json!(["utf-16"])
            );
            assert_eq!(
                params["capabilities"]["textDocument"]["completion"]["completionItem"]["snippetSupport"],
                json!(false)
            );
            assert_eq!(params["rootUri"], json!("file:///tmp/ws"));
            assert_eq!(
                params["workspaceFolders"][0]["uri"],
                json!("file:///tmp/ws")
            );
            assert_eq!(params["workspaceFolders"][0]["name"], json!("ws"));
        });
        let _client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn document_sync_notifications_reach_the_server() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            server.handshake().await;

            let open = server.recv().await;
            assert_eq!(open["method"], "textDocument/didOpen");
            let doc = &open["params"]["textDocument"];
            assert_eq!(doc["uri"], json!("file:///tmp/my%20ws/main.rs"));
            assert_eq!(doc["languageId"], json!("rust"));
            assert_eq!(doc["version"], json!(0));
            assert_eq!(doc["text"], json!("fn main() {}\n"));

            let change = server.recv().await;
            assert_eq!(change["method"], "textDocument/didChange");
            assert_eq!(change["params"]["textDocument"]["version"], json!(1));
            let changes = &change["params"]["contentChanges"];
            assert_eq!(changes.as_array().map(Vec::len), Some(1));
            // Full-text sync: the event carries only `text`, never a range.
            assert_eq!(changes[0], json!({"text": "fn main() { }\n"}));

            let save = server.recv().await;
            assert_eq!(save["method"], "textDocument/didSave");
            assert_eq!(save["params"]["text"], json!("fn main() { }\n"));

            let close = server.recv().await;
            assert_eq!(close["method"], "textDocument/didClose");
            assert_eq!(
                close["params"]["textDocument"]["uri"],
                json!("file:///tmp/my%20ws/main.rs")
            );
        });

        let client = LspClient::connect(read, write, Path::new("/tmp/my ws")).await?;
        let doc = Path::new("/tmp/my ws/main.rs");
        client.did_open(doc, "rust", 0, "fn main() {}\n").await?;
        client.did_change(doc, 1, "fn main() { }\n").await?;
        client.did_save(doc, Some("fn main() { }\n")).await?;
        client.did_close(doc).await?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn server_initiated_requests_are_answered() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            server.handshake().await;

            server
                .send(&json!({
                    "jsonrpc": "2.0", "id": 100, "method": "workspace/configuration",
                    "params": {"items": [{"section": "rust"}, {"section": "fmt"}]}
                }))
                .await;
            let answer = server.recv().await;
            assert_eq!(answer["id"], json!(100));
            assert_eq!(answer["result"], json!([null, null]));

            // String ids must be echoed verbatim.
            server
                .send(&json!({
                    "jsonrpc": "2.0", "id": "reg-1", "method": "client/registerCapability",
                    "params": {"registrations": []}
                }))
                .await;
            let answer = server.recv().await;
            assert_eq!(answer["id"], json!("reg-1"));
            assert_eq!(answer["result"], json!(null));

            server
                .send(&json!({
                    "jsonrpc": "2.0", "id": 101, "method": "window/workDoneProgress/create",
                    "params": {"token": "t"}
                }))
                .await;
            let answer = server.recv().await;
            assert_eq!(answer["result"], json!(null));

            server
                .send(&json!({
                    "jsonrpc": "2.0", "id": 102, "method": "window/showMessageRequest",
                    "params": {"type": 1, "message": "hi"}
                }))
                .await;
            let answer = server.recv().await;
            assert_eq!(answer["id"], json!(102));
            assert_eq!(answer["error"]["code"], json!(-32601));
        });

        let _client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn published_diagnostics_are_broadcast_and_mapped() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            server.handshake().await;
            server
                .send(&json!({
                    "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": "file:///tmp/ws/a.rs",
                        "diagnostics": [{
                            "range": {"start": {"line": 2, "character": 4},
                                      "end": {"line": 2, "character": 9}},
                            "severity": 2,
                            "message": "unused variable",
                            "source": "rustc",
                            "tags": [1]
                        }]
                    }
                }))
                .await;
            server // an unknown notification must be ignored without breaking the stream
                .send(&json!({"jsonrpc": "2.0", "method": "window/logMessage",
                              "params": {"type": 3, "message": "noise"}}))
                .await;
        });

        let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
        let mut rx = client.diagnostics();
        let (path, diags) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await??;
        assert_eq!(path, PathBuf::from("/tmp/ws/a.rs"));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].message, "unused variable");
        assert_eq!(diags[0].range.start, LineCol::new(2, 4));
        assert_eq!(diags[0].range.end, LineCol::new(2, 9));
        assert_eq!(diags[0].tags, vec![karet_core::DiagnosticTag::Unnecessary]);
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn responses_correlate_out_of_order() -> TestResult {
        let ((read, write), mut server) = wire();
        let connection = conn::Connection::start(read, write);
        let server_task = tokio::spawn(async move {
            let first = server.recv().await;
            let second = server.recv().await;
            assert_eq!(first["method"], "test/one");
            assert_eq!(second["method"], "test/two");
            // Answer in reverse order.
            let second_id = second["id"].clone();
            let first_id = first["id"].clone();
            server.respond(&second_id, json!("two")).await;
            server.respond(&first_id, json!("one")).await;
        });

        let (one, two) = tokio::join!(
            connection.request::<_, String>("test/one", Value::Null),
            connection.request::<_, String>("test/two", Value::Null),
        );
        assert_eq!(one?, "one");
        assert_eq!(two?, "two");
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn error_responses_map_to_server_errors() -> TestResult {
        let ((read, write), mut server) = wire();
        let connection = conn::Connection::start(read, write);
        let server_task = tokio::spawn(async move {
            let req = server.recv().await;
            server
                .send(&json!({"jsonrpc": "2.0", "id": req["id"],
                              "error": {"code": -32000, "message": "boom"}}))
                .await;
        });
        let err = connection
            .request::<_, Value>("test/fails", Value::Null)
            .await;
        let Err(LspError::Server(message)) = err else {
            return Err("expected a server error".into());
        };
        assert!(message.contains("boom"), "unexpected message: {message}");
        assert!(message.contains("-32000"), "unexpected message: {message}");
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn unanswered_requests_time_out() -> TestResult {
        let ((read, write), server) = wire();
        let connection = conn::Connection::start(read, write);
        // Keep the server end alive but silent, so the failure is a timeout,
        // not a closed connection.
        let err = connection
            .request_with::<_, Value>("test/silence", Value::Null, Duration::from_millis(50))
            .await;
        assert!(matches!(err, Err(LspError::Timeout)));
        drop(server);
        Ok(())
    }

    #[tokio::test]
    async fn eof_fails_in_flight_requests_with_closed() -> TestResult {
        let ((read, write), mut server) = wire();
        let connection = conn::Connection::start(read, write);
        let server_task = tokio::spawn(async move {
            let req = server.recv().await;
            assert_eq!(req["method"], "test/doomed");
            drop(server); // hang up without answering
        });
        let err = connection
            .request::<_, Value>("test/doomed", Value::Null)
            .await;
        assert!(matches!(err, Err(LspError::Closed)));
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn requests_after_eof_fail_fast_with_closed() -> TestResult {
        let ((read, write), server) = wire();
        let connection = conn::Connection::start(read, write);
        drop(server); // the server is gone before any request is issued
        // Give the reader task a chance to observe the EOF.
        tokio::task::yield_now().await;
        // A generous deadline proves we do NOT wait it out: the request must
        // fail promptly with Closed, not eventually with Timeout.
        let started = std::time::Instant::now();
        let err = connection
            .request_with::<_, Value>("test/late", Value::Null, Duration::from_secs(30))
            .await;
        assert!(matches!(err, Err(LspError::Closed)), "got {err:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        Ok(())
    }

    #[tokio::test]
    async fn malformed_json_frames_are_skipped() -> TestResult {
        let ((read, write), mut server) = wire();
        let connection = conn::Connection::start(read, write);
        let server_task = tokio::spawn(async move {
            let req = server.recv().await;
            // A well-framed but non-JSON body must not kill the connection …
            let _ = codec::write_frame(&mut server.writer, b"this is not json").await;
            // … nor a JSON body with no JSON-RPC shape.
            server.send(&json!(["still", "not", "jsonrpc"])).await;
            let id = req["id"].clone();
            server.respond(&id, json!("survived")).await;
        });
        let result: String = connection.request("test/resilient", Value::Null).await?;
        assert_eq!(result, "survived");
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn completion_end_to_end() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            server.handshake().await;

            // 1: a CompletionList (isIncomplete flattened) with a snippet edit.
            let req = server.recv().await;
            assert_eq!(req["method"], "textDocument/completion");
            assert_eq!(
                req["params"]["textDocument"]["uri"],
                json!("file:///tmp/ws/a.rs")
            );
            // UTF-16 position passthrough.
            assert_eq!(
                req["params"]["position"],
                json!({"line": 3, "character": 7})
            );
            let id = req["id"].clone();
            server
                .respond(
                    &id,
                    json!({
                        "isIncomplete": true,
                        "items": [
                            {
                                "label": "push",
                                "kind": 2,
                                "detail": "fn push(&mut self, ch: char)",
                                "sortText": "0000",
                                "insertTextFormat": 2,
                                "textEdit": {
                                    "range": {"start": {"line": 3, "character": 5},
                                              "end": {"line": 3, "character": 7}},
                                    "newText": "push(${1:ch})$0"
                                },
                                "tags": [1]
                            },
                            {"label": "plain"}
                        ]
                    }),
                )
                .await;

            // 2: a bare array response.
            let req = server.recv().await;
            let id = req["id"].clone();
            server.respond(&id, json!([{"label": "sole"}])).await;

            // 3: a null response (no completions).
            let req = server.recv().await;
            let id = req["id"].clone();
            server.respond(&id, Value::Null).await;
        });

        let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
        let doc = Path::new("/tmp/ws/a.rs");

        let items = client.completion(doc, LineCol::new(3, 7)).await?;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "push");
        assert_eq!(items[0].kind, karet_core::CompletionKind::Method);
        assert_eq!(
            items[0].detail.as_deref(),
            Some("fn push(&mut self, ch: char)")
        );
        assert_eq!(items[0].sort_text.as_deref(), Some("0000"));
        assert_eq!(items[0].insert_text, "push(ch)"); // snippet degraded
        assert!(items[0].deprecated); // via tag
        let edit = items[0].edit.clone().ok_or("expected a text edit")?;
        assert_eq!(edit.range.start, LineCol::new(3, 5));
        assert_eq!(edit.range.end, LineCol::new(3, 7));
        assert_eq!(edit.new_text, "push(ch)");
        assert_eq!(items[1].label, "plain");
        assert_eq!(items[1].insert_text, "plain"); // label fallback
        assert_eq!(items[1].kind, karet_core::CompletionKind::Text);

        let items = client.completion(doc, LineCol::new(0, 0)).await?;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "sole");

        let items = client.completion(doc, LineCol::new(0, 0)).await?;
        assert!(items.is_empty());

        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_performs_the_handshake() -> TestResult {
        let ((read, write), mut server) = wire();
        let server_task = tokio::spawn(async move {
            server.handshake().await;
            let shutdown = server.recv().await;
            assert_eq!(shutdown["method"], "shutdown");
            let id = shutdown["id"].clone();
            server.respond(&id, Value::Null).await;
            let exit = server.recv().await;
            assert_eq!(exit["method"], "exit");
        });
        let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
        client.shutdown().await?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn spawn_missing_binary_is_a_spawn_error() {
        let spec = LspSpec {
            command: "karet-lsp-test-no-such-binary".into(),
            args: vec![],
            languages: vec!["rust".into()],
        };
        let err = LspClient::spawn(spec, Path::new("/tmp")).await;
        assert!(matches!(err, Err(LspError::Spawn)));
    }

    #[test]
    fn provider_wraps_symbols() {
        let ds = DocumentSymbols::new(Vec::new());
        assert!(ds.symbols().is_empty());
    }

    #[test]
    fn error_displays() {
        assert_eq!(LspError::Timeout.to_string(), "request timed out");
        assert_eq!(
            LspError::Closed.to_string(),
            "connection to the language server closed"
        );
        assert_eq!(
            LspError::Protocol("bad frame".into()).to_string(),
            "protocol error: bad frame"
        );
    }
}
