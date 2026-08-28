//! GitHub view, dashboard, and pull-request page state.

use super::*;

/// Dashboard subsection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GithubSection {
    /// Repository issues.
    #[default]
    Issues,
    /// Repository pull requests.
    PullRequests,
    /// GitHub Actions workflows and runs.
    Actions,
}

/// One editable field in a GitHub creation form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GithubFormField {
    /// Title.
    #[default]
    Title,
    /// Markdown description.
    Body,
    /// Assignee login list.
    Assignees,
    /// Label name list.
    Labels,
    /// Milestone number.
    Milestone,
    /// Issue type identifier.
    IssueType,
    /// Pull request source branch.
    Head,
    /// Pull request destination branch.
    Base,
}

/// GitHub-parity pull-request subsection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GithubPullRequestSection {
    /// Description, comments, activity, checks, and merge controls.
    #[default]
    Conversation,
    /// Pull-request commits.
    Commits,
    /// The existing comparison/diff view for the pull request range.
    FilesChanged,
}

/// Active Markdown editor inside a pull-request conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GithubPullRequestEditor {
    /// Editable pull-request body.
    Body,
    /// New timeline comment.
    Comment,
}

/// Stateful GitHub pull-request page.
#[derive(Debug)]
pub(crate) struct GithubPullRequestView {
    pub(crate) pull_request: GithubPullRequest,
    pub(crate) comments: GithubPage<GithubComment>,
    pub(crate) commits: Vec<GithubPullRequestCommit>,
    pub(crate) checks: Vec<GithubCheckRun>,
    pub(crate) activity: Vec<GithubPullRequestActivity>,
    pub(crate) activity_error: Option<String>,
    pub(crate) can_write: bool,
    pub(crate) section: GithubPullRequestSection,
    pub(crate) pending: Option<RequestId>,
    pub(crate) loading_since: Pending,
    pub(crate) error: Option<String>,
    pub(crate) scroll: u16,
    pub(crate) commit_cursor: usize,
    pub(crate) commit_offset: u16,
    pub(crate) body_edit: Option<String>,
    pub(crate) comment_edit: String,
    pub(crate) editor: Option<GithubPullRequestEditor>,
    pub(crate) preview: bool,
    pub(crate) section_hits: Vec<(GithubPullRequestSection, Rect)>,
    pub(crate) body_rect: Rect,
    pub(crate) comment_rect: Rect,
    pub(crate) merge_rect: Rect,
    pub(crate) draft_rect: Rect,
    pub(crate) check_hits: Vec<(String, Rect)>,
    pub(crate) commits_rect: Rect,
}

/// Detail collections fetched alongside a pull request's primary resource.
pub(crate) struct GithubPullRequestSupplement {
    pub(crate) commits: Vec<GithubPullRequestCommit>,
    pub(crate) checks: Vec<GithubCheckRun>,
    pub(crate) activity: Vec<GithubPullRequestActivity>,
    pub(crate) activity_error: Option<String>,
}

/// Pinned dashboard state.
#[derive(Debug)]
pub(crate) struct GithubDashboard {
    pub(crate) repository: GithubRepository,
    pub(crate) auth: GithubAuth,
    pub(crate) section: GithubSection,
    pub(crate) query: String,
    pub(crate) query_focused: bool,
    pub(crate) issues: GithubPage<GithubIssue>,
    pub(crate) pull_requests: GithubPage<GithubPullRequest>,
    pub(crate) workflows: GithubPage<GithubWorkflow>,
    pub(crate) runs: GithubPage<GithubWorkflowRun>,
    pub(crate) cursor: usize,
    pub(crate) selected: BTreeSet<usize>,
    pub(crate) loading_since: Option<Pending>,
    pub(crate) pending: Option<RequestId>,
    pub(crate) error: Option<String>,
    pub(crate) login_editing: bool,
    pub(crate) login_token: String,
    pub(crate) login_pending: Option<RequestId>,
    pub(crate) section_hits: Vec<(GithubSection, Rect)>,
    pub(crate) query_rect: Rect,
    pub(crate) auth_rect: Rect,
    pub(crate) table_rect: Rect,
    pub(crate) first_visible: usize,
}

impl GithubDashboard {
    fn new(repository: GithubRepository, auth: GithubAuth) -> Self {
        Self {
            repository,
            auth,
            section: GithubSection::Issues,
            query: "is:open sort:updated-desc".to_string(),
            query_focused: false,
            issues: empty_page(),
            pull_requests: empty_page(),
            workflows: empty_page(),
            runs: empty_page(),
            cursor: 0,
            selected: BTreeSet::new(),
            loading_since: None,
            pending: None,
            error: None,
            login_editing: false,
            login_token: String::new(),
            login_pending: None,
            section_hits: Vec::new(),
            query_rect: Rect::default(),
            auth_rect: Rect::default(),
            table_rect: Rect::default(),
            first_visible: 0,
        }
    }

    pub(crate) fn row_count(&self) -> usize {
        match self.section {
            GithubSection::Issues => self.issues.items.len(),
            GithubSection::PullRequests => self.pull_requests.items.len(),
            GithubSection::Actions => self.runs.items.len(),
        }
    }

    pub(super) fn reset_navigation(&mut self) {
        self.cursor = 0;
        self.first_visible = 0;
        self.selected.clear();
        self.error = None;
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

/// Content shown by a GitHub tab.
#[derive(Debug)]
pub(crate) enum GithubViewState {
    /// The special pinned repository dashboard.
    Dashboard(GithubDashboard),
    /// An issue detail request or loaded issue.
    Issue {
        number: u64,
        issue: Option<GithubIssue>,
        comments: GithubPage<GithubComment>,
        pending: Option<RequestId>,
        loading_since: Pending,
        error: Option<String>,
        scroll: u16,
    },
    /// New issue form.
    NewIssue {
        repository: GithubRepository,
        form: GithubIssueForm,
    },
    /// Pull request detail from a search result.
    PullRequest(GithubPullRequestView),
    /// A selected GitHub Actions workflow run.
    WorkflowRun {
        repository: GithubRepository,
        workflow: Option<GithubWorkflow>,
        run: GithubWorkflowRun,
        scroll: u16,
    },
    /// New pull request form.
    NewPullRequest {
        repository: GithubRepository,
        form: GithubPullRequestForm,
    },
}

impl GithubViewState {
    pub(crate) fn dashboard(repository: GithubRepository, auth: GithubAuth) -> Self {
        Self::Dashboard(GithubDashboard::new(repository, auth))
    }

    /// The page's name, as shown on whatever strip is hosting it.
    ///
    /// Derived from the state rather than stored beside it: a creation form
    /// *becomes* the resource it created once the response lands (see
    /// `apply_github_issue`), so a stored copy would have to be rewritten in
    /// lockstep with every such change — and would silently go stale the one time
    /// it was not.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Dashboard(_) => "GitHub".to_string(),
            Self::Issue { number, .. } => format!("Issue #{number}"),
            Self::NewIssue { .. } => "New GitHub Issue".to_string(),
            Self::PullRequest(view) => format!("Pull Request #{}", view.pull_request.number),
            Self::WorkflowRun { run, .. } => format!("Actions #{}", run.run_number),
            Self::NewPullRequest { .. } => "New Pull Request".to_string(),
        }
    }

    pub(crate) fn is_pinned(&self) -> bool {
        matches!(self, Self::Dashboard(_))
    }
}
