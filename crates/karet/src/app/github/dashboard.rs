//! Dashboard accessors, section requests, navigation, and sign-in.

use super::*;

impl App {
    /// The dashboard, but only while it is the page in front — what a key or a click
    /// means. `n` on an issue page must not open a form for the dashboard's hidden
    /// cursor row.
    pub(super) fn active_dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        self.github.active_dashboard_mut()
    }

    /// The dashboard wherever the user currently is — what a reply means.
    pub(super) fn dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        self.github.dashboard_mut()
    }

    pub(super) fn request_github_section(&mut self) {
        let Some((section, query)) = self.dashboard_mut().map(|dashboard| {
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
        if let Some(dashboard) = self.dashboard_mut() {
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
