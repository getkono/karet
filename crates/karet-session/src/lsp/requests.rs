//! Forwarding one document request to the server that serves its language.
//!
//! Split from `lsp`, which owns resolution and the document-sync lifecycle. Every
//! method here answers the same question -- is there a live server for this
//! language, and did the command reach it -- and returns `false` when there is
//! not, so the caller answers the request itself rather than leaving it hanging.

use super::*;

impl LspManager {
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
                self.servers.iter().find_map(|(key, slot)| {
                    (slot.documents.contains(&path) && key.provider.key() == provider)
                        .then_some(slot)
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
