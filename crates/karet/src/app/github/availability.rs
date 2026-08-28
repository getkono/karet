//! Installation and removal of the workspace's GitHub surface.
//!
//! The surface belongs to the *workspace*, not to a pane: exactly one exists while the
//! repository is eligible, and none when it is not. There is only ever one
//! [`GithubSurface`], so that is a property of the type rather than an invariant the
//! tab system had to be policed into — which is what retired the pinned-tab guards.

use super::*;

impl App {
    /// Install or update the GitHub surface for current eligibility.
    ///
    /// The in-place update is load-bearing rather than a nicety. Availability is
    /// re-emitted on every refresh — `.git/config` watch churn included — so rebuilding
    /// the surface would reset the query, cursor, and selection and drop every detail
    /// page the user had open, each time git touched its own config.
    pub(in crate::app) fn apply_github_availability(
        &mut self,
        repository: Option<GithubRepository>,
        auth: GithubAuth,
    ) {
        let Some(repository) = repository else {
            self.github.clear();
            return;
        };
        if let Some(dashboard) = self.github.dashboard_mut() {
            dashboard.repository = repository;
            dashboard.auth = auth;
            dashboard.login_editing = false;
            dashboard.login_token.clear();
            dashboard.login_pending = None;
            dashboard.error = None;
            return;
        }
        self.github
            .install(GithubViewState::dashboard(repository, auth));
        self.request_github_section();
    }
}
