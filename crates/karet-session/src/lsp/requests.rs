//! Forwarding one document request to the server that serves its language.
//!
//! Split from `lsp`, which owns resolution and the document-sync lifecycle. Every
//! method here answers the same question -- is there a live server for this
//! language, and did the command reach it -- and returns `false` when there is
//! not, so the caller answers the request itself rather than leaving it hanging.

use karet_core::CompletionItem;
use karet_core::CompletionTrigger;
use karet_core::InlayHint;
use karet_core::Range;

use super::*;
use crate::api::LocationKind;

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
        (position, trigger): (LineCol, CompletionTrigger),
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
            trigger,
        })
        .is_ok()
    }

    /// Forward a completion-item resolve to the server that answers `path`'s
    /// completions. Returns whether a live server accepted it -- when
    /// `false`, the caller answers that nothing was resolved.
    pub(crate) fn resolve_completion(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        path: &Path,
        item: CompletionItem,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        let item = Box::new(item);
        tx.try_send(ServerCmd::ResolveCompletion { request, doc, item })
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

    /// Forward an inlay-hint request (`range` already in UTF-16 columns).
    /// Returns whether a live server accepted it.
    pub(crate) fn inlay_hints(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        range: Range,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::InlayHints {
            request,
            doc,
            version,
            path,
            range,
        })
        .is_ok()
    }

    /// Forward an inlay-hint resolve to the server that answers `path`'s
    /// hints. Hands the hint back when no live server accepted it, so the
    /// caller can answer with it unchanged.
    pub(crate) fn resolve_inlay_hint(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        path: &Path,
        hint: InlayHint,
    ) -> Option<InlayHint> {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return Some(hint);
        };
        let unchanged = hint.clone();
        tx.try_send(ServerCmd::ResolveInlayHint { request, doc, hint })
            .is_err()
            .then_some(unchanged)
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

    /// Forward a signature-help request; `false` when no server serves `path`
    /// or its queue is full, so the caller answers it empty.
    pub(crate) fn signature_help(
        &self,
        language: Option<&str>,
        (request, doc): (RequestId, DocumentId),
        path: &Path,
        position: LineCol,
        manual: bool,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::SignatureHelp {
            request,
            doc,
            path,
            position,
            manual,
        })
        .is_ok()
    }

    /// Forward a selection-range request (`positions` already in UTF-16
    /// columns); `false` when no server serves `path` or its queue is full,
    /// so the caller answers it empty.
    pub(crate) fn selection_ranges(
        &self,
        language: Option<&str>,
        (request, doc, version): (RequestId, DocumentId, u64),
        path: &Path,
        positions: Vec<LineCol>,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::SelectionRanges {
            request,
            doc,
            version,
            path,
            positions,
        })
        .is_ok()
    }

    /// Forward a document-highlight request (`position` already in UTF-16
    /// columns); `false` when no server serves `path` or its queue is full,
    /// so the caller answers it with no highlights.
    pub(crate) fn document_highlights(
        &self,
        language: Option<&str>,
        (request, doc, version): (RequestId, DocumentId, u64),
        path: &Path,
        position: LineCol,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::DocumentHighlights {
            request,
            doc,
            version,
            path,
            position,
        })
        .is_ok()
    }

    /// Forward a request an edit makes (`ask` already in UTF-16 columns);
    /// `false` when no server serves `path` or its queue is full, so the
    /// caller answers it as no server would.
    pub(crate) fn editing(
        &self,
        language: Option<&str>,
        (request, doc, version): (RequestId, DocumentId, u64),
        path: &Path,
        ask: EditingAsk,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::Editing {
            request,
            doc,
            version,
            path,
            ask,
        })
        .is_ok()
    }

    /// Forward a definition request, or with a `kind` the navigation request a
    /// [`crate::api::Command::FindLocations`] names.
    #[allow(clippy::too_many_arguments)] // one request's identity, flattened as its siblings are
    pub(crate) fn definition(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        position: LineCol,
        kind: Option<LocationKind>,
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
            kind,
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

    #[allow(clippy::too_many_arguments)] // one request's identity, flattened as its siblings are
    pub(crate) fn rename(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
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
            doc,
            version,
            path,
            position,
            new_name,
        })
        .is_ok()
    }

    /// Ask the document's server whether the symbol at `position` can be
    /// renamed. `false` when no server runs for it, so the caller answers.
    /// `position` is in UTF-16 columns and `caret` the same point in buffer
    /// columns.
    #[allow(clippy::too_many_arguments)] // one request's identity, flattened as its siblings are
    pub(crate) fn prepare_rename(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        position: LineCol,
        caret: LineCol,
    ) -> bool {
        let path = absolute_path(path);
        let Some(tx) = self.existing_server(language, &path) else {
            return false;
        };
        tx.try_send(ServerCmd::PrepareRename {
            request,
            doc,
            version,
            path,
            position,
            caret,
        })
        .is_ok()
    }

    /// Hand one code-action step to the server serving `path`, the document
    /// of `language` the actions are for.
    ///
    /// Gives the step back when no server took it, so the caller answers it.
    /// A `List` step's own path is made absolute here like every request's.
    pub(crate) fn code_action(
        &self,
        language: Option<&str>,
        path: &Path,
        mut cmd: CodeActionCmd,
    ) -> Option<CodeActionCmd> {
        let path = absolute_path(path);
        if let CodeActionCmd::List { path: target, .. } = &mut cmd {
            *target = path.clone();
        }
        let Some(tx) = self.existing_server(language, &path) else {
            return Some(cmd);
        };
        match tx.try_send(ServerCmd::CodeAction(cmd)) {
            Ok(()) => None,
            Err(error) => match error.into_inner() {
                ServerCmd::CodeAction(cmd) => Some(cmd),
                _ => None,
            },
        }
    }

    /// Hand one code-lens step to the server serving `path`, the document of
    /// `language` the lenses are for.
    ///
    /// Gives the step back when no server took it, so the caller answers it.
    /// A `List` step's own path is made absolute here like every request's.
    pub(crate) fn code_lens(
        &self,
        language: Option<&str>,
        path: &Path,
        mut cmd: CodeLensCmd,
    ) -> Option<CodeLensCmd> {
        let path = absolute_path(path);
        if let CodeLensCmd::List { path: target, .. } = &mut cmd {
            *target = path.clone();
        }
        let Some(tx) = self.existing_server(language, &path) else {
            return Some(cmd);
        };
        match tx.try_send(ServerCmd::CodeLens(cmd)) {
            Ok(()) => None,
            Err(error) => match error.into_inner() {
                ServerCmd::CodeLens(cmd) => Some(cmd),
                _ => None,
            },
        }
    }

    /// Forward a `textDocument/formatting` request, indented and bounded as
    /// `ask` says.
    ///
    /// `ask` is resolved by the caller against the *document's* language and
    /// the editor settings, because that is where they live; this manager only
    /// knows languages by key. See [`FormattingAsk`].
    pub(crate) fn formatting(
        &self,
        language: Option<&str>,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: &Path,
        ask: FormattingAsk,
    ) -> bool {
        // Every other request reaches its server through `existing_server`, which
        // declines when language servers are switched off. This one resolves its
        // own slot to honour the per-language `formatter` preference, so it has to
        // ask the same question itself.
        if !self.settings.enabled {
            return false;
        }
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
            ask,
        })
        .is_ok()
    }
}
