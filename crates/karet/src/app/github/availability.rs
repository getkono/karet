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
            self.active = 0;
        } else {
            self.active = self.active.saturating_add(1);
        }
        self.request_dashboard_section(0);
    }

    fn remove_github_dashboard(&mut self) {
        self.tabs.retain(|tab| !tab.is_github_dashboard());
        for pane in self.stored.values_mut() {
            pane.tabs.retain(|tab| !tab.is_github_dashboard());
            if pane.tabs.is_empty() {
                pane.tabs.push(Tab::welcome());
                pane.active = 0;
            } else {
                pane.active = pane.active.min(pane.tabs.len() - 1);
            }
        }
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
            self.active = 0;
        } else {
            self.active = self.active.min(self.tabs.len() - 1);
        }
    }
}
