//! Turning a provider's facts into the words and colours the manager shows.
//!
//! Pure over the status model: no view state, no frame, no hit testing. Both
//! the inventory table and the detail pane read their vocabulary from here, so
//! the two cannot disagree about what `circuit open` looks like.

use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerSource;
use karet_session::LanguageServerStatus;

use super::*;

pub(super) fn inventory_summary(
    theme: &Theme,
    status: &LanguageServerStatus,
    width: u16,
) -> Line<'static> {
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
    let style = |role| theme.style(role);
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
    karet_widgets::text::fit_end(text, max)
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

pub(super) fn source_label(source: LanguageServerSource) -> &'static str {
    match source {
        LanguageServerSource::Configured => "configured",
        LanguageServerSource::ProjectLocal => "project",
        LanguageServerSource::Path => "PATH",
        LanguageServerSource::Managed => "managed",
        LanguageServerSource::Unavailable => "unavailable",
        _ => "other",
    }
}

pub(super) fn source_role(source: LanguageServerSource) -> ThemeRole {
    match source {
        LanguageServerSource::Managed => ThemeRole::DiagnosticHint,
        LanguageServerSource::Unavailable => ThemeRole::DiagnosticError,
        LanguageServerSource::Configured
        | LanguageServerSource::ProjectLocal
        | LanguageServerSource::Path => ThemeRole::DiagnosticInfo,
        _ => ThemeRole::Muted,
    }
}

pub(super) fn runtime_role(state: LanguageServerRuntimeState) -> ThemeRole {
    match state {
        LanguageServerRuntimeState::Idle => ThemeRole::Muted,
        LanguageServerRuntimeState::Starting => ThemeRole::DiagnosticInfo,
        LanguageServerRuntimeState::Running => ThemeRole::DiagnosticHint,
        LanguageServerRuntimeState::Retrying => ThemeRole::DiagnosticWarning,
        // An open breaker reads as an error here, not as the protective success it
        // also is. The breaker is karet's own mechanism; what the user has is a
        // provider that crashed five times in a minute and will not be retried for
        // the next five. Colouring that as a hint made this table disagree with the
        // editor's own badge about the same condition.
        LanguageServerRuntimeState::CircuitOpen | LanguageServerRuntimeState::Unavailable => {
            ThemeRole::DiagnosticError
        },
        _ => ThemeRole::Muted,
    }
}

pub(super) fn runtime_label(state: LanguageServerRuntimeState) -> &'static str {
    match state {
        LanguageServerRuntimeState::Idle => "idle",
        LanguageServerRuntimeState::Starting => "starting",
        LanguageServerRuntimeState::Running => "running",
        LanguageServerRuntimeState::Retrying => "retrying",
        LanguageServerRuntimeState::CircuitOpen => "circuit open",
        LanguageServerRuntimeState::Unavailable => "unavailable",
        _ => "unknown",
    }
}
