//! Scrolling a GitHub page, independent of whatever is hosting it.
//!
//! These live beside the pages rather than in the tab-kind match they were spelled
//! out in, so the offsets stay reachable once a view rather than a tab owns them.

use super::*;
use crate::app::scroll::clamp_u16;
use crate::app::scroll::cursor_in_window;

impl GithubViewState {
    /// Step this page's offset by `delta` rows.
    pub(crate) fn scroll_lines(&mut self, delta: i32) {
        match self {
            Self::Issue { scroll, .. } | Self::WorkflowRun { scroll, .. } => {
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                *scroll = next as u16;
            },
            Self::PullRequest(view) => {
                let next =
                    (i64::from(view.scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                view.scroll = next as u16;
            },
            // The dashboard moves a cursor and the forms move between fields; neither
            // carries a row offset a wheel could write to.
            Self::Dashboard(_) | Self::NewIssue { .. } | Self::NewPullRequest { .. } => {},
        }
    }

    /// Jump to the top (`top`) or the bottom of this page.
    pub(crate) fn scroll_edge(&mut self, top: bool) {
        match self {
            Self::Issue { scroll, .. } | Self::WorkflowRun { scroll, .. } => {
                *scroll = if top { 0 } else { u16::MAX };
            },
            Self::PullRequest(view) => view.scroll = if top { 0 } else { u16::MAX },
            Self::Dashboard(_) | Self::NewIssue { .. } | Self::NewPullRequest { .. } => {},
        }
    }

    /// Land on an absolute row offset — the scrollbar's counterpart to
    /// [`scroll_lines`](Self::scroll_lines).
    pub(crate) fn scroll_to(&mut self, position: usize, viewport: usize) {
        match self {
            Self::Issue { scroll, .. } | Self::WorkflowRun { scroll, .. } => {
                *scroll = clamp_u16(position);
            },
            Self::PullRequest(view) => view.scroll = clamp_u16(position),
            // The dashboard recomputes `first_visible` from its cursor every frame,
            // and its extent counts items rather than rows.
            Self::Dashboard(dashboard) => {
                let len = dashboard.row_count();
                dashboard.cursor = cursor_in_window(dashboard.cursor, position, viewport, len);
                dashboard.first_visible = position;
            },
            Self::NewIssue { .. } | Self::NewPullRequest { .. } => {},
        }
    }

    /// Land the pull-request commit list on an absolute offset. Separate from
    /// [`scroll_to`](Self::scroll_to): the commit list scrolls independently of the
    /// conversation it sits beside.
    pub(crate) fn scroll_commits_to(&mut self, position: usize) {
        if let Self::PullRequest(view) = self {
            view.commit_offset = clamp_u16(position);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> GithubViewState {
        GithubViewState::Issue {
            number: 208,
            issue: None,
            comments: GithubPage {
                items: Vec::new(),
                page: 1,
                next_page: None,
                total_count: None,
            },
            pending: None,
            loading_since: Pending::start(),
            error: None,
            scroll: 0,
        }
    }

    #[test]
    fn a_scrollable_page_steps_and_stops_at_its_ends() {
        let mut page = issue();
        page.scroll_lines(5);
        let GithubViewState::Issue { scroll, .. } = &page else {
            unreachable!("built an issue")
        };
        assert_eq!(*scroll, 5);

        // Scrolling past the top clamps rather than wrapping through zero.
        page.scroll_lines(-40);
        let GithubViewState::Issue { scroll, .. } = &page else {
            unreachable!("built an issue")
        };
        assert_eq!(*scroll, 0);

        page.scroll_edge(false);
        let GithubViewState::Issue { scroll, .. } = &page else {
            unreachable!("built an issue")
        };
        assert_eq!(*scroll, u16::MAX);
    }

    #[test]
    fn a_page_without_a_row_offset_ignores_the_wheel() {
        // The forms move between fields and the dashboard moves a cursor; neither has
        // an offset a wheel could write to, and neither may panic when one arrives.
        let mut form = GithubViewState::NewPullRequest {
            repository: GithubRepository {
                owner: "getkono".to_string(),
                repo: "karet".to_string(),
            },
            form: GithubPullRequestForm::default(),
        };
        form.scroll_lines(3);
        form.scroll_edge(false);
        form.scroll_to(9, 20);
        assert!(matches!(form, GithubViewState::NewPullRequest { .. }));
    }
}
