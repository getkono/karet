//! The Problems panel: a filter/severity toolbar over the diagnostics of every
//! open document, grouped by file.

use super::*;
use crate::app::ProblemRow;

/// The severity letter a problem row leads with: one ASCII cell in every icon
/// style, coloured by severity, so the column of marks never drifts.
fn severity_mark(severity: Severity) -> char {
    match severity {
        Severity::Error => 'E',
        Severity::Warning => 'W',
        Severity::Information => 'I',
        Severity::Hint => 'H',
        _ => '?',
    }
}

/// Draw the Problems panel into `area`, recording the hit-test chrome.
pub(super) fn draw_problems_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    // The severity chip's width: the widest label plus its brackets and a pad.
    const CHIP_W: u16 = 13;
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(CHIP_W)]).split(rows[0]);
    app.problems_ui.results_rect = rows[1];
    app.problems_ui.offset = 0;
    app.problems_ui.action_hits = vec![
        (
            cols[0].x,
            cols[0].x + cols[0].width,
            rows[0].y,
            Command::ProblemsFilter,
        ),
        (
            cols[1].x,
            cols[1].x + cols[1].width,
            rows[0].y,
            Command::ProblemsCycleSeverity,
        ),
    ];

    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let problems = &app.problems;
    let editing = problems.filter_input && app.focus == Focus::Sidebar;

    // The filter field: what is typed (with a caret while typing), else a hint.
    let filter = if problems.filter.is_empty() && !editing {
        Line::styled(" / filter", muted)
    } else {
        let caret = if editing { "\u{258f}" } else { "" };
        let budget = usize::from(cols[0].width).saturating_sub(4);
        let text = karet_widgets::text::fit_start(&problems.filter, budget);
        Line::from(vec![
            Span::styled(" / ", dim),
            Span::styled(format!("{text}{caret}"), Style::default().fg(accent)),
        ])
    };
    f.render_widget(Paragraph::new(filter), cols[0]);
    f.render_widget(
        Paragraph::new(Line::styled(
            format!(" [{}]", problems.severity.label()),
            Style::default().fg(accent),
        )),
        cols[1],
    );

    if problems.rows.is_empty() {
        let message = if problems.entries.is_empty() {
            "  no problems in open files"
        } else {
            "  no problems match the filter"
        };
        f.render_widget(Paragraph::new(Line::styled(message, dim)), rows[1]);
        return;
    }

    // The list reserves a scrollbar track, so the text budget is one cell short
    // of the pane (`reserve_tracks` below carves the same column off).
    let list_width = usize::from(rows[1].width.saturating_sub(1));
    let items: Vec<ListItem> = problems
        .rows
        .iter()
        .map(|row| match *row {
            ProblemRow::File { entry, count } => {
                let path = &problems.entries[entry].path;
                let name = path
                    .strip_prefix(&app.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned();
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {name} ")),
                    Span::styled(format!("({count})"), dim),
                ]))
            },
            // The severity mark leads, then the message, then the position:
            // the message yields to the position, which tells rows apart.
            ProblemRow::Item { entry } => {
                let diagnostic = &problems.entries[entry].diagnostic;
                let start = diagnostic.range.start;
                let location = format!(
                    "{}:{}",
                    start.line.saturating_add(1),
                    start.col.saturating_add(1)
                );
                let lead = format!("   {} ", severity_mark(diagnostic.severity));
                let spent =
                    karet_widgets::text::width(&lead) + karet_widgets::text::width(&location);
                let first_line = diagnostic.message.lines().next().unwrap_or_default();
                let message =
                    karet_widgets::text::fit_end(first_line, list_width.saturating_sub(spent + 1));
                let role = karet_core::severity_role(diagnostic.severity);
                ListItem::new(Line::from(vec![
                    Span::styled(lead, theme.style(role).add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{message} ")),
                    Span::styled(location, dim),
                ]))
            },
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(problems.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = problems.rows.len();
    let (results, tracks) = reserve_tracks(rows[1], ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.problems_ui.offset = state.offset();
    app.problems_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, state.offset(), results.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::ProblemResults,
    );
}
