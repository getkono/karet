use super::*;

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
            Command::DeclineLanguageServer { server, scope } => {
                let Some(root) = self.config.lsp_registry_dir.clone() else {
                    return true;
                };
                let version = crate::lsp_registry::installed_version(Some(&root), server);
                let declined = crate::lsp_registry::Declined::now(*scope, version);
                if let Err(message) = crate::lsp_registry::write_declined(&root, server, &declined)
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
                if let Err(message) = crate::lsp_registry::clear_declined(&root, server) {
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
            Command::RestartLanguageServer { server } => {
                self.restart_lsp(server.clone());
                return true;
            },
            _ => return false,
        };
        self.queue_lsp_registry(id, job);
        true
    }
}
