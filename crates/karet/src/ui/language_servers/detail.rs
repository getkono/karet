//! The pane under the table: everything known about the selected provider.

use karet_session::LanguageServerInstanceStatus;

use super::*;

pub(super) fn draw_detail(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &LanguageServersViewState,
) {
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
    let style = |role| theme.style(role);
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
                theme.style(ThemeRole::DiagnosticError),
            ));
        }
    }
    if status.cleanup_pending {
        lines.push(Line::styled(
            "Payload cleanup pending: another shared Karet process still owns it",
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    if status.declined {
        // Without this the refusal is invisible: the provider simply stops being
        // offered, with nothing on screen to explain why or to take it back.
        lines.push(Line::styled(
            "Declined: karet will not offer to install this — press o to offer it again",
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Selected server ")
                .borders(Borders::TOP)
                .border_style(theme.style(ThemeRole::IndentGuide)),
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
    let style = |role| theme.style(role);
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
