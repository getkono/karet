//! The per-row action strip: which buttons a provider offers, how they wrap,
//! and how they are painted and hit-tested.
//!
//! Split from `language_servers.rs` so the table, the detail pane and the
//! buttons each stay well inside the workspace file-size ceiling.

use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerStatus;

use super::*;

#[derive(Clone)]
pub(super) struct RowAction {
    label: String,
    action: Option<LanguageServerAction>,
}

pub(super) fn server_actions(
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

pub(super) fn restartable(status: &LanguageServerStatus) -> bool {
    status.instances.iter().any(|instance| {
        instance.open_documents > 0
            || !matches!(
                instance.runtime,
                LanguageServerRuntimeState::Idle | LanguageServerRuntimeState::Stopped
            )
    })
}

pub(super) fn action_line_count(actions: &[RowAction], width: u16) -> u16 {
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

pub(super) fn row_heights_through_selection(
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

pub(super) fn render_server_actions(
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

pub(super) fn action_style(
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

pub(super) fn contains(rect: Rect, (column, row): (u16, u16)) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}
