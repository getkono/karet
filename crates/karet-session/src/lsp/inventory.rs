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
                    enabled: self.settings.enabled,
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

    fn inventory_instance(
        &self,
        server: &LanguageServerId,
        languages: &BTreeSet<String>,
        root: &Path,
    ) -> LanguageServerInstanceStatus {
        let configured = self.settings.servers.get(server.key());
        let language = languages.iter().next().map_or(server.key(), String::as_str);
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
