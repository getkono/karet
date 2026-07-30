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
        .as_ref()
        .is_some_and(|pending| pending.kind == LanguageServerPendingKind::CheckAll);
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
    } else if has_installed && view.pending.is_none() {
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
            action_style(theme, hovered, action.is_none()),
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
    let filter = view.error.clone().unwrap_or_else(|| {
        if view.filter.is_empty() {
            "Filter: all".to_string()
        } else {
            format!("Filter: {}", view.filter)
        }
    });
    f.render_widget(
        Paragraph::new(filter)
            .style(Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())),
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
                area,
            );
        }
        return;
    }

    view.selected = view.selected.min(visible.len().saturating_sub(1));
    if area.height == 0 {
        return;
    }
    let stacked = area.width < 60;
    let action_width = if stacked {
        area.width
    } else {
        (area.width / 3).clamp(20, 38)
    };
    let meta_width = if stacked {
        area.width
    } else {
        area.width.saturating_sub(action_width).saturating_sub(1)
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
        Rect::new(area.x, area.y, meta_width, 1),
    );
    if !stacked {
        f.render_widget(
            Paragraph::new("Actions").style(Style::default().add_modifier(Modifier::BOLD)),
            Rect::new(
                area.right().saturating_sub(action_width),
                area.y,
                action_width,
                1,
            ),
        );
    }

    let content_height = area.height.saturating_sub(1);
    view.offset = view.offset.min(view.selected);
    while row_heights_through_selection(view, &visible, view.offset, action_width, stacked)
        > content_height
        && view.offset < view.selected
    {
        view.offset += 1;
    }

    let mut y = area.y.saturating_add(1);
    for (visible_index, &server_index) in visible.iter().enumerate().skip(view.offset) {
        let Some(status) = view.servers.get(server_index).cloned() else {
            continue;
        };
        let actions = server_actions(view, &status);
        let action_lines = action_line_count(&actions, action_width);
        let wanted_height = if stacked {
            1_u16.saturating_add(action_lines)
        } else {
            action_lines.max(1)
        };
        if y >= area.bottom() {
            break;
        }
        let height = wanted_height.min(area.bottom().saturating_sub(y));
        let row_rect = Rect::new(area.x, y, area.width, height);
        let selected = visible_index == view.selected;
        let row_style = if selected {
            Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui())
        } else {
            Style::default()
        };
        f.render_widget(Block::default().style(row_style), row_rect);
        f.render_widget(
            Paragraph::new(inventory_summary(&status, meta_width)).style(row_style),
            Rect::new(area.x, y, meta_width, 1),
        );

        let (action_x, action_y) = if stacked {
            (area.x, y.saturating_add(1))
        } else {
            (area.right().saturating_sub(action_width), y)
        };
        render_server_actions(
            f,
            theme,
            view,
            &status,
            &actions,
            Rect::new(
                action_x,
                action_y,
                action_width,
                row_rect.bottom().saturating_sub(action_y),
            ),
        );
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
        .as_ref()
        .filter(|pending| pending.server.as_ref() == Some(&status.server))
    {
        let label = match pending.kind {
            LanguageServerPendingKind::CheckSelected => "Checking…",
            LanguageServerPendingKind::DiscoverInstall => "Checking…",
            LanguageServerPendingKind::Install => "Installing…",
            LanguageServerPendingKind::Update => "Updating…",
            LanguageServerPendingKind::Uninstall => "Uninstalling…",
            LanguageServerPendingKind::CheckAll => "Checking…",
        };
        actions.push(RowAction {
            label: label.to_string(),
            action: None,
        });
    } else if view.pending.is_none() {
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
            if stacked {
                1_u16.saturating_add(lines)
            } else {
                lines.max(1)
            }
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

fn action_style(theme: &Theme, hovered: bool, pending: bool) -> Style {
    if pending {
        return Style::default()
            .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
            .fg(theme.role(ThemeRole::Muted).to_ratatui());
    }
    if hovered {
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
            .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui())
    }
}

fn contains(rect: Rect, (column, row): (u16, u16)) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}

fn inventory_summary(status: &LanguageServerStatus, width: u16) -> String {
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
    let runtime = runtime_summary(status);
    let summary = if width >= 72 {
        format!(
            "{:<20} {:<16} {:<12} {:<14} {}",
            status.server.display_name(),
            status.languages.join(", "),
            source,
            installed,
            runtime
        )
    } else if width >= 42 {
        format!(
            "{:<20} {:<14} {}",
            status.server.display_name(),
            installed,
            runtime
        )
    } else {
        format!("{} · {runtime}", status.server.display_name())
    };
    fit_columns(&summary, usize::from(width))
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
                " · update {} → {}",
                change.current.as_deref().unwrap_or("missing"),
                change.target
            )
        })
        .unwrap_or_default();
    let ownership = if status.managed {
        "Karet-managed"
    } else {
        "external"
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                status.server.display_name(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " · {ownership} · {}{}",
                if status.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                update
            )),
        ]),
        Line::raw(format!("Languages: {}", status.languages.join(", "))),
    ];
    for instance in status.instances.iter().take(3) {
        lines.push(instance_line(instance));
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

fn instance_line(instance: &LanguageServerInstanceStatus) -> Line<'static> {
    let command = instance.command.as_deref().unwrap_or("not found");
    let args = if instance.args.is_empty() {
        String::new()
    } else {
        format!(" {}", instance.args.join(" "))
    };
    Line::raw(format!(
        "{} · {} · {} · {}{} · {} document(s)",
        instance.root.display(),
        source_label(instance.source),
        runtime_label(instance.runtime),
        command,
        args,
        instance.open_documents
    ))
}

fn runtime_summary(status: &LanguageServerStatus) -> String {
    let Some(first) = status.instances.first() else {
        return "idle".to_string();
    };
    let first = runtime_label(first.runtime);
    if status
        .instances
        .iter()
        .all(|instance| runtime_label(instance.runtime) == first)
    {
        first.to_string()
    } else {
        "mixed".to_string()
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
