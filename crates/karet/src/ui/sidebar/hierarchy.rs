//! The Hierarchy panel: a status line naming which way the tree grows, over
//! the call or type tree itself, one row per visible node.

use super::*;

/// Draw the Hierarchy panel into `area`, recording the hit-test chrome.
pub(super) fn draw_hierarchy_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    app.hierarchy_ui.results_rect = rows[1];
    app.hierarchy_ui.offset = 0;
    app.hierarchy_ui.action_hits = Vec::new();

    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let Some(tree) = app.hierarchy.tree.as_ref() else {
        return;
    };
    // The note waits out the shared reveal delay, so a fast server never
    // flashes it; the status line keeps its place either way.
    let loading = tree.loading().is_some_and(crate::pending::Pending::visible);
    let status = if loading {
        format!(" {} · loading…", tree.title())
    } else {
        format!(" {}", tree.title())
    };
    f.render_widget(Paragraph::new(Line::styled(status, muted)), rows[0]);

    let labels = tree.rows();
    if labels.is_empty() {
        if loading {
            f.render_widget(Paragraph::new(Line::styled("  loading…", dim)), rows[1]);
        }
        return;
    }

    // The list reserves a scrollbar track, so the text budget is one cell short
    // of the pane (`reserve_tracks` below carves the same column off).
    let list_width = usize::from(rows[1].width.saturating_sub(1));
    let hints = tree.row_hints();
    let items: Vec<ListItem> = labels
        .iter()
        .zip(hints.iter())
        .map(|(label, hint)| {
            let hint = hint.as_deref().unwrap_or_default();
            // The label yields to the location only down to half the row: a
            // name cut to nothing tells the rows apart no better than no hint.
            let hint_w = karet_widgets::text::width(hint);
            let label_budget = list_width.saturating_sub(hint_w + 3).max(list_width / 2);
            let label = karet_widgets::text::fit_end(label, label_budget);
            ListItem::new(Line::from(vec![
                Span::raw(format!(" {label}  ")),
                Span::styled(hint.to_owned(), dim),
            ]))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(tree.selected()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = labels.len();
    let (results, tracks) = reserve_tracks(rows[1], ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.hierarchy_ui.offset = state.offset();
    app.hierarchy_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, state.offset(), results.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::HierarchyResults,
    );
}
