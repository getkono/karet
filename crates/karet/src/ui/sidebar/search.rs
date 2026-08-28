//! The workspace Search panel: the find/replace fields and option toggles over
//! the grouped results list.

use super::*;

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
        let has_results = !app.search.results.is_empty();
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

    let search = &app.search;
    if search.results.is_empty() {
        let hint = theme.style(ThemeRole::LineNumber);
        let msg = if search.query.is_empty() {
            "  type a query, Enter to search"
        } else {
            "  no results"
        };
        f.render_widget(Paragraph::new(Line::styled(msg, hint)), rows[2]);
        return;
    }

    let items: Vec<ListItem> = search
        .results
        .iter()
        .map(|hit| {
            let name = hit
                .path
                .strip_prefix(&app.root)
                .unwrap_or(&hit.path)
                .to_string_lossy()
                .into_owned();
            ListItem::new(Line::from(vec![
                Span::raw(format!(" {name} ")),
                Span::styled(
                    format!("({})", hit.matches.len()),
                    theme.style(ThemeRole::LineNumber),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(search.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = search.results.len();
    let (results, tracks) = reserve_tracks(rows[2], ScrollAxes::VERTICAL);
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
