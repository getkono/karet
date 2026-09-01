//! GitHub creation-form state, keyboard editing, and submission.

use super::*;

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

pub(crate) fn auth_label(auth: &karet_session::GithubAuth) -> String {
    if let Some(login) = auth.viewer_login.as_deref() {
        return format!("Signed in as @{login}");
    }
    match auth.source {
        karet_session::GithubAuthSource::Anonymous => "Not signed in".to_string(),
        karet_session::GithubAuthSource::GithubToken => "Signed in with GITHUB_TOKEN".to_string(),
        karet_session::GithubAuthSource::GhToken => "Signed in with GH_TOKEN".to_string(),
        karet_session::GithubAuthSource::GithubCli => "Signed in with GitHub CLI".to_string(),
        karet_session::GithubAuthSource::Explicit => "Signed in for this session".to_string(),
    }
}

pub(super) fn edit_issue_form(form: &mut GithubIssueForm, key: KeyEvent) {
    if key.code == KeyCode::Tab {
        form.field = match form.field {
            GithubFormField::Title => GithubFormField::Body,
            GithubFormField::Body => GithubFormField::Assignees,
            GithubFormField::Assignees => GithubFormField::Labels,
            GithubFormField::Labels => GithubFormField::Milestone,
            GithubFormField::Milestone => GithubFormField::IssueType,
            _ => GithubFormField::Title,
        };
        return;
    }
    if form.field == GithubFormField::Assignees {
        let suggestion_count = form.assignee_suggestions().len();
        match key.code {
            KeyCode::Down if suggestion_count > 0 => {
                form.assignee_cursor = (form.assignee_cursor + 1).min(suggestion_count - 1);
                return;
            },
            KeyCode::Up if suggestion_count > 0 => {
                form.assignee_cursor = form.assignee_cursor.saturating_sub(1);
                return;
            },
            KeyCode::Enter if suggestion_count > 0 => {
                accept_assignee(form);
                return;
            },
            _ => {},
        }
    }
    if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
        form.preview = !form.preview;
        return;
    }
    edit_text(issue_field_mut(form), key);
    form.assignee_cursor = 0;
}

fn accept_assignee(form: &mut GithubIssueForm) {
    let selected = form
        .assignee_suggestions()
        .get(form.assignee_cursor)
        .map(|login| (*login).to_string());
    let Some(selected) = selected else {
        return;
    };
    if let Some((prefix, _)) = form.assignees.rsplit_once(',') {
        form.assignees = format!("{}, {selected}, ", prefix.trim_end());
    } else {
        form.assignees = format!("{selected}, ");
    }
    form.assignee_cursor = 0;
}

fn issue_field_mut(form: &mut GithubIssueForm) -> &mut String {
    match form.field {
        GithubFormField::Title => &mut form.title,
        GithubFormField::Body => &mut form.body,
        GithubFormField::Assignees => &mut form.assignees,
        GithubFormField::Labels => &mut form.labels,
        GithubFormField::Milestone => &mut form.milestone,
        GithubFormField::IssueType => &mut form.issue_type,
        _ => &mut form.title,
    }
}

pub(super) fn edit_pull_request_form(form: &mut GithubPullRequestForm, key: KeyEvent) {
    if key.code == KeyCode::Tab {
        form.field = match form.field {
            GithubFormField::Title => GithubFormField::Head,
            GithubFormField::Head => GithubFormField::Base,
            GithubFormField::Base => GithubFormField::Body,
            _ => GithubFormField::Title,
        };
        return;
    }
    if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
        form.preview = !form.preview;
        return;
    }
    edit_text(
        match form.field {
            GithubFormField::Head => &mut form.head,
            GithubFormField::Base => &mut form.base,
            GithubFormField::Body => &mut form.body,
            _ => &mut form.title,
        },
        key,
    );
}

fn edit_text(target: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            target.pop();
        },
        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::CONTROL) => target.push('\n'),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            target.push(c);
        },
        _ => {},
    }
}

pub(super) fn comma_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl App {
    pub(super) fn github_form_key(&mut self, key: KeyEvent) -> bool {
        let special = key.code == KeyCode::Char('p') || key.code == KeyCode::Enter;
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && !special
        {
            return false;
        }
        let Some(view) = self.github.active_page_mut() else {
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
        let submission = match self.github.active_page() {
            Some(GithubViewState::NewIssue { form, .. }) => {
                if form.title.trim().is_empty() {
                    self.notify(
                        Report::Refusal,
                        NotificationKind::System,
                        "issue title is required",
                    );
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
            Some(GithubViewState::NewPullRequest { form, .. }) => {
                if form.title.trim().is_empty()
                    || form.head.trim().is_empty()
                    || form.base.trim().is_empty()
                {
                    self.notify(
                        Report::Refusal,
                        NotificationKind::System,
                        "pull request title, head, and base are required",
                    );
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
        let request = self.send(command);
        if let Some(view) = self.github.active_page_mut() {
            match view {
                GithubViewState::NewIssue { form, .. } => form.submitting = request,
                GithubViewState::NewPullRequest { form, .. } => form.submitting = request,
                _ => {},
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_selector_filters_and_accepts_repository_options() {
        let mut form = GithubIssueForm {
            field: GithubFormField::Assignees,
            assignees: "ali".to_string(),
            assignee_options: vec!["bob".to_string(), "alice".to_string()],
            ..GithubIssueForm::default()
        };

        assert_eq!(form.assignee_suggestions(), ["alice"]);
        edit_issue_form(&mut form, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(form.assignees, "alice, ");
        assert!(
            form.assignee_suggestions()
                .iter()
                .all(|login| *login != "alice")
        );
    }
}
