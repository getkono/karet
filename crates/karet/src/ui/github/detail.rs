//! GitHub issue and pull-request conversation rendering.

use super::*;

pub(super) struct DetailCard<'a> {
    pub(super) kind: &'a str,
    pub(super) number: u64,
    pub(super) title: &'a str,
    pub(super) creator: Option<&'a str>,
    pub(super) created: i64,
    pub(super) updated: i64,
    pub(super) state: &'a str,
    pub(super) blocked: bool,
    pub(super) labels: &'a [karet_session::GithubLabel],
    pub(super) body: Option<&'a str>,
}

pub(super) fn draw_detail_page(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    detail: DetailCard<'_>,
    comments: &karet_session::GithubPage<karet_session::GithubComment>,
    error: Option<&str>,
    scroll: u16,
) {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);
    let state_style = workflow_result_style(detail.state, theme)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED);
    let header = vec![
        Line::from(vec![
            Span::styled(
                detail.title.to_string(),
                Style::default()
                    .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  #{}", detail.number),
                Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui()),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!(" {} ", detail.state), state_style),
            Span::styled(
                format!(
                    "  @{} opened this {} {} · {} comment{}",
                    detail.creator.unwrap_or("ghost"),
                    detail.kind.to_ascii_lowercase(),
                    relative_time(detail.created),
                    comments.items.len(),
                    if comments.items.len() == 1 { "" } else { "s" },
                ),
                muted_style(theme),
            ),
        ]),
        Line::styled(
            "━".repeat(usize::from(area.width)),
            Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
        ),
    ];
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: false }), rows[0]);

    let columns = if rows[1].width >= 72 {
        Layout::horizontal([Constraint::Min(40), Constraint::Length(28)]).split(rows[1])
    } else {
        Layout::horizontal([Constraint::Percentage(100), Constraint::Length(0)]).split(rows[1])
    };
    let content_width = columns[0].width.saturating_sub(4).max(1);
    let mut conversation = vec![Line::from(vec![
        Span::styled(
            format!("@{}", detail.creator.unwrap_or("ghost")),
            Style::default()
                .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" wrote {}", relative_time(detail.created)),
            muted_style(theme),
        ),
    ])];
    conversation.push(Line::default());
    append_rendered_markdown(
        &mut conversation,
        detail.body.unwrap_or("_No description provided._"),
        content_width,
        theme,
    );
    for comment in &comments.items {
        conversation.push(Line::default());
        conversation.push(Line::styled(
            "─".repeat(usize::from(content_width)),
            Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
        ));
        conversation.push(Line::from(vec![
            Span::styled(
                format!("@{}", comment.creator.as_deref().unwrap_or("ghost")),
                Style::default()
                    .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" commented {}", relative_time(comment.created_unix)),
                muted_style(theme),
            ),
        ]));
        conversation.push(Line::default());
        append_rendered_markdown(&mut conversation, &comment.body, content_width, theme);
    }
    if let Some(error) = error {
        conversation.push(Line::default());
        conversation.push(error_line(error, theme));
    }
    f.render_widget(
        Paragraph::new(conversation)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Conversation ")
                    .border_style(
                        Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
                    ),
            )
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        columns[0],
    );

    if columns[1].width > 0 {
        let mut details = vec![muted_line(
            &format!("Updated {}", relative_time(detail.updated)),
            theme,
        )];
        if detail.blocked {
            details.push(Line::styled(
                " BLOCKED ",
                Style::default()
                    .fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui())
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
        }
        details.push(Line::default());
        details.push(heading("Labels", theme));
        if detail.labels.is_empty() {
            details.push(muted_line("None", theme));
        } else {
            details.extend(detail.labels.iter().map(|label| {
                Line::from(Span::styled(
                    format!(" {} ", label.name),
                    github_label_badge_style(&label.color, theme),
                ))
            }));
        }
        f.render_widget(
            Paragraph::new(details)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Details ")
                        .border_style(
                            Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
                        ),
                )
                .wrap(Wrap { trim: false }),
            columns[1],
        );
    }
}

pub(super) fn draw_pending_detail(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    error: Option<&str>,
    reveal_loading: bool,
) {
    let mut lines = vec![heading(title.to_string(), theme), Line::default()];
    if let Some(error) = error {
        lines.push(error_line(error, theme));
    } else if reveal_loading {
        lines.push(muted_line("Loading conversation…", theme));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

pub(super) fn append_rendered_markdown(
    lines: &mut Vec<Line<'static>>,
    source: &str,
    width: u16,
    theme: &Theme,
) {
    let wrapped = karet_markdown::parse(source).wrap(width.max(1));
    lines.extend(karet_markdown::view::to_ratatui(&wrapped, theme));
}

pub(super) fn draw_pull_request_page(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    if view.section != GithubPullRequestSection::Conversation {
        view.body_rect = Rect::default();
        view.comment_rect = Rect::default();
        view.merge_rect = Rect::default();
        view.draft_rect = Rect::default();
        view.check_hits.clear();
    }
    if view.section != GithubPullRequestSection::Commits {
        view.commits_rect = Rect::default();
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);
    draw_pull_request_header(f, theme, rows[0], view);
    draw_pull_request_tabs(f, theme, rows[1], view);
    match view.section {
        GithubPullRequestSection::Conversation => {
            draw_pull_request_conversation(f, theme, rows[2], view);
        },
        GithubPullRequestSection::Commits => draw_pull_request_commits(f, theme, rows[2], view),
        GithubPullRequestSection::FilesChanged => {
            view.commits_rect = Rect::default();
            f.render_widget(
                Paragraph::new(vec![
                    heading("Files changed", theme),
                    Line::default(),
                    muted_line("Opening the existing comparison/diff view…", theme),
                    muted_line(
                        "Press 3 again to retry if the local revisions were not available.",
                        theme,
                    ),
                ]),
                rows[2],
            );
        },
    }
}

fn draw_pull_request_header(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &GithubPullRequestView,
) {
    let pull = &view.pull_request;
    let state = if pull.merged {
        "Merged"
    } else if pull.draft {
        "Draft"
    } else if pull.state.eq_ignore_ascii_case("open") {
        "Open"
    } else {
        "Closed"
    };
    let state_style =
        workflow_result_style(if pull.merged { "success" } else { &pull.state }, theme)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
    let lines = vec![
        Line::from(vec![
            Span::styled(
                pull.title.clone(),
                Style::default()
                    .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  #{}", pull.number), muted_style(theme)),
        ]),
        Line::from(vec![
            Span::styled(format!(" {state} "), state_style),
            Span::styled(
                format!(
                    "  @{} wants to merge {} commit{} · Ctrl+R refresh",
                    pull.creator.as_deref().unwrap_or("ghost"),
                    view.commits.len(),
                    if view.commits.len() == 1 { "" } else { "s" },
                ),
                muted_style(theme),
            ),
        ]),
        Line::styled(
            "━".repeat(usize::from(area.width)),
            Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
        ),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_pull_request_tabs(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    // GitHub's separate Checks tab is intentionally omitted. Check results and their
    // web links live at the end of Conversation, so karet does not create a parallel
    // PR “Actions” view that would duplicate GitHub's richer web UI.
    let tabs = [
        (GithubPullRequestSection::Conversation, "Conversation"),
        (GithubPullRequestSection::Commits, "Commits"),
        (GithubPullRequestSection::FilesChanged, "Files changed"),
    ];
    view.section_hits.clear();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (index, (section, label)) in tabs.into_iter().enumerate() {
        let text = format!("  {} {label}  ", index + 1);
        let width =
            u16::try_from(unicode_width::UnicodeWidthStr::width(text.as_str())).unwrap_or(u16::MAX);
        view.section_hits
            .push((section, Rect::new(x, area.y, width, 1)));
        x = x.saturating_add(width);
        let style = if view.section == section {
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            muted_style(theme)
        };
        spans.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_pull_request_conversation(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    let footer_height = area.height.min(12);
    let sections = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(footer_height.min(7)),
        Constraint::Length(footer_height.saturating_sub(footer_height.min(7))),
    ])
    .split(area);
    let conversation_area = sections[0];
    let status_area = sections[1];
    let comment_area = sections[2];
    let content_width = conversation_area.width.saturating_sub(4).max(1);
    view.body_rect = if view.scroll == 0 {
        Rect::new(
            conversation_area.x.saturating_add(1),
            conversation_area.y.saturating_add(1),
            conversation_area.width.saturating_sub(2),
            conversation_area.height.saturating_sub(2).min(8),
        )
    } else {
        Rect::default()
    };
    let mut lines = Vec::new();
    append_pull_request_body(&mut lines, content_width, theme, view);

    enum Timeline<'a> {
        Comment(&'a karet_session::GithubComment),
        Activity(&'a karet_session::GithubPullRequestActivity),
    }
    let mut timeline: Vec<(i64, Timeline<'_>)> = view
        .comments
        .items
        .iter()
        .map(|comment| (comment.created_unix, Timeline::Comment(comment)))
        .chain(view.activity.iter().map(|activity| {
            (
                activity.created_unix.unwrap_or(i64::MAX),
                Timeline::Activity(activity),
            )
        }))
        .collect();
    timeline.sort_by_key(|(created, _)| *created);
    for (_, item) in timeline {
        lines.push(Line::default());
        lines.push(Line::styled(
            "─".repeat(usize::from(content_width)),
            Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
        ));
        match item {
            Timeline::Comment(comment) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("@{}", comment.creator.as_deref().unwrap_or("ghost")),
                        Style::default()
                            .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" commented {}", relative_time(comment.created_unix)),
                        muted_style(theme),
                    ),
                ]));
                lines.push(Line::default());
                append_rendered_markdown(&mut lines, &comment.body, content_width, theme);
            },
            Timeline::Activity(activity) => lines.push(activity_line(activity, theme)),
        }
    }
    if let Some(error) = &view.activity_error {
        lines.push(Line::default());
        lines.push(muted_line(
            &format!("Some activity is unavailable from GitHub: {error}"),
            theme,
        ));
    }
    if let Some(error) = &view.error {
        lines.push(Line::default());
        lines.push(error_line(error, theme));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Conversation ")
                    .border_style(
                        Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
                    ),
            )
            .scroll((view.scroll, 0))
            .wrap(Wrap { trim: false }),
        conversation_area,
    );
    draw_pull_request_status(f, theme, status_area, view);
    draw_pull_request_comment(f, theme, comment_area, view);

    if view.pending.is_some()
        && view.error.is_none()
        && view.loading_since.elapsed() >= crate::app::LOADING_REVEAL_DELAY
    {
        let loading = Rect::new(status_area.x, status_area.y, status_area.width.min(14), 1);
        f.render_widget(Paragraph::new(muted_line("Refreshing…", theme)), loading);
    }
}

fn append_pull_request_body(
    lines: &mut Vec<Line<'static>>,
    width: u16,
    theme: &Theme,
    view: &mut GithubPullRequestView,
) {
    let border = Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui());
    lines.push(Line::from(vec![
        Span::styled("╭─ ", border),
        Span::styled(
            format!(
                "@{}",
                view.pull_request.creator.as_deref().unwrap_or("ghost")
            ),
            Style::default()
                .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" wrote {}", relative_time(view.pull_request.created_unix)),
            muted_style(theme),
        ),
        Span::styled(
            if view.can_write {
                "  · click to edit"
            } else {
                ""
            },
            muted_style(theme),
        ),
    ]));
    let mut body_lines = Vec::new();
    if let Some(source) = view.body_edit.as_deref() {
        if view.preview {
            append_rendered_markdown(&mut body_lines, source, width.saturating_sub(2), theme);
        } else {
            body_lines.extend(source.lines().map(|line| Line::raw(line.to_string())));
            if source.is_empty() {
                body_lines.push(Line::default());
            }
        }
        body_lines.push(Line::default());
        body_lines.push(muted_line(
            "Ctrl+Enter save · Ctrl+P preview · Esc cancel",
            theme,
        ));
    } else {
        append_rendered_markdown(
            &mut body_lines,
            view.pull_request
                .body
                .as_deref()
                .unwrap_or("_No description provided._"),
            width.saturating_sub(2),
            theme,
        );
    }
    for line in body_lines {
        let mut spans = vec![Span::styled("│ ", border)];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    lines.push(Line::styled(
        format!("╰{}", "─".repeat(usize::from(width.saturating_sub(1)))),
        border,
    ));
}

fn activity_line(
    activity: &karet_session::GithubPullRequestActivity,
    theme: &Theme,
) -> Line<'static> {
    let actor = activity.actor.as_deref().unwrap_or("github");
    let short = |value: &str| value.chars().take(7).collect::<String>();
    let action = match activity.kind.as_str() {
        "committed" => format!(
            "committed {}",
            activity.commit_id.as_deref().map(short).unwrap_or_default()
        ),
        "head_ref_force_pushed" => format!(
            "force-pushed {} → {}",
            activity.before.as_deref().map(short).unwrap_or_default(),
            activity.after.as_deref().map(short).unwrap_or_default(),
        ),
        "ready_for_review" => "marked this pull request ready for review".to_string(),
        "converted_to_draft" => "converted this pull request to draft".to_string(),
        "merged" => "merged this pull request".to_string(),
        "closed" => "closed this pull request".to_string(),
        "reopened" => "reopened this pull request".to_string(),
        other => other.replace('_', " "),
    };
    let when = activity.created_unix.map_or_else(String::new, |created| {
        format!(" {}", relative_time(created))
    });
    Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Blue)),
        Span::styled(
            format!("@{actor}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {action}{when}"), muted_style(theme)),
    ])
}

fn draw_pull_request_status(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    view.check_hits.clear();
    let failed = view.checks.iter().any(|check| {
        matches!(
            check.conclusion.as_deref(),
            Some("failure" | "cancelled" | "timed_out" | "action_required")
        )
    });
    let successful = !view.checks.is_empty()
        && view.checks.iter().all(|check| {
            matches!(
                check.conclusion.as_deref(),
                Some("success" | "neutral" | "skipped")
            )
        });
    let (summary, summary_color) = if failed {
        ("Some checks were not successful", Color::Red)
    } else if successful {
        ("All checks have passed", Color::Green)
    } else if view.checks.is_empty() {
        (
            "No checks reported",
            theme.role(ThemeRole::Muted).to_ratatui(),
        )
    } else {
        (
            "Checks are still running",
            theme.role(ThemeRole::DiagnosticWarning).to_ratatui(),
        )
    };
    let mut lines = vec![Line::styled(
        summary,
        Style::default()
            .fg(summary_color)
            .add_modifier(Modifier::BOLD),
    )];
    let available = usize::from(area.height.saturating_sub(4));
    for check in view.checks.iter().take(available) {
        let result = check.conclusion.as_deref().unwrap_or(&check.status);
        lines.push(Line::from(vec![
            Span::styled(
                if matches!(result, "success" | "neutral" | "skipped") {
                    "✓ "
                } else if matches!(result, "failure" | "cancelled" | "timed_out") {
                    "✗ "
                } else {
                    "● "
                },
                if matches!(result, "success" | "neutral" | "skipped") {
                    Style::default().fg(Color::Green)
                } else if matches!(result, "failure" | "cancelled" | "timed_out") {
                    Style::default().fg(Color::Red)
                } else {
                    workflow_result_style(result, theme)
                },
            ),
            Span::styled(
                check.name.clone(),
                Style::default()
                    .fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui())
                    .add_modifier(Modifier::UNDERLINED),
            ),
            Span::styled(format!("  {result}"), muted_style(theme)),
        ]));
    }
    let button_row = area.bottom().saturating_sub(2);
    let merge_ready = view.can_write
        && view.pending.is_none()
        && !view.pull_request.draft
        && !view.pull_request.merged
        && view.pull_request.state.eq_ignore_ascii_case("open")
        && view.pull_request.mergeable != Some(false)
        && !failed;
    let merge_text = if view.pull_request.merged {
        " Merged "
    } else {
        " Merge pull request "
    };
    let merge_width = u16::try_from(merge_text.len()).unwrap_or(u16::MAX);
    view.merge_rect = Rect::new(area.x.saturating_add(1), button_row, merge_width, 1);
    let draft_text = if view.pull_request.draft {
        " Ready for review "
    } else {
        " Convert to draft "
    };
    let draft_width = u16::try_from(draft_text.len()).unwrap_or(u16::MAX);
    view.draft_rect = Rect::new(
        view.merge_rect.right().saturating_add(2),
        button_row,
        draft_width,
        1,
    );
    let buttons = Line::from(vec![
        Span::styled(
            merge_text,
            if merge_ready {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else {
                Style::default()
                    .fg(theme.role(ThemeRole::Muted).to_ratatui())
                    .bg(Color::DarkGray)
            }
            .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            draft_text,
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .bg(Color::DarkGray),
        ),
    ]);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Status ")
            .border_style(Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui())),
        area,
    );
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    f.render_widget(Paragraph::new(lines), inner);
    f.render_widget(
        Paragraph::new(buttons),
        Rect::new(
            area.x.saturating_add(1),
            button_row,
            area.width.saturating_sub(2),
            1,
        ),
    );
    for (index, check) in view.checks.iter().take(available).enumerate() {
        view.check_hits.push((
            check.html_url.clone(),
            Rect::new(
                area.x.saturating_add(1),
                area.y
                    .saturating_add(2 + u16::try_from(index).unwrap_or(u16::MAX)),
                area.width.saturating_sub(2),
                1,
            ),
        ));
    }
}

fn draw_pull_request_comment(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    view.comment_rect = area;
    if area.height == 0 {
        return;
    }
    let focused = view.editor == Some(crate::app::github::GithubPullRequestEditor::Comment);
    let lines = if !view.can_write {
        vec![muted_line("Sign in to comment.", theme)]
    } else if view.preview {
        let mut lines = Vec::new();
        append_rendered_markdown(
            &mut lines,
            &view.comment_edit,
            area.width.saturating_sub(2),
            theme,
        );
        lines
    } else if view.comment_edit.is_empty() {
        vec![muted_line(
            if focused {
                "Write Markdown…  Ctrl+Enter comment · Ctrl+P preview"
            } else {
                "Click to leave a comment…"
            },
            theme,
        )]
    } else {
        view.comment_edit
            .lines()
            .map(|line| Line::raw(line.to_string()))
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Leave a comment · Markdown ")
                    .border_style(
                        Style::default().fg(theme
                            .role(if focused {
                                ThemeRole::DiagnosticInfo
                            } else {
                                ThemeRole::IndentGuide
                            })
                            .to_ratatui()),
                    ),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_pull_request_commits(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut GithubPullRequestView,
) {
    view.commits_rect = area;
    let short_hashes: Vec<String> = view
        .commits
        .iter()
        .map(|commit| commit.sha.chars().take(7).collect())
        .collect();
    let entries: Vec<crate::ui::commit::CommitListEntry<'_>> = view
        .commits
        .iter()
        .zip(short_hashes.iter())
        .enumerate()
        .map(
            |(index, (commit, short_hash))| crate::ui::commit::CommitListEntry {
                hash: &commit.sha,
                short_hash,
                summary: &commit.summary,
                time: commit.committed_unix,
                parents: &commit.parents,
                head: index + 1 == view.commits.len(),
            },
        )
        .collect();
    let items =
        crate::ui::commit::commit_list_items(theme, &entries, Some(view.commit_cursor), false);
    let height = usize::from(area.height);
    let mut offset = usize::from(view.commit_offset);
    if view.commit_cursor < offset {
        offset = view.commit_cursor;
    } else if height > 0 && view.commit_cursor >= offset + height {
        offset = view.commit_cursor + 1 - height;
    }
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    view.commit_offset = u16::try_from(state.offset()).unwrap_or(u16::MAX);
}
