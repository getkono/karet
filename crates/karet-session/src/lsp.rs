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

mod catalog;
mod connector;
mod inventory;
mod jdtls;
mod lifecycle;
mod message;
mod provider;
mod runtime;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

pub(crate) use catalog::managed_arguments;
pub(crate) use connector::Connector;
use connector::spawn_connector;
use karet_core::LineCol;
use karet_core::WorkspaceEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
pub(crate) use message::LspUpdate;
use message::ServerCmd;
use provider::absolute_path;
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
use crate::api::LanguageServerRuntimeState;
use crate::api::LanguageServerSource;
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

/// Lazy per-language language-server orchestration (see the module docs).
pub(crate) struct LspManager {
    settings: LspSettings,
    generation: u64,
    root: Option<PathBuf>,
    registry_root: Option<PathBuf>,
    servers: HashMap<String, ServerSlot>,
    missing_reported: HashSet<LanguageServerId>,
    /// The cached jdtls JDK preflight: `None` until first checked, then the
    /// diagnosis (`None` = a usable JDK was found). Reset on reconfigure so a
    /// settings reload re-probes a fixed PATH.
    jdtls_preflight: Option<Option<String>>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
    runtime_states:
        HashMap<(LanguageServerId, PathBuf), (LanguageServerRuntimeState, Option<String>)>,
}

struct ServerSlot {
    tx: mpsc::Sender<ServerCmd>,
    documents: HashSet<PathBuf>,
    provider: Option<LanguageServerId>,
    primary: bool,
    root: PathBuf,
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
                root: root.map(|path| absolute_path(&path)),
                registry_root: registry_root.clone(),
                servers: HashMap::new(),
                missing_reported: HashSet::new(),
                jdtls_preflight: None,
                updates,
                connector: spawn_connector(supervisor, registry_root),
                runtime_states: HashMap::new(),
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
        self.runtime_states.clear();
        self.jdtls_preflight = None;
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
            | LspUpdate::ServerStatus { generation, .. }
            | LspUpdate::SpawnFailed { generation, .. }
            | LspUpdate::PreflightFailed { generation, .. }
            | LspUpdate::ServerDied { generation, .. }
            | LspUpdate::InstallRequired { generation, .. }
            | LspUpdate::ManualInstallRequired { generation, .. }
            | LspUpdate::RuntimeState { generation, .. } => *generation,
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
                Some(LanguageServerId::new(language.to_owned())),
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
        self.resolve_builtin(provider, language, root, fallback)
            .map(|(spec, _)| spec)
    }

    fn resolve_builtin(
        &self,
        provider: &LanguageServerId,
        language: &str,
        root: &Path,
        fallback: LspSpec,
    ) -> Option<(LspSpec, LanguageServerSource)> {
        project_local_spec(root, &fallback)
            .map(|spec| (spec, LanguageServerSource::ProjectLocal))
            .or_else(|| {
                executable_exists(OsStr::new(&fallback.command))
                    .then_some((fallback, LanguageServerSource::Path))
            })
            .or_else(|| {
                crate::lsp_registry::installed_spec(
                    self.registry_root.as_deref(),
                    provider,
                    language,
                )
                .map(|spec| (spec, LanguageServerSource::Managed))
            })
    }

    /// The task inbox for `language`, spawning the server task on first use.
    /// `None` when LSP is disabled or no server is configured for the language.
    /// Report a provider that could not be resolved, at most once per manager
    /// generation.
    ///
    /// The two outcomes are deliberately different events. karet offers to
    /// install only what it can actually install; for everything else it says
    /// what the user has to do instead. Sending the offer unconditionally is
    /// what produced the "taplo is not installed · type install" prompt whose
    /// install then failed with "taplo has no managed installer" — and, under
    /// `managedDownloads: "auto"`, queued that doomed job with no prompt at all.
    fn report_unresolved(&mut self, provider: LanguageServerId, language: &str) {
        if !self.missing_reported.insert(provider.clone()) {
            return;
        }
        let update = match crate::lsp_registry::manual_install_reason(&provider) {
            None => LspUpdate::InstallRequired {
                generation: self.generation,
                server: provider,
                language: language.to_owned(),
            },
            Some(reason) => LspUpdate::ManualInstallRequired {
                generation: self.generation,
                command: builtin_spec(&provider, language).command,
                server: provider,
                reason,
            },
        };
        let _ = self.updates.send(update);
    }

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
        let (mut spec, provider) = match self.spec_for(&language, &root) {
            Some(spec) => spec,
            None => {
                if let Some(provider) = builtin_server(&language) {
                    self.report_unresolved(provider, &language);
                }
                return None;
            },
        };
        if !self.jdtls_launch_gate(&mut spec, &root) {
            return None;
        }
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
            let runtime_provider = provider
                .clone()
                .unwrap_or_else(|| LanguageServerId::new(provider_key.clone()));
            handle.spawn(runtime::server_task(runtime::ServerTask {
                spec: spec.clone(),
                root,
                language: key.clone(),
                provider: runtime_provider,
                rx,
                updates: self.updates.clone(),
                connector: Arc::clone(&self.connector),
                generation: self.generation,
            }));
            self.servers.insert(
                key.clone(),
                ServerSlot {
                    tx,
                    documents: HashSet::new(),
                    provider,
                    primary: true,
                    root: nearest_repository_root(path, self.root.as_deref()),
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
            self.report_unresolved(provider, language);
            return None;
        };
        let key = format!("{}@{}", provider.key(), root.to_string_lossy());
        if !self.servers.contains_key(&key) {
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
            handle.spawn(runtime::server_task(runtime::ServerTask {
                spec: spec.clone(),
                root,
                language: key.clone(),
                provider: provider.clone(),
                rx,
                updates: self.updates.clone(),
                connector: Arc::clone(&self.connector),
                generation: self.generation,
            }));
            self.servers.insert(
                key.clone(),
                ServerSlot {
                    tx,
                    documents: HashSet::new(),
                    provider: Some(provider),
                    primary: false,
                    root: nearest_repository_root(path, self.root.as_deref()),
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
        selector: Option<&str>,
        lsp_language_id: Option<&str>,
        path: &Path,
        version: u64,
        text: impl FnOnce() -> String,
    ) {
        let path = absolute_path(path);
        let selector = language_key(selector);
        // The primary is optional. Diagnostics are explicitly a merged layer,
        // not the primary's to grant: a Python repository with Ruff installed
        // and Pyright missing must still get Ruff's diagnostics, and returning
        // here meant it got nothing at all -- an installed, configured provider
        // that silently never ran.
        let mut targets = self
            .ensure_server(selector.as_deref(), &path)
            .map(|(tx, key)| vec![(tx.clone(), key)])
            .unwrap_or_default();
        let root = nearest_repository_root(&path, self.root.as_deref());
        if let Some(language_key) = selector.as_deref() {
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
                    &path,
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
                        self.ensure_additional_provider(provider, language_key, &path)
                {
                    targets.push(target);
                }
            }
        }
        if targets.is_empty() {
            return;
        }
        let document_language = lsp_language_id
            .map(str::to_owned)
            .unwrap_or_else(|| selector.unwrap_or_default());
        let document_text = text();
        let mut seen_targets = HashSet::new();
        for (tx, key) in targets {
            if !seen_targets.insert(key.clone()) {
                continue;
            }
            if let Some(slot) = self.servers.get_mut(&key) {
                slot.documents.insert(path.clone());
            }
            let _ = tx.try_send(ServerCmd::DidOpen {
                path: path.clone(),
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
        let path = absolute_path(path);
        let senders: Vec<_> = self
            .servers
            .values()
            .filter(|slot| slot.documents.contains(&path))
            .map(|slot| slot.tx.clone())
            .collect();
        if senders.is_empty() {
            return;
        }
        let text = text();
        for tx in senders {
            let _ = tx.try_send(ServerCmd::DidChange {
                path: path.clone(),
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
        let path = absolute_path(path);
        let keys: Vec<_> = self
            .servers
            .iter()
            .filter(|(_, slot)| slot.documents.contains(&path))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            let remove = self.servers.get_mut(&key).is_some_and(|slot| {
                let _ = slot.tx.try_send(ServerCmd::DidClose { path: path.clone() });
                slot.documents.remove(&path);
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
        let path = absolute_path(path);
        let senders: Vec<_> = self
            .servers
            .values()
            .filter(|slot| slot.documents.contains(&path))
            .map(|slot| slot.tx.clone())
            .collect();
        if senders.is_empty() {
            return;
        }
        let text = text();
        for tx in senders {
            let _ = tx.try_send(ServerCmd::DidSave {
                path: path.clone(),
                text: text.clone(),
            });
        }
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
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::Completion {
            request,
            doc,
            version,
            path,
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
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            path,
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
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::Hover {
            request,
            doc,
            version,
            path,
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
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::Definition {
            request,
            doc,
            version,
            path,
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
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::Rename {
            request,
            path,
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
        let path = absolute_path(path);
        let preferred = self
            .settings
            .languages
            .get(&language_key)
            .and_then(|selection| selection.formatter.as_deref());
        let repository_default = if preferred.is_none() && language_key == "python" {
            Some(python_diagnostic_provider(&nearest_repository_root(
                &path,
                self.root.as_deref(),
            )))
        } else if preferred.is_none()
            && matches!(
                language_key.as_str(),
                "javascript" | "typescript" | "jsx" | "tsx"
            )
            && uses_biome(&nearest_repository_root(&path, self.root.as_deref()))
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
                    slot.documents.contains(&path)
                        && slot
                            .provider
                            .as_ref()
                            .is_some_and(|id| id.key() == provider)
                })
            })
            .or_else(|| {
                self.servers
                    .values()
                    .find(|slot| slot.primary && slot.documents.contains(&path))
            })
            .map(|slot| &slot.tx);
        let Some(tx) = tx else {
            return false;
        };
        tx.try_send(ServerCmd::Formatting {
            request,
            doc,
            version,
            path,
        })
        .is_ok()
    }
}
