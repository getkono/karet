//! GitHub dashboard, detail, and creation-form application state.

mod events;
mod forms;
mod pull_request;
mod state;

pub(crate) use forms::auth_label;
use forms::comma_list;
use forms::edit_issue_form;
use forms::edit_pull_request_form;
use forms::nonempty;
use karet_session::GithubAuth;
use karet_session::GithubCheckRun;
use karet_session::GithubComment;
use karet_session::GithubIssue;
use karet_session::GithubNewIssue;
use karet_session::GithubNewPullRequest;
use karet_session::GithubPage;
use karet_session::GithubPullRequest;
use karet_session::GithubPullRequestActivity;
use karet_session::GithubPullRequestCommit;
use karet_session::GithubRepository;
use karet_session::GithubWorkflow;
use karet_session::GithubWorkflowRun;
pub(crate) use state::GithubFormField;
pub(crate) use state::GithubPullRequestEditor;
pub(crate) use state::GithubPullRequestSection;
pub(crate) use state::GithubPullRequestSupplement;
pub(crate) use state::GithubPullRequestView;
pub(crate) use state::GithubSection;

use super::*;

/// Fixed visual height of each dashboard result card.
pub(crate) const DASHBOARD_ROW_HEIGHT: usize = 3;

/// New-issue editor state.
#[derive(Debug, Default)]
pub(crate) struct GithubIssueForm {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) assignees: String,
    pub(crate) labels: String,
    pub(crate) milestone: String,
    pub(crate) issue_type: String,
    pub(crate) assignee_options: Vec<String>,
    pub(crate) assignee_cursor: usize,
    pub(crate) metadata_pending: Option<RequestId>,
    pub(crate) field: GithubFormField,
    pub(crate) preview: bool,
    pub(crate) submitting: Option<RequestId>,
    pub(crate) error: Option<String>,
}

impl GithubIssueForm {
    /// Repository assignees matching the fragment after the final comma.
    pub(crate) fn assignee_suggestions(&self) -> Vec<&str> {
        let fragment = self
            .assignees
            .rsplit_once(',')
            .map_or(self.assignees.as_str(), |(_, fragment)| fragment)
            .trim()
            .to_ascii_lowercase();
        let selected = comma_list(&self.assignees);
        self.assignee_options
            .iter()
            .filter(|login| {
                !selected.iter().any(|value| value == *login)
                    && login.to_ascii_lowercase().contains(&fragment)
            })
            .map(String::as_str)
            .collect()
    }
}

// TODO(spargen-project-items): replace the label/milestone/type text inputs with
// repository-aware selector islands and add project/custom-field controls.
// Project-item request bodies remain typed manual adapters while spargen#46
// tracks their unsupported oneOf property-presence constraints. Do not add
// untyped JSON calls here as a temporary workaround.

/// New-pull-request editor state.
#[derive(Debug)]
pub(crate) struct GithubPullRequestForm {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) head: String,
    pub(crate) base: String,
    pub(crate) field: GithubFormField,
    pub(crate) preview: bool,
    pub(crate) draft: bool,
    pub(crate) maintainer_can_modify: bool,
    pub(crate) submitting: Option<RequestId>,
    pub(crate) error: Option<String>,
}

impl Default for GithubPullRequestForm {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            head: String::new(),
            base: "main".to_string(),
            field: GithubFormField::Title,
            preview: false,
            draft: false,
            maintainer_can_modify: true,
            submitting: None,
            error: None,
        }
    }
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
    pub(crate) loading_since: Option<Instant>,
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

    fn reset_navigation(&mut self) {
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
        repository: GithubRepository,
        number: u64,
        issue: Option<GithubIssue>,
        comments: GithubPage<GithubComment>,
        pending: Option<RequestId>,
        loading_since: Instant,
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

    pub(crate) fn is_pinned(&self) -> bool {
        matches!(self, Self::Dashboard(_))
    }
}

impl App {
    /// Install or remove the singleton pinned dashboard for current eligibility.
    pub(super) fn apply_github_availability(
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
        let only_landing = self.tabs.len() == 1
            && (matches!(self.tabs[0].kind, TabKind::Welcome) || self.tabs[0].is_preview);
        if matches!(
            self.tabs.as_slice(),
            [Tab {
                kind: TabKind::Welcome,
                ..
            }]
        ) {
            self.tabs.clear();
        }
        self.tabs.insert(0, tab);
        if only_landing {
            self.active = 0;
            self.focus = Focus::Editor;
        } else {
            self.active = self.active.saturating_add(1);
        }
        self.request_github_section();
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

    fn active_dashboard_mut(&mut self) -> Option<&mut GithubDashboard> {
        match self.tabs.get_mut(self.active).map(|tab| &mut tab.kind) {
            Some(TabKind::Github(GithubViewState::Dashboard(dashboard))) => Some(dashboard),
            _ => None,
        }
    }

    fn request_github_section(&mut self) {
        let Some((section, query)) = self.active_dashboard_mut().map(|dashboard| {
            dashboard.loading_since = Some(Instant::now());
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
        let request = self.send_command_id(command);
        if let Some(dashboard) = self.active_dashboard_mut() {
            dashboard.pending = request;
        }
    }

    fn set_github_section(&mut self, section: GithubSection) {
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

    /// Handle dashboard and form keys before the ordinary editor keymap.
    pub(super) fn github_key(&mut self, key: KeyEvent) -> bool {
        if self.focus != Focus::Editor {
            return false;
        }
        let Some(kind) = self.tabs.get(self.active).map(|tab| &tab.kind) else {
            return false;
        };
        if !matches!(kind, TabKind::Github(_)) {
            return false;
        }
        if self.github_form_key(key) {
            return true;
        }
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.refresh_active_github();
        }
        if self.github_pull_request_key(key) {
            return true;
        }
        if self.active_dashboard_mut().is_none() {
            return match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll_lines(1);
                    true
                },
                KeyCode::Up | KeyCode::Char('k') => {
                    self.scroll_lines(-1);
                    true
                },
                KeyCode::PageDown => {
                    self.scroll_lines(12);
                    true
                },
                KeyCode::PageUp => {
                    self.scroll_lines(-12);
                    true
                },
                _ => false,
            };
        }
        let query_focused = self.active_dashboard_mut().is_some_and(|d| d.query_focused);
        let login_editing = self
            .active_dashboard_mut()
            .is_some_and(|dashboard| dashboard.login_editing);
        if login_editing {
            match key.code {
                KeyCode::Esc => {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_editing = false;
                        dashboard.login_token.clear();
                    }
                },
                KeyCode::Enter => self.submit_github_login(),
                KeyCode::Backspace => {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_token.pop();
                    }
                },
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_token.push(character);
                    }
                },
                _ => {},
            }
            return true;
        }
        if query_focused {
            match key.code {
                KeyCode::Esc => self.active_dashboard_mut().map(|d| d.query_focused = false),
                KeyCode::Enter => {
                    if let Some(d) = self.active_dashboard_mut() {
                        d.query_focused = false;
                        d.reset_navigation();
                    }
                    self.request_github_section();
                    None
                },
                KeyCode::Backspace => self.active_dashboard_mut().map(|d| {
                    d.query.pop();
                }),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.active_dashboard_mut().map(|d| d.query.push(c))
                },
                _ => None,
            };
            return true;
        }
        match key.code {
            KeyCode::Char('1') => self.set_github_section(GithubSection::Issues),
            KeyCode::Char('2') => self.set_github_section(GithubSection::PullRequests),
            KeyCode::Char('3') => self.set_github_section(GithubSection::Actions),
            KeyCode::Char('/') => {
                if let Some(d) = self.active_dashboard_mut() {
                    d.query_focused = true;
                }
            },
            KeyCode::Char('r') => self.request_github_section(),
            KeyCode::Char('l') => {
                if let Some(dashboard) = self.active_dashboard_mut()
                    && !dashboard.auth.can_write
                    && dashboard.login_pending.is_none()
                {
                    dashboard.login_editing = true;
                    dashboard.login_token.clear();
                    dashboard.error = None;
                }
            },
            KeyCode::Char('n') => self.open_github_creation_form(),
            KeyCode::Down | KeyCode::Char('j') => {
                self.github_move_cursor(1, key.modifiers.contains(KeyModifiers::SHIFT));
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.github_move_cursor(-1, key.modifiers.contains(KeyModifiers::SHIFT));
            },
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(d) = self.active_dashboard_mut() {
                    d.selected = (0..d.row_count()).collect();
                }
            },
            KeyCode::Char(' ') => {
                if let Some(d) = self.active_dashboard_mut()
                    && !d.selected.remove(&d.cursor)
                {
                    d.selected.insert(d.cursor);
                }
            },
            KeyCode::Enter => self.open_github_selection(),
            _ => return false,
        }
        true
    }

    fn refresh_active_github(&mut self) -> bool {
        enum Refresh {
            Dashboard,
            Issue(u64),
            PullRequest(u64),
            Actions,
            IssueMetadata,
        }
        let refresh = match self.tabs.get(self.active).map(|tab| &tab.kind) {
            Some(TabKind::Github(GithubViewState::Dashboard(_))) => Refresh::Dashboard,
            Some(TabKind::Github(GithubViewState::Issue { number, .. })) => Refresh::Issue(*number),
            Some(TabKind::Github(GithubViewState::PullRequest(view))) => {
                Refresh::PullRequest(view.pull_request.number)
            },
            Some(TabKind::Github(GithubViewState::WorkflowRun { .. })) => Refresh::Actions,
            Some(TabKind::Github(GithubViewState::NewIssue { .. })) => Refresh::IssueMetadata,
            _ => return false,
        };
        if matches!(refresh, Refresh::Dashboard) {
            self.request_github_section();
            return true;
        }
        let command = match refresh {
            Refresh::Dashboard => return true,
            Refresh::Issue(number) => SessionCommand::GithubIssue { number },
            Refresh::PullRequest(number) => SessionCommand::GithubPullRequest { number },
            Refresh::Actions => SessionCommand::GithubActions { page: 1 },
            Refresh::IssueMetadata => SessionCommand::GithubIssueMetadata,
        };
        let request = self.send_command_id(command);
        let now = Instant::now();
        if let Some(TabKind::Github(view)) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            match view {
                GithubViewState::Issue {
                    pending,
                    loading_since,
                    error,
                    ..
                } => {
                    *pending = request;
                    *loading_since = now;
                    *error = None;
                },
                GithubViewState::PullRequest(view) => {
                    view.pending = request;
                    view.loading_since = now;
                    view.error = None;
                },
                GithubViewState::NewIssue { form, .. } => {
                    form.metadata_pending = request;
                    form.error = None;
                },
                _ => {},
            }
        }
        true
    }

    fn submit_github_login(&mut self) {
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
        let request = self.send_command_id(SessionCommand::GithubLogin {
            token: karet_session::GithubToken::new(token),
        });
        if let Some(dashboard) = self.active_dashboard_mut() {
            dashboard.login_editing = false;
            dashboard.login_pending = request;
        }
    }

    fn github_move_cursor(&mut self, delta: i32, extend: bool) {
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

    fn open_github_selection(&mut self) {
        enum Selection {
            Issue(GithubRepository, u64),
            PullRequest(GithubRepository, GithubPullRequest, bool),
            WorkflowRun(GithubRepository, Option<GithubWorkflow>, GithubWorkflowRun),
        }
        let selection = self.active_dashboard_mut().and_then(|dashboard| {
            let repository = dashboard.repository.clone();
            match dashboard.section {
                GithubSection::Issues => dashboard
                    .issues
                    .items
                    .get(dashboard.cursor)
                    .map(|issue| Selection::Issue(repository, issue.number)),
                GithubSection::PullRequests => dashboard
                    .pull_requests
                    .items
                    .get(dashboard.cursor)
                    .cloned()
                    .map(|pull_request| {
                        Selection::PullRequest(repository, pull_request, dashboard.auth.can_write)
                    }),
                GithubSection::Actions => {
                    dashboard
                        .runs
                        .items
                        .get(dashboard.cursor)
                        .cloned()
                        .map(|run| {
                            let workflow = dashboard
                                .workflows
                                .items
                                .iter()
                                .find(|workflow| workflow.id == run.workflow_id)
                                .cloned();
                            Selection::WorkflowRun(repository, workflow, run)
                        })
                },
            }
        });
        let Some(selection) = selection else {
            return;
        };
        match selection {
            Selection::Issue(repository, number) => {
                let request = self.send_command_id(SessionCommand::GithubIssue { number });
                self.push_tab(Tab::github_issue(repository, number, request));
            },
            Selection::PullRequest(repository, pull_request, can_write) => {
                let request = self.send_command_id(SessionCommand::GithubPullRequest {
                    number: pull_request.number,
                });
                self.push_tab(Tab::github_pull_request(
                    repository,
                    pull_request,
                    can_write,
                    request,
                ));
            },
            Selection::WorkflowRun(repository, workflow, run) => {
                self.push_tab(Tab::github_workflow_run(repository, workflow, run));
            },
        }
    }

    fn open_github_creation_form(&mut self) {
        let Some((repository, section, can_write)) = self
            .active_dashboard_mut()
            .map(|d| (d.repository.clone(), d.section, d.auth.can_write))
        else {
            return;
        };
        if !can_write {
            self.status = Some("GitHub sign-in is required to create items".to_string());
            return;
        }
        match section {
            GithubSection::Issues => {
                let pending = self.send_command_id(SessionCommand::GithubIssueMetadata);
                self.push_tab(Tab::github_new_issue(repository, pending));
            },
            GithubSection::PullRequests => self.push_tab(Tab::github_new_pull_request(repository)),
            GithubSection::Actions => {
                self.status = Some("workflow dispatch is not available in this build".to_string());
            },
        }
    }

    fn github_form_key(&mut self, key: KeyEvent) -> bool {
        let special = key.code == KeyCode::Char('p') || key.code == KeyCode::Enter;
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && !special
        {
            return false;
        }
        let Some(TabKind::Github(view)) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        else {
            return false;
        };
        match view {
            GithubViewState::NewIssue { form, .. } => edit_issue_form(form, key),
            GithubViewState::NewPullRequest { form, .. } => edit_pull_request_form(form, key),
            _ => return false,
        }
        if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.submit_github_form();
        }
        true
    }

    fn submit_github_form(&mut self) {
        enum Submission {
            Issue(GithubNewIssue),
            PullRequest(GithubNewPullRequest),
        }
        let submission = match self.tabs.get(self.active).map(|tab| &tab.kind) {
            Some(TabKind::Github(GithubViewState::NewIssue { form, .. })) => {
                if form.title.trim().is_empty() {
                    self.status = Some("issue title is required".to_string());
                    return;
                }
                Submission::Issue(GithubNewIssue {
                    title: form.title.trim().to_string(),
                    body: form.body.clone(),
                    assignees: comma_list(&form.assignees),
                    labels: comma_list(&form.labels),
                    milestone: form.milestone.trim().parse().ok(),
                    issue_type: nonempty(&form.issue_type),
                })
            },
            Some(TabKind::Github(GithubViewState::NewPullRequest { form, .. })) => {
                if form.title.trim().is_empty()
                    || form.head.trim().is_empty()
                    || form.base.trim().is_empty()
                {
                    self.status =
                        Some("pull request title, head, and base are required".to_string());
                    return;
                }
                Submission::PullRequest(GithubNewPullRequest {
                    title: form.title.trim().to_string(),
                    head: form.head.trim().to_string(),
                    base: form.base.trim().to_string(),
                    body: form.body.clone(),
                    draft: form.draft,
                    maintainer_can_modify: form.maintainer_can_modify,
                })
            },
            _ => return,
        };
        let command = match submission {
            Submission::Issue(issue) => SessionCommand::GithubCreateIssue { issue },
            Submission::PullRequest(pull_request) => {
                SessionCommand::GithubCreatePullRequest { pull_request }
            },
        };
        let request = self.send_command_id(command);
        if let Some(TabKind::Github(view)) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            match view {
                GithubViewState::NewIssue { form, .. } => form.submitting = request,
                GithubViewState::NewPullRequest { form, .. } => form.submitting = request,
                _ => {},
            }
        }
    }

    /// Handle a click or wheel gesture within the active GitHub dashboard table.
    pub(super) fn github_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self
            .tabs
            .get(self.active)
            .is_some_and(|tab| matches!(tab.kind, TabKind::Github(GithubViewState::PullRequest(_))))
        {
            return self.github_pull_request_mouse(mouse);
        }
        let point = (mouse.column, mouse.row);
        let Some((section_hit, query_hit, auth_hit, table_rect, first_visible, row_count)) =
            self.active_dashboard_mut().map(|dashboard| {
                (
                    dashboard.section_hits.iter().find_map(|(section, rect)| {
                        rect_contains(*rect, point).then_some(*section)
                    }),
                    rect_contains(dashboard.query_rect, point),
                    rect_contains(dashboard.auth_rect, point),
                    dashboard.table_rect,
                    dashboard.first_visible,
                    dashboard.row_count(),
                )
            })
        else {
            return false;
        };

        if let Some(section) = section_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.set_github_section(section);
            }
            return true;
        }
        if query_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(dashboard) = self.active_dashboard_mut()
                && dashboard.section != GithubSection::Actions
            {
                dashboard.query_focused = true;
            }
            return true;
        }
        if auth_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(dashboard) = self.active_dashboard_mut()
                && !dashboard.auth.can_write
                && dashboard.login_pending.is_none()
            {
                dashboard.login_editing = true;
                dashboard.login_token.clear();
                dashboard.error = None;
            }
            return true;
        }
        if !rect_contains(table_rect, point) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.github_move_cursor(3, false),
            MouseEventKind::ScrollUp => self.github_move_cursor(-3, false),
            MouseEventKind::Down(MouseButton::Left) => {
                let row = first_visible
                    + usize::from(mouse.row.saturating_sub(table_rect.y)) / DASHBOARD_ROW_HEIGHT;
                if row < row_count {
                    let modified = mouse
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                    let open = !modified && self.click_streak(mouse.column, mouse.row) >= 2;
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            let (start, end) = if dashboard.cursor <= row {
                                (dashboard.cursor, row)
                            } else {
                                (row, dashboard.cursor)
                            };
                            dashboard.selected.extend(start..=end);
                        } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            if !dashboard.selected.remove(&row) {
                                dashboard.selected.insert(row);
                            }
                        } else {
                            dashboard.selected.clear();
                            dashboard.selected.insert(row);
                        }
                        dashboard.cursor = row;
                    }
                    if open {
                        self.open_github_selection();
                    }
                }
            },
            _ => {},
        }
        true
    }
}
