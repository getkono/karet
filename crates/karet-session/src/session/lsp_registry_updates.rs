use super::*;

impl Session {
    /// Adopt a completed registry operation.
    pub(crate) fn apply_lsp_registry_update(
        &mut self,
        update: crate::lsp_registry::RegistryUpdate,
    ) {
        use crate::lsp_registry::RegistryUpdate;
        match update {
            RegistryUpdate::Plan {
                request,
                plan,
                changes,
            } => {
                self.emit(
                    Some(request),
                    Event::LanguageServerUpdatePlan { plan, changes },
                );
            },
            RegistryUpdate::Changed {
                request,
                server,
                version,
                was_installed,
            } => {
                self.lsp.installed(server.clone());
                let restart_required = was_installed && self.lsp.is_running(&server);
                if !was_installed {
                    self.reopen_lsp_documents(Some(server.clone()));
                }
                self.emit(
                    Some(request),
                    Event::LanguageServerChanged {
                        server,
                        version,
                        restart_required,
                    },
                );
            },
            RegistryUpdate::Removed {
                request,
                server,
                cleanup_pending,
            } => {
                if self.lsp.restart(server.clone()) {
                    self.reopen_lsp_documents(None);
                }
                self.emit(
                    Some(request),
                    Event::LanguageServerRemoved {
                        server,
                        cleanup_pending,
                    },
                );
            },
            RegistryUpdate::Progress {
                server,
                downloaded,
                total,
            } => self.emit(
                None,
                Event::LanguageServerProgress {
                    server,
                    downloaded,
                    total,
                },
            ),
            RegistryUpdate::Complete { request } => {
                self.emit(
                    Some(request),
                    Event::Notification {
                        severity: Severity::Information,
                        kind: NotificationKind::Lsp,
                        message: "language-server update plan applied".into(),
                    },
                );
            },
            RegistryUpdate::Failed { request, message } => self.emit(
                Some(request),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Lsp,
                    message: format!("language-server registry: {message}"),
                },
            ),
        }
    }
}
