//! LSP orchestration: lazy per-language server tasks and completion serving.
//!
//! The [`LspManager`] lives on the session actor and owns one background task per
//! language. A task is spawned lazily on the first open of a matching document; it
//! owns the [`LspClient`], serializes document sync ahead of requests (a
//! completion always sees the latest text), debounces full-text `didChange`
//! forwards, and reports back to the actor on an [`LspUpdate`] channel — the
//! session converts positions (LSP UTF-16 ↔ buffer UTF-32) there, where the
//! buffer lives, and emits the answering [`Event`](crate::api::Event).
//!
//! Failure policy: a server that cannot spawn (missing binary) is reported
//! **once** and its task thereafter answers completion requests with an empty
//! set — the manager keeps the entry, so re-opening documents never causes a
//! respawn storm. A server that dies mid-session is likewise reported once and
//! its language goes quiet until the session restarts.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use karet_core::CompletionItem;
use karet_core::LineCol;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
use tokio::sync::mpsc;

use crate::api::DocumentId;
use crate::api::RequestId;
use crate::config::schema::Lsp as LspSettings;

/// How long an edited document may sit before its full text is forwarded as
/// `didChange`. A pending forward is also flushed immediately ahead of any
/// request, so completions never see stale text.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(150);

/// A command for one per-language server task.
pub(crate) enum ServerCmd {
    /// Forward `textDocument/didOpen`.
    DidOpen {
        /// The document path.
        path: PathBuf,
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
}

/// A result flowing from a server task back to the session actor.
pub(crate) enum LspUpdate {
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
    /// The server binary could not be started (reported once per language).
    SpawnFailed {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The language the server was for.
        language: String,
        /// The executable that failed to start.
        command: String,
    },
    /// A running server's connection closed (reported once per language).
    ServerDied {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The language whose server died.
        language: String,
    },
}

/// How the manager establishes a client for a spec — [`LspClient::spawn`] in
/// production; tests inject an in-memory duplex connection instead.
pub(crate) type Connector = Arc<
    dyn Fn(LspSpec, PathBuf) -> Pin<Box<dyn Future<Output = Result<LspClient, LspError>> + Send>>
        + Send
        + Sync,
>;

/// The production connector: spawn the server process on `PATH`.
fn spawn_connector() -> Connector {
    Arc::new(|spec, root| Box::pin(async move { LspClient::spawn(spec, &root).await }))
}

/// The built-in default servers, used when `lsp.servers` has no entry for a
/// language. Keys are lowercase language names (the same keys user config uses).
fn builtin_spec(language: &str) -> Option<LspSpec> {
    let (command, args): (&str, &[&str]) = match language {
        "rust" => ("rust-analyzer", &[]),
        "typescript" | "javascript" => ("typescript-language-server", &["--stdio"]),
        "python" => ("pyright-langserver", &["--stdio"]),
        _ => return None,
    };
    Some(LspSpec {
        command: command.to_owned(),
        args: args.iter().map(|&a| a.to_owned()).collect(),
        languages: vec![language.to_owned()],
    })
}

/// The lookup/settings key for a document's display language (`"Rust"` →
/// `"rust"`), doubling as the LSP `languageId`.
fn language_key(language: Option<&str>) -> Option<String> {
    language.map(str::to_ascii_lowercase)
}

/// Clamp a buffer version into LSP's `i32` version space (monotonic for any
/// realistic session; documents do not see 2³¹ edits).
fn version_i32(version: u64) -> i32 {
    i32::try_from(version % 2_147_483_647).unwrap_or(0)
}

/// Lazy per-language language-server orchestration (see the module docs).
pub(crate) struct LspManager {
    settings: LspSettings,
    generation: u64,
    root: Option<PathBuf>,
    servers: HashMap<String, mpsc::UnboundedSender<ServerCmd>>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
}

impl LspManager {
    /// Create a manager and the update stream the actor drains.
    pub(crate) fn new(
        settings: LspSettings,
        root: Option<PathBuf>,
    ) -> (Self, mpsc::UnboundedReceiver<LspUpdate>) {
        let (updates, rx) = mpsc::unbounded_channel();
        (
            Self {
                settings,
                generation: 0,
                root,
                servers: HashMap::new(),
                updates,
                connector: spawn_connector(),
            },
            rx,
        )
    }

    /// Replace the connector (tests inject an in-memory server here).
    #[cfg(test)]
    pub(crate) fn set_connector(&mut self, connector: Connector) {
        self.connector = connector;
    }

    /// Apply new settings, retiring every task created under the old snapshot.
    /// Returns whether documents need to be reopened against fresh servers.
    pub(crate) fn reconfigure(&mut self, settings: LspSettings) -> bool {
        if self.settings == settings {
            return false;
        }
        self.settings = settings;
        self.generation = self.generation.wrapping_add(1);
        self.servers.clear();
        true
    }

    /// Whether an asynchronous update belongs to the current server generation.
    pub(crate) fn accepts(&self, update: &LspUpdate) -> bool {
        let generation = match update {
            LspUpdate::Completions { generation, .. }
            | LspUpdate::SpawnFailed { generation, .. }
            | LspUpdate::ServerDied { generation, .. } => *generation,
        };
        generation == self.generation
    }

    /// The launch spec for `language`: user config first, then the built-ins.
    fn spec_for(&self, language: &str) -> Option<LspSpec> {
        if let Some(server) = self.settings.servers.get(language) {
            return Some(LspSpec {
                command: server.command.clone(),
                args: server.args.clone(),
                languages: vec![language.to_owned()],
            });
        }
        builtin_spec(language)
    }

    /// The task inbox for `language`, spawning the server task on first use.
    /// `None` when LSP is disabled or no server is configured for the language.
    fn ensure_server(
        &mut self,
        language: Option<&str>,
    ) -> Option<(&mpsc::UnboundedSender<ServerCmd>, String)> {
        if !self.settings.enabled {
            return None;
        }
        let key = language_key(language)?;
        if !self.servers.contains_key(&key) {
            let spec = self.spec_for(&key)?;
            // Server tasks need an async runtime; a session driven synchronously
            // (unit tests, bare library use) simply runs without LSP.
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::unbounded_channel();
            let root = self.root.clone().or_else(|| std::env::current_dir().ok())?;
            handle.spawn(server_task(
                spec,
                root,
                key.clone(),
                rx,
                self.updates.clone(),
                Arc::clone(&self.connector),
                self.generation,
            ));
            self.servers.insert(key.clone(), tx);
        }
        self.servers.get(&key).map(|tx| (tx, key))
    }

    /// The running task inbox for `language`, when one was already spawned.
    fn existing_server(&self, language: Option<&str>) -> Option<&mpsc::UnboundedSender<ServerCmd>> {
        if !self.settings.enabled {
            return None;
        }
        self.servers.get(&language_key(language)?)
    }

    /// Forward a document open, lazily starting the language's server. `text`
    /// is only invoked when a server will actually receive it.
    pub(crate) fn document_opened(
        &mut self,
        language: Option<&str>,
        path: &Path,
        version: u64,
        text: impl FnOnce() -> String,
    ) {
        let Some((tx, _key)) = self.ensure_server(language) else {
            return;
        };
        let _ = tx.send(ServerCmd::DidOpen {
            path: path.to_path_buf(),
            version: version_i32(version),
            text: text(),
        });
    }

    /// Forward an edit (full text, debounced by the server task). A no-op for
    /// languages without a running server.
    pub(crate) fn document_changed(
        &mut self,
        language: Option<&str>,
        path: &Path,
        version: u64,
        text: impl FnOnce() -> String,
    ) {
        let Some(tx) = self.existing_server(language) else {
            return;
        };
        let _ = tx.send(ServerCmd::DidChange {
            path: path.to_path_buf(),
            version: version_i32(version),
            text: text(),
        });
    }

    /// Forward a document close. A no-op for languages without a running server.
    pub(crate) fn document_closed(&mut self, language: Option<&str>, path: &Path) {
        let Some(tx) = self.existing_server(language) else {
            return;
        };
        let _ = tx.send(ServerCmd::DidClose {
            path: path.to_path_buf(),
        });
    }

    /// Forward a completion request (`position` already in UTF-16 columns).
    /// Returns whether it was forwarded — when `false`, no server serves this
    /// language and the caller must answer the request itself (empty set).
    pub(crate) fn completion(
        &mut self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        position: LineCol,
    ) -> bool {
        let Some(tx) = self.existing_server(language) else {
            return false;
        };
        tx.send(ServerCmd::Completion {
            request,
            doc,
            version,
            path: path.to_path_buf(),
            position,
        })
        .is_ok()
    }
}

/// Answer a completion command with an empty set (used whenever no live server
/// can answer, so the client is never left waiting).
fn answer_empty(updates: &mpsc::UnboundedSender<LspUpdate>, cmd: ServerCmd, generation: u64) {
    if let ServerCmd::Completion {
        request,
        doc,
        version,
        ..
    } = cmd
    {
        let _ = updates.send(LspUpdate::Completions {
            generation,
            request,
            doc,
            version,
            items: Vec::new(),
        });
    }
}

/// The per-language server task: connect, then serialize document sync and
/// requests against the client (see the module docs for the failure policy).
async fn server_task(
    spec: LspSpec,
    root: PathBuf,
    language: String,
    mut rx: mpsc::UnboundedReceiver<ServerCmd>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
    generation: u64,
) {
    let client = match connector(spec.clone(), root).await {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(language, command = %spec.command, error = %e, "language server failed to start");
            let _ = updates.send(LspUpdate::SpawnFailed {
                generation,
                language,
                command: spec.command,
            });
            // Stay alive answering requests empty; the manager keeps this entry,
            // so the failure is remembered and nothing respawns.
            while let Some(cmd) = rx.recv().await {
                answer_empty(&updates, cmd, generation);
            }
            return;
        },
    };

    // The one pending (coalesced) didChange, flushed on a quiet period or
    // immediately ahead of any other traffic.
    let mut pending: Option<(PathBuf, i32, String)> = None;
    let mut dead = false;
    loop {
        let cmd = if pending.is_some() && !dead {
            match tokio::time::timeout(CHANGE_DEBOUNCE, rx.recv()).await {
                Ok(cmd) => cmd,
                Err(_quiet) => {
                    flush_pending(
                        &client,
                        &mut pending,
                        &mut dead,
                        &updates,
                        &language,
                        generation,
                    )
                    .await;
                    continue;
                },
            }
        } else {
            rx.recv().await
        };
        let Some(cmd) = cmd else {
            break; // the session dropped the manager
        };
        if dead {
            answer_empty(&updates, cmd, generation);
            continue;
        }
        match cmd {
            ServerCmd::DidChange {
                path,
                version,
                text,
            } => {
                // Coalesce successive edits to the same document; an edit to a
                // different document flushes the previous one first (order).
                if pending.as_ref().is_some_and(|(p, ..)| *p != path) {
                    flush_pending(
                        &client,
                        &mut pending,
                        &mut dead,
                        &updates,
                        &language,
                        generation,
                    )
                    .await;
                }
                if !dead {
                    pending = Some((path, version, text));
                }
            },
            ServerCmd::DidOpen {
                path,
                version,
                text,
            } => {
                flush_pending(
                    &client,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                if !dead {
                    let result = client.did_open(&path, &language, version, &text).await;
                    note_failure(result, &mut dead, &updates, &language, generation);
                }
            },
            ServerCmd::DidClose { path } => {
                flush_pending(
                    &client,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                if !dead {
                    let result = client.did_close(&path).await;
                    note_failure(result, &mut dead, &updates, &language, generation);
                }
            },
            ServerCmd::Completion {
                request,
                doc,
                version,
                path,
                position,
            } => {
                // The server must see the latest text before completing in it.
                flush_pending(
                    &client,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let items = if dead {
                    Vec::new()
                } else {
                    match client.completion(&path, position).await {
                        Ok(items) => items,
                        Err(e) => {
                            note_failure::<()>(Err(e), &mut dead, &updates, &language, generation);
                            Vec::new()
                        },
                    }
                };
                let _ = updates.send(LspUpdate::Completions {
                    generation,
                    request,
                    doc,
                    version,
                    items,
                });
            },
        }
    }
    // Session shutdown: hang up politely.
    let _ = client.shutdown().await;
}

/// Send the pending `didChange`, if any.
async fn flush_pending(
    client: &LspClient,
    pending: &mut Option<(PathBuf, i32, String)>,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    language: &str,
    generation: u64,
) {
    if *dead {
        *pending = None;
        return;
    }
    if let Some((path, version, text)) = pending.take() {
        let result = client.did_change(&path, version, &text).await;
        note_failure(result, dead, updates, language, generation);
    }
}

/// Record a client-call failure: a closed connection kills the server slot
/// (reported once); other errors are logged and the task keeps going.
fn note_failure<T>(
    result: Result<T, LspError>,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    language: &str,
    generation: u64,
) {
    match result {
        Ok(_) => {},
        Err(LspError::Closed) => {
            if !*dead {
                *dead = true;
                let _ = updates.send(LspUpdate::ServerDied {
                    generation,
                    language: language.to_owned(),
                });
            }
        },
        Err(e) => {
            tracing::warn!(language, error = %e, "language server call failed");
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use karet_core::Change;
    use karet_core::NotificationKind;
    use karet_core::Range;
    use karet_core::TextEdit;
    use karet_text::EditCause;
    use serde_json::Value;
    use serde_json::json;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::io::DuplexStream;
    use tokio::io::ReadHalf;
    use tokio::io::WriteHalf;

    use super::*;
    use crate::api::Command;
    use crate::api::Event;
    use crate::backend::Backend;
    use crate::backend::local;
    use crate::session::EventRx;
    use crate::session::Session;
    use crate::session::SessionConfig;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn reconfigure_retires_updates_from_old_server_tasks() {
        let (mut manager, _updates) = LspManager::new(LspSettings::default(), None);
        let old = LspUpdate::Completions {
            generation: 0,
            request: RequestId(1),
            doc: DocumentId(1),
            version: 1,
            items: Vec::new(),
        };
        assert!(manager.accepts(&old));

        let settings = LspSettings {
            enabled: false,
            ..LspSettings::default()
        };
        assert!(manager.reconfigure(settings.clone()));
        assert!(!manager.accepts(&old));
        assert!(
            !manager.reconfigure(settings),
            "an identical snapshot is a no-op"
        );
    }

    // --- a minimal LSP wire for the fake server (framing + JSON) -----------

    async fn read_msg(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Option<Value> {
        let mut len: Option<usize> = None;
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line).await.ok()? == 0 {
                return None;
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end();
            if text.is_empty() {
                break;
            }
            if let Some(value) = text.strip_prefix("Content-Length:") {
                len = value.trim().parse().ok();
            }
        }
        let mut body = vec![0_u8; len?];
        reader.read_exact(&mut body).await.ok()?;
        serde_json::from_slice(&body).ok()
    }

    async fn write_msg(writer: &mut WriteHalf<DuplexStream>, message: &Value) {
        let body = serde_json::to_vec(message).unwrap_or_default();
        let head = format!("Content-Length: {}\r\n\r\n", body.len());
        let _ = writer.write_all(head.as_bytes()).await;
        let _ = writer.write_all(&body).await;
        let _ = writer.flush().await;
    }

    /// What the scripted server should do after the initialize handshake.
    #[derive(Clone, Copy)]
    enum Behavior {
        /// Serve completions; echo every received message to `observed`.
        Normal,
        /// Hang up right after the handshake (a crashing server).
        DieAfterHandshake,
    }

    /// A connector that runs a scripted in-memory server per "spawn".
    fn test_connector(
        behavior: Behavior,
        observed: Option<mpsc::UnboundedSender<Value>>,
        spawns: Arc<AtomicUsize>,
    ) -> Connector {
        Arc::new(move |_spec, root| {
            let observed = observed.clone();
            let spawns = Arc::clone(&spawns);
            Box::pin(async move {
                spawns.fetch_add(1, Ordering::SeqCst);
                let (client_end, server_end) = tokio::io::duplex(1 << 20);
                let (server_read, mut server_write) = tokio::io::split(server_end);
                tokio::spawn(async move {
                    let mut reader = BufReader::new(server_read);
                    // Handshake.
                    let Some(init) = read_msg(&mut reader).await else {
                        return;
                    };
                    write_msg(
                        &mut server_write,
                        &json!({"jsonrpc": "2.0", "id": init["id"],
                                "result": {"capabilities": {}}}),
                    )
                    .await;
                    let _initialized = read_msg(&mut reader).await;
                    if matches!(behavior, Behavior::DieAfterHandshake) {
                        return; // both halves drop: the client sees EOF
                    }
                    while let Some(msg) = read_msg(&mut reader).await {
                        if let Some(tx) = &observed {
                            let _ = tx.send(msg.clone());
                        }
                        match msg["method"].as_str() {
                            Some("textDocument/completion") => {
                                // A fixed item whose textEdit range is in UTF-16:
                                // chars 2..4 on the requested line.
                                let line = msg["params"]["position"]["line"].clone();
                                write_msg(
                                    &mut server_write,
                                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": [{
                                        "label": "emoji_aware",
                                        "kind": 5,
                                        "textEdit": {
                                            "range": {
                                                "start": {"line": line, "character": 2},
                                                "end": {"line": line, "character": 4}
                                            },
                                            "newText": "emoji_aware"
                                        }
                                    }]}),
                                )
                                .await;
                            },
                            Some("shutdown") => {
                                write_msg(
                                    &mut server_write,
                                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}),
                                )
                                .await;
                            },
                            Some("exit") => break,
                            _ => {},
                        }
                    }
                });
                let (read, write) = tokio::io::split(client_end);
                LspClient::connect(read, write, &root).await
            })
        })
    }

    /// A connector that always fails as if the binary were missing.
    fn failing_connector(spawns: Arc<AtomicUsize>) -> Connector {
        Arc::new(move |_spec, _root| {
            spawns.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(LspError::Spawn) })
        })
    }

    // --- session-level helpers ---------------------------------------------

    fn rust_file(dir: &tempfile::TempDir, name: &str, text: &str) -> Option<PathBuf> {
        let path = dir.path().join(name);
        std::fs::write(&path, text).ok()?;
        Some(path)
    }

    async fn next_event(events: &mut EventRx) -> Option<(Option<RequestId>, Event)> {
        tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .ok()
            .flatten()
    }

    async fn await_opened(events: &mut EventRx) -> Option<(DocumentId, u64)> {
        while let Some((_, event)) = next_event(events).await {
            if let Event::Opened { doc, version } = event {
                return Some((doc, version));
            }
        }
        None
    }

    async fn await_completions(
        events: &mut EventRx,
    ) -> Option<(Option<RequestId>, DocumentId, u64, Vec<CompletionItem>)> {
        while let Some((rid, event)) = next_event(events).await {
            if let Event::Completions {
                doc,
                version,
                items,
            } = event
            {
                return Some((rid, doc, version, items));
            }
        }
        None
    }

    fn session_with_connector(connector: Connector) -> (Session, EventRx) {
        let (mut session, events, _snaps) = Session::new(SessionConfig::default());
        session.set_lsp_connector(connector);
        (session, events)
    }

    // --- the tests -----------------------------------------------------------

    #[tokio::test]
    async fn completion_round_trips_with_utf16_conversion() -> TestResult {
        let dir = tempfile::tempdir()?;
        // '😀' is 1 buffer column but 2 UTF-16 units.
        let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
        let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
        let spawns = Arc::new(AtomicUsize::new(0));
        let (session, mut events) =
            session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
        let backend = local(session);

        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
        let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

        // Caret after "😀ab" = buffer col 3.
        let request = backend.next_id();
        backend.send(
            request,
            Command::Completion {
                doc,
                position: LineCol::new(0, 3),
            },
        )?;

        let (rid, cdoc, cversion, items) = await_completions(&mut events)
            .await
            .ok_or("no Completions event")?;
        assert_eq!(rid, Some(request), "answer tagged with the request id");
        assert_eq!(cdoc, doc);
        assert_eq!(cversion, version);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "emoji_aware");
        // The server's UTF-16 range 2..4 is buffer cols 1..3 (after the emoji).
        let edit = items[0].edit.clone().ok_or("expected an edit")?;
        assert_eq!(edit.range.start, LineCol::new(0, 1));
        assert_eq!(edit.range.end, LineCol::new(0, 3));

        // And the outgoing request carried the UTF-16 position (col 3 → 4).
        let mut saw_utf16 = false;
        while let Ok(msg) = tokio::time::timeout(Duration::from_secs(5), observed_rx.recv()).await {
            let Some(msg) = msg else { break };
            if msg["method"] == "textDocument/completion" {
                assert_eq!(
                    msg["params"]["position"],
                    json!({"line": 0, "character": 4})
                );
                saw_utf16 = true;
                break;
            }
        }
        assert!(saw_utf16, "the completion request should reach the server");
        Ok(())
    }

    #[tokio::test]
    async fn open_and_debounced_changes_reach_the_server() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = rust_file(&dir, "lib.rs", "fn a() {}\n").ok_or("write failed")?;
        let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
        let spawns = Arc::new(AtomicUsize::new(0));
        let (session, mut events) =
            session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
        let backend = local(session);

        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
        let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

        // Two rapid single-char inserts; the debounce coalesces them.
        for (i, ch) in ["x", "y"].iter().enumerate() {
            let range =
                Range::new(LineCol::new(1, 0), LineCol::new(1, 0)).map_err(|e| format!("{e}"))?;
            backend.send(
                backend.next_id(),
                Command::ApplyChange {
                    doc,
                    change: Change::new(
                        version + i as u64,
                        vec![TextEdit {
                            range,
                            new_text: (*ch).to_owned(),
                        }],
                    ),
                    cause: EditCause::Type,
                },
            )?;
        }
        // A completion flushes the pending change ahead of itself.
        backend.send(
            backend.next_id(),
            Command::Completion {
                doc,
                position: LineCol::new(1, 1),
            },
        )?;
        let _ = await_completions(&mut events).await.ok_or("no answer")?;

        // Server-side order: didOpen (with the original text), then didChange(s)
        // whose final text is the current buffer, then the completion.
        let mut methods = Vec::new();
        let mut last_change_text = String::new();
        while let Ok(Some(msg)) =
            tokio::time::timeout(Duration::from_secs(5), observed_rx.recv()).await
        {
            let method = msg["method"].as_str().unwrap_or_default().to_owned();
            if method == "textDocument/didChange" {
                last_change_text = msg["params"]["contentChanges"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
            }
            if method == "textDocument/didOpen" {
                assert_eq!(msg["params"]["textDocument"]["text"], json!("fn a() {}\n"));
                assert_eq!(msg["params"]["textDocument"]["languageId"], json!("rust"));
            }
            let done = method == "textDocument/completion";
            methods.push(method);
            if done {
                break;
            }
        }
        assert_eq!(
            methods.first().map(String::as_str),
            Some("textDocument/didOpen")
        );
        assert_eq!(
            methods.last().map(String::as_str),
            Some("textDocument/completion")
        );
        assert!(
            methods.iter().any(|m| m == "textDocument/didChange"),
            "edits must be forwarded, got {methods:?}"
        );
        // Both single-char inserts are visible in the last forwarded text.
        assert_eq!(last_change_text, "fn a() {}\nyx");
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_language_answers_empty_immediately() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = rust_file(&dir, "notes.txt", "plain text\n").ok_or("write failed")?;
        let spawns = Arc::new(AtomicUsize::new(0));
        let (session, mut events) =
            session_with_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
        let backend = local(session);

        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
        let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
        let request = backend.next_id();
        backend.send(
            request,
            Command::Completion {
                doc,
                position: LineCol::new(0, 0),
            },
        )?;
        let (rid, cdoc, cversion, items) =
            await_completions(&mut events).await.ok_or("no answer")?;
        assert_eq!(rid, Some(request));
        assert_eq!((cdoc, cversion), (doc, version));
        assert!(items.is_empty());
        assert_eq!(spawns.load(Ordering::SeqCst), 0, "no server for .txt");
        Ok(())
    }

    #[tokio::test]
    async fn disabled_setting_spawns_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
        let spawns = Arc::new(AtomicUsize::new(0));
        let mut config = SessionConfig::default();
        config.settings.lsp.enabled = false;
        let (mut session, mut events, _snaps) = Session::new(config);
        session.set_lsp_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
        let backend = local(session);

        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
        let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
        let request = backend.next_id();
        backend.send(
            request,
            Command::Completion {
                doc,
                position: LineCol::new(0, 0),
            },
        )?;
        let (rid, _, _, items) = await_completions(&mut events).await.ok_or("no answer")?;
        assert_eq!(rid, Some(request));
        assert!(items.is_empty());
        assert_eq!(spawns.load(Ordering::SeqCst), 0, "disabled means no spawns");
        Ok(())
    }

    #[tokio::test]
    async fn missing_binary_warns_once_and_answers_empty() -> TestResult {
        let dir = tempfile::tempdir()?;
        let first = rust_file(&dir, "a.rs", "fn a() {}\n").ok_or("write failed")?;
        let second = rust_file(&dir, "b.rs", "fn b() {}\n").ok_or("write failed")?;
        let spawns = Arc::new(AtomicUsize::new(0));
        let (session, mut events) = session_with_connector(failing_connector(Arc::clone(&spawns)));
        let backend = local(session);

        // Two documents of the same language: one spawn attempt, one warning.
        for path in [first, second] {
            backend.send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None,
                },
            )?;
        }
        let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
        let request = backend.next_id();
        backend.send(
            request,
            Command::Completion {
                doc,
                position: LineCol::new(0, 0),
            },
        )?;

        // Drain until the completion answer; count LSP warnings seen on the way.
        let mut lsp_warnings = 0;
        let mut answered = false;
        while let Some((rid, event)) = next_event(&mut events).await {
            match event {
                Event::Notification {
                    kind: NotificationKind::Lsp,
                    ..
                } => lsp_warnings += 1,
                Event::Completions { items, .. } => {
                    assert_eq!(rid, Some(request));
                    assert!(items.is_empty());
                    answered = true;
                    break;
                },
                _ => {},
            }
        }
        assert!(answered, "a dead server must still answer completions");
        assert_eq!(lsp_warnings, 1, "exactly one missing-binary warning");
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "one attempt, remembered");
        Ok(())
    }

    #[tokio::test]
    async fn server_death_is_reported_and_completions_stay_answered() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
        let spawns = Arc::new(AtomicUsize::new(0));
        let (session, mut events) = session_with_connector(test_connector(
            Behavior::DieAfterHandshake,
            None,
            Arc::clone(&spawns),
        ));
        let backend = local(session);

        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
        let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
        let request = backend.next_id();
        backend.send(
            request,
            Command::Completion {
                doc,
                position: LineCol::new(0, 0),
            },
        )?;

        let mut died_notice = false;
        let mut answered = false;
        while let Some((rid, event)) = next_event(&mut events).await {
            match event {
                Event::Notification {
                    kind: NotificationKind::Lsp,
                    message,
                    ..
                } => {
                    assert!(message.contains("stopped"), "unexpected: {message}");
                    died_notice = true;
                    if answered {
                        break;
                    }
                },
                Event::Completions { items, .. } => {
                    assert_eq!(rid, Some(request));
                    assert!(items.is_empty());
                    answered = true;
                    if died_notice {
                        break;
                    }
                },
                _ => {},
            }
        }
        assert!(answered && died_notice);
        Ok(())
    }

    // --- unit tests for the pure pieces --------------------------------------

    #[test]
    fn builtin_specs_cover_the_documented_languages() {
        let rust = builtin_spec("rust");
        assert_eq!(rust.map(|s| s.command), Some("rust-analyzer".to_owned()));
        for lang in ["typescript", "javascript"] {
            let spec = builtin_spec(lang);
            assert_eq!(
                spec.as_ref().map(|s| s.command.as_str()),
                Some("typescript-language-server")
            );
            assert_eq!(spec.map(|s| s.args), Some(vec!["--stdio".to_owned()]));
        }
        let py = builtin_spec("python");
        assert_eq!(
            py.map(|s| (s.command, s.args)),
            Some(("pyright-langserver".to_owned(), vec!["--stdio".to_owned()]))
        );
        assert!(builtin_spec("cobol").is_none());
    }

    #[test]
    fn user_config_overrides_builtins() {
        let mut settings = LspSettings::default();
        settings.servers.insert(
            "rust".to_owned(),
            crate::config::schema::LspServer {
                command: "my-ra".to_owned(),
                args: vec!["--custom".to_owned()],
            },
        );
        // And extends to languages with no builtin.
        settings.servers.insert(
            "zig".to_owned(),
            crate::config::schema::LspServer {
                command: "zls".to_owned(),
                args: Vec::new(),
            },
        );
        let (manager, _rx) = LspManager::new(settings, None);
        let rust = manager.spec_for("rust");
        assert_eq!(
            rust.map(|s| (s.command, s.args)),
            Some(("my-ra".to_owned(), vec!["--custom".to_owned()]))
        );
        assert_eq!(
            manager.spec_for("zig").map(|s| s.command),
            Some("zls".to_owned())
        );
        // Untouched languages keep their builtin.
        assert_eq!(
            manager.spec_for("python").map(|s| s.command),
            Some("pyright-langserver".to_owned())
        );
    }

    #[test]
    fn language_keys_lowercase_display_names() {
        assert_eq!(language_key(Some("Rust")), Some("rust".to_owned()));
        assert_eq!(
            language_key(Some("TypeScript")),
            Some("typescript".to_owned())
        );
        assert_eq!(language_key(None), None);
    }

    #[test]
    fn versions_clamp_into_i32() {
        assert_eq!(version_i32(0), 0);
        assert_eq!(version_i32(41), 41);
        assert!(version_i32(u64::MAX) >= 0);
    }
}
