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

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use karet_core::CompletionItem;
use karet_core::LineCol;
use karet_core::Symbol;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
use tokio::sync::mpsc;

use crate::api::DocumentId;
use crate::api::LanguageServerId;
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
    /// A built-in provider is locally absent. No network operation was attempted.
    InstallRequired {
        /// The manager generation that observed the missing installation.
        generation: u64,
        /// Missing managed provider.
        server: LanguageServerId,
    },
}

/// How the manager establishes a client for a spec — [`LspClient::spawn`] in
/// production; tests inject an in-memory duplex connection instead.
pub(crate) type Connector = Arc<
    dyn Fn(LspSpec, PathBuf) -> Pin<Box<dyn Future<Output = Result<LspClient, LspError>> + Send>>
        + Send
        + Sync,
>;

/// The production connector: run the server through karet's crash-safe process
/// supervisor. A headless host that supplied no supervisor fails closed.
fn spawn_connector(supervisor: Option<PathBuf>) -> Connector {
    Arc::new(move |spec, root| {
        let supervisor = supervisor.clone();
        Box::pin(async move {
            let supervisor = supervisor.ok_or(LspError::Spawn)?;
            let command = crate::process_supervisor::command(
                &supervisor,
                spec.command.clone(),
                spec.args.clone(),
                &root,
            )
            .map_err(|_| LspError::Spawn)?;
            LspClient::spawn_command(command, &spec.command, &root).await
        })
    })
}

/// The built-in default servers, used when `lsp.servers` has no entry for a
/// language. Keys are lowercase language names (the same keys user config uses).
pub(crate) fn builtin_server(language: &str) -> Option<LanguageServerId> {
    match language {
        "rust" => Some(LanguageServerId::RustAnalyzer),
        "typescript" | "javascript" => Some(LanguageServerId::TypeScript),
        "python" => Some(LanguageServerId::Pyright),
        "tex" => Some(LanguageServerId::Texlab),
        _ => None,
    }
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
    registry_root: Option<PathBuf>,
    servers: HashMap<String, ServerSlot>,
    missing_reported: HashSet<LanguageServerId>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
}

struct ServerSlot {
    tx: mpsc::UnboundedSender<ServerCmd>,
    documents: HashSet<PathBuf>,
    provider: Option<LanguageServerId>,
}

impl LspManager {
    /// Create a manager and the update stream the actor drains.
    pub(crate) fn new(
        settings: LspSettings,
        root: Option<PathBuf>,
        supervisor: Option<PathBuf>,
        registry_root: Option<PathBuf>,
    ) -> (Self, mpsc::UnboundedReceiver<LspUpdate>) {
        let (updates, rx) = mpsc::unbounded_channel();
        (
            Self {
                settings,
                generation: 0,
                root,
                registry_root,
                servers: HashMap::new(),
                missing_reported: HashSet::new(),
                updates,
                connector: spawn_connector(supervisor),
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
            | LspUpdate::Symbols { generation, .. }
            | LspUpdate::SpawnFailed { generation, .. }
            | LspUpdate::ServerDied { generation, .. }
            | LspUpdate::InstallRequired { generation, .. } => *generation,
        };
        generation == self.generation
    }

    /// The launch spec for `language`: user config first, then the built-ins.
    fn spec_for(&self, language: &str) -> Option<(LspSpec, Option<LanguageServerId>)> {
        if let Some(server) = self.settings.servers.get(language) {
            return Some((
                LspSpec {
                    command: server.command.clone(),
                    args: server.args.clone(),
                    languages: vec![language.to_owned()],
                },
                None,
            ));
        }
        let provider = builtin_server(language)?;
        #[cfg(test)]
        let spec =
            crate::lsp_registry::installed_spec(self.registry_root.as_deref(), provider, language)
                .or_else(|| {
                    Some(LspSpec {
                        command: provider.key().to_owned(),
                        args: match provider {
                            LanguageServerId::TypeScript | LanguageServerId::Pyright => {
                                vec!["--stdio".into()]
                            },
                            LanguageServerId::RustAnalyzer | LanguageServerId::Texlab => Vec::new(),
                        },
                        languages: vec![language.to_owned()],
                    })
                });
        #[cfg(not(test))]
        let spec =
            crate::lsp_registry::installed_spec(self.registry_root.as_deref(), provider, language);
        spec.map(|spec| (spec, Some(provider)))
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
        let language = language_key(language)?;
        let (spec, provider) = match self.spec_for(&language) {
            Some(spec) => spec,
            None => {
                if let Some(provider) = builtin_server(&language)
                    && self.missing_reported.insert(provider)
                {
                    let _ = self.updates.send(LspUpdate::InstallRequired {
                        generation: self.generation,
                        server: provider,
                    });
                }
                return None;
            },
        };
        // Built-in JavaScript and TypeScript share one provider process. Custom
        // entries remain language-keyed because independent config entries may
        // intentionally name different executables.
        let key = provider.map_or_else(|| language.clone(), |server| server.key().to_owned());
        if !self.servers.contains_key(&key) {
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
            self.servers.insert(
                key.clone(),
                ServerSlot {
                    tx,
                    documents: HashSet::new(),
                    provider,
                },
            );
        }
        self.servers.get(&key).map(|slot| (&slot.tx, key))
    }

    /// The running task inbox for `language`, when one was already spawned.
    fn existing_server(&self, language: Option<&str>) -> Option<&mpsc::UnboundedSender<ServerCmd>> {
        if !self.settings.enabled {
            return None;
        }
        let language = language_key(language)?;
        self.servers
            .get(&self.server_key(&language))
            .map(|slot| &slot.tx)
    }

    fn server_key(&self, language: &str) -> String {
        if self.settings.servers.contains_key(language) {
            return language.to_owned();
        }
        builtin_server(language)
            .map_or_else(|| language.to_owned(), |server| server.key().to_owned())
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
        let language = language_key(language);
        let Some((tx, key)) = self.ensure_server(language.as_deref()) else {
            return;
        };
        let tx = tx.clone();
        if let Some(slot) = self.servers.get_mut(&key) {
            slot.documents.insert(path.to_path_buf());
        }
        let _ = tx.send(ServerCmd::DidOpen {
            path: path.to_path_buf(),
            language: language.unwrap_or_default(),
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
        let Some(language) = language_key(language) else {
            return;
        };
        let key = self.server_key(&language);
        let Some(slot) = self.servers.get_mut(&key) else {
            return;
        };
        let _ = slot.tx.send(ServerCmd::DidClose {
            path: path.to_path_buf(),
        });
        slot.documents.remove(path);
        if slot.documents.is_empty() {
            // Dropping the final sender makes the task perform the normal LSP
            // shutdown handshake. Its supervised process then has no idle owner.
            self.servers.remove(&key);
        }
    }

    /// Whether this session currently owns a process for `provider`.
    pub(crate) fn is_running(&self, provider: LanguageServerId) -> bool {
        self.servers
            .values()
            .any(|slot| slot.provider == Some(provider))
    }

    /// Retire live tasks after an explicit install or restart request.
    ///
    /// All tasks are retired together so late task updates are rejected by one
    /// generation boundary. The session immediately reopens its documents.
    pub(crate) fn restart(&mut self, provider: LanguageServerId) -> bool {
        self.missing_reported.remove(&provider);
        let running = self.is_running(provider);
        if running {
            self.generation = self.generation.wrapping_add(1);
            self.servers.clear();
        }
        running
    }

    /// Forget a missing-provider suppression after its installation activates.
    pub(crate) fn installed(&mut self, provider: LanguageServerId) {
        self.missing_reported.remove(&provider);
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

    /// Forward a document-symbol request. Returns whether a live server accepted it.
    pub(crate) fn document_symbols(
        &mut self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
    ) -> bool {
        let Some(tx) = self.existing_server(language) else {
            return false;
        };
        tx.send(ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            path: path.to_path_buf(),
        })
        .is_ok()
    }
}

/// Answer a request command with an empty set (used whenever no live server can
/// answer, so the client is never left waiting).
fn answer_empty(updates: &mpsc::UnboundedSender<LspUpdate>, cmd: ServerCmd, generation: u64) {
    match cmd {
        ServerCmd::Completion {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Completions {
                generation,
                request,
                doc,
                version,
                items: Vec::new(),
            });
        },
        ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Symbols {
                generation,
                request,
                doc,
                version,
                symbols: Vec::new(),
            });
        },
        ServerCmd::DidOpen { .. } | ServerCmd::DidChange { .. } | ServerCmd::DidClose { .. } => {},
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
                language: document_language,
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
                    let result = client
                        .did_open(&path, &document_language, version, &text)
                        .await;
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
            ServerCmd::DocumentSymbols {
                request,
                doc,
                version,
                path,
            } => {
                // Symbol ranges must describe the same text revision as the request.
                flush_pending(
                    &client,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    match client.document_symbols(&path).await {
                        Ok(symbols) => symbols,
                        Err(error) => {
                            note_failure::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
                            Vec::new()
                        },
                    }
                };
                let _ = updates.send(LspUpdate::Symbols {
                    generation,
                    request,
                    doc,
                    version,
                    symbols,
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
