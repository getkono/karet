//! GitHub issue and pull-request form rendering.

use super::*;

pub(super) fn draw_issue_form(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    repository: &karet_session::GithubRepository,
    form: &GithubIssueForm,
) {
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(vec![
            heading(
                format!(
                    "Create a new issue in {}/{}",
                    repository.owner, repository.repo
                ),
                theme,
            ),
            muted_line("Required fields are marked *", theme),
        ]),
        rows[0],
    );

    let columns = if rows[1].width >= 72 {
        Layout::horizontal([Constraint::Min(40), Constraint::Length(30)]).split(rows[1])
    } else {
        Layout::horizontal([Constraint::Percentage(100), Constraint::Length(0)]).split(rows[1])
    };
    let editor = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(columns[0]);
    draw_form_field(
        f,
        theme,
        editor[0],
        "Title *",
        &form.title,
        "Add a title",
        form.field == GithubFormField::Title,
    );
    let description_title = if form.preview {
        " Description   Write  [Preview] "
    } else {
        " Description   [Write]  Preview "
    };
    let description_border = if form.field == GithubFormField::Body {
        ThemeRole::DiagnosticInfo
    } else {
        ThemeRole::IndentGuide
    };
    let description = if form.preview {
        let wrapped =
            karet_markdown::parse(&form.body).wrap(editor[1].width.saturating_sub(2).max(1));
        karet_markdown::view::to_ratatui(&wrapped, theme)
    } else if form.body.is_empty() {
        vec![muted_line("Add your description in Markdown…", theme)]
    } else {
        form.body
            .lines()
            .map(|line| Line::raw(line.to_string()))
            .collect()
    };
    f.render_widget(
        Paragraph::new(description)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(description_title)
                    .border_style(Style::default().fg(theme.role(description_border).to_ratatui())),
            )
            .wrap(Wrap { trim: false }),
        editor[1],
    );

    if columns[1].width > 0 {
        let sidebar = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(columns[1]);
        draw_form_field(
            f,
            theme,
            sidebar[0],
            "Assignees",
            &form.assignees,
            "No one",
            form.field == GithubFormField::Assignees,
        );
        draw_form_field(
            f,
            theme,
            sidebar[1],
            "Labels",
            &form.labels,
            "None yet",
            form.field == GithubFormField::Labels,
        );
        draw_form_field(
            f,
            theme,
            sidebar[2],
            "Type",
            &form.issue_type,
            "No type",
            form.field == GithubFormField::IssueType,
        );
        draw_form_field(
            f,
            theme,
            sidebar[3],
            "Milestone",
            &form.milestone,
            "No milestone",
            form.field == GithubFormField::Milestone,
        );
        let suggestion_lines = if form.metadata_pending.is_some() {
            vec![muted_line("Loading assignees…", theme)]
        } else if form.field == GithubFormField::Assignees {
            form.assignee_suggestions()
                .into_iter()
                .take(5)
                .enumerate()
                .map(|(index, login)| {
                    let marker = if index == form.assignee_cursor {
                        "›"
                    } else {
                        " "
                    };
                    muted_line(&format!("{marker} @{login}"), theme)
                })
                .collect()
        } else {
            Vec::new()
        };
        f.render_widget(Paragraph::new(suggestion_lines), sidebar[4]);
    }

    let mut status = vec![muted_line(
        "Tab next field · Ctrl+P write/preview · Ctrl+Enter create issue",
        theme,
    )];
    if form.submitting.is_some() {
        status.push(muted_line("Creating issue…", theme));
    } else if let Some(error) = form.error.as_deref() {
        status.push(error_line(error, theme));
    }
    f.render_widget(Paragraph::new(status).wrap(Wrap { trim: false }), rows[2]);
}

fn draw_form_field(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    label: &'static str,
    value: &str,
    placeholder: &'static str,
    focused: bool,
) {
    let border_role = if focused {
        ThemeRole::DiagnosticInfo
    } else {
        ThemeRole::IndentGuide
    };
    let line = if value.is_empty() {
        muted_line(placeholder, theme)
    } else {
        Line::raw(value.to_string())
    };
    f.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {label} "))
                .border_style(Style::default().fg(theme.role(border_role).to_ratatui())),
        ),
        area,
    );
}

pub(super) fn draw_pull_request_form(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    repository: &karet_session::GithubRepository,
    form: &GithubPullRequestForm,
) {
    let mut lines = vec![
        heading(
            format!(
                "New pull request · {}/{}",
                repository.owner, repository.repo
            ),
            theme,
        ),
        muted_line(
            "Tab next field · Ctrl+P edit/preview · Ctrl+Enter submit",
            theme,
        ),
        Line::default(),
        form_line(
            "Title",
            &form.title,
            form.field == GithubFormField::Title,
            theme,
        ),
        form_line(
            "Head",
            &form.head,
            form.field == GithubFormField::Head,
            theme,
        ),
        form_line(
            "Base",
            &form.base,
            form.field == GithubFormField::Base,
            theme,
        ),
        Line::raw(format!(
            "Draft: {}  ·  Maintainer edits: {}",
            form.draft, form.maintainer_can_modify
        )),
        Line::default(),
        heading(
            if form.preview {
                "Description · Preview"
            } else {
                "Description · Write"
            },
            theme,
        ),
    ];
    append_markdown_editor(&mut lines, &form.body, form.preview, area.width, theme);
    append_form_status(&mut lines, form.submitting, form.error.as_deref(), theme);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn append_form_status(
    lines: &mut Vec<Line<'static>>,
    pending: Option<karet_session::RequestId>,
    error: Option<&str>,
    theme: &Theme,
) {
    lines.push(Line::default());
    if pending.is_some() {
        lines.push(muted_line("Submitting…", theme));
    }
    if let Some(error) = error {
        lines.push(error_line(error, theme));
    }
}

fn append_markdown_editor(
    lines: &mut Vec<Line<'static>>,
    source: &str,
    preview: bool,
    width: u16,
    theme: &Theme,
) {
    if preview {
        let wrapped = karet_markdown::parse(source).wrap(width.max(1));
        lines.extend(karet_markdown::view::to_ratatui(&wrapped, theme));
    } else {
        lines.extend(source.lines().map(|line| Line::raw(line.to_string())));
    }
}

fn form_line(label: &str, value: &str, focused: bool, theme: &Theme) -> Line<'static> {
    let style = if focused {
        Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui())
    } else {
        Style::default()
    };
    Line::styled(format!("{label:<12} {value}"), style)
}
