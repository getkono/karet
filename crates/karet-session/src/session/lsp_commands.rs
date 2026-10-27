//! [`LspCommand`] and [`LanguageServerCommand`] dispatch: language
//! intelligence requests about a document, and the management of the
//! language servers that answer them.

use super::*;
use crate::api::LanguageServerCommand;
use crate::api::LspCommand;

mod code_lens;
mod link_color;

impl Session {
    /// Serve one [`LspCommand`].
    pub(super) fn handle_lsp(&mut self, id: RequestId, command: LspCommand) {
        match command {
            LspCommand::Completion {
                doc,
                position,
                trigger,
            } => self.completion(id, doc, (position, trigger)),
            LspCommand::ResolveCompletion { doc, item } => self.resolve_completion(id, doc, item),
            LspCommand::InlayHints { doc, range } => self.inlay_hints(id, doc, range),
            LspCommand::ResolveInlayHint { doc, hint } => self.resolve_inlay_hint(id, doc, hint),
            LspCommand::InlineCompletion {
                doc,
                position,
                trigger,
            } => self.inline_completion(id, doc, (position, trigger)),
            LspCommand::AcceptInlineCompletion { doc, server, item } => {
                self.accept_inline_completion(id, doc, &server, item);
            },
            LspCommand::CodeLenses { doc, lines } => self.code_lenses(id, doc, lines),
            LspCommand::RunCodeLens { doc, lens } => self.run_code_lens(id, doc, lens),
            LspCommand::LinksAndColors { doc } => self.links_and_colors(id, doc),
            LspCommand::ResolveDocumentLink { doc, link } => {
                self.resolve_document_link(id, doc, link);
            },
            LspCommand::Hover { doc, position } => self.hover(id, doc, position),
            LspCommand::SignatureHelp {
                doc,
                position,
                manual,
            } => self.signature_help(id, doc, position, manual),
            LspCommand::SelectionRanges { doc, positions } => {
                self.selection_ranges(id, doc, &positions);
            },
            LspCommand::DocumentHighlights { doc, position } => {
                self.document_highlights(id, doc, position);
            },
            LspCommand::LinkedEditingRanges { doc, position } => {
                self.linked_editing_ranges(id, doc, position);
            },
            LspCommand::FormatOnType { doc, position, ch } => {
                self.format_on_type(id, doc, position, ch);
            },
            LspCommand::Definition { doc, position } => self.definition(id, doc, position, None),
            LspCommand::FindLocations {
                doc,
                position,
                kind,
            } => self.definition(id, doc, position, Some(kind)),
            LspCommand::PrepareHierarchy {
                doc,
                position,
                direction,
            } => self.prepare_hierarchy(id, doc, position, direction),
            LspCommand::ExpandHierarchy {
                doc,
                item,
                direction,
            } => self.expand_hierarchy(id, doc, *item, direction),
            LspCommand::DocumentSymbols { doc } => self.document_symbols(id, doc),
            LspCommand::WorkspaceSymbols { query } => self.workspace_symbols(id, query),
            LspCommand::PrepareRename { doc, position } => self.prepare_rename(id, doc, position),
            LspCommand::Rename {
                doc,
                position,
                new_name,
            } => self.rename(id, doc, position, new_name),
            LspCommand::Format(request) => self.format(id, request),
            LspCommand::CodeActions {
                doc,
                range,
                kinds,
                trigger,
            } => self.code_actions(id, doc, range, kinds, trigger),
            LspCommand::ApplyCodeAction {
                doc,
                version,
                action,
            } => self.apply_code_action(id, doc, version, action),
        }
    }

    /// Serve one [`LanguageServerCommand`]. Registry work (install, update
    /// checks, plans, uninstall) is queued on the registry worker; the rest is
    /// answered here.
    pub(super) fn handle_language_server(&mut self, id: RequestId, command: LanguageServerCommand) {
        let job = match command {
            LanguageServerCommand::Status => {
                let paths = self
                    .store
                    .docs
                    .values()
                    .map(|document| document.path.clone())
                    .collect::<Vec<_>>();
                let servers = self.lsp.inventory(paths);
                self.emit(Some(id), Event::LanguageServerStatus { servers });
                return;
            },
            LanguageServerCommand::Install { server } => {
                crate::lsp_registry::RegistryJob::Install {
                    request: id,
                    server,
                }
            },
            // The refusal is a local file write, not registry work: it must land
            // even when the registry worker is gone, and it has nothing to download.
            LanguageServerCommand::Decline { server } => {
                let Some(root) = self.config.lsp_registry_dir.clone() else {
                    return;
                };
                let declined = crate::lsp_registry::store::Declined::now();
                if let Err(message) =
                    crate::lsp_registry::store::write_declined(&root, &server, &declined)
                {
                    self.emit(
                        Some(id),
                        Event::Notification {
                            severity: Severity::Error,
                            kind: NotificationKind::Lsp,
                            message: format!("could not record the refusal: {message}"),
                        },
                    );
                }
                return;
            },
            LanguageServerCommand::Undecline { server } => {
                let Some(root) = self.config.lsp_registry_dir.clone() else {
                    return;
                };
                if let Err(message) = crate::lsp_registry::store::clear_declined(&root, &server) {
                    self.emit(
                        Some(id),
                        Event::Notification {
                            severity: Severity::Error,
                            kind: NotificationKind::Lsp,
                            message: format!("could not clear the refusal: {message}"),
                        },
                    );
                }
                return;
            },
            LanguageServerCommand::CheckUpdates { server } => {
                crate::lsp_registry::RegistryJob::Check {
                    request: id,
                    server,
                }
            },
            LanguageServerCommand::ApplyPlan { plan, servers } => {
                crate::lsp_registry::RegistryJob::Apply {
                    request: id,
                    plan,
                    servers,
                }
            },
            LanguageServerCommand::Uninstall { server } => {
                crate::lsp_registry::RegistryJob::Uninstall {
                    request: id,
                    server,
                }
            },
            LanguageServerCommand::Restart { server } => {
                self.restart_lsp(server);
                return;
            },
            LanguageServerCommand::CancelWork {
                server,
                root,
                token,
            } => {
                self.lsp.cancel_work(&server, &root, &token);
                return;
            },
            LanguageServerCommand::AnswerMessage { request, action } => {
                self.lsp.window.answer(request, action);
                return;
            },
            LanguageServerCommand::Log { server } => {
                let entries = self.lsp.window.entries(&server);
                self.emit(Some(id), Event::LanguageServerLog { server, entries });
                return;
            },
            LanguageServerCommand::SignIn { server } => {
                self.language_server_account(id, &server, true);
                return;
            },
            LanguageServerCommand::SignOut { server } => {
                self.language_server_account(id, &server, false);
                return;
            },
        };
        self.queue_lsp_registry(id, job);
    }
}
