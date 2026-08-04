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
