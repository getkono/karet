//! The Debug panel: call stack, lazily-fetched variables tree, evaluate log,
//! and the console tail (ANSI-styled via `karet_widgets::ansi`).

use super::*;
use crate::app::DebugRow;

/// Draw the Debug panel into `area`, recording the hit-test chrome.
pub(super) fn draw_debug_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let panel = &app.debug_panel;
    let output = &app.debug_output;

    let items: Vec<ListItem> = panel
        .rows
        .iter()
        .map(|row| match *row {
            DebugRow::Section(name) => ListItem::new(Line::styled(
                format!(" {name}"),
                dim.add_modifier(Modifier::BOLD),
            )),
            DebugRow::Note(text) => ListItem::new(Line::styled(format!("   {text}"), muted)),
            DebugRow::Frame(index) => {
                let Some(frame) = panel.stack.get(index) else {
                    return ListItem::new(Line::raw(""));
                };
                let marker = if panel.selected_frame == Some(frame.id) {
                    "▶"
                } else {
                    " "
                };
                let location = frame.path.as_deref().map_or(String::new(), |path| {
                    let file = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    format!("{file}:{}", frame.line.saturating_add(1))
                });
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {marker} "), Style::default().fg(accent)),
                    Span::raw(frame.name.clone()),
                    Span::styled(format!("  {location}"), dim),
                ]))
            },
            DebugRow::Scope(index) => {
                let Some(scope) = panel.scopes.get(index) else {
                    return ListItem::new(Line::raw(""));
                };
                let chevron = if panel.expanded.contains(&scope.reference) {
                    '▾'
                } else {
                    '▸'
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {chevron} "), dim),
                    Span::styled(scope.name.clone(), Style::default().fg(accent)),
                ]))
            },
            DebugRow::Variable {
                parent,
                index,
                depth,
            } => {
                let Some(variable) = panel
                    .variables
                    .get(&parent)
                    .and_then(|children| children.get(index))
                else {
                    return ListItem::new(Line::raw(""));
                };
                let indent = "  ".repeat(usize::from(depth) + 1);
                let chevron = if variable.reference == 0 {
                    ' '
                } else if panel.expanded.contains(&variable.reference) {
                    '▾'
                } else {
                    '▸'
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {indent}{chevron} "), dim),
                    Span::raw(variable.name.clone()),
                    Span::styled(" = ", dim),
                    Span::styled(variable.value.clone(), muted),
                ]))
            },
            DebugRow::Repl(index) => ListItem::new(Line::styled(
                format!("   {}", panel.repl.get(index).cloned().unwrap_or_default()),
                muted,
            )),
            DebugRow::Output(index) => {
                let Some((category, text)) = output.get(index) else {
                    return ListItem::new(Line::raw(""));
                };
                let mut spans = vec![Span::raw("   ")];
                let styled = karet_widgets::ansi::ansi_spans(text);
                if category == "stderr" && styled.iter().all(|span| span.style == Style::default())
                {
                    spans.push(Span::styled(
                        text.clone(),
                        theme.style(ThemeRole::DiagnosticError),
                    ));
                } else {
                    spans.extend(styled);
                }
                ListItem::new(Line::from(spans))
            },
        })
        .collect();

    if items.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled("  no debug session", dim)),
            area,
        );
        return;
    }
    let mut state = ListState::default();
    state.select(Some(panel.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = app.debug_panel.rows.len();
    let (results, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.debug_ui.offset = state.offset();
    app.debug_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, state.offset(), results.height as usize),
            ScrollExtent::default(),
        ),
        ScrollSurface::DebugResults,
    );
}
