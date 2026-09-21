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
mod commands;
mod connector;
mod forward;
mod health;
mod inventory;
mod jdtls;
mod lifecycle;
mod message;
mod provider;
mod requests;
mod runtime;
mod slot;
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
pub(crate) use catalog::serves_language;
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
pub(crate) use slot::Retired;
use slot::ServerSlot;
pub(crate) use slot::SlotKey;
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
/// How long a dead provider's diagnostics stay on screen before being dropped.
///
/// Not zero, because the common death is followed by a reconnect at 250ms and
/// another at 500ms; clearing immediately would flicker every marker off and back
/// on for an outage the user would otherwise never have noticed. Long enough to
/// outlast both, short enough that a server which is really gone does not leave
/// its squiggles pointing at lines the user has since edited away.
const DIAGNOSTIC_GRACE: Duration = Duration::from_secs(1);

/// Lazy per-language language-server orchestration (see the module docs).
pub(crate) struct LspManager {
    settings: LspSettings,
    generation: u64,
    root: Option<PathBuf>,
    registry_root: Option<PathBuf>,
    servers: HashMap<SlotKey, ServerSlot>,
    missing_reported: HashSet<LanguageServerId>,
    /// The cached jdtls JDK preflight: `None` until first checked, then the
    /// diagnosis (`None` = a usable JDK was found). Reset on reconfigure so a
    /// settings reload re-probes a fixed PATH.
    jdtls_preflight: Option<Option<String>>,
    /// Slots whose document sync has already been reported as failing.
    ///
    /// A full queue is not a one-off: the task is blocked, so *every* subsequent
    /// keystroke fails to enqueue. The resulting notification is a persistent
    /// card -- warnings never auto-dismiss, and only transient cards are evicted --
    /// so reporting per failure grew an unbounded stack the user had to clear by
    /// hand. Reported once per outage instead, and cleared by the first delivery
    /// that succeeds.
    sync_failure_reported: HashSet<SlotKey>,
    /// Providers whose launch preflight has already been reported in this
    /// generation, so a failed one is explained once rather than per document.
    preflight_reported: HashSet<LanguageServerId>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
    /// Source of slot tokens. Never reused, and never zero, so a task holding a
    /// token can always be told apart from every task that held its key before.
    next_token: u64,
}

/// What the user's `lsp.servers` table says about one provider id.
enum Configured {
    /// No entry: the built-in launch table decides.
    Absent,
    /// An entry that forbids a launch -- `enabled = false`, or an empty command.
    Suppressed,
    /// An entry naming exactly what to run.
    Spec(LspSpec),
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
                sync_failure_reported: HashSet::new(),
                jdtls_preflight: None,
                preflight_reported: HashSet::new(),
                updates,
                connector: spawn_connector(supervisor, registry_root),
                next_token: 1,
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
    ///
    /// `None` means the settings are unchanged and nothing need happen. `Some`
    /// carries the retirement the caller must adopt, and is also its signal to
    /// reopen documents against fresh servers.
    pub(crate) fn reconfigure(&mut self, settings: LspSettings) -> Option<Retired> {
        if self.settings == settings {
            return None;
        }
        self.settings = settings;
        self.generation = self.generation.wrapping_add(1);
        let retired = self.retire_matching(|_| true);
        self.jdtls_preflight = None;
        self.preflight_reported.clear();
        Some(retired)
    }

    /// Whether an asynchronous update is still worth adopting.
    ///
    /// Two rules, because there are exactly two kinds of message.
    ///
    /// A task's report *about its own slot* -- its state, its diagnostics, its
    /// death -- is worth adopting only while it still holds that slot. The token
    /// says which incarnation is speaking, and a key is re-taken unchanged, so
    /// without it a retired task and its replacement are indistinguishable. That
    /// is one predicate covering every such message; the attempt this replaces
    /// had four, one per message shape, and each was separately wrong.
    ///
    /// An *answer to a request* is different, and deliberately not fenced on the
    /// slot: a task that has since been retired still gives the right answer to
    /// the question that was put to it, and dropping it would leave the caller
    /// waiting for a reply that never comes. Those keep the generation fence,
    /// which exists to discard work begun under a settings snapshot that is gone.
    pub(crate) fn accepts(&self, update: &LspUpdate) -> bool {
        match update {
            LspUpdate::ServerStatus { token, key, .. }
            | LspUpdate::Diagnostics {
                token, server: key, ..
            }
            | LspUpdate::DiagnosticsCleared {
                token, server: key, ..
            }
            | LspUpdate::SpawnFailed { token, key, .. }
            | LspUpdate::ServerDied { token, key, .. }
            | LspUpdate::RuntimeState { token, key, .. } => self
                .servers
                .get(key)
                .is_some_and(|slot| slot.token == *token),
            LspUpdate::Completions { generation, .. }
            | LspUpdate::Symbols { generation, .. }
            | LspUpdate::Hover { generation, .. }
            | LspUpdate::Definitions { generation, .. }
            | LspUpdate::WorkspaceSymbols { generation, .. }
            | LspUpdate::WorkspaceEdit { generation, .. }
            | LspUpdate::Formatting { generation, .. }
            | LspUpdate::SyncFailed { generation, .. }
            | LspUpdate::PreflightFailed { generation, .. }
            | LspUpdate::InstallRequired { generation, .. }
            | LspUpdate::ManualInstallRequired { generation, .. } => *generation == self.generation,
        }
    }

    /// What the user's configuration says about `language`'s primary server:
    /// the id `lsp.languages.<language>.servers` names, else an entry named
    /// after the language itself, together with that entry's verdict.
    ///
    /// One lookup, three readers. [`Self::spec_for`] turns a verdict into a
    /// launch; `ensure_server` asks it *why* there was no launch — a provider
    /// the user switched off is a decision, not a missing install — and whether
    /// the command about to run is the user's rather than karet's.
    fn configured_primary(&self, language: &str) -> Option<(LanguageServerId, Configured)> {
        let named = self
            .settings
            .languages
            .get(language)
            .and_then(|selection| selection.servers.first())
            .map(String::as_str);
        named
            .into_iter()
            .chain(std::iter::once(language))
            .find_map(
                |server_id| match self.configured_spec(server_id, language) {
                    Configured::Absent => None,
                    verdict => Some((LanguageServerId::new(server_id.to_owned()), verdict)),
                },
            )
    }

    /// The launch spec for `language`: user config first, then the built-ins.
    ///
    /// The provider is not optional: every launch names one, either the id the
    /// user configured or the built-in for the language. It used to be an
    /// `Option` whose `None` arm made callers invent a provider id out of the
    /// language name -- an identity no other part of the manager ever produced.
    fn spec_for(&self, language: &str, root: &Path) -> Option<(LspSpec, LanguageServerId)> {
        match self.configured_primary(language) {
            Some((server_id, Configured::Spec(spec))) => return Some((spec, server_id)),
            // An entry that forbids a launch is the whole answer: the built-in
            // table does not get a second vote on a server switched off.
            Some((_, Configured::Suppressed)) => return None,
            Some((_, Configured::Absent)) | None => {},
        }
        let provider = builtin_server(language)?;
        let spec = self.resolve_provider(&provider, language, root);
        #[cfg(test)]
        let spec = spec.or_else(|| builtin_spec(&provider, language));
        spec.map(|spec| (spec, provider))
    }

    /// What `lsp.servers` says about the provider id `server_id`.
    ///
    /// The single reader of a user's server entry, so every launch honours the
    /// same three fields -- `command`, `args`, `enabled` -- however the id was
    /// named: as a language's primary, or in that language's `diagnostics`
    /// companions. A companion used to skip this entirely and be launched as a
    /// bare `<id>` off `PATH`, arguments and `enabled = false` alike ignored.
    fn configured_spec(&self, server_id: &str, language: &str) -> Configured {
        let Some(server) = self.settings.servers.get(server_id) else {
            return Configured::Absent;
        };
        if !server.enabled || server.command.is_empty() {
            return Configured::Suppressed;
        }
        Configured::Spec(LspSpec::new(
            server.command.clone(),
            server.args.clone(),
            vec![language.to_owned()],
        ))
    }

    fn resolve_provider(
        &self,
        provider: &LanguageServerId,
        language: &str,
        root: &Path,
    ) -> Option<LspSpec> {
        let fallback = builtin_spec(provider, language)?;
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

    /// Give Astro the TypeScript SDK path it refuses to start without.
    ///
    /// Only a karet-managed installation carried one, because only the install
    /// recorded it -- so the ordinary way an Astro project installs the server,
    /// `@astrojs/language-server` in `node_modules`, resolved first and was
    /// launched with no `typescript.tsdk` at all, and Astro declined the
    /// handshake. The project's own TypeScript is the right SDK for that
    /// install; a managed one keeps the bundle it was installed with.
    ///
    /// Only for a launch karet itself chose — the caller skips it for a command
    /// out of `lsp.servers`, where refusing would override the user rather than
    /// diagnose karet.
    ///
    /// Returns whether the launch may proceed.
    fn astro_launch_gate(
        &mut self,
        spec: &mut LspSpec,
        provider: Option<&LanguageServerId>,
        language: &str,
        root: &Path,
    ) -> bool {
        if !provider::is_astro(provider, spec) || spec.initialization_options.is_some() {
            return true;
        }
        let managed = || {
            crate::lsp_registry::installed_spec(
                self.registry_root.as_deref(),
                &LanguageServerId::new(provider::ASTRO),
                language,
            )
            .and_then(|managed| managed.initialization_options)
        };
        if let Some(options) = provider::project_typescript_sdk(Path::new(&spec.command), root)
            .map(|tsdk| provider::typescript_sdk_options(&tsdk))
            .or_else(managed)
        {
            spec.initialization_options = Some(options);
            return true;
        }
        // Refusing to launch is the honest outcome: Astro would reject the
        // handshake anyway, and "no TypeScript" says why while a rejected
        // handshake does not.
        if self
            .preflight_reported
            .insert(LanguageServerId::new(provider::ASTRO))
        {
            let _ = self.updates.send(LspUpdate::PreflightFailed {
                generation: self.generation,
                message: "the Astro language server needs a TypeScript SDK: install typescript in \
                          this project (npm install -D typescript), or let karet install Astro, \
                          which bundles one"
                    .to_owned(),
            });
        }
        false
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
                command: builtin_spec(&provider, language)
                    .map_or_else(|| provider.key().to_owned(), |spec| spec.command),
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
    ) -> Option<(&mpsc::Sender<ServerCmd>, SlotKey)> {
        if !self.settings.enabled {
            return None;
        }
        let language = language_key(language)?;
        let root = nearest_repository_root(path, self.root.as_deref());
        let configured = self.configured_primary(&language);
        let (mut spec, provider) = match self.spec_for(&language, &root) {
            Some(spec) => spec,
            None => {
                // A provider the user switched off has nothing to report:
                // turning a server off is a decision, and answering it with
                // "install it yourself" is answering a question nobody asked.
                if !matches!(configured, Some((_, Configured::Suppressed)))
                    && let Some(provider) = builtin_server(&language)
                {
                    self.report_unresolved(provider, &language);
                }
                return None;
            },
        };
        if !self.jdtls_launch_gate(&mut spec, &root) {
            return None;
        }
        // The preflight diagnoses a command *karet* chose. A user who named
        // their own is entitled to have it run: a wrapper that supplies its own
        // `tsdk` is exactly why someone configures one.
        if !matches!(configured, Some((_, Configured::Spec(_))))
            && !self.astro_launch_gate(&mut spec, Some(&provider), &language, &root)
        {
            return None;
        }
        // Built-in JavaScript and TypeScript share one provider process, so the
        // key names the *provider*, not the language that selected it.
        let key = SlotKey::new(provider, root.clone());
        if !self.servers.contains_key(&key) {
            // Server tasks need an async runtime; a session driven synchronously
            // (unit tests, bare library use) simply runs without LSP.
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
            let token = self.take_token();
            // Inserted before the task is spawned, not after. The task's first
            // act is to report `Starting`, and a report is accepted only against
            // a live slot -- so spawning first leaves a window in which the
            // provider's own opening move is thrown away.
            self.servers
                .insert(key.clone(), ServerSlot::new(token, tx, true));
            handle.spawn(runtime::server_task(runtime::ServerTask {
                spec: spec.clone(),
                key: key.clone(),
                token,
                rx,
                updates: self.updates.clone(),
                connector: Arc::clone(&self.connector),
                generation: self.generation,
            }));
        }
        self.servers.get(&key).map(|slot| (&slot.tx, key))
    }

    fn ensure_additional_provider(
        &mut self,
        provider: LanguageServerId,
        language: &str,
        path: &Path,
    ) -> Option<(mpsc::Sender<ServerCmd>, SlotKey)> {
        let root = nearest_repository_root(path, self.root.as_deref());
        let spec = match self.configured_spec(provider.key(), language) {
            Configured::Spec(spec) => Some(spec),
            // `enabled = false` is a decision about the provider, not about the
            // slot it was named in: a companion the user switched off stays off.
            Configured::Suppressed => return None,
            Configured::Absent => {
                let spec = self.resolve_provider(&provider, language, &root);
                #[cfg(test)]
                let spec = spec.or_else(|| builtin_spec(&provider, language));
                spec
            },
        };
        let Some(spec) = spec else {
            self.report_unresolved(provider, language);
            return None;
        };
        let key = SlotKey::new(provider, root);
        if !self.servers.contains_key(&key) {
            let handle = tokio::runtime::Handle::try_current().ok()?;
            let (tx, rx) = mpsc::channel(SERVER_COMMAND_CAPACITY);
            let token = self.take_token();
            // Insert before spawn -- see `ensure_server`.
            self.servers
                .insert(key.clone(), ServerSlot::new(token, tx, false));
            handle.spawn(runtime::server_task(runtime::ServerTask {
                spec: spec.clone(),
                key: key.clone(),
                token,
                rx,
                updates: self.updates.clone(),
                connector: Arc::clone(&self.connector),
                generation: self.generation,
            }));
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
    ) -> Retired {
        // Checked here, not only in `ensure_server`: companions are attached
        // below without going through it, so once the primary stopped being
        // required this was the only remaining gate on the whole feature.
        if !self.settings.enabled {
            return Retired::none();
        }
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
            return Retired::none();
        }
        let document_language = lsp_language_id
            .map(str::to_owned)
            .unwrap_or_else(|| selector.unwrap_or_default());
        let document_text = text();
        let mut seen_targets = HashSet::new();
        let mut undelivered = Vec::new();
        for (tx, key) in targets {
            if !seen_targets.insert(key.clone()) {
                continue;
            }
            // A slot that already holds this document has already been told
            // about it, and `didOpen` for an open document is a protocol error.
            // This is reachable whenever a reopen fans out to a provider that
            // was not retired: scoping a restart to one provider leaves its
            // language's companions running, and installing a provider reopens
            // documents whose other providers never went anywhere.
            let newly_attached = self
                .servers
                .get_mut(&key)
                .is_none_or(|slot| slot.documents.insert(path.clone()));
            if !newly_attached {
                continue;
            }
            if tx
                .try_send(ServerCmd::DidOpen {
                    path: path.clone(),
                    language: document_language.clone(),
                    version: version_i32(version),
                    text: document_text.clone(),
                })
                .is_err()
            {
                undelivered.push(key);
            } else {
                // Delivery works again: forget the suppression so a *later* outage
                // is reported rather than silenced by one that has since cleared.
                self.sync_failure_reported.remove(&key);
            }
        }
        // A dropped `didOpen` used to be silent, and was the worst of the silent
        // drops: the server never learns the document exists, the task never adds
        // it to its replay set, and no later restart fixes it. The file simply has
        // no language support, with nothing anywhere saying why.
        let mut retired = Retired::none();
        for key in undelivered {
            retired.absorb(self.report_undelivered(&key, "the server's command queue is full"));
        }
        retired
    }

    /// Report that a document-sync command never reached its server, and retire
    /// the slot if the task behind it is gone.
    ///
    /// Both `try_send` failures leave the server's copy of the document out of
    /// step with the buffer, which is exactly the state that reads to a user as
    /// "the LSP stopped working for this file". A closed channel additionally
    /// means the task has exited, so the slot is dropped and the next open builds
    /// a fresh one rather than writing into a sender nobody reads.
    fn report_undelivered(&mut self, key: &SlotKey, reason: &str) -> Retired {
        let Some(slot) = self.servers.get(key) else {
            return Retired::none();
        };
        let closed = slot.tx.is_closed();
        if self.sync_failure_reported.insert(key.clone()) {
            let _ = self.updates.send(LspUpdate::SyncFailed {
                generation: self.generation,
                server: key.provider.clone(),
                reason: reason.to_owned(),
            });
        }
        if closed {
            self.retire(key)
        } else {
            Retired::none()
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
    ) -> Retired {
        if language_key(language).is_none() {
            return Retired::none();
        }
        let path = absolute_path(path);
        let senders: Vec<_> = self
            .servers
            .iter()
            .filter(|(_, slot)| slot.documents.contains(&path))
            .map(|(key, slot)| (key.clone(), slot.tx.clone()))
            .collect();
        if senders.is_empty() {
            return Retired::none();
        }
        let text = text();
        let mut undelivered = Vec::new();
        for (key, tx) in senders {
            if tx
                .try_send(ServerCmd::DidChange {
                    path: path.clone(),
                    version: version_i32(version),
                    text: text.clone(),
                })
                .is_err()
            {
                undelivered.push(key);
            } else {
                self.sync_failure_reported.remove(&key);
            }
        }
        // A dropped `didChange` leaves the server's copy of the file behind the
        // buffer, so every answer it gives is about text the user no longer has.
        // Sync is full-text, so the next edit that *does* land repairs it -- but
        // until then the condition is real and was previously invisible.
        let mut retired = Retired::none();
        for key in undelivered {
            retired.absorb(self.report_undelivered(&key, "the server's command queue is full"));
        }
        retired
    }

    /// Forward a document close, retiring any slot it was the last document for.
    pub(crate) fn document_closed(&mut self, language: Option<&str>, path: &Path) -> Retired {
        let Some(_language) = language_key(language) else {
            return Retired::none();
        };
        let path = absolute_path(path);
        let keys: Vec<_> = self
            .servers
            .iter()
            .filter(|(_, slot)| slot.documents.contains(&path))
            .map(|(key, _)| key.clone())
            .collect();
        let mut retired = Retired::none();
        for key in keys {
            let remove = self.servers.get_mut(&key).is_some_and(|slot| {
                let _ = slot.tx.try_send(ServerCmd::DidClose { path: path.clone() });
                slot.documents.remove(&path);
                slot.documents.is_empty()
            });
            if remove {
                retired.absorb(self.retire(&key));
            }
        }
        retired
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
}
