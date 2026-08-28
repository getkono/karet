use super::*;
use crate::session::notify_text::one_line;

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
            RegistryUpdate::Failed { request, message } => {
                // The evidence in full goes to the log; the toast gets one
                // printable line. A registry failure quotes text karet did not
                // write -- a failing npm's stderr, a path out of a downloaded
                // archive -- and none of that is written for a terminal.
                tracing::warn!(%message, "language-server registry operation failed");
                self.emit(
                    Some(request),
                    Event::Notification {
                        severity: Severity::Error,
                        kind: NotificationKind::Lsp,
                        message: format!("language-server registry: {}", one_line(&message)),
                    },
                )
            },
        }
    }
}

#[cfg(test)]
mod registry_update_tests {
    use super::*;
    use crate::lsp_registry::RegistryUpdate;
    use crate::session::SessionConfig;

    /// A registry failure quotes text karet did not write -- most of it a
    /// failing npm's stderr, which is as long and as hostile as the package
    /// that produced it. Passed on verbatim, it repaints the terminal.
    #[test]
    fn a_registry_failure_cannot_repaint_the_terminal() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let hostile = format!(
            "npm ERR! code 1\n\u{1b}[2J\u{202e}denwo\u{200b}{}",
            "x".repeat(500)
        );
        session.apply_lsp_registry_update(RegistryUpdate::Failed {
            request: RequestId(7),
            message: hostile,
        });
        let mut shown = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::Notification { message, .. } = event {
                shown = Some(message);
            }
        }
        let shown = shown.unwrap_or_default();
        assert!(
            !shown.contains(|character: char| character.is_control()),
            "{shown}"
        );
        assert!(!shown.contains('\u{202e}'), "{shown}");
        assert!(!shown.contains('\u{200b}'), "{shown}");
        assert!(
            shown.chars().count() <= 200,
            "{} characters",
            shown.chars().count()
        );
        assert!(shown.starts_with("language-server registry: npm ERR! code 1"));
    }

    #[test]
    fn a_plain_registry_failure_is_still_readable() {
        assert_eq!(one_line("  npm  install\n failed "), "npm install failed");
    }
}
