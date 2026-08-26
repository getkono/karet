//! Dashboard accessors, section requests, navigation, and sign-in.

use super::*;

impl App {
    pub(super) fn active_dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        self.dashboard_at_mut(self.active)
    }

    /// The dashboard at `index` in the focused pane, if that tab is one. Used to
    /// drive the dashboard that was just installed, before it is the active tab.
    pub(super) fn dashboard_at_mut(&mut self, index: usize) -> Option<&mut GithubDashboard> {
        match self.tabs.get_mut(index).map(|tab| &mut tab.kind) {
            Some(TabKind::Github(GithubViewState::Dashboard(dashboard))) => Some(dashboard),
            _ => None,
        }
    }

    pub(super) fn request_github_section(&mut self) {
        self.request_dashboard_section(self.active);
    }

    pub(super) fn request_dashboard_section(&mut self, index: usize) {
        let Some((section, query)) = self.dashboard_at_mut(index).map(|dashboard| {
            dashboard.loading_since = Some(Pending::start());
            dashboard.error = None;
            (dashboard.section, dashboard.query.clone())
        }) else {
            return;
        };
        let command = match section {
            GithubSection::Issues => SessionCommand::GithubSearchIssues { query, page: 1 },
            GithubSection::PullRequests => {
                SessionCommand::GithubSearchPullRequests { query, page: 1 }
            },
            GithubSection::Actions => SessionCommand::GithubActions { page: 1 },
        };
        let request = self.send(command);
        if let Some(dashboard) = self.dashboard_at_mut(index) {
            dashboard.pending = request;
        }
    }

    pub(super) fn set_github_section(&mut self, section: GithubSection) {
        let changed = self.active_dashboard_mut().is_some_and(|dashboard| {
            if dashboard.section == section {
                false
            } else {
                dashboard.section = section;
                dashboard.reset_navigation();
                true
            }
        });
        if changed {
            self.request_github_section();
        }
    }
    pub(super) fn submit_github_login(&mut self) {
        let token = self
            .active_dashboard_mut()
            .map(|dashboard| std::mem::take(&mut dashboard.login_token))
            .unwrap_or_default();
        if token.trim().is_empty() {
            if let Some(dashboard) = self.active_dashboard_mut() {
                dashboard.error =
                    Some("Enter a GitHub personal access token to sign in.".to_string());
            }
            return;
        }
        let request = self.send(SessionCommand::GithubLogin {
            token: karet_session::GithubToken::new(token),
        });
        if let Some(dashboard) = self.active_dashboard_mut() {
            dashboard.login_editing = false;
            dashboard.login_pending = request;
        }
    }

    pub(super) fn github_move_cursor(&mut self, delta: i32, extend: bool) {
        let Some(dashboard) = self.active_dashboard_mut() else {
            return;
        };
        let previous = dashboard.cursor;
        let last = dashboard.row_count().saturating_sub(1) as i64;
        dashboard.cursor = (dashboard.cursor as i64 + i64::from(delta)).clamp(0, last) as usize;
        if extend {
            let (start, end) = if previous <= dashboard.cursor {
                (previous, dashboard.cursor)
            } else {
                (dashboard.cursor, previous)
            };
            dashboard.selected.extend(start..=end);
        }
    }
}
