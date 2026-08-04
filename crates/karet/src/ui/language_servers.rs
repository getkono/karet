use karet_session::LanguageServerInstanceStatus;
use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerSource;
use karet_session::LanguageServerStatus;

use super::*;
use crate::app::LOADING_REVEAL_DELAY;
use crate::tab::LanguageServerAction;
use crate::tab::LanguageServerActionHit;
use crate::tab::LanguageServerPendingKind;
use crate::tab::LanguageServersViewState;

pub(super) fn draw_language_servers(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut LanguageServersViewState,
) {
    let detail_height = if area.height >= 16 { 7 } else { 4 };
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(detail_height),
    ])
    .split(area);
    draw_actions(f, theme, sections[0], view);
    draw_inventory(f, theme, sections[1], view);
    draw_detail(f, theme, sections[2], view);
}

fn draw_actions(f: &mut Frame, theme: &Theme, area: Rect, view: &mut LanguageServersViewState) {
    view.action_hits.clear();
    let mut x = area.x;
    let y = area.y;
    let refreshing = view.inventory_request.is_some();
    let check_all_pending = view
        .pending
        .iter()
        .any(|pending| pending.kind == LanguageServerPendingKind::CheckAll);
    let has_installed = view
        .servers
        .iter()
        .any(|status| status.managed && status.installed.is_some());
    let mut buttons = vec![(
        if refreshing {
            "Refreshing…"
        } else {
            "↻ Refresh"
        },
        (!refreshing).then_some(LanguageServerAction::Refresh),
    )];
    if check_all_pending {
        buttons.push(("Checking all…", None));
    } else if has_installed {
        buttons.push(("U Check all", Some(LanguageServerAction::CheckAll)));
    }
    buttons.push(("/ Filter", Some(LanguageServerAction::Filter)));
    let mut spans = Vec::new();
    for (label, action) in buttons {
        let width = u16::try_from(label.width() + 2).unwrap_or(u16::MAX);
        if x.saturating_add(width) > area.right() {
            break;
        }
        let rect = Rect::new(x, y, width, 1);
        let hovered =
            action.is_some_and(|_| view.action_hover.is_some_and(|point| contains(rect, point)));
        spans.push(Span::styled(
            format!(" {label} "),
            action_style(theme, hovered, action.is_none(), action),
        ));
        if let Some(action) = action {
            view.action_hits.push(LanguageServerActionHit {
                rect,
                action,
                server: None,
            });
        }
        x = x.saturating_add(width + 1);
        spans.push(Span::raw(" "));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );
    let (filter, filter_role) = view.error.clone().map_or_else(
        || {
            (
                if view.filter.is_empty() {
                    "Filter: all".to_string()
                } else {
                    format!("Filter: {}", view.filter)
                },
                if view.filter.is_empty() {
                    ThemeRole::Muted
                } else {
                    ThemeRole::DiagnosticInfo
                },
            )
        },
        |error| (error, ThemeRole::DiagnosticError),
    );
    f.render_widget(
        Paragraph::new(filter).style(Style::default().fg(theme.role(filter_role).to_ratatui())),
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
    );
}

fn draw_inventory(f: &mut Frame, theme: &Theme, area: Rect, view: &mut LanguageServersViewState) {
    view.table_rect = area;
    view.row_hits.clear();
    let border_style = Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui());
    let table_block = Block::default()
        .title(" Language servers ")
        .borders(Borders::ALL)
        .border_style(border_style);
    let content = table_block.inner(area);
    f.render_widget(table_block, area);

    let visible = view.visible_indices();
    if visible.is_empty() {
        let message = if view.servers.is_empty() {
            view.error.as_deref().or_else(|| {
                view.loading_since
                    .filter(|since| since.elapsed() >= LOADING_REVEAL_DELAY)
                    .map(|_| "Loading language servers…")
            })
        } else {
            Some("No language servers match the filter")
        };
        if let Some(message) = message {
            f.render_widget(
                Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())),
                content,
            );
        }
        return;
    }

    view.selected = view.selected.min(visible.len().saturating_sub(1));
    if content.height == 0 || content.width == 0 {
        return;
    }
    let stacked = content.width < 60;
    let action_column_width = if stacked {
        content.width
    } else {
        (content.width / 3).clamp(20, 38)
    };
    let meta_width = if stacked {
        content.width
    } else {
        content.width.saturating_sub(action_column_width)
    };
    let action_width = if stacked {
        action_column_width
    } else {
        action_column_width.saturating_sub(1)
    };
    let header = if stacked {
        "Server · runtime"
    } else if meta_width >= 72 {
        "Server               Languages        Source       Installed      Runtime"
    } else if meta_width >= 42 {
        "Server               Installed      Runtime"
    } else {
        "Server · runtime"
    };
    f.render_widget(
        Paragraph::new(header).style(Style::default().add_modifier(Modifier::BOLD)),
        Rect::new(content.x, content.y, meta_width, 1),
    );
    if !stacked {
        let action_header = Rect::new(
            content.x.saturating_add(meta_width),
            content.y,
            action_column_width,
            1,
        );
        let divider = Block::default()
            .borders(Borders::LEFT)
            .border_style(border_style);
        let action_header_inner = divider.inner(action_header);
        f.render_widget(divider, action_header);
        f.render_widget(
            Paragraph::new("Actions").style(Style::default().add_modifier(Modifier::BOLD)),
            action_header_inner,
        );
    }

    let content_height = content.height.saturating_sub(1);
    view.offset = view.offset.min(view.selected);
    while row_heights_through_selection(view, &visible, view.offset, action_width, stacked)
        > content_height
        && view.offset < view.selected
    {
        view.offset += 1;
    }

    let mut y = content.y.saturating_add(1);
    for (visible_index, &server_index) in visible.iter().enumerate().skip(view.offset) {
        let Some(status) = view.servers.get(server_index).cloned() else {
            continue;
        };
        let actions = server_actions(view, &status);
        let action_lines = action_line_count(&actions, action_width);
        let row_content_height = if stacked {
            1_u16.saturating_add(action_lines)
        } else {
            action_lines.max(1)
        };
        let wanted_height = row_content_height.saturating_add(1);
        if y >= content.bottom() {
            break;
        }
        let height = wanted_height.min(content.bottom().saturating_sub(y));
        let row_rect = Rect::new(content.x, y, content.width, height);
        let selected = visible_index == view.selected;
        let row_style = if selected {
            Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui())
        } else {
            Style::default()
        };
        let row_block = Block::default()
            .borders(Borders::TOP)
            .border_style(border_style)
            .style(row_style);
        let row_inner = row_block.inner(row_rect);
        f.render_widget(row_block, row_rect);
        f.render_widget(
            Paragraph::new(inventory_summary(theme, &status, meta_width)).style(row_style),
            Rect::new(row_inner.x, row_inner.y, meta_width, 1),
        );

        let (action_x, action_y) = if stacked {
            (row_inner.x, row_inner.y.saturating_add(1))
        } else {
            (row_inner.x.saturating_add(meta_width), row_inner.y)
        };
        let mut action_area = Rect::new(
            action_x,
            action_y,
            if stacked {
                action_width
            } else {
                action_column_width
            },
            row_inner.bottom().saturating_sub(action_y),
        );
        if !stacked {
            let divider = Block::default()
                .borders(Borders::LEFT)
                .border_style(border_style)
                .style(row_style);
            let inner = divider.inner(action_area);
            f.render_widget(divider, action_area);
            action_area = inner;
        }
        render_server_actions(f, theme, view, &status, &actions, action_area);
        view.row_hits.push((row_rect, status.server.clone()));
        y = y.saturating_add(wanted_height);
    }
}

#[derive(Clone)]
struct RowAction {
    label: String,
    action: Option<LanguageServerAction>,
}

fn server_actions(
    view: &LanguageServersViewState,
    status: &LanguageServerStatus,
) -> Vec<RowAction> {
    let mut actions = Vec::new();
    if let Some(pending) = view
        .pending
        .iter()
        .find(|pending| pending.server.as_ref() == Some(&status.server))
    {
        let mut label = match pending.kind {
            LanguageServerPendingKind::CheckSelected => "Checking…",
            LanguageServerPendingKind::Install => "Installing…",
            LanguageServerPendingKind::Update => "Updating…",
            LanguageServerPendingKind::Uninstall => "Uninstalling…",
            LanguageServerPendingKind::CheckAll => "Checking…",
        }
        .to_string();
        if let Some(downloaded) = pending.downloaded {
            if let Some(total) = pending.total.filter(|total| *total > 0) {
                label = format!("{label} {}%", downloaded.saturating_mul(100) / total);
            } else {
                label = format!("{label} {downloaded} B");
            }
        }
        actions.push(RowAction {
            label,
            action: None,
        });
    } else {
        if let Some(change) = view
            .changes
            .iter()
            .find(|change| change.server == status.server)
        {
            actions.push(RowAction {
                label: if change.current.is_none() {
                    "Install"
                } else {
                    "Update"
                }
                .to_string(),
                action: Some(LanguageServerAction::Primary),
            });
        } else if status.managed {
            actions.push(RowAction {
                label: if status.installed.is_some() {
                    "Check updates"
                } else {
                    "Install"
                }
                .to_string(),
                action: Some(LanguageServerAction::Primary),
            });
        } else if status.manual_install_reason.is_some()
            && status
                .instances
                .iter()
                .all(|instance| instance.command.is_none())
        {
            actions.push(RowAction {
                label: "Install manually".to_string(),
                action: None,
            });
        }
        if status.managed && status.installed.is_some() {
            actions.push(RowAction {
                label: "Uninstall".to_string(),
                action: Some(LanguageServerAction::Uninstall),
            });
        }
    }
    if restartable(status) {
        actions.insert(
            actions.len().min(1),
            RowAction {
                label: "Restart".to_string(),
                action: Some(LanguageServerAction::Restart),
            },
        );
    }
    actions
}

fn restartable(status: &LanguageServerStatus) -> bool {
    status.instances.iter().any(|instance| {
        instance.open_documents > 0
            || !matches!(
                instance.runtime,
                LanguageServerRuntimeState::Idle | LanguageServerRuntimeState::Stopped
            )
    })
}

fn action_line_count(actions: &[RowAction], width: u16) -> u16 {
    if actions.is_empty() || width == 0 {
        return 0;
    }
    let mut lines = 1_u16;
    let mut used = 0_u16;
    for action in actions {
        let button = u16::try_from(action.label.width() + 2).unwrap_or(u16::MAX);
        let needed = button.saturating_add(u16::from(used > 0));
        if used > 0 && used.saturating_add(needed) > width {
            lines = lines.saturating_add(1);
            used = button;
        } else {
            used = used.saturating_add(needed);
        }
    }
    lines
}

fn row_heights_through_selection(
    view: &LanguageServersViewState,
    visible: &[usize],
    offset: usize,
    action_width: u16,
    stacked: bool,
) -> u16 {
    visible
        .iter()
        .enumerate()
        .skip(offset)
        .take(view.selected.saturating_sub(offset).saturating_add(1))
        .filter_map(|(_, index)| view.servers.get(*index))
        .map(|status| {
            let lines = action_line_count(&server_actions(view, status), action_width);
            let content = if stacked {
                1_u16.saturating_add(lines)
            } else {
                lines.max(1)
            };
            content.saturating_add(1)
        })
        .fold(0_u16, u16::saturating_add)
}

fn render_server_actions(
    f: &mut Frame,
    theme: &Theme,
    view: &mut LanguageServersViewState,
    status: &LanguageServerStatus,
    actions: &[RowAction],
    area: Rect,
) {
    let mut x = area.x;
    let mut y = area.y;
    for item in actions {
        let width = u16::try_from(item.label.width() + 2).unwrap_or(u16::MAX);
        if x > area.x && x.saturating_add(width) > area.right() {
            x = area.x;
            y = y.saturating_add(1);
        }
        if y >= area.bottom() {
            break;
        }
        let width = width.min(area.right().saturating_sub(x));
        let rect = Rect::new(x, y, width, 1);
        let hovered = item
            .action
            .is_some_and(|_| view.action_hover.is_some_and(|point| contains(rect, point)));
        f.render_widget(
            Paragraph::new(format!(" {} ", item.label)).style(action_style(
                theme,
                hovered,
                item.action.is_none(),
                item.action,
            )),
            rect,
        );
        if let Some(action) = item.action {
            view.action_hits.push(LanguageServerActionHit {
                rect,
                action,
                server: Some(status.server.clone()),
            });
        }
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn action_style(
    theme: &Theme,
    hovered: bool,
    pending: bool,
    action: Option<LanguageServerAction>,
) -> Style {
    if pending {
        return Style::default()
            .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
            .fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    }
    if hovered {
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD)
    } else {
        let role = match action {
            Some(LanguageServerAction::Primary) => ThemeRole::DiagnosticHint,
            Some(LanguageServerAction::Restart) => ThemeRole::DiagnosticWarning,
            Some(LanguageServerAction::Uninstall) => ThemeRole::DiagnosticError,
            Some(LanguageServerAction::Refresh | LanguageServerAction::CheckAll) => {
                ThemeRole::DiagnosticInfo
            },
            Some(LanguageServerAction::Filter) | None => ThemeRole::StatusBarForeground,
        };
        Style::default()
            .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
            .fg(theme.role(role).to_ratatui())
    }
}

fn contains(rect: Rect, (column, row): (u16, u16)) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}

fn inventory_summary(theme: &Theme, status: &LanguageServerStatus, width: u16) -> Line<'static> {
    let source = status.instances.first().map_or("unavailable", |first| {
        if status
            .instances
            .iter()
            .all(|instance| instance.source == first.source)
        {
            source_label(first.source)
        } else {
            "mixed"
        }
    });
    let installed = status.installed.as_deref().unwrap_or("—");
    let (runtime, runtime_role) = runtime_summary(status);
    let server_role = if status.enabled {
        ThemeRole::Foreground
    } else {
        ThemeRole::DiagnosticError
    };
    let installed_role = if status.installed.is_some() {
        ThemeRole::DiagnosticHint
    } else if status.managed {
        ThemeRole::DiagnosticWarning
    } else {
        ThemeRole::Muted
    };
    let source_role = match source {
        "unavailable" => ThemeRole::DiagnosticError,
        "managed" => ThemeRole::DiagnosticHint,
        "mixed" => ThemeRole::DiagnosticWarning,
        _ => ThemeRole::DiagnosticInfo,
    };
    let style = |role| Style::default().fg(theme.role(role).to_ratatui());
    let bold = |role| style(role).add_modifier(Modifier::BOLD);

    if width >= 72 {
        Line::from(vec![
            styled_field(status.server.display_name(), 21, bold(server_role)),
            styled_field(
                &status.languages.join(", "),
                17,
                style(ThemeRole::DiagnosticInfo),
            ),
            styled_field(source, 13, style(source_role)),
            styled_field(installed, 15, style(installed_role)),
            Span::styled(
                fit_columns(runtime, usize::from(width.saturating_sub(66))),
                bold(runtime_role),
            ),
        ])
    } else if width >= 42 {
        Line::from(vec![
            styled_field(status.server.display_name(), 21, bold(server_role)),
            styled_field(installed, 15, style(installed_role)),
            Span::styled(
                fit_columns(runtime, usize::from(width.saturating_sub(36))),
                bold(runtime_role),
            ),
        ])
    } else {
        let runtime_width = runtime.width();
        let server_width = usize::from(width)
            .saturating_sub(runtime_width)
            .saturating_sub(3);
        Line::from(vec![
            Span::styled(
                fit_columns(status.server.display_name(), server_width),
                bold(server_role),
            ),
            Span::raw(" · "),
            Span::styled(runtime, bold(runtime_role)),
        ])
    }
}

fn styled_field(text: &str, width: usize, style: Style) -> Span<'static> {
    let mut text = fit_columns(text, width);
    text.push_str(&" ".repeat(width.saturating_sub(text.width())));
    Span::styled(text, style)
}

fn fit_columns(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0_usize;
    for character in text.chars() {
        let width = character.to_string().width();
        if used.saturating_add(width) > max.saturating_sub(1) {
            break;
        }
        result.push(character);
        used = used.saturating_add(width);
    }
    result.push('…');
    result
}

fn draw_detail(f: &mut Frame, theme: &Theme, area: Rect, view: &LanguageServersViewState) {
    let Some(status) = view.selected_server() else {
        return;
    };
    let update = view
        .changes
        .iter()
        .find(|change| change.server == status.server)
        .map(|change| {
            format!(
                "update {} → {}",
                change.current.as_deref().unwrap_or("missing"),
                change.target
            )
        });
    let ownership = if status.managed {
        "Karet-managed"
    } else if status.manual_install_reason.is_some() {
        "manual install"
    } else {
        "external"
    };
    let style = |role| Style::default().fg(theme.role(role).to_ratatui());
    let mut identity = vec![
        Span::styled(
            status.server.display_name().to_owned(),
            style(if status.enabled {
                ThemeRole::Foreground
            } else {
                ThemeRole::DiagnosticError
            })
            .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::styled(
            ownership,
            style(if status.managed {
                ThemeRole::DiagnosticHint
            } else if status.manual_install_reason.is_some() {
                ThemeRole::DiagnosticWarning
            } else {
                ThemeRole::DiagnosticInfo
            }),
        ),
        Span::raw(" · "),
        Span::styled(
            if status.enabled {
                "enabled"
            } else {
                "disabled"
            },
            style(if status.enabled {
                ThemeRole::DiagnosticHint
            } else {
                ThemeRole::DiagnosticError
            })
            .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(update) = update {
        identity.push(Span::raw(" · "));
        identity.push(Span::styled(
            update,
            style(ThemeRole::DiagnosticWarning).add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![
        Line::from(identity),
        Line::from(vec![
            Span::raw("Languages: "),
            Span::styled(
                status.languages.join(", "),
                style(ThemeRole::DiagnosticInfo),
            ),
        ]),
    ];
    if let Some(reason) = &status.manual_install_reason {
        lines.push(Line::from(vec![
            Span::raw("Install: "),
            Span::styled(reason.clone(), style(ThemeRole::DiagnosticWarning)),
        ]));
    }
    for instance in status.instances.iter().take(3) {
        lines.push(instance_line(theme, instance));
        if let Some(error) = instance.error.as_deref() {
            lines.push(Line::styled(
                format!("  Error: {error}"),
                Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui()),
            ));
        }
    }
    if status.cleanup_pending {
        lines.push(Line::styled(
            "Payload cleanup pending: another shared Karet process still owns it",
            Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui()),
        ));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Selected server ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui())),
        ),
        area,
    );
}

fn instance_line(theme: &Theme, instance: &LanguageServerInstanceStatus) -> Line<'static> {
    let command = instance.command.as_deref().unwrap_or("not found");
    let args = if instance.args.is_empty() {
        String::new()
    } else {
        format!(" {}", instance.args.join(" "))
    };
    let style = |role| Style::default().fg(theme.role(role).to_ratatui());
    Line::from(vec![
        Span::raw(instance.root.display().to_string()),
        Span::raw(" · "),
        Span::styled(
            source_label(instance.source),
            style(source_role(instance.source)),
        ),
        Span::raw(" · "),
        Span::styled(
            runtime_label(instance.runtime),
            style(runtime_role(instance.runtime)).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::styled(
            format!("{command}{args}"),
            style(if instance.command.is_some() {
                ThemeRole::DiagnosticHint
            } else {
                ThemeRole::DiagnosticError
            }),
        ),
        Span::raw(" · "),
        Span::styled(
            format!("{} document(s)", instance.open_documents),
            style(if instance.open_documents > 0 {
                ThemeRole::DiagnosticHint
            } else {
                ThemeRole::Muted
            }),
        ),
    ])
}

fn runtime_summary(status: &LanguageServerStatus) -> (&'static str, ThemeRole) {
    let Some(first) = status.instances.first() else {
        return ("idle", ThemeRole::Muted);
    };
    let first_state = first.runtime;
    if status
        .instances
        .iter()
        .all(|instance| instance.runtime == first_state)
    {
        (runtime_label(first_state), runtime_role(first_state))
    } else {
        ("mixed", ThemeRole::DiagnosticWarning)
    }
}

fn source_label(source: LanguageServerSource) -> &'static str {
    match source {
        LanguageServerSource::Configured => "configured",
        LanguageServerSource::ProjectLocal => "project",
        LanguageServerSource::Path => "PATH",
        LanguageServerSource::Managed => "managed",
        LanguageServerSource::Unavailable => "unavailable",
        _ => "other",
    }
}

fn source_role(source: LanguageServerSource) -> ThemeRole {
    match source {
        LanguageServerSource::Managed => ThemeRole::DiagnosticHint,
        LanguageServerSource::Unavailable => ThemeRole::DiagnosticError,
        LanguageServerSource::Configured
        | LanguageServerSource::ProjectLocal
        | LanguageServerSource::Path => ThemeRole::DiagnosticInfo,
        _ => ThemeRole::Muted,
    }
}

fn runtime_role(state: LanguageServerRuntimeState) -> ThemeRole {
    match state {
        LanguageServerRuntimeState::Idle => ThemeRole::Muted,
        LanguageServerRuntimeState::Starting => ThemeRole::DiagnosticInfo,
        LanguageServerRuntimeState::Running => ThemeRole::DiagnosticHint,
        LanguageServerRuntimeState::Retrying => ThemeRole::DiagnosticWarning,
        // The breaker being open is a successful protective state; its associated
        // runtime failure remains separately rendered in the error color.
        LanguageServerRuntimeState::CircuitOpen => ThemeRole::DiagnosticHint,
        LanguageServerRuntimeState::Stopped => ThemeRole::DiagnosticError,
        _ => ThemeRole::Muted,
    }
}

fn runtime_label(state: LanguageServerRuntimeState) -> &'static str {
    match state {
        LanguageServerRuntimeState::Idle => "idle",
        LanguageServerRuntimeState::Starting => "starting",
        LanguageServerRuntimeState::Running => "running",
        LanguageServerRuntimeState::Retrying => "retrying",
        LanguageServerRuntimeState::CircuitOpen => "circuit open",
        LanguageServerRuntimeState::Stopped => "stopped",
        _ => "unknown",
    }
}
