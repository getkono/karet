use super::*;

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
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(cell_width(&right))])
        .split(area);
    let left = cols[0];

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

    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), left);
    f.render_widget(
        Paragraph::new(right).style(bar).alignment(Alignment::Right),
        cols[1],
    );
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
        StatusKind::Untracked => ('U', ThemeRole::DiffAdded),
        StatusKind::Conflicted => ('!', ThemeRole::DiagnosticError),
        _ => ('•', ThemeRole::Foreground),
    }
}
