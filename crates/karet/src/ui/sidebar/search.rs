//! The workspace Search panel: the find/replace fields and option toggles over
//! the grouped results list.

use super::*;
use crate::app::SearchRow;

pub(super) fn draw_search_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    use crate::tab::SearchField;

    // Right-hand slot on the find/replace rows for the option toggles / replace-all.
    const SLOT_W: u16 = 10;
    let replace_visible = app.search.replace_visible;
    let replace_h = u16::from(replace_visible);
    let rows = Layout::vertical([
        Constraint::Length(1),         // find field
        Constraint::Length(replace_h), // replace field (collapsible)
        Constraint::Min(0),            // results
    ])
    .split(area);
    app.search_ui.results_rect = rows[2];
    app.search_ui.offset = 0;
    app.search_ui.action_hits = Vec::new();

    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.role(ThemeRole::LineNumber).to_ratatui();
    let fg = theme.role(ThemeRole::Foreground).to_ratatui();
    let editing_find = app.search.input && app.search.field == SearchField::Find;
    let editing_replace = app.search.input && app.search.field == SearchField::Replace;

    // Find row: query on the left, the option toggles (.* Aa \b) on the right.
    let find_cols =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(SLOT_W)]).split(rows[0]);
    let find_style = if editing_find {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg)
    };
    let find_prefix = Rect {
        width: find_cols[0].width.min(3),
        ..find_cols[0]
    };
    let find_field = Rect {
        x: find_prefix.right(),
        width: find_cols[0].width.saturating_sub(find_prefix.width),
        ..find_cols[0]
    };
    app.search_ui.query_rect = find_field;
    app.search
        .query_edit
        .ensure_cursor_visible(&app.search.query, find_field.width);
    f.render_widget(Paragraph::new(Line::styled(" › ", find_style)), find_prefix);
    let selection = theme.role(ThemeRole::Selection).to_ratatui();
    f.render_widget(
        Paragraph::new(text_field_text(
            &app.search.query,
            &app.search.query_edit,
            editing_find,
            find_style,
            find_style.bg(selection),
            Style::default().fg(accent),
        ))
        .scroll((0, app.search.query_edit.scroll)),
        find_field,
    );
    let toggles = [
        (".*", app.search.regex, Command::SearchToggleRegex),
        ("Aa", app.search.case_sensitive, Command::SearchToggleCase),
        ("\\b", app.search.whole_word, Command::SearchToggleWord),
    ];
    let mut toggle_spans = Vec::with_capacity(toggles.len());
    for (i, (label, on, cmd)) in toggles.into_iter().enumerate() {
        let x = find_cols[1].x + i as u16 * 3;
        app.search_ui.action_hits.push((x, x + 2, rows[0].y, cmd));
        let style = if on {
            Style::default().fg(accent).add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(dim)
        };
        toggle_spans.push(Span::styled(label, style));
        toggle_spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(toggle_spans)), find_cols[1]);

    // Replace row (collapsible): replacement on the left, a replace-all button right.
    if replace_visible {
        let rep_cols =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(SLOT_W)]).split(rows[1]);
        let rep_style = if editing_replace {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };
        let rep_prefix = Rect {
            width: rep_cols[0].width.min(3),
            ..rep_cols[0]
        };
        let rep_field = Rect {
            x: rep_prefix.right(),
            width: rep_cols[0].width.saturating_sub(rep_prefix.width),
            ..rep_cols[0]
        };
        app.search_ui.replace_rect = Some(rep_field);
        app.search
            .replace_edit
            .ensure_cursor_visible(&app.search.replace, rep_field.width);
        f.render_widget(Paragraph::new(Line::styled(" ⇄ ", rep_style)), rep_prefix);
        f.render_widget(
            Paragraph::new(text_field_text(
                &app.search.replace,
                &app.search.replace_edit,
                editing_replace,
                rep_style,
                rep_style.bg(selection),
                Style::default().fg(accent),
            ))
            .scroll((0, app.search.replace_edit.scroll)),
            rep_field,
        );
        // "replace all" button, active only when there are results to replace.
        let has_results = !app.search.hits.is_empty();
        let btn_style = if has_results {
            Style::default().fg(accent)
        } else {
            Style::default().fg(dim)
        };
        app.search_ui.action_hits.push((
            rep_cols[1].x,
            rep_cols[1].x + SLOT_W,
            rows[1].y,
            Command::SearchReplaceAll,
        ));
        f.render_widget(
            Paragraph::new(Line::styled(" ⟳ all", btn_style)),
            rep_cols[1],
        );
    } else {
        app.search_ui.replace_rect = None;
    }

    // Status: what this result set is, stated plainly. A big repository search
    // is otherwise indistinguishable from a hung one.
    let search = &app.search;
    let status = if let Some(error) = &search.error {
        Some((
            format!("  {error}"),
            theme.style(ThemeRole::DiagnosticError),
        ))
    } else if search.searching.is_some() {
        search
            .started
            .is_some_and(crate::app::Pending::visible)
            .then(|| {
                (
                    format!(
                        "  searching… {} files · {} matches",
                        search.files_scanned, search.matches_found
                    ),
                    theme.style(ThemeRole::Muted),
                )
            })
    } else if search.truncated {
        Some((
            format!("  {} matches shown (limit reached)", search.matches_found),
            theme.style(ThemeRole::Muted),
        ))
    } else if search.searched && !search.hits.is_empty() {
        Some((
            format!(
                "  {} matches in {} files",
                search.matches_found,
                search.hits.len()
            ),
            theme.style(ThemeRole::Muted),
        ))
    } else {
        None
    };
    let body = if let Some((text, style)) = status {
        let split = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(rows[2]);
        f.render_widget(Paragraph::new(Line::styled(text, style)), split[0]);
        split[1]
    } else {
        rows[2]
    };

    if search.rows.is_empty() {
        let hint = theme.style(ThemeRole::LineNumber);
        let msg = if search.query.is_empty() {
            "  type a query, Enter to search"
        } else if search.searching.is_some() {
            "  searching the workspace…"
        } else if search.error.is_some() {
            "" // the status line above already says why
        } else {
            "  no results"
        };
        f.render_widget(Paragraph::new(Line::styled(msg, hint)), body);
        app.search_ui.results_rect = body;
        return;
    }

    let items = result_items(app, theme, body.width);
    let mut state = ListState::default();
    state.select(Some(app.search.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = app.search.rows.len();
    let (results, tracks) = reserve_tracks(body, ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.search_ui.offset = state.offset();
    app.search_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, app.search_ui.offset, results.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::SearchResults,
    );
}

/// Build the grouped result rows: a heading per file, its matches indented under
/// it, each with the matched span highlighted and its line number right-aligned.
fn result_items<'a>(app: &App, theme: &Theme, width: u16) -> Vec<ListItem<'a>> {
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let fg = theme.style(ThemeRole::Foreground);
    let hit_style = Style::default().bg(theme.role(ThemeRole::SearchMatch).to_ratatui());
    let search = &app.search;

    // One number column for the whole list, sized from the widest line number in
    // the result set rather than the visible window, so it does not jitter while
    // scrolling. The list reserves a scrollbar track, hence the extra cell.
    let widest = search
        .hits
        .iter()
        .flat_map(|hit| hit.matches.iter())
        .map(|m| m.range.start.line.saturating_add(1))
        .max()
        .unwrap_or(1);
    let num_w = widest.to_string().len();
    let list_width = usize::from(width).saturating_sub(1);

    search
        .rows
        .iter()
        .map(|row| match *row {
            SearchRow::File {
                hit,
                count,
                expanded,
            } => {
                let Some(file) = search.hits.get(hit) else {
                    return ListItem::new(Line::default());
                };
                let path = file.path.strip_prefix(&app.root).unwrap_or(&file.path);
                // The filename is what identifies the row, so it is bright and it
                // is the part that survives a narrow sidebar; the directory dims
                // and truncates from the left, keeping the meaningful tail.
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let dir = path
                    .parent()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .filter(|dir| !dir.is_empty());
                let chevron = karet_filetype::chevron(expanded, app.icon_style);
                let count = format!(" ({count})");
                let mut spans = vec![
                    Span::styled(format!("{chevron} "), dim),
                    Span::styled(name.clone(), fg.add_modifier(Modifier::BOLD)),
                ];
                if let Some(dir) = dir.as_deref() {
                    let spent =
                        karet_widgets::text::width(&name) + karet_widgets::text::width(&count) + 3;
                    let room = list_width.saturating_sub(spent);
                    if room > 1 {
                        spans.push(Span::styled(
                            format!("  {}", karet_widgets::text::fit_start(dir, room)),
                            dim,
                        ));
                    }
                }
                spans.push(Span::styled(count, dim));
                ListItem::new(Line::from(spans))
            },
            SearchRow::Match { hit, index } => {
                let Some(m) = search
                    .hits
                    .get(hit)
                    .and_then(|file| file.matches.get(index))
                else {
                    return ListItem::new(Line::default());
                };
                let number = m.range.start.line.saturating_add(1).to_string();
                // Budget the preview so the number column always survives: it is
                // what tells two otherwise similar rows apart.
                let room = list_width.saturating_sub(num_w + 5);
                let (start, end) = (m.preview_start as usize, m.preview_end as usize);
                // Slice the preview at the backend's byte offsets. A `get` that
                // fails would mean inconsistent offsets, so fall back to the plain
                // line rather than blanking the row.
                let parts = m
                    .line_text
                    .get(..start)
                    .zip(m.line_text.get(start..end))
                    .zip(m.line_text.get(end..));
                let mut spans = vec![Span::raw("    ")];
                match parts {
                    Some(((before, hit_text), after)) => {
                        let before = karet_widgets::text::fit_start(before, room);
                        let budget = room.saturating_sub(karet_widgets::text::width(&before));
                        let hit_text = karet_widgets::text::fit_end(hit_text, budget);
                        let budget = budget.saturating_sub(karet_widgets::text::width(&hit_text));
                        spans.push(Span::styled(before, muted));
                        spans.push(Span::styled(hit_text, hit_style));
                        spans.push(Span::styled(
                            karet_widgets::text::fit_end(after, budget),
                            muted,
                        ));
                    },
                    None => spans.push(Span::styled(
                        karet_widgets::text::fit_end(&m.line_text, room),
                        muted,
                    )),
                }
                // Pad the number out to the right edge so the numbers line up into
                // a column the eye can run down.
                let spent: usize = spans
                    .iter()
                    .map(|span| karet_widgets::text::width(&span.content))
                    .sum();
                let pad = list_width.saturating_sub(spent + number.len());
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(number, dim));
                ListItem::new(Line::from(spans))
            },
        })
        .collect()
}
