use super::*;
use karet_session::LanguageServerInstanceStatus;
use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerSource;
use karet_session::LanguageServerStatus;
use ratatui::widgets::Cell;
use ratatui::widgets::Row;
use ratatui::widgets::Table;
use ratatui::widgets::TableState;

use crate::app::LOADING_REVEAL_DELAY;
use crate::tab::LanguageServerAction;
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
    let buttons = [
        ("↻ Refresh", LanguageServerAction::Refresh),
        ("u Check", LanguageServerAction::CheckSelected),
        ("U Check all", LanguageServerAction::CheckAll),
        ("Enter Install/Update", LanguageServerAction::Primary),
        ("R Restart", LanguageServerAction::Restart),
        ("x Uninstall", LanguageServerAction::Uninstall),
        ("/ Filter", LanguageServerAction::Filter),
    ];
    let mut spans = Vec::new();
    for (label, action) in buttons {
        let width = u16::try_from(label.width() + 2).unwrap_or(u16::MAX);
        if x.saturating_add(width) > area.right() {
            break;
        }
        spans.push(Span::styled(
            format!(" {label} "),
            Style::default()
                .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
                .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui()),
        ));
        view.action_hits.push((Rect::new(x, y, width, 1), action));
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
    let rows = visible.iter().filter_map(|&index| {
        view.servers
            .get(index)
            .map(|status| inventory_row(status, area.width))
    });
    let (headers, widths) = table_shape(area.width);
    let header = Row::new(headers)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()));
    let mut state = TableState::default();
    state.select(Some(view.selected));
    *state.offset_mut() = view.offset.min(view.selected);
    f.render_stateful_widget(table, area, &mut state);
    view.offset = state.offset();
}

fn table_shape(width: u16) -> (Vec<Cell<'static>>, Vec<Constraint>) {
    if width >= 110 {
        (
            vec![
                Cell::from("Server"),
                Cell::from("Languages"),
                Cell::from("Source"),
                Cell::from("Installed"),
                Cell::from("Runtime"),
                Cell::from("Root / instances"),
            ],
            vec![
                Constraint::Length(22),
                Constraint::Length(18),
                Constraint::Length(13),
                Constraint::Length(18),
                Constraint::Length(14),
                Constraint::Min(16),
            ],
        )
    } else if width >= 72 {
        (
            vec![
                Cell::from("Server"),
                Cell::from("Languages"),
                Cell::from("Source"),
                Cell::from("Installed"),
                Cell::from("Runtime"),
            ],
            vec![
                Constraint::Length(20),
                Constraint::Length(16),
                Constraint::Length(12),
                Constraint::Length(16),
                Constraint::Min(12),
            ],
        )
    } else {
        (
            vec![
                Cell::from("Server"),
                Cell::from("Source"),
                Cell::from("Installed"),
                Cell::from("Runtime"),
            ],
            vec![
                Constraint::Length(18),
                Constraint::Length(11),
                Constraint::Length(15),
                Constraint::Min(10),
            ],
        )
    }
}

fn inventory_row(status: &LanguageServerStatus, width: u16) -> Row<'static> {
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
    let root = match status.instances.as_slice() {
        [] => "—".to_string(),
        [instance] => instance.root.display().to_string(),
        instances => format!("{} roots", instances.len()),
    };
    let mut cells = vec![
        Cell::from(status.server.display_name().to_string()),
        Cell::from(status.languages.join(", ")),
        Cell::from(source.to_string()),
        Cell::from(installed.to_string()),
        Cell::from(runtime),
        Cell::from(root),
    ];
    if width < 72 {
        cells.remove(1);
        cells.truncate(4);
    } else if width < 110 {
        cells.truncate(5);
    }
    Row::new(cells)
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
