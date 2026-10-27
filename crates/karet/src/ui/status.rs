use super::*;

/// What joins the right strip's segments. Three cells wide.
const SEPARATOR: &str = " \u{b7} ";

pub(super) fn draw_status(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    frame: &mut FrameHits,
) {
    frame.status_rect = area;

    let focus = match app.keyboard.focus() {
        Focus::Sidebar => "SIDEBAR",
        // The content area belongs to whichever view is showing, so name that
        // rather than the editor shell behind it.
        Focus::Editor => app.view.focus_label(),
        Focus::Outline => "OUTLINE",
    };
    // Likewise for the right-hand strip: in a non-editor view the tab-derived half
    // of it (language, spelling, LSP badge, cursor, encoding) would describe a
    // document the user cannot see. The workspace-wide segments below still apply.
    let active_tab = (app.view == View::Editor)
        .then(|| app.tabs.get(app.active))
        .flatten();
    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let key = bar.add_modifier(Modifier::BOLD);

    // The right column is a fixed-width strip, assembled as segments rather than
    // one joined string. The LSP badge needs both its own span (to carry its
    // colour) and its own column range (to be clickable), and recovering either
    // from a joined string meant re-finding the label inside it — which broke as
    // soon as the label could appear twice or the strip was truncated.
    //
    // Segment order, left to right: the workspace's error and warning counts,
    // cursor position (code tabs only), encoding and EOL, today's coding total,
    // the debug session, the language, its spelling language, the work its
    // language server reports, and the language-server badge.
    let mut segments: Vec<String> = Vec::new();
    // Each clickable segment: its index, its colour, and the command it runs.
    let mut actions: Vec<(usize, Style, Command)> = Vec::new();
    // The counts are workspace-wide, so they show in every view, and always —
    // zero included — so the way into the Problems panel never moves.
    let (errors, warnings) = app.problems.counts();
    for (count, letter, role) in [
        (errors, 'E', ThemeRole::DiagnosticError),
        (warnings, 'W', ThemeRole::DiagnosticWarning),
    ] {
        let style = if count > 0 {
            bar.fg(theme.role(role).to_ratatui())
                .add_modifier(Modifier::BOLD)
        } else {
            bar
        };
        actions.push((
            segments.len(),
            style,
            Command::SelectPanel(SidebarPanel::Problems),
        ));
        segments.push(format!("{letter} {count}"));
    }
    if let Some(
        tab @ Tab {
            kind: TabKind::Code { .. },
            ..
        },
    ) = active_tab
    {
        segments.push(cursor_status_label(tab));
        if let Some(encoding) = tab.encoding_label() {
            segments.push(encoding);
        }
    }
    if let Some(today) = app
        .wakatime_status
        .as_deref()
        .filter(|_| app.settings.wakatime.enabled && app.settings.wakatime.status_bar)
    {
        segments.push(today.to_owned());
    }
    if let Some(segment) = app.debug_status_segment() {
        segments.push(segment);
    }
    // Pushed even when empty: with no tab the strip is a bare pair of spaces, and
    // dropping the segment entirely would collapse the strip's padding with it.
    segments.push(active_tab.map_or("", Tab::language).to_owned());
    if let Some(spelling) = active_tab.and_then(|tab| match &tab.kind {
        TabKind::Code { doc: Some(doc), .. } => app
            .docs
            .settings
            .get(doc)
            .and_then(|settings| settings.spelling_language),
        _ => None,
    }) {
        segments.push(spelling.display_name().to_owned());
    }
    // The work the file's server reports, just before its badge. A click
    // cancels it when the server allows that, so only then is it a target.
    if let Some(work) = active_tab.and(app.active_language_server_work()) {
        let mut label = work.label();
        if work.cancellable {
            actions.push((
                segments.len(),
                bar.fg(theme.role(ThemeRole::DiagnosticInfo).to_ratatui()),
                Command::CancelLanguageServerWork,
            ));
            label.push_str(" \u{d7}");
        }
        segments.push(label);
    }
    if let Some(badge) = active_tab.and(app.active_language_server_badge()) {
        actions.push((
            segments.len(),
            bar.fg(theme.role(lsp_badge::role(badge.state)).to_ratatui())
                .add_modifier(Modifier::BOLD),
            Command::ManageLanguageServers,
        ));
        segments.push(lsp_badge::spelled(badge));
    }

    let right_width = status_right_width(&segments);
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
    frame.status_hits.push((x, x + fw, Command::ToggleFocus));
    x += fw;
    // A modal preset names its mode beside the focus, with the macro being
    // recorded and the command typed so far (vim's `showcmd`).
    if let Some((mode, recording, typed)) = app.vi_status() {
        let mut chip = format!(" {} ", mode.label());
        if let Some(register) = recording {
            chip.push_str(&format!("recording @{register} "));
        }
        if !typed.is_empty() {
            chip.push_str(&typed);
            chip.push(' ');
        }
        x += cell_width(&chip);
        spans.push(Span::styled(chip, key));
    }
    let gutter = "   ";
    spans.push(Span::styled(gutter.to_string(), bar));
    x += cell_width(gutter);
    let avail = left.width.saturating_sub(x - left.x);

    // The remaining space belongs to the keymap: an in-progress chord's completions,
    // else the active context's key hints. Messages never land here — they are
    // notifications, which carry a severity colour and a lifetime of their own. A
    // message rendered in the bar's own style read as invisible, and covering the
    // hints also took their click targets with it.
    if !app.pending.is_empty() {
        let ctx = Context::focus(app.focus_target()).with_editing(app.vi_editing());
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
            &mut frame.status_hits,
            &mut x,
            rest,
            bar,
            key,
        );
    } else {
        let hints = keymap::hints_for(app.input_context(), ChordStyle::Caret);
        render_hints(
            &hints,
            &mut spans,
            &mut frame.status_hits,
            &mut x,
            avail,
            bar,
            key,
        );
    }

    // The widget right-aligns the strip in exactly `right_width` columns, so its
    // first column is derivable -- but only while the strip fits. On a terminal too
    // narrow for it the alignment truncates and the columns no longer describe what
    // is painted, so no click target is claimed rather than a wrong one.
    let strip_x = area
        .x
        .saturating_add(area.width.saturating_sub(right_width));
    let (right_line, action_hits) = status_right(&segments, &actions, strip_x, bar);
    if right_width <= area.width {
        frame.status_hits.extend(action_hits);
    }
    karet_widgets::status::StatusBar {
        bar,
        left: Line::from(spans),
        right: right_line,
    }
    .draw(f, area);
}

/// The columns the right strip will occupy: one space of padding either side,
/// with a three-cell separator between segments.
fn status_right_width(segments: &[String]) -> u16 {
    let content = segments
        .iter()
        .map(|segment| cell_width(segment))
        .fold(0u16, |total, width| total.saturating_add(width));
    let separators = u16::try_from(segments.len().saturating_sub(1))
        .unwrap_or(u16::MAX)
        .saturating_mul(3);
    content.saturating_add(separators).saturating_add(2)
}

/// Assemble the right strip, colouring each action segment and reporting the
/// columns it occupies with the command a click there runs.
///
/// `actions` names each clickable segment by index, with its style and command;
/// the index is passed rather than searched for because the caller is the only
/// thing that knows where it pushed it, and a label can appear twice.
fn status_right(
    segments: &[String],
    actions: &[(usize, Style, Command)],
    strip_x: u16,
    bar: Style,
) -> (Line<'static>, Vec<(u16, u16, Command)>) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut hits = Vec::new();
    let mut x = strip_x.saturating_add(1);
    spans.push(Span::styled(" ".to_owned(), bar));
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR.to_owned(), bar));
            x = x.saturating_add(cell_width(SEPARATOR));
        }
        let width = cell_width(segment);
        let style = match actions.iter().find(|&&(at, ..)| at == index) {
            Some(&(_, style, command)) => {
                hits.push((x, x.saturating_add(width), command));
                style
            },
            None => bar,
        };
        spans.push(Span::styled(segment.clone(), style));
        x = x.saturating_add(width);
    }
    spans.push(Span::styled(" ".to_owned(), bar));
    (Line::from(spans), hits)
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
