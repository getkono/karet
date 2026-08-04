//! Keyboard editing helpers for GitHub creation forms.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::GithubFormField;
use super::GithubIssueForm;
use super::GithubPullRequestForm;

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
