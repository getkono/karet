//! The GitHub view's own stack of pages.
//!
//! GitHub is a *view*, not a tab, so it carries its own navigation rather than
//! borrowing the pane's. [`GithubSurface`] is a strip and a stack at once: the
//! dashboard sits at the bottom and never closes, detail pages push on top of it and
//! render as a strip along the view, and `Esc` pops the one in front.
//!
//! Exactly one surface exists, which is what makes "one dashboard per workspace" a
//! property of the type instead of an invariant the tab system had to be policed
//! into — the pinned-tab guards it replaces did nothing else.

use std::collections::HashSet;

use super::*;

/// The pages the GitHub view is showing, and which of them is in front.
#[derive(Debug, Default)]
pub(crate) struct GithubSurface {
    /// Empty while the workspace is not GitHub-eligible. Otherwise `pages[0]` is the
    /// dashboard, and it is never removed except by ineligibility.
    pages: Vec<GithubViewState>,
    /// Index into `pages` of the page in front. Always valid when `pages` is not empty.
    active: usize,
    /// Strip hit regions: `(page index, span)`.
    pub(crate) page_hits: Vec<(usize, Rect)>,
    /// Strip close-button hit regions: `(page index, span)`.
    pub(crate) close_hits: Vec<(usize, Rect)>,
    /// Requests belonging to pages that have since closed, so their late replies do
    /// not surface as errors for something the user already backed out of.
    abandoned: HashSet<RequestId>,
}

impl GithubSurface {
    /// Whether the workspace is GitHub-eligible at all.
    pub(crate) fn is_active(&self) -> bool {
        !self.pages.is_empty()
    }

    /// Every page, bottom-first. The dashboard is `pages[0]` when there is one.
    pub(crate) fn pages(&self) -> &[GithubViewState] {
        &self.pages
    }

    /// Every page, mutably — the event handlers correlate replies across all of them.
    pub(crate) fn pages_mut(&mut self) -> &mut [GithubViewState] {
        &mut self.pages
    }

    /// The index of the page in front.
    pub(crate) fn active(&self) -> usize {
        self.active
    }

    /// The page in front.
    pub(crate) fn active_page(&self) -> Option<&GithubViewState> {
        self.pages.get(self.active)
    }

    /// The page in front, mutably.
    pub(crate) fn active_page_mut(&mut self) -> Option<&mut GithubViewState> {
        self.pages.get_mut(self.active)
    }

    /// The dashboard, read-only. Assertions want to look without driving anything;
    /// every production reader either mutates or walks [`pages`](Self::pages).
    #[cfg(test)]
    pub(crate) fn dashboard(&self) -> Option<&GithubDashboard> {
        match self.pages.first() {
            Some(GithubViewState::Dashboard(dashboard)) => Some(dashboard),
            _ => None,
        }
    }

    /// The dashboard, wherever the user currently is.
    ///
    /// Distinct from [`active_dashboard_mut`](Self::active_dashboard_mut): replies and
    /// availability always mean the dashboard, while keys and clicks only mean it when
    /// it is the page actually on screen.
    pub(crate) fn dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        match self.pages.first_mut() {
            Some(GithubViewState::Dashboard(dashboard)) => Some(dashboard),
            _ => None,
        }
    }

    /// The dashboard, but only while it is the page in front.
    pub(crate) fn active_dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        match self.pages.get_mut(self.active) {
            Some(GithubViewState::Dashboard(dashboard)) => Some(dashboard),
            _ => None,
        }
    }

    /// Seat a freshly built dashboard as the only page. Used when availability first
    /// arrives; a dashboard that already exists is updated in place instead.
    pub(crate) fn install(&mut self, dashboard: GithubViewState) {
        self.pages = vec![dashboard];
        self.active = 0;
        self.abandoned.clear();
    }

    /// Withdraw the surface when the workspace stops being GitHub-eligible.
    pub(crate) fn clear(&mut self) {
        self.pages.clear();
        self.active = 0;
        self.abandoned.clear();
    }

    /// Bring `page` to the front, reusing the page already showing that resource.
    ///
    /// Opening the same issue twice focuses the one that is open rather than stacking
    /// a second copy: under tabs a duplicate was at least visible and closeable, but a
    /// stack the user pops with `Esc` would just accumulate them.
    pub(crate) fn push(&mut self, page: GithubViewState) {
        if let Some(index) = self.pages.iter().position(|open| open.same_resource(&page)) {
            self.active = index;
            return;
        }
        self.pages.push(page);
        self.active = self.pages.len() - 1;
    }

    /// Focus the page at `index`, if there is one.
    pub(crate) fn select(&mut self, index: usize) {
        if index < self.pages.len() {
            self.active = index;
        }
    }

    /// Close the page in front and fall back to the one beneath it.
    ///
    /// The dashboard is never closed: it is the surface's floor, and the view would
    /// have nothing to show without it. Returns whether anything closed, so a key
    /// binding can decline the event and let it fall through.
    pub(crate) fn close_active(&mut self) -> bool {
        if self.active == 0 || self.active >= self.pages.len() {
            return false;
        }
        let closed = self.pages.remove(self.active);
        self.abandon(&closed);
        self.active = self.active.saturating_sub(1);
        true
    }

    /// Record a closed page's in-flight requests so a late reply stays quiet.
    fn abandon(&mut self, page: &GithubViewState) {
        self.abandoned.extend(page.pending_requests());
    }

    /// Whether `id` belongs to a page that has since closed. Consumes the record: a
    /// request is answered once, and the set must not grow without bound.
    pub(crate) fn was_abandoned(&mut self, id: Option<RequestId>) -> bool {
        id.is_some_and(|id| self.abandoned.remove(&id))
    }
}

impl App {
    /// Bring a page to the front of the GitHub surface.
    pub(in crate::app) fn push_github_page(&mut self, page: GithubViewState) {
        self.github.push(page);
    }

    /// Close the GitHub page in front. Reports whether anything closed, so the
    /// binding can fall through when the dashboard is all that is left.
    pub(in crate::app) fn close_github_page(&mut self) -> bool {
        self.github.close_active()
    }

    /// Scroll the GitHub page in front by `delta` rows. Always reports handled: the
    /// gesture belongs to this view even on a page with nothing to scroll, and letting
    /// it fall through would move the document drawn behind it.
    pub(in crate::app) fn scroll_github_page(&mut self, delta: i32) -> bool {
        if let Some(page) = self.github.active_page_mut() {
            page.scroll_lines(delta);
        }
        true
    }

    /// Jump the GitHub page in front to its top or bottom.
    pub(in crate::app) fn scroll_github_page_edge(&mut self, top: bool) -> bool {
        if let Some(page) = self.github.active_page_mut() {
            page.scroll_edge(top);
        }
        true
    }

    /// Route a click on the page strip: close a page, or bring one to the front.
    pub(super) fn github_strip_click(&mut self, point: (u16, u16)) -> bool {
        if let Some(index) = self
            .github
            .close_hits
            .iter()
            .find_map(|&(index, rect)| rect_contains(rect, point).then_some(index))
        {
            self.github.select(index);
            self.close_github_page();
            return true;
        }
        if let Some(index) = self
            .github
            .page_hits
            .iter()
            .find_map(|&(index, rect)| rect_contains(rect, point).then_some(index))
        {
            self.github.select(index);
            return true;
        }
        false
    }
}

/// A lazily loaded issue detail page.
pub(crate) fn github_issue(number: u64, pending: Option<RequestId>) -> GithubViewState {
    GithubViewState::Issue {
        number,
        issue: None,
        comments: empty_page(),
        pending,
        loading_since: Pending::start(),
        error: None,
        scroll: 0,
    }
}

/// A pull-request detail page seeded from its search result.
pub(crate) fn github_pull_request(
    pull_request: GithubPullRequest,
    can_write: bool,
    pending: Option<RequestId>,
) -> GithubViewState {
    GithubViewState::PullRequest(GithubPullRequestView {
        pull_request,
        comments: empty_page(),
        commits: Vec::new(),
        checks: Vec::new(),
        activity: Vec::new(),
        activity_error: None,
        can_write,
        section: GithubPullRequestSection::Conversation,
        pending,
        loading_since: Pending::start(),
        error: None,
        scroll: 0,
        commit_cursor: 0,
        commit_offset: 0,
        body_edit: None,
        comment_edit: String::new(),
        editor: None,
        preview: false,
        section_hits: Vec::new(),
        body_rect: Rect::default(),
        comment_rect: Rect::default(),
        merge_rect: Rect::default(),
        draft_rect: Rect::default(),
        check_hits: Vec::new(),
        commits_rect: Rect::default(),
    })
}

/// A read-only GitHub Actions workflow-run detail page.
pub(crate) fn github_workflow_run(
    repository: GithubRepository,
    workflow: Option<GithubWorkflow>,
    run: GithubWorkflowRun,
) -> GithubViewState {
    GithubViewState::WorkflowRun {
        repository,
        workflow,
        run,
        scroll: 0,
    }
}

/// A new-issue form page.
pub(crate) fn github_new_issue(
    repository: GithubRepository,
    metadata_pending: Option<RequestId>,
) -> GithubViewState {
    GithubViewState::NewIssue {
        repository,
        form: GithubIssueForm {
            metadata_pending,
            ..GithubIssueForm::default()
        },
    }
}

/// A new-pull-request form page.
pub(crate) fn github_new_pull_request(repository: GithubRepository) -> GithubViewState {
    GithubViewState::NewPullRequest {
        repository,
        form: GithubPullRequestForm::default(),
    }
}

fn empty_page<T>() -> GithubPage<T> {
    GithubPage {
        items: Vec::new(),
        page: 1,
        next_page: None,
        total_count: None,
    }
}
