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
    let panel = &app.debugger.panel;
    let output = &app.debugger.output;

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
                let location = frame.source_path.as_deref().map_or(String::new(), |path| {
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
                let chevron = if panel.expanded.contains(&scope.variables_reference) {
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
                let chevron = if !variable.variables_reference.has_children() {
                    ' '
                } else if panel.expanded.contains(&variable.variables_reference) {
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
                if *category == karet_core::DebugOutputCategory::Stderr
                    && styled.iter().all(|span| span.style == Style::default())
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
    let total = app.debugger.panel.rows.len();
    let (results, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.debugger.chrome.offset = state.offset();
    app.debugger.chrome.results_rect = results;
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

#[cfg(test)]
mod tests {
    use karet_core::DebugOutputCategory;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    /// The foreground of the first cell anywhere in `buffer` painting `symbol`.
    fn fg_of(buffer: &ratatui::buffer::Buffer, symbol: &str) -> Option<Color> {
        buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == symbol)
            .map(|cell| cell.fg)
    }

    #[test]
    fn plain_stderr_output_is_tinted_and_other_streams_are_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(dir.path().to_path_buf(), Vec::new(), Vec::new(), false);
        app.debugger.output.extend([
            (DebugOutputCategory::Stdout, "%".to_owned()),
            (DebugOutputCategory::Stderr, "#".to_owned()),
            (
                DebugOutputCategory::Other("stderr-ish".to_owned()),
                "@".to_owned(),
            ),
        ]);
        app.debugger.rebuild_rows();
        let theme = app.theme.clone();
        let mut terminal = Terminal::new(TestBackend::new(30, 12))?;
        terminal.draw(|frame| {
            let area = frame.area();
            draw_debug_panel(frame, &mut app, &theme, area, &mut ScrollHits::default());
        })?;
        let buffer = terminal.backend().buffer();
        let error = theme.style(ThemeRole::DiagnosticError).fg;
        assert!(error.is_some());
        assert_eq!(fg_of(buffer, "#"), error, "stderr takes the error tint");
        assert!(fg_of(buffer, "%").is_some());
        assert_ne!(fg_of(buffer, "%"), error, "stdout stays plain");
        assert_ne!(
            fg_of(buffer, "@"),
            error,
            "an unknown category is not stderr"
        );
        Ok(())
    }
}
