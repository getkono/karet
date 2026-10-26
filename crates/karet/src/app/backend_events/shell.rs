//! Shell/workspace backend-event handlers: filesystem changes, configuration
//! reloads, and the notification funnel. Called only from the
//! [`App::on_backend_event`] router.

use super::*;

impl App {
    /// Keep live surfaces current after filesystem changes: nested-repository
    /// badges, a running workspace search, and the GitHub availability probe
    /// when git metadata changed.
    pub(super) fn on_fs_changed(&mut self, paths: &[PathBuf]) {
        // No extra debouncing needed here — the watcher already debounces at
        // the source, and the result cap keeps a search re-run cheap.
        self.invalidate_nested_repository_statuses(paths);
        self.file_index_fs_changed(paths);
        if !self.search.query.is_empty() {
            self.run_global_search();
        }
        if paths.iter().any(|path| {
            path.file_name().is_some_and(|name| name == "config")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == ".git")
        }) {
            self.send_command(SessionCommand::GithubRefresh);
        }
    }

    /// Adopt a reloaded configuration: apply it, refresh open inspectors,
    /// surface load diagnostics, and revalidate terminal-dependent settings.
    pub(super) fn on_config_changed(&mut self, report: LoadedConfig) {
        // The Spelling panel's results are a function of these settings, so a
        // change to them (a dictionary word, a scope toggle, the locale) makes
        // the list wrong. Compare before `apply_loaded_config` overwrites them,
        // and only for `spellcheck` — an unrelated edit must not cost a walk.
        let spellcheck_changed = report.settings.spellcheck != self.settings.spellcheck;
        self.apply_loaded_config(report.clone(), false);
        if spellcheck_changed {
            // Turning the checker off retires the panel; otherwise the list it is
            // showing is now wrong and has to be walked again.
            self.sync_spelling_availability();
            self.sync_todos_availability();
            self.invalidate_spelling();
        }
        for tab in self.all_tabs_mut() {
            if let TabKind::LoadedConfig {
                report: open_report,
                ..
            } = &mut tab.kind
            {
                *open_report = report.clone();
            }
        }
        for diag in std::mem::take(&mut self.config_diagnostics) {
            self.notify(
                Report::from_severity(diag.severity),
                NotificationKind::System,
                format!("config: {}", diag.message),
            );
        }
        let graphical_cursor_requested = self.tabs.get(self.active).is_some_and(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .graphical_cursor()
                == Some(true)
        });
        if graphical_cursor_requested && !self.graphical_cursor_compatible() {
            self.notify(
                Report::Failure,
                NotificationKind::System,
                "graphical cursor is not compatible with this terminal",
            );
        }
        let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .completion()
                .enabled()
        });
        if !completion_enabled {
            self.dismiss_completion();
        }
    }

    /// The backend-condition funnel: clear whichever pending state the failed
    /// request owned, then surface the message as a notification (unless a
    /// language-server surface already presented it).
    pub(super) fn on_notification(
        &mut self,
        id: Option<RequestId>,
        severity: Severity,
        kind: NotificationKind,
        message: String,
    ) {
        let language_server_operation_failed =
            id.is_some_and(|request| self.fail_language_server_operation(request, &message));
        // A discard is the one repository write that answers with a reason instead
        // of an outcome event: the message below carries it, so the card has done
        // its job. Every other write ends at `on_vcs_operation_finished`, which
        // retires its own card.
        self.notifications.dismiss_tagged(Self::VCS_DISCARD_TAG);
        if id.is_some() && id == self.commit_input.pending {
            self.commit_input.pending = None;
            self.notifications.dismiss_tagged(Self::VCS_COMMIT_TAG);
            // A refusal always shows its log: the reason is the whole point, and
            // a toast is too small to carry a hook's output.
            self.commit_console_finished(crate::app::commit_console::ConsoleOutcome::Failed(
                crate::app::scm::aicommit::one_line(&message),
            ));
        }
        if id.is_some() && id == self.scm.repository_request {
            self.scm.repository_request = None;
            self.scm.repository_loading_since = None;
        }
        if id.is_some() && id == self.pending_pull_requests {
            self.pending_pull_requests = None;
            self.notifications.dismiss_tagged(Self::PULL_REQUESTS_TAG);
            self.pull_request_items.clear();
            self.pull_request_remote = None;
        }
        if let Some(pending) = self.pending_blame.filter(|pending| Some(pending.0) == id) {
            self.pending_blame = None;
            self.failed_blame = Some((pending.1, pending.2, pending.3));
        }
        if let Some(req) = id {
            // An open fails with an error notification and nothing else; its
            // warnings (a bad `.editorconfig`) still end in `Opened`.
            if severity == Severity::Error {
                self.on_open_failed(req, &message);
            }
            self.fail_pending_commit_detail(req, &message);
            self.fail_hierarchy_request(req);
            // A range diff has no failure event of its own: it answers with this
            // notification, which carries its request id.
            self.fail_pending_compare(req, &message);
            if let Some((view, _)) = self.pending_merge_conflicts.remove(&req)
                && let Some(tab) = self.all_tabs_mut().find(|tab| tab.view == view)
                && let Some(conflict) = tab.merge_conflict.as_mut()
            {
                conflict.error = Some(message.clone());
            }
        }
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind
                && id.is_some()
                && view.inventory_request == id
            {
                if view.inventory_request == id {
                    view.inventory_request = None;
                }
                view.loading_since = None;
                view.error = Some(message.clone());
            }
        }
        if !language_server_operation_failed {
            self.notify(Report::from_severity(severity), kind, message);
        }
    }
}

/// [`App::on_config_changed`]'s side effects beyond applying the settings.
#[cfg(test)]
mod tests {
    use crate::app::tests::support::app;
    use crate::app::tests::support::text_tab;
    use crate::app::*;
    use crate::completion::PendingCompletion;

    const INCOMPATIBLE: &str = "graphical cursor is not compatible with this terminal";

    /// Deliver `settings` as the watcher's reload would.
    fn reload(app: &mut App, settings: Settings) {
        reload_report(app, LoadedConfig::from_settings(settings));
    }

    /// Deliver `report` as the watcher's reload would.
    fn reload_report(app: &mut App, report: LoadedConfig) {
        app.on_backend_event(
            None,
            SessionEvent::ConfigChanged {
                report: Box::new(report),
            },
        );
    }

    /// The active notifications' severities and titles, newest first.
    fn notes(app: &App) -> Vec<(Severity, String)> {
        app.notifications
            .active()
            .into_iter()
            .map(|note| (note.severity, note.title.clone()))
            .collect()
    }

    /// An app whose active tab is a code tab, so per-language settings apply.
    fn code_app() -> App {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        app
    }

    /// Every open Loaded Settings tab shows the reloaded report, not the one it
    /// was opened with.
    #[test]
    fn open_loaded_settings_tabs_show_the_reloaded_report() {
        let mut app = app();
        app.push_tab(Tab::loaded_config(LoadedConfig::from_settings(
            Settings::default(),
        )));
        app.push_tab(Tab::loaded_config(LoadedConfig::from_settings(
            Settings::default(),
        )));
        let mut settings = Settings::default();
        settings.editor.tab_size = 7;
        reload(&mut app, settings);
        let sizes: Vec<_> = app
            .tabs
            .iter()
            .filter_map(|tab| match &tab.kind {
                TabKind::LoadedConfig { report, .. } => Some(report.settings.editor.tab_size),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![7, 7]);
    }

    /// Each load problem the reload carries becomes one `config:` notification
    /// at its own severity, and is reported once rather than on every reload.
    #[test]
    fn reload_diagnostics_are_notified_once() {
        let mut app = app();
        let mut report = LoadedConfig::from_settings(Settings::default());
        report.diagnostics = vec![
            ConfigDiagnostic {
                path: PathBuf::from("/c/setting.jsonc"),
                message: "bad section".into(),
                severity: Severity::Warning,
            },
            ConfigDiagnostic {
                path: PathBuf::from("/c/setting.jsonc"),
                message: "unreadable".into(),
                severity: Severity::Error,
            },
        ];
        reload_report(&mut app, report);
        assert_eq!(
            notes(&app),
            vec![
                (Severity::Error, "config: unreadable".into()),
                (Severity::Warning, "config: bad section".into()),
            ]
        );
        assert!(
            app.config_diagnostics.is_empty(),
            "they are taken, not kept"
        );
    }

    /// Asking for the graphical cursor on a terminal that cannot draw it warns;
    /// a terminal that can, or a setting left unset, stays quiet.
    #[test]
    fn a_graphical_cursor_the_terminal_cannot_draw_is_reported() {
        let rows = [
            ("requested, incompatible", Some(true), false, true),
            ("requested, compatible", Some(true), true, false),
            ("unset, incompatible", None, false, false),
            ("declined, incompatible", Some(false), false, false),
        ];
        for (name, requested, compatible, warns) in rows {
            let mut app = code_app();
            app.caps.kitty_keyboard = compatible;
            app.caps.kitty_graphics = compatible;
            app.caps.graphics = if compatible {
                GraphicsProtocol::Kitty
            } else {
                GraphicsProtocol::Halfblocks
            };
            let mut settings = Settings::default();
            settings.editor.graphical_cursor = requested;
            reload(&mut app, settings);
            let warned = notes(&app).contains(&(Severity::Error, INCOMPATIBLE.into()));
            assert_eq!(warned, warns, "{name}");
        }
    }

    /// Turning completion off closes a waiting completion request; leaving it on
    /// keeps it.
    #[test]
    fn disabling_completion_closes_a_pending_request() {
        for (enabled, kept) in [(false, false), (true, true)] {
            let mut app = code_app();
            app.completion.pending = Some(PendingCompletion {
                id: RequestId(1),
                doc: DocumentId(9),
                anchor: LineCol::new(0, 0),
                position: LineCol::new(0, 0),
            });
            let mut settings = Settings::default();
            settings.editor.completion.enabled = enabled;
            reload(&mut app, settings);
            assert_eq!(app.completion.pending.is_some(), kept, "enabled={enabled}");
        }
    }
}
