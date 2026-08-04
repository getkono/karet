//! GitHub dashboard, detail, and form rendering.

mod detail;
mod forms;

use detail::*;
use forms::*;

use super::*;
use crate::app::github::DASHBOARD_ROW_HEIGHT;
use crate::app::github::GithubDashboard;
use crate::app::github::GithubFormField;
use crate::app::github::GithubIssueForm;
use crate::app::github::GithubPullRequestForm;
use crate::app::github::GithubPullRequestSection;
use crate::app::github::GithubPullRequestView;
use crate::app::github::GithubSection;
use crate::app::github::GithubViewState;
use crate::app::github::auth_label;

pub(super) fn draw_github(f: &mut Frame, theme: &Theme, area: Rect, view: &mut GithubViewState) {
    match view {
        GithubViewState::Dashboard(dashboard) => draw_dashboard(f, theme, area, dashboard),
        GithubViewState::Issue {
            repository: _,
            number,
            issue,
            comments,
            pending,
            loading_since,
            error,
            scroll,
        } => {
            if let Some(issue) = issue {
                draw_detail_page(
                    f,
                    theme,
                    area,
                    DetailCard {
                        kind: "Issue",
                        number: issue.number,
                        title: &issue.title,
                        creator: issue.creator.as_deref(),
                        created: issue.created_unix,
                        updated: issue.updated_unix,
                        state: &issue.state,
                        blocked: issue.blocked,
                        labels: &issue.labels,
                        body: issue.body.as_deref(),
                    },
                    comments,
                    error.as_deref(),
                    *scroll,
                );
            } else {
                draw_pending_detail(
                    f,
                    theme,
                    area,
                    &format!("Issue #{number}"),
                    error.as_deref(),
                    pending.is_some()
                        && loading_since.elapsed() >= crate::app::LOADING_REVEAL_DELAY,
                );
            }
        },
        GithubViewState::PullRequest(view) => draw_pull_request_page(f, theme, area, view),
        GithubViewState::WorkflowRun {
            repository,
            workflow,
            run,
            scroll,
        } => {
            let workflow_name = workflow
                .as_ref()
                .map_or("Workflow", |item| item.name.as_str());
            let result = run
                .conclusion
                .as_deref()
                .or(run.status.as_deref())
                .unwrap_or("queued");
            let mut lines = vec![
                heading(
                    format!(
                        "{}/{}  Actions · {workflow_name} #{}",
                        repository.owner, repository.repo, run.run_number
                    ),
                    theme,
                ),
                Line::default(),
                heading(run.title.clone(), theme),
                Line::from(vec![
                    Span::styled(
                        result.to_string(),
                        workflow_result_style(result, theme).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            " · {} · {} · triggered by @{} · {}",
                            run.branch.as_deref().unwrap_or("detached"),
                            run.event,
                            run.actor.as_deref().unwrap_or("unknown"),
                            relative_time(run.created_unix),
                        ),
                        muted_style(theme),
                    ),
                ]),
                Line::default(),
                Line::from(vec![
                    Span::styled("Commit  ", muted_style(theme)),
                    Span::styled(
                        run.head_sha.clone(),
                        Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui()),
                    ),
                ]),
            ];
            if let Some(workflow) = workflow {
                lines.push(Line::from(vec![
                    Span::styled("Workflow  ", muted_style(theme)),
                    Span::raw(workflow.path.clone()),
                ]));
            }
            f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
        },
        GithubViewState::NewIssue { repository, form } => {
            draw_issue_form(f, theme, area, repository, form);
        },
        GithubViewState::NewPullRequest { repository, form } => {
            draw_pull_request_form(f, theme, area, repository, form);
        },
    }
}

fn draw_dashboard(f: &mut Frame, theme: &Theme, area: Rect, state: &mut GithubDashboard) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .split(area);

    let tabs = [
        (GithubSection::Issues, "Issues"),
        (GithubSection::PullRequests, "Pull requests"),
        (GithubSection::Actions, "Actions"),
    ];
    let mut tab_line = Vec::new();
    state.section_hits.clear();
    let mut tab_x = rows[0].x;
    for (section, label) in tabs {
        let style = if state.section == section {
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())
        };
        let text = format!("  {label}  ");
        let width =
            u16::try_from(unicode_width::UnicodeWidthStr::width(text.as_str())).unwrap_or(u16::MAX);
        state
            .section_hits
            .push((section, Rect::new(tab_x, rows[0].y, width, 1)));
        tab_x = tab_x.saturating_add(width);
        tab_line.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(tab_line)), rows[0]);

    let search = if state.section == GithubSection::Actions {
        "Workflows and recent runs".to_string()
    } else {
        format!(
            "{} Search: {}{}",
            if state.query_focused { "▶" } else { "/" },
            state.query,
            if state.query_focused { "▌" } else { "" }
        )
    };
    f.render_widget(
        Paragraph::new(search).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())),
        ),
        rows[1],
    );
    state.query_rect = rows[1];

    let table = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(rows[2]);
    let header = match state.section {
        GithubSection::Issues => "Issues · title / description / author and activity",
        GithubSection::PullRequests => "Pull requests · title / description / author and activity",
        GithubSection::Actions => "Actions · workflow / run / branch and result",
    };
    f.render_widget(Paragraph::new(muted_line(header, theme)), table[0]);
    state.table_rect = table[1];
    draw_dashboard_rows(f, theme, table[1], state);
    let total = match state.section {
        GithubSection::Issues => state.issues.total_count,
        GithubSection::PullRequests => state.pull_requests.total_count,
        GithubSection::Actions => state.runs.total_count,
    };
    let account = if state.login_editing {
        format!(
            "Account · Token: {}▌  Enter sign in · Esc cancel",
            "•".repeat(state.login_token.chars().count())
        )
    } else if state.login_pending.is_some() {
        "Account · Signing in to GitHub…".to_string()
    } else {
        let action = if state.auth.can_write {
            ""
        } else {
            " · l sign in"
        };
        format!(
            "{}/{} · {}{action}",
            state.repository.owner,
            state.repository.repo,
            auth_label(&state.auth)
        )
    };
    let account_style = if state.auth.can_write {
        Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui())
    } else {
        Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui())
    };
    f.render_widget(
        Paragraph::new(Line::styled(account, account_style)),
        rows[3],
    );
    state.auth_rect = Rect::new(rows[3].x, rows[3].y, rows[3].width, 1);

    let status = if let Some(error) = &state.error {
        error_line(error, theme)
    } else {
        muted_line(
            &format!(
                "{} rows{} · {} selected · ↑↓ move · Space select · Enter open · n new · Ctrl+R refresh · 1/2/3 switch",
                state.row_count(),
                total.map_or_else(String::new, |n| format!(" of {n}")),
                state.selected.len(),
            ),
            theme,
        )
    };
    let status_area = Rect::new(rows[3].x, rows[3].y.saturating_add(1), rows[3].width, 1);
    f.render_widget(Paragraph::new(status), status_area);
}

fn draw_dashboard_rows(f: &mut Frame, theme: &Theme, area: Rect, state: &mut GithubDashboard) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible = usize::from(area.height).div_ceil(DASHBOARD_ROW_HEIGHT);
    if state.cursor < state.first_visible {
        state.first_visible = state.cursor;
    } else if state.cursor >= state.first_visible.saturating_add(visible) {
        state.first_visible = state.cursor.saturating_add(1).saturating_sub(visible);
    }
    if state.row_count() == 0 {
        let line = if state
            .loading_since
            .is_some_and(|since| since.elapsed() >= crate::app::LOADING_REVEAL_DELAY)
        {
            "Loading GitHub data…"
        } else if state.loading_since.is_some() {
            ""
        } else {
            "No results"
        };
        f.render_widget(Paragraph::new(muted_line(line, theme)), area);
        return;
    }
    for index in state.first_visible..state.row_count().min(state.first_visible + visible) {
        let slot = index.saturating_sub(state.first_visible) * DASHBOARD_ROW_HEIGHT;
        let row_area = Rect::new(
            area.x,
            area.y
                .saturating_add(u16::try_from(slot).unwrap_or(u16::MAX)),
            area.width,
            area.height
                .saturating_sub(u16::try_from(slot).unwrap_or(u16::MAX))
                .min(u16::try_from(DASHBOARD_ROW_HEIGHT).unwrap_or(3)),
        );
        let lines = dashboard_row_lines(state, index, area.width, theme);
        let style = if index == state.cursor {
            Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui())
        } else {
            Style::default()
        };
        f.render_widget(Paragraph::new(lines).style(style), row_area);
    }
}

fn dashboard_row_lines(
    state: &GithubDashboard,
    index: usize,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let checked = state.selected.contains(&index);
    match state.section {
        GithubSection::Issues => {
            let issue = &state.issues.items[index];
            issue_or_pull_request_lines(
                RowCard {
                    checked,
                    number: issue.number,
                    title: &issue.title,
                    body: issue.body.as_deref(),
                    state: &issue.state,
                    creator: issue.creator.as_deref(),
                    creator_id: issue.creator_id,
                    created: issue.created_unix,
                    updated: issue.updated_unix,
                    labels: &issue.labels,
                    badge: issue.blocked.then_some("BLOCKED"),
                },
                state.auth.viewer_id,
                width,
                theme,
            )
        },
        GithubSection::PullRequests => {
            let pull = &state.pull_requests.items[index];
            issue_or_pull_request_lines(
                RowCard {
                    checked,
                    number: pull.number,
                    title: &pull.title,
                    body: pull.body.as_deref(),
                    state: &pull.state,
                    creator: pull.creator.as_deref(),
                    creator_id: pull.creator_id,
                    created: pull.created_unix,
                    updated: pull.updated_unix,
                    labels: &pull.labels,
                    badge: pull.draft.then_some("DRAFT"),
                },
                state.auth.viewer_id,
                width,
                theme,
            )
        },
        GithubSection::Actions => action_lines(state, index, checked, width, theme),
    }
}

struct RowCard<'a> {
    checked: bool,
    number: u64,
    title: &'a str,
    body: Option<&'a str>,
    state: &'a str,
    creator: Option<&'a str>,
    creator_id: Option<u64>,
    created: i64,
    updated: i64,
    labels: &'a [karet_session::GithubLabel],
    badge: Option<&'a str>,
}

fn issue_or_pull_request_lines(
    card: RowCard<'_>,
    viewer_id: Option<u64>,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let checkbox = if card.checked { "[x]" } else { "[ ]" };
    let title_width = usize::from(width).saturating_sub(13);
    let description_width = usize::from(width).saturating_sub(6);
    let mut metadata = vec![
        Span::raw("    "),
        Span::styled(
            card.state.to_ascii_uppercase(),
            workflow_result_style(card.state, theme).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  @", muted_style(theme)),
        author_span(card.creator, card.creator_id, viewer_id, theme),
        Span::styled(
            format!(
                " · opened {} · updated {}",
                relative_time(card.created),
                relative_time(card.updated)
            ),
            muted_style(theme),
        ),
    ];
    if let Some(badge) = card.badge {
        metadata.push(Span::styled(
            format!("  {badge}"),
            Style::default()
                .fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ));
    }
    for label in card.labels {
        metadata.push(Span::styled("  ", muted_style(theme)));
        metadata.push(Span::styled(
            label.name.clone(),
            github_label_style(&label.color, theme),
        ));
    }
    vec![
        Line::from(vec![
            Span::styled(
                format!("{checkbox} "),
                Style::default()
                    .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("#{:<5} ", card.number),
                Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui()),
            ),
            Span::styled(
                fit_one_line(card.title, title_width),
                Style::default()
                    .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                fit_one_line(card.body.unwrap_or("No description"), description_width),
                muted_style(theme).add_modifier(Modifier::ITALIC),
            ),
        ]),
        Line::from(metadata),
    ]
}

fn action_lines(
    state: &GithubDashboard,
    index: usize,
    checked: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let run = &state.runs.items[index];
    let workflow = state
        .workflows
        .items
        .iter()
        .find(|workflow| workflow.id == run.workflow_id)
        .map_or("Workflow", |workflow| workflow.name.as_str());
    let result = run
        .conclusion
        .as_deref()
        .or(run.status.as_deref())
        .unwrap_or("queued");
    let checkbox = if checked { "[x]" } else { "[ ]" };
    vec![
        Line::from(vec![
            Span::styled(
                format!("{checkbox} "),
                Style::default()
                    .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                fit_one_line(workflow, usize::from(width).saturating_sub(16)),
                Style::default()
                    .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  Run #{}", run.run_number),
                Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui()),
            ),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                fit_one_line(&run.title, usize::from(width).saturating_sub(6)),
                muted_style(theme).add_modifier(Modifier::ITALIC),
            ),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                result.to_string(),
                workflow_result_style(result, theme).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " · {} · {} · @{} · {}",
                    run.branch.as_deref().unwrap_or("detached"),
                    run.event,
                    run.actor.as_deref().unwrap_or("unknown"),
                    relative_time(run.created_unix)
                ),
                muted_style(theme),
            ),
        ]),
    ]
}

fn heading(text: impl Into<String>, theme: &Theme) -> Line<'static> {
    Line::styled(
        text.into(),
        Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD),
    )
}

fn muted_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::styled(
        text.to_string(),
        Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui()),
    )
}

fn muted_style(theme: &Theme) -> Style {
    Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())
}

fn author_span(
    creator: Option<&str>,
    creator_id: Option<u64>,
    viewer_id: Option<u64>,
    theme: &Theme,
) -> Span<'static> {
    let is_viewer = creator_id
        .zip(viewer_id)
        .is_some_and(|(creator, viewer)| creator == viewer);
    let color = if is_viewer {
        Color::Yellow
    } else {
        theme.role(ThemeRole::DiagnosticInfo).to_ratatui()
    };
    Span::styled(
        creator.unwrap_or("ghost").to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn workflow_result_style(result: &str, theme: &Theme) -> Style {
    let role = match result.to_ascii_lowercase().as_str() {
        "success" | "open" | "completed" => ThemeRole::DiagnosticHint,
        "failure" | "cancelled" | "timed_out" | "closed" => ThemeRole::DiagnosticError,
        "queued" | "pending" | "in_progress" | "waiting" => ThemeRole::DiagnosticWarning,
        _ => ThemeRole::DiagnosticInfo,
    };
    Style::default().fg(theme.role(role).to_ratatui())
}

fn github_label_style(value: &str, theme: &Theme) -> Style {
    let color = parse_github_color(value)
        .unwrap_or_else(|| theme.role(ThemeRole::DiagnosticInfo).to_ratatui());
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn github_label_badge_style(value: &str, theme: &Theme) -> Style {
    let background = parse_github_color(value)
        .unwrap_or_else(|| theme.role(ThemeRole::DiagnosticInfo).to_ratatui());
    let foreground = match background {
        Color::Rgb(red, green, blue)
            if u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 > 128_000 =>
        {
            Color::Black
        },
        _ => Color::White,
    };
    Style::default()
        .fg(foreground)
        .bg(background)
        .add_modifier(Modifier::BOLD)
}

fn parse_github_color(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }
    let red = u8::from_str_radix(&value[0..2], 16).ok()?;
    let green = u8::from_str_radix(&value[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some(Color::Rgb(red, green, blue))
}

fn error_line(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "Error · ",
            Style::default()
                .fg(theme.role(ThemeRole::DiagnosticError).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            text.to_string(),
            Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui()),
        ),
    ])
}

fn fit_one_line(text: &str, max: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if unicode_width::UnicodeWidthStr::width(normalized.as_str()) <= max {
        return normalized;
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let mut shortened = String::new();
    let mut used = 1;
    for character in normalized.chars() {
        let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > max {
            break;
        }
        shortened.push(character);
        used += width;
    }
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dashboard_with_issue(selected: bool) -> GithubDashboard {
        let view = GithubViewState::dashboard(
            karet_session::GithubRepository {
                owner: "getkono".to_string(),
                repo: "karet".to_string(),
            },
            karet_session::GithubAuth {
                source: karet_session::GithubAuthSource::Explicit,
                can_write: true,
                viewer_id: Some(42),
                viewer_login: Some("octocat".to_string()),
            },
        );
        let GithubViewState::Dashboard(mut dashboard) = view else {
            unreachable!();
        };
        dashboard.issues.items.push(karet_session::GithubIssue {
            number: 12,
            title: "Stable title".to_string(),
            body: Some("A useful description".to_string()),
            state: "open".to_string(),
            creator: Some("octocat".to_string()),
            creator_id: Some(42),
            created_unix: 1,
            updated_unix: 2,
            labels: Vec::new(),
            blocked: false,
            html_url: String::new(),
        });
        if selected {
            dashboard.selected.insert(0);
        }
        dashboard
    }

    #[test]
    fn github_colors_require_exact_six_digit_rgb() {
        assert_eq!(
            parse_github_color("#12aBef"),
            Some(Color::Rgb(0x12, 0xab, 0xef))
        );
        assert_eq!(parse_github_color("12ab"), None);
        assert_eq!(parse_github_color("yellow"), None);
    }

    #[test]
    fn fitted_row_text_respects_terminal_cell_width() {
        let fitted = fit_one_line("alpha 界界 omega", 10);
        assert!(unicode_width::UnicodeWidthStr::width(fitted.as_str()) <= 10);
        assert!(fitted.ends_with('…'));
    }

    #[test]
    fn checkbox_state_does_not_move_the_row_title() {
        let theme = Theme::dark();
        let unchecked = dashboard_row_lines(&dashboard_with_issue(false), 0, 80, &theme);
        let checked = dashboard_row_lines(&dashboard_with_issue(true), 0, 80, &theme);
        let text = |line: &Line<'_>| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };
        let unchecked = text(&unchecked[0]);
        let checked = text(&checked[0]);
        assert!(unchecked.starts_with("[ ]"));
        assert!(checked.starts_with("[x]"));
        assert_eq!(unchecked.find("Stable title"), checked.find("Stable title"));
    }

    #[test]
    fn viewer_highlight_matches_only_stable_account_id() {
        let theme = Theme::dark();
        let viewer = author_span(Some("octocat"), Some(42), Some(42), &theme);
        let same_login_wrong_id = author_span(Some("octocat"), Some(7), Some(42), &theme);
        assert_eq!(viewer.style.fg, Some(Color::Yellow));
        assert_ne!(same_login_wrong_id.style.fg, Some(Color::Yellow));
    }

    #[test]
    fn detail_markdown_is_rendered_instead_of_shown_as_source() {
        let theme = Theme::dark();
        let mut lines = Vec::new();
        append_rendered_markdown(&mut lines, "**important**", 40, &theme);
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, "important");
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| { span.style.add_modifier.contains(Modifier::BOLD) })
        );
    }

    #[test]
    fn github_labels_render_as_filled_badges() {
        let theme = Theme::dark();
        let style = github_label_badge_style("d73a4a", &theme);
        assert_eq!(style.bg, Some(Color::Rgb(0xd7, 0x3a, 0x4a)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn errors_keep_details_readable_instead_of_painting_everything_red() {
        let theme = Theme::dark();
        let line = error_line("HTTP 403: resource not accessible", &theme);
        assert_eq!(line.spans.len(), 2);
        assert_eq!(
            line.spans[0].style.fg,
            Some(theme.role(ThemeRole::DiagnosticError).to_ratatui())
        );
        assert_eq!(
            line.spans[1].style.fg,
            Some(theme.role(ThemeRole::Foreground).to_ratatui())
        );
    }
}
