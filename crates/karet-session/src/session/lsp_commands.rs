use super::*;

mod code_lens;

impl Session {
    pub(super) fn handle_lsp_command(&mut self, id: RequestId, command: &Command) -> bool {
        let job = match command {
            Command::LanguageServerStatus => {
                let paths = self
                    .store
                    .docs
                    .values()
                    .map(|document| document.path.clone())
                    .collect::<Vec<_>>();
                let servers = self.lsp.inventory(paths);
                self.emit(Some(id), Event::LanguageServerStatus { servers });
                return true;
            },
            Command::InstallLanguageServer { server } => {
                crate::lsp_registry::RegistryJob::Install {
                    request: id,
                    server: server.clone(),
                }
            },
            // The refusal is a local file write, not registry work: it must land
            // even when the registry worker is gone, and it has nothing to download.
            Command::DeclineLanguageServer { server } => {
                let Some(root) = self.config.lsp_registry_dir.clone() else {
                    return true;
                };
                let declined = crate::lsp_registry::store::Declined::now();
                if let Err(message) =
                    crate::lsp_registry::store::write_declined(&root, server, &declined)
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
                return true;
            },
            Command::UndeclineLanguageServer { server } => {
                let Some(root) = self.config.lsp_registry_dir.clone() else {
                    return true;
                };
                if let Err(message) = crate::lsp_registry::store::clear_declined(&root, server) {
                    self.emit(
                        Some(id),
                        Event::Notification {
                            severity: Severity::Error,
                            kind: NotificationKind::Lsp,
                            message: format!("could not clear the refusal: {message}"),
                        },
                    );
                }
                return true;
            },
            Command::CheckLanguageServerUpdates { server } => {
                crate::lsp_registry::RegistryJob::Check {
                    request: id,
                    server: server.clone(),
                }
            },
            Command::ApplyLanguageServerPlan { plan, servers } => {
                crate::lsp_registry::RegistryJob::Apply {
                    request: id,
                    plan: *plan,
                    servers: servers.clone(),
                }
            },
            Command::UninstallLanguageServer { server } => {
                crate::lsp_registry::RegistryJob::Uninstall {
                    request: id,
                    server: server.clone(),
                }
            },
            Command::Format(request) => {
                self.format(id, request.clone());
                return true;
            },
            Command::SelectionRanges { doc, positions } => {
                self.selection_ranges(id, *doc, positions);
                return true;
            },
            Command::DocumentHighlights { doc, position } => {
                self.document_highlights(id, *doc, *position);
                return true;
            },
            Command::PrepareHierarchy {
                doc,
                position,
                direction,
            } => {
                self.prepare_hierarchy(id, *doc, *position, *direction);
                return true;
            },
            Command::ExpandHierarchy {
                doc,
                item,
                direction,
            } => {
                self.expand_hierarchy(id, *doc, (**item).clone(), *direction);
                return true;
            },
            Command::LinkedEditingRanges { doc, position } => {
                self.linked_editing_ranges(id, *doc, *position);
                return true;
            },
            Command::FormatOnType { doc, position, ch } => {
                self.format_on_type(id, *doc, *position, *ch);
                return true;
            },
            Command::InlineCompletion {
                doc,
                position,
                trigger,
            } => {
                self.inline_completion(id, *doc, (*position, *trigger));
                return true;
            },
            Command::SignInLanguageServer { server } => {
                self.language_server_account(id, server, true);
                return true;
            },
            Command::SignOutLanguageServer { server } => {
                self.language_server_account(id, server, false);
                return true;
            },
            Command::AcceptInlineCompletion { doc, server, item } => {
                self.accept_inline_completion(id, *doc, server, item.clone());
                return true;
            },
            Command::CodeLenses { doc, lines } => {
                self.code_lenses(id, *doc, *lines);
                return true;
            },
            Command::RunCodeLens { doc, lens } => {
                self.run_code_lens(id, *doc, lens.clone());
                return true;
            },
            Command::RestartLanguageServer { server } => {
                self.restart_lsp(server.clone());
                return true;
            },
            Command::CancelLanguageServerWork {
                server,
                root,
                token,
            } => {
                self.lsp.cancel_work(server, root, token);
                return true;
            },
            Command::AnswerLanguageServerMessage { request, action } => {
                self.lsp.window.answer(*request, *action);
                return true;
            },
            Command::LanguageServerLog { server } => {
                let entries = self.lsp.window.entries(server);
                self.emit(
                    Some(id),
                    Event::LanguageServerLog {
                        server: server.clone(),
                        entries,
                    },
                );
                return true;
            },
            _ => return false,
        };
        self.queue_lsp_registry(id, job);
        true
    }
}
