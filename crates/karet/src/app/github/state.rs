//! Pull-request page state shared by input and rendering.

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
    pub(crate) repository: GithubRepository,
    pub(crate) pull_request: GithubPullRequest,
    pub(crate) comments: GithubPage<GithubComment>,
    pub(crate) commits: Vec<GithubPullRequestCommit>,
    pub(crate) checks: Vec<GithubCheckRun>,
    pub(crate) activity: Vec<GithubPullRequestActivity>,
    pub(crate) activity_error: Option<String>,
    pub(crate) can_write: bool,
    pub(crate) section: GithubPullRequestSection,
    pub(crate) pending: Option<RequestId>,
    pub(crate) loading_since: Instant,
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
