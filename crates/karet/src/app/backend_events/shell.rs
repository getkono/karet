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
                diag.severity,
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
                Severity::Error,
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
        if id.is_some() && id == self.commit_input.pending {
            self.commit_input.pending = None;
        }
        if id.is_some() && id == self.scm.repository_request {
            self.scm.repository_request = None;
            self.scm.repository_loading_since = None;
        }
        if id.is_some() && id == self.pending_pull_requests {
            self.pending_pull_requests = None;
            self.pull_request_items.clear();
            self.pull_request_remote = None;
        }
        if let Some(pending) = self.pending_blame.filter(|pending| Some(pending.0) == id) {
            self.pending_blame = None;
            self.failed_blame = Some((pending.1, pending.2, pending.3));
        }
        if let Some(req) = id {
            self.fail_pending_commit_detail(req, &message);
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
            self.notify(severity, kind, message);
        }
    }
}
