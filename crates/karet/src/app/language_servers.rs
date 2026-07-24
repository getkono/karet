use super::*;

impl App {
    pub(super) fn prompt_language_server_install(&mut self, server: LanguageServerId) {
        if self.overlay.is_none() {
            self.overlay = Some(Overlay::text(
                format!(
                    "{} is not installed · type install to download it",
                    server.display_name()
                ),
                TextPurpose::InstallLanguageServer { server },
            ));
        } else {
            self.notify(
                Severity::Warning,
                NotificationKind::Lsp,
                format!(
                    "{} is not installed; reopen a matching file to install it",
                    server.display_name()
                ),
            );
        }
    }

    pub(super) fn show_language_server_status(&mut self, servers: Vec<LanguageServerStatus>) {
        let installed = servers
            .iter()
            .filter(|server| server.installed.is_some())
            .count();
        self.status = Some(format!(
            "{installed}/{} managed language servers installed",
            servers.len()
        ));
    }

    pub(super) fn prompt_language_server_updates(
        &mut self,
        plan: LanguageServerPlanId,
        changes: Vec<LanguageServerChange>,
    ) {
        if changes.is_empty() {
            self.notify(
                Severity::Information,
                NotificationKind::Lsp,
                "language servers are up to date",
            );
            return;
        }
        let summary = changes
            .iter()
            .map(|change| {
                format!(
                    "{} {} → {}",
                    change.server.display_name(),
                    change.current.as_deref().unwrap_or("missing"),
                    change.target
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.overlay = Some(Overlay::text(
            format!("{summary} · type update to approve these exact versions"),
            TextPurpose::ApplyLanguageServerPlan { plan },
        ));
    }

    pub(super) fn show_language_server_progress(
        &mut self,
        server: LanguageServerId,
        downloaded: u64,
        total: Option<u64>,
    ) {
        self.status = Some(total.map_or_else(
            || format!("downloading {}: {downloaded} bytes", server.display_name()),
            |total| {
                format!(
                    "downloading {}: {downloaded}/{total} bytes",
                    server.display_name()
                )
            },
        ));
    }

    pub(super) fn finish_language_server_change(
        &mut self,
        server: LanguageServerId,
        version: String,
        restart_required: bool,
    ) {
        self.notify(
            Severity::Information,
            NotificationKind::Lsp,
            format!("installed {} {version}", server.display_name()),
        );
        if restart_required {
            self.overlay = Some(Overlay::text(
                format!(
                    "{} was updated · type restart to use it in this session",
                    server.display_name()
                ),
                TextPurpose::RestartLanguageServer { server },
            ));
        }
    }
}
