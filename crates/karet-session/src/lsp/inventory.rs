use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::provider::builtin_catalog;
use super::*;
use crate::api::LanguageServerInstanceStatus;
use crate::api::LanguageServerStatus;

impl LspManager {
    /// Build a complete, network-free provider inventory for known repository roots.
    pub(crate) fn inventory(
        &self,
        document_paths: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<LanguageServerStatus> {
        let mut providers =
            BTreeMap::<String, (LanguageServerId, BTreeSet<String>, bool, Option<String>)>::new();
        for descriptor in builtin_catalog() {
            providers.insert(
                descriptor.server.key().to_owned(),
                (
                    descriptor.server,
                    descriptor.languages.into_iter().collect(),
                    descriptor.managed,
                    descriptor.manual_install_reason,
                ),
            );
        }
        for (language, selection) in &self.settings.languages {
            let selected = selection
                .servers
                .iter()
                .chain(selection.formatter.iter())
                .chain(selection.semantic_tokens.iter())
                .chain(selection.diagnostics.iter());
            for server in selected {
                let entry = providers.entry(server.clone()).or_insert_with(|| {
                    (
                        LanguageServerId::new(server.clone()),
                        BTreeSet::new(),
                        false,
                        None,
                    )
                });
                entry.1.insert(language.clone());
            }
        }
        for server in self.settings.servers.keys() {
            let entry = providers.entry(server.clone()).or_insert_with(|| {
                (
                    LanguageServerId::new(server.clone()),
                    BTreeSet::new(),
                    false,
                    None,
                )
            });
            if entry.1.is_empty() {
                entry.1.insert(server.clone());
            }
        }

        let mut known_roots: BTreeSet<PathBuf> = document_paths
            .into_iter()
            .map(|path| absolute_path(&path))
            .map(|path| nearest_repository_root(&path, self.root.as_deref()))
            .collect();
        if let Some(root) = &self.root {
            known_roots.insert(root.clone());
        }
        known_roots.extend(self.servers.values().map(|slot| slot.root.clone()));
        if known_roots.is_empty() {
            known_roots.insert(PathBuf::from("."));
        }

        providers
            .into_values()
            .map(|(server, languages, managed, manual_install_reason)| {
                let installed =
                    crate::lsp_registry::installed_version(self.registry_root.as_deref(), &server);
                let instances = known_roots
                    .iter()
                    .map(|root| self.inventory_instance(&server, &languages, root))
                    .collect();
                let root = self.registry_root.as_deref();
                LanguageServerStatus {
                    // Both switches, not just the global one. A provider turned off
                    // by its own `lsp.servers.<id>.enabled = false` is never
                    // launched, so reporting it as enabled made the editor badge it
                    // as healthy-but-idle: a state indistinguishable from a working
                    // provider that simply has not been needed yet, for something
                    // the user had explicitly switched off.
                    enabled: self.settings.enabled && !self.provider_disabled(&server, &languages),
                    installed,
                    ever_installed: crate::lsp_registry::ever_installed(root, &server),
                    declined: crate::lsp_registry::read_declined(root, &server).is_some(),
                    cleanup_pending: crate::lsp_registry::cleanup_pending(
                        self.registry_root.as_deref(),
                        &server,
                    ),
                    server,
                    languages: languages.into_iter().collect(),
                    managed,
                    manual_install_reason,
                    instances,
                }
            })
            .collect()
    }

    /// Whether the user's settings suppress this provider for every language it
    /// covers.
    ///
    /// Asked through `configured_primary`, which is the *only* rule that decides
    /// whether a launch happens, rather than by looking up the provider's own id
    /// in `lsp.servers`. Those are not the same lookup: settings select a provider
    /// by the id named in `lsp.languages.<language>.servers`, or by the language's
    /// own name -- never by a built-in provider id. So
    /// `lsp.servers."rust-analyzer".enabled = false` with no `lsp.languages.rust`
    /// entry does *not* stop rust-analyzer running, and reporting it as disabled
    /// would make the inventory lie in the opposite direction: a server that is
    /// spawned and serving, badged `off`, with its real runtime state and error
    /// hidden from the manager.
    fn provider_disabled(&self, server: &LanguageServerId, languages: &BTreeSet<String>) -> bool {
        !languages.is_empty()
            && languages.iter().all(|language| {
                matches!(
                    self.configured_primary(&language.to_ascii_lowercase()),
                    Some((id, Configured::Suppressed)) if id == *server
                )
            })
    }

    fn inventory_instance(
        &self,
        server: &LanguageServerId,
        languages: &BTreeSet<String>,
        root: &Path,
    ) -> LanguageServerInstanceStatus {
        let configured = self.settings.servers.get(server.key());
        let language = languages.iter().next().map_or(server.key(), String::as_str);
        // A provider its own entry disables resolves to nothing at all. Falling
        // through to the built-in table here reported a command that would never be
        // run, which read as an available provider.
        if self.provider_disabled(server, languages) {
            return LanguageServerInstanceStatus {
                root: root.to_path_buf(),
                source: LanguageServerSource::Unavailable,
                command: None,
                args: Vec::new(),
                runtime: LanguageServerRuntimeState::Idle,
                open_documents: 0,
                error: None,
            };
        }
        let resolved = configured
            .filter(|setting| setting.enabled && !setting.command.is_empty())
            .map(|setting| {
                (
                    LspSpec::new(
                        setting.command.clone(),
                        setting.args.clone(),
                        vec![language.to_owned()],
                    ),
                    LanguageServerSource::Configured,
                )
            })
            .or_else(|| {
                let fallback = builtin_spec(server, language)?;
                self.resolve_builtin(server, language, root, fallback)
            });
        let slot = self
            .servers
            .values()
            .find(|slot| slot.provider.as_ref() == Some(server) && slot.root == root);
        let runtime = self
            .runtime_states
            .get(&(server.clone(), root.to_path_buf()));
        let (command, args, source) = resolved.map_or(
            (None, Vec::new(), LanguageServerSource::Unavailable),
            |(spec, source)| (Some(spec.command), spec.args, source),
        );
        LanguageServerInstanceStatus {
            root: root.to_path_buf(),
            source,
            command,
            args,
            runtime: runtime.map_or_else(
                || {
                    if slot.is_some() {
                        LanguageServerRuntimeState::Starting
                    } else {
                        LanguageServerRuntimeState::Idle
                    }
                },
                |(state, _)| *state,
            ),
            open_documents: slot.map_or(0, |slot| slot.documents.len()),
            error: runtime.and_then(|(_, error)| error.clone()),
        }
    }
}
