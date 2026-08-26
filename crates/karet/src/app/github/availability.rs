//! Installation and removal of the singleton pinned GitHub dashboard.

use super::*;

impl App {
    /// Install or remove the singleton pinned dashboard for current eligibility.
    pub(in crate::app) fn apply_github_availability(
        &mut self,
        repository: Option<GithubRepository>,
        auth: GithubAuth,
    ) {
        let Some(repository) = repository else {
            self.remove_github_dashboard();
            return;
        };
        if let Some(dashboard) = self.all_tabs_mut().find_map(|tab| match &mut tab.kind {
            TabKind::Github(GithubViewState::Dashboard(dashboard)) => Some(dashboard),
            _ => None,
        }) {
            dashboard.repository = repository;
            dashboard.auth = auth;
            dashboard.login_editing = false;
            dashboard.login_token.clear();
            dashboard.login_pending = None;
            dashboard.error = None;
            return;
        }

        let mut tab = Tab::github_dashboard(repository, auth);
        tab.view = self.alloc_view();
        // Availability lands asynchronously, so installing the dashboard must never
        // move the user: it slots in leftmost and the active tab rides along. The
        // lone exception is a bare Welcome tab, which the dashboard replaces
        // outright and is then the only tab there is. Focus is never touched — the
        // startup panel (or `--focus`) keeps it.
        let replacing_welcome = matches!(
            self.tabs.as_slice(),
            [Tab {
                kind: TabKind::Welcome,
                ..
            }]
        );
        if replacing_welcome {
            self.tabs.clear();
        }
        self.tabs.insert(0, tab);
        if replacing_welcome {
            self.set_active(0);
        } else {
            self.set_active(self.active.saturating_add(1));
        }
        self.request_dashboard_section(0);
    }

    /// Withdraw the dashboard from every pane. A pane that was showing it falls back
    /// to the tab it had in front most recently, the same way an ordinary close does.
    fn remove_github_dashboard(&mut self) {
        let active_view = self.tabs.get(self.active).map(|tab| tab.view);
        self.tabs.retain(|tab| !tab.is_github_dashboard());
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
            self.set_active(0);
        } else {
            let fallback = self.active.min(self.tabs.len() - 1);
            let next =
                Self::refocus_after_removal(&self.view_history, &self.tabs, active_view, fallback);
            self.set_active(next);
        }
        let App {
            stored,
            view_history,
            ..
        } = self;
        for pane in stored.values_mut() {
            let active_view = pane.tabs.get(pane.active).map(|tab| tab.view);
            pane.tabs.retain(|tab| !tab.is_github_dashboard());
            if pane.tabs.is_empty() {
                pane.tabs.push(Tab::welcome());
                pane.active = 0;
            } else {
                let fallback = pane.active.min(pane.tabs.len() - 1);
                pane.active =
                    Self::refocus_after_removal(view_history, &pane.tabs, active_view, fallback);
            }
        }
    }
}
