use super::*;
use crate::app::LanguageServerBadge;

pub(super) fn draw_status(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.status_rect = area;
    app.status_hits.clear();

    let focus = match app.focus {
        Focus::Sidebar => "SIDEBAR",
        Focus::Editor => "EDITOR",
        Focus::Outline => "OUTLINE",
    };
    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let key = bar.add_modifier(Modifier::BOLD);

    // The right column is a fixed-width strip: cursor position (code tabs only),
    // encoding/EOL, then the language/kind label — the hints get everything else.
    let language = app.tabs.get(app.active).map_or("", Tab::language);
    let language = match app.tabs.get(app.active).and_then(|tab| match &tab.kind {
        TabKind::Code { doc: Some(doc), .. } => app
            .docs
            .settings
            .get(doc)
            .and_then(|settings| settings.spelling_language),
        _ => None,
    }) {
        Some(spelling) => format!("{language} · {}", spelling.display_name()),
        None => language.to_owned(),
    };
    let lsp_badge = app.active_language_server_badge();
    let lsp_label = lsp_badge.map(language_server_badge_label);
    let language = lsp_label.map_or(language.clone(), |badge| format!("{language} · {badge}"));
    // Today's coding total leads the strip while WakaTime tracking is on.
    let language = match app
        .wakatime_status
        .as_deref()
        .filter(|_| app.settings.wakatime.enabled && app.settings.wakatime.status_bar)
    {
        Some(today) => format!("{today} · {language}"),
        None => language,
    };
    let right = match app.tabs.get(app.active) {
        Some(
            tab @ Tab {
                kind: TabKind::Code { .. },
                ..
            },
        ) => {
            let cursor_label = cursor_status_label(tab);
            match tab.encoding_label() {
                Some(enc) => format!(" {cursor_label} · {enc} · {language} "),
                None => format!(" {cursor_label} · {language} "),
            }
        },
        _ => format!(" {language} "),
    };
    let right_width = cell_width(&right);
    let left = Rect {
        width: area.width.saturating_sub(right_width),
        ..area
    };

    // The focus chip, then a gutter, then the responsive hint region.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut x = left.x;
    let focus_text = format!(" {focus} ");
    let fw = cell_width(&focus_text);
    spans.push(Span::styled(focus_text, key));
    app.status_hits.push((x, x + fw, Command::ToggleFocus));
    x += fw;
    let gutter = "   ";
    spans.push(Span::styled(gutter.to_string(), bar));
    x += cell_width(gutter);
    let avail = left.width.saturating_sub(x - left.x);

    // Priority for the remaining space: an in-progress chord's completions, then any
    // transient message, then the active context's key hints — all keymap-derived.
    if !app.pending.is_empty() {
        let ctx = Context::focus(app.focus_target());
        let prefix = app
            .pending
            .iter()
            .map(|c| c.display(ChordStyle::Caret))
            .collect::<Vec<_>>()
            .join(" ");
        let comps = keymap::completions_for(ctx, &app.pending, ChordStyle::Caret);
        spans.push(Span::styled(prefix.clone(), key));
        spans.push(Span::styled(" → ".to_string(), bar));
        x += cell_width(&prefix) + cell_width(" → ");
        let rest = avail.saturating_sub(cell_width(&prefix) + cell_width(" → "));
        render_hints(
            &comps,
            &mut spans,
            &mut app.status_hits,
            &mut x,
            rest,
            bar,
            key,
        );
    } else if let Some(msg) = app.status.clone() {
        spans.push(Span::styled(format!("{msg} "), bar));
    } else {
        let hints = keymap::hints_for(app.input_context(), ChordStyle::Caret);
        render_hints(
            &hints,
            &mut spans,
            &mut app.status_hits,
            &mut x,
            avail,
            bar,
            key,
        );
    }

    let right_line = match lsp_badge.zip(lsp_label) {
        Some((badge, label)) => styled_status_right(&right, label, badge, bar, theme),
        None => Line::styled(right, bar),
    };
    karet_widgets::status::StatusBar {
        bar,
        left: Line::from(spans),
        right: right_line,
    }
    .draw(f, area);
}

fn language_server_badge_label(badge: LanguageServerBadge) -> &'static str {
    match badge {
        LanguageServerBadge::Idle => "LSP idle",
        LanguageServerBadge::Starting => "LSP starting",
        LanguageServerBadge::InSync => "LSP in sync",
        LanguageServerBadge::Retrying => "LSP retrying",
        LanguageServerBadge::Crashed => "LSP crashed",
        LanguageServerBadge::Unavailable => "LSP unavailable",
    }
}

fn language_server_badge_role(badge: LanguageServerBadge) -> ThemeRole {
    match badge {
        LanguageServerBadge::InSync => ThemeRole::DiagnosticHint,
        LanguageServerBadge::Starting | LanguageServerBadge::Retrying => {
            ThemeRole::DiagnosticWarning
        },
        LanguageServerBadge::Crashed | LanguageServerBadge::Unavailable => {
            ThemeRole::DiagnosticError
        },
        LanguageServerBadge::Idle => ThemeRole::Muted,
    }
}

fn styled_status_right(
    right: &str,
    label: &str,
    badge: LanguageServerBadge,
    bar: Style,
    theme: &Theme,
) -> Line<'static> {
    let Some(start) = right.rfind(label) else {
        return Line::styled(right.to_owned(), bar);
    };
    let end = start.saturating_add(label.len());
    Line::from(vec![
        Span::styled(right[..start].to_owned(), bar),
        Span::styled(
            label.to_owned(),
            bar.fg(theme.role(language_server_badge_role(badge)).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(right[end..].to_owned(), bar),
    ])
}

/// The status bar's cursor-position label for a code tab: `"Ln {line}, Col
/// {col}"` (1-based), with a `"(N selected)"` / `"(N lines selected)"` suffix
/// when the primary selection is non-empty.
pub(super) fn cursor_status_label(tab: &Tab) -> String {
    let primary = tab.editor.cursors().primary();
    let head = primary.head;
    let mut label = format!("Ln {}, Col {}", head.line + 1, head.col + 1);
    let range = primary.range();
    if range.start != range.end {
        if range.start.line == range.end.line {
            let n = range.end.col.saturating_sub(range.start.col);
            label.push_str(&format!(" ({n} selected)"));
        } else {
            let lines = range.end.line - range.start.line + 1;
            label.push_str(&format!(" ({lines} lines selected)"));
        }
    }
    label
}

/// The single-letter status glyph and its color role for a changed file.
pub(super) fn status_glyph(kind: StatusKind) -> (char, ThemeRole) {
    match kind {
        StatusKind::Added => ('A', ThemeRole::DiffAdded),
        StatusKind::Modified => ('M', ThemeRole::DiagnosticWarning),
        StatusKind::Deleted => ('D', ThemeRole::DiagnosticError),
        StatusKind::Renamed => ('R', ThemeRole::DiagnosticInfo),
        StatusKind::Copied => ('C', ThemeRole::DiagnosticInfo),
        StatusKind::Untracked => ('U', ThemeRole::DiffAdded),
        StatusKind::Conflicted => ('!', ThemeRole::DiagnosticError),
        _ => ('•', ThemeRole::Foreground),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_kind_has_its_own_glyph() {
        // `StatusKind` is `#[non_exhaustive]`, so a new variant silently falls into
        // the `•` arm until it is given a letter here.
        let kinds = [
            (StatusKind::Added, 'A'),
            (StatusKind::Modified, 'M'),
            (StatusKind::Deleted, 'D'),
            (StatusKind::Renamed, 'R'),
            (StatusKind::Copied, 'C'),
            (StatusKind::Untracked, 'U'),
            (StatusKind::Conflicted, '!'),
        ];
        for (kind, expected) in kinds {
            assert_eq!(status_glyph(kind).0, expected, "{kind:?}");
        }
    }
}
