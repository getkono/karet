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
//! Failure policy: launch failures and closed connections are reported once per
//! outage. The task retains authoritative open-document text, reconnects with
//! exponential backoff, replays every `didOpen`, and opens a cooldown circuit
//! after repeated failures instead of creating a respawn storm.

mod provider;
mod runtime;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use karet_core::CompletionItem;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
pub(crate) use provider::builtin_server;
use provider::builtin_spec;
use provider::executable_exists;
use provider::language_key;
use provider::nearest_repository_root;
use provider::project_local_spec;
use provider::python_diagnostic_provider;
use provider::uses_biome;
pub(crate) use provider::version_i32;
use tokio::sync::mpsc;

use crate::api::DocumentId;
use crate::api::LanguageServerId;
use crate::api::RequestId;
use crate::config::schema::Lsp as LspSettings;

/// How long an edited document may sit before its full text is forwarded as
/// `didChange`. A pending forward is also flushed immediately ahead of any
/// request, so completions never see stale text.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(150);
const SERVER_COMMAND_CAPACITY: usize = 256;
const RESTART_MIN_DELAY: Duration = Duration::from_millis(250);
const RESTART_MAX_DELAY: Duration = Duration::from_secs(30);
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const RESTART_LIMIT: usize = 5;
const CIRCUIT_COOLDOWN: Duration = Duration::from_secs(300);

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
fn spawn_connector(supervisor: Option<PathBuf>, registry_root: Option<PathBuf>) -> Connector {
    Arc::new(move |spec, root| {
        let supervisor = supervisor.clone();
        let registry_root = registry_root.clone();
        Box::pin(async move {
            let supervisor = supervisor.ok_or(LspError::Spawn)?;
            if let Some(registry_root) = registry_root {
                let stream = crate::lsp_broker::connect(&supervisor, &registry_root, &spec, &root)
                    .await
                    .map_err(|error| {
                        tracing::warn!(error = %error, "shared LSP broker connection failed");
                        LspError::Spawn
                    })?;
                let (read, write) = tokio::io::split(stream);
                return LspClient::connect(read, write, &root).await;
            }
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
    tx: mpsc::Sender<ServerCmd>,
    documents: HashSet<PathBuf>,
    provider: Option<LanguageServerId>,
    primary: bool,
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
                registry_root: registry_root.clone(),
                servers: HashMap::new(),
                missing_reported: HashSet::new(),
                updates,
                connector: spawn_connector(supervisor, registry_root),
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
            | LspUpdate::Hover { generation, .. }
            | LspUpdate::Definitions { generation, .. }
            | LspUpdate::WorkspaceSymbols { generation, .. }
            | LspUpdate::WorkspaceEdit { generation, .. }
            | LspUpdate::Formatting { generation, .. }
            | LspUpdate::Diagnostics { generation, .. }
            | LspUpdate::SpawnFailed { generation, .. }
            | LspUpdate::ServerDied { generation, .. }
            | LspUpdate::InstallRequired { generation, .. } => *generation,
        };
        generation == self.generation
    }

    /// The launch spec for `language`: user config first, then the built-ins.
    fn spec_for(&self, language: &str, root: &Path) -> Option<(LspSpec, Option<LanguageServerId>)> {
        if let Some(selection) = self.settings.languages.get(language)
            && let Some(server_id) = selection.servers.first()
            && let Some(server) = self.settings.servers.get(server_id)
        {
            return (server.enabled && !server.command.is_empty()).then(|| {
                (
                    LspSpec {
                        command: server.command.clone(),
                        args: server.args.clone(),
                        languages: vec![language.to_owned()],
                    },
                    Some(LanguageServerId::new(server_id.clone())),
                )
            });
        }
        if let Some(server) = self.settings.servers.get(language) {
            if !server.enabled || server.command.is_empty() {
                return None;
            }
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
        let spec = self.resolve_provider(&provider, language, root);
        #[cfg(test)]
        let spec = spec.or_else(|| Some(builtin_spec(&provider, language)));
        spec.map(|spec| (spec, Some(provider)))
    }

    fn resolve_provider(
        &self,
        provider: &LanguageServerId,
        language: &str,
        root: &Path,
    ) -> Option<LspSpec> {
        let fallback = builtin_spec(provider, language);
        project_local_spec(root, &fallback)
            .or_else(|| executable_exists(OsStr::new(&fallback.command)).then_some(fallback))
            .or_else(|| {
                crate::lsp_registry::installed_spec(
                    self.registry_root.as_deref(),
                    provider,
                    language,
                )
            })
    }

    /// The task inbox for `language`, spawning the server task on first use.
    /// `None` when LSP is disabled or no server is configured for the language.
    fn ensure_server(
        &mut self,
        language: Option<&str>,
        path: &Path,
    ) -> Option<(&mpsc::Sender<ServerCmd>, String)> {
        if !self.settings.enabled {
            return None;
        }
        let language = language_key(language)?;
        let root = nearest_repository_root(path, self.root.as_deref());
        let (spec, provider) = match self.spec_for(&language, &root) {
            Some(spec) => spec,
            None => {
                if let Some(provider) = builtin_server(&language)
                    && self.missing_reported.insert(provider.clone())
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
        let provider_key = provider
            .as_ref()
            .map_or_else(|| language.clone(), |server| server.key().to_owned());
        let key = format!("{provider_key}@{}", root.to_string_lossy());
        if !self.servers.contains_key(&key) {
            // Server tasks need an async runtime; a session driven synchronously
            // (unit tests, bare library use) simply runs without LSP.
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
            handle.spawn(runtime::server_task(
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
                    primary: true,
                },
            );
        }
        self.servers.get(&key).map(|slot| (&slot.tx, key))
    }

    fn ensure_additional_provider(
        &mut self,
        provider: LanguageServerId,
        language: &str,
        path: &Path,
    ) -> Option<(mpsc::Sender<ServerCmd>, String)> {
        let root = nearest_repository_root(path, self.root.as_deref());
        let spec = self.resolve_provider(&provider, language, &root);
        #[cfg(test)]
        let spec = spec.or_else(|| Some(builtin_spec(&provider, language)));
        let Some(spec) = spec else {
            if self.missing_reported.insert(provider.clone()) {
                let _ = self.updates.send(LspUpdate::InstallRequired {
                    generation: self.generation,
                    server: provider,
                });
            }
            return None;
        };
        let key = format!("{}@{}", provider.key(), root.to_string_lossy());
        if !self.servers.contains_key(&key) {
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
            handle.spawn(runtime::server_task(
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
                    provider: Some(provider),
                    primary: false,
                },
            );
        }
        self.servers.get(&key).map(|slot| (slot.tx.clone(), key))
    }

    /// The running task inbox for `language`, when one was already spawned.
    fn existing_server(
        &self,
        language: Option<&str>,
        path: &Path,
    ) -> Option<&mpsc::Sender<ServerCmd>> {
        if !self.settings.enabled {
            return None;
        }
        let _language = language_key(language)?;
        self.servers
            .values()
            .find(|slot| slot.primary && slot.documents.contains(path))
            .map(|slot| &slot.tx)
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
        let Some((tx, key)) = self.ensure_server(language.as_deref(), path) else {
            return;
        };
        let mut targets = vec![(tx.clone(), key)];
        let root = nearest_repository_root(path, self.root.as_deref());
        if let Some(language_key) = language.as_deref() {
            let configured_diagnostics = self
                .settings
                .languages
                .get(language_key)
                .map(|selection| selection.diagnostics.clone())
                .unwrap_or_default();
            for provider in configured_diagnostics {
                if let Some(target) = self.ensure_additional_provider(
                    LanguageServerId::new(provider),
                    language_key,
                    path,
                ) {
                    targets.push(target);
                }
            }
            if !self.settings.languages.contains_key(language_key) {
                let default_diagnostic = if language_key == "python" {
                    Some(python_diagnostic_provider(&root))
                } else if matches!(language_key, "javascript" | "typescript" | "jsx" | "tsx")
                    && uses_biome(&root)
                {
                    Some(LanguageServerId::Biome)
                } else {
                    None
                };
                if let Some(provider) = default_diagnostic
                    && let Some(target) =
                        self.ensure_additional_provider(provider, language_key, path)
                {
                    targets.push(target);
                }
            }
        }
        let document_language = language.unwrap_or_default();
        let document_text = text();
        let mut seen_targets = HashSet::new();
        for (tx, key) in targets {
            if !seen_targets.insert(key.clone()) {
                continue;
            }
            if let Some(slot) = self.servers.get_mut(&key) {
                slot.documents.insert(path.to_path_buf());
            }
            let _ = tx.try_send(ServerCmd::DidOpen {
                path: path.to_path_buf(),
                language: document_language.clone(),
                version: version_i32(version),
                text: document_text.clone(),
            });
        }
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
        if language_key(language).is_none() {
            return;
        }
        let senders: Vec<_> = self
            .servers
            .values()
            .filter(|slot| slot.documents.contains(path))
            .map(|slot| slot.tx.clone())
            .collect();
        if senders.is_empty() {
            return;
        }
        let text = text();
        for tx in senders {
            let _ = tx.try_send(ServerCmd::DidChange {
                path: path.to_path_buf(),
                version: version_i32(version),
                text: text.clone(),
            });
        }
    }

    /// Forward a document close. A no-op for languages without a running server.
    pub(crate) fn document_closed(&mut self, language: Option<&str>, path: &Path) {
        let Some(_language) = language_key(language) else {
            return;
        };
        let keys: Vec<_> = self
            .servers
            .iter()
            .filter(|(_, slot)| slot.documents.contains(path))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let remove = self.servers.get_mut(&key).is_some_and(|slot| {
                let _ = slot.tx.try_send(ServerCmd::DidClose {
                    path: path.to_path_buf(),
                });
                slot.documents.remove(path);
                slot.documents.is_empty()
            });
            if remove {
                self.servers.remove(&key);
            }
        }
    }

    /// Forward a successful save to every server attached to the document.
    pub(crate) fn document_saved(
        &self,
        language: Option<&str>,
        path: &Path,
        text: impl FnOnce() -> String,
    ) {
        if language_key(language).is_none() {
            return;
        }
        let senders: Vec<_> = self
            .servers
            .values()
            .filter(|slot| slot.documents.contains(path))
            .map(|slot| slot.tx.clone())
            .collect();
        if senders.is_empty() {
            return;
        }
        let text = text();
        for tx in senders {
            let _ = tx.try_send(ServerCmd::DidSave {
                path: path.to_path_buf(),
                text: text.clone(),
            });
        }
    }

    /// Whether this session currently owns a process for `provider`.
    pub(crate) fn is_running(&self, provider: &LanguageServerId) -> bool {
        self.servers
            .values()
            .any(|slot| slot.provider.as_ref() == Some(provider))
    }

    /// Retire live tasks after an explicit install or restart request.
    ///
    /// All tasks are retired together so late task updates are rejected by one
    /// generation boundary. The session immediately reopens its documents.
    pub(crate) fn restart(&mut self, provider: LanguageServerId) -> bool {
        self.missing_reported.remove(&provider);
        let running = self.is_running(&provider);
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
        let Some(tx) = self.existing_server(language, path) else {
            return false;
        };
        tx.try_send(ServerCmd::Completion {
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
        let Some(tx) = self.existing_server(language, path) else {
            return false;
        };
        tx.try_send(ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            path: path.to_path_buf(),
        })
        .is_ok()
    }

    pub(crate) fn hover(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        position: LineCol,
    ) -> bool {
        let Some(tx) = self.existing_server(language, path) else {
            return false;
        };
        tx.try_send(ServerCmd::Hover {
            request,
            doc,
            version,
            path: path.to_path_buf(),
            position,
        })
        .is_ok()
    }

    pub(crate) fn definition(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        position: LineCol,
    ) -> bool {
        let Some(tx) = self.existing_server(language, path) else {
            return false;
        };
        tx.try_send(ServerCmd::Definition {
            request,
            doc,
            version,
            path: path.to_path_buf(),
            position,
        })
        .is_ok()
    }

    pub(crate) fn workspace_symbols(&self, request: RequestId, query: String) -> bool {
        let Some(tx) = self
            .servers
            .values()
            .find(|slot| slot.primary)
            .map(|slot| &slot.tx)
        else {
            return false;
        };
        tx.try_send(ServerCmd::WorkspaceSymbols { request, query })
            .is_ok()
    }

    pub(crate) fn rename(
        &self,
        language: Option<&str>,
        request: RequestId,
        path: &Path,
        position: LineCol,
        new_name: String,
    ) -> bool {
        let Some(tx) = self.existing_server(language, path) else {
            return false;
        };
        tx.try_send(ServerCmd::Rename {
            request,
            path: path.to_path_buf(),
            position,
            new_name,
        })
        .is_ok()
    }

    pub(crate) fn formatting(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
    ) -> bool {
        let Some(language_key) = language_key(language) else {
            return false;
        };
        let preferred = self
            .settings
            .languages
            .get(&language_key)
            .and_then(|selection| selection.formatter.as_deref());
        let repository_default = if preferred.is_none() && language_key == "python" {
            Some(python_diagnostic_provider(&nearest_repository_root(
                path,
                self.root.as_deref(),
            )))
        } else if preferred.is_none()
            && matches!(
                language_key.as_str(),
                "javascript" | "typescript" | "jsx" | "tsx"
            )
            && uses_biome(&nearest_repository_root(path, self.root.as_deref()))
        {
            Some(LanguageServerId::Biome)
        } else {
            None
        };
        let selected = preferred
            .map(str::to_owned)
            .or_else(|| repository_default.map(|provider| provider.key().to_owned()));
        let tx = selected
            .as_deref()
            .and_then(|provider| {
                self.servers.values().find(|slot| {
                    slot.documents.contains(path)
                        && slot
                            .provider
                            .as_ref()
                            .is_some_and(|id| id.key() == provider)
                })
            })
            .or_else(|| {
                self.servers
                    .values()
                    .find(|slot| slot.primary && slot.documents.contains(path))
            })
            .map(|slot| &slot.tx);
        let Some(tx) = tx else {
            return false;
        };
        tx.try_send(ServerCmd::Formatting {
            request,
            doc,
            version,
            path: path.to_path_buf(),
        })
        .is_ok()
    }
}
