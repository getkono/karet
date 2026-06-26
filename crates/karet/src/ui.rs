//! Ratatui layout and drawing: file list, diff panel (unified or side-by-side),
//! and the status bar (with the detected-language indicator in the corner).

use karet_core::ThemeRole;
use karet_theme::{Rgba, Theme};
use karet_vcs::StatusKind;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, Pane, Section, ViewMode};
use crate::render;

/// Draw one frame: file list (left), diff (right), status bar (bottom).
pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = app.theme.clone();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());
    let cols = Layout::horizontal([Constraint::Percentage(22), Constraint::Min(0)]).split(rows[0]);

    // Retain the laid-out regions so the next mouse event can hit-test against them.
    app.regions.file_list = cols[0];
    app.regions.diff = cols[1];
    app.regions.status = rows[1];

    draw_file_list(f, app, &theme, cols[0]);
    draw_diff(f, app, &theme, cols[1]);
    draw_status(f, app, &theme, rows[1]);
}

fn draw_file_list(f: &mut Frame, app: &mut App, theme: &Theme, area: ratatui::layout::Rect) {
    let staged = app.staged_count();
    let working = app.files.len() - staged;

    // Build the list with a header before each non-empty group, tracking which display
    // row holds the focused file (headers are not entries in `app.files`). `rows` maps
    // each display row to its file index (or `None` for a group header) for click hit-tests.
    let mut items: Vec<ListItem> = Vec::new();
    let mut rows: Vec<Option<usize>> = Vec::new();
    let mut selected_row = 0usize;
    let mut last: Option<Section> = None;
    for (i, fv) in app.files.iter().enumerate() {
        if last != Some(fv.section) {
            let (label, count) = match fv.section {
                Section::Staged => ("STAGED CHANGES", staged),
                Section::Working => ("CHANGES", working),
            };
            items.push(section_header(label, count, theme));
            rows.push(None);
            last = Some(fv.section);
        }
        if i == app.current {
            selected_row = items.len();
        }
        rows.push(Some(i));
        let (glyph, role) = status_glyph(fv.change.status);
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!(" {glyph} "), fg(theme.role(role))),
            Span::raw(fv.change.path.to_string_lossy().into_owned()),
        ])));
    }

    let mut state = ListState::default();
    state.select(Some(selected_row));
    let list = List::new(items)
        .block(Block::new().borders(Borders::RIGHT))
        .highlight_style(
            Style::default()
                .bg(theme.role(ThemeRole::Selection).to_ratatui())
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, area, &mut state);
    app.regions.list_offset = state.offset();
    app.regions.list_rows = rows;
}

/// A bold, dimmed group header row ("STAGED CHANGES (2)") for the file list.
fn section_header(label: &str, count: usize, theme: &Theme) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        format!(" {label} ({count})"),
        Style::default()
            .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
            .add_modifier(Modifier::BOLD),
    )))
}

fn draw_diff(f: &mut Frame, app: &mut App, theme: &Theme, area: ratatui::layout::Rect) {
    let sel = app.selection_span();
    let sel_bg = theme.role(ThemeRole::Selection);
    app.regions.sbs = None; // default; the side-by-side arm overwrites it
    let Some(file) = app.files.get(app.current) else {
        return;
    };
    match app.view {
        ViewMode::Unified => {
            let mut lines = render::unified_lines(file, theme);
            if let Some((Pane::Unified, start, end)) = sel {
                render::apply_selection(&mut lines, start, end, sel_bg);
            }
            let max = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_sub(area.height);
            app.scroll = app.scroll.min(max);
            f.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), area);
        }
        ViewMode::SideBySide => {
            let (mut left, mut right) = render::side_by_side_lines(file, theme);
            match sel {
                Some((Pane::Left, start, end)) => {
                    render::apply_selection(&mut left, start, end, sel_bg);
                }
                Some((Pane::Right, start, end)) => {
                    render::apply_selection(&mut right, start, end, sel_bg);
                }
                _ => {}
            }
            let height = left.len().max(right.len());
            let max = u16::try_from(height)
                .unwrap_or(u16::MAX)
                .saturating_sub(area.height);
            app.scroll = app.scroll.min(max);
            let panes = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);
            app.regions.sbs = Some((panes[0], panes[2]));
            f.render_widget(Paragraph::new(left).scroll((app.scroll, 0)), panes[0]);
            f.render_widget(Block::new().borders(Borders::LEFT), panes[1]);
            f.render_widget(Paragraph::new(right).scroll((app.scroll, 0)), panes[2]);
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, theme: &Theme, area: ratatui::layout::Rect) {
    let staged = app.staged_count();
    let working = app.files.len() - staged;
    let view = match app.view {
        ViewMode::Unified => "unified",
        ViewMode::SideBySide => "side-by-side",
    };
    let left = format!(
        " {staged} staged · {working} changes  {}/{}  {view}   q quit · Tab layout · j/k scroll · h/l file ",
        app.current + 1,
        app.files.len()
    );
    let language = app
        .files
        .get(app.current)
        .map_or("plaintext", |fv| fv.language);
    let label = format!(" {language} ");

    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let cols = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(label.len()).unwrap_or(0)),
    ])
    .split(area);
    f.render_widget(Paragraph::new(left).style(bar), cols[0]);
    f.render_widget(
        Paragraph::new(label).style(bar).alignment(Alignment::Right),
        cols[1],
    );
}

/// The single-letter status glyph and its color role for a changed file.
fn status_glyph(kind: StatusKind) -> (char, ThemeRole) {
    match kind {
        StatusKind::Added => ('A', ThemeRole::DiffAdded),
        StatusKind::Modified => ('M', ThemeRole::DiagnosticWarning),
        StatusKind::Deleted => ('D', ThemeRole::DiagnosticError),
        StatusKind::Renamed => ('R', ThemeRole::DiagnosticInfo),
        StatusKind::Untracked => ('U', ThemeRole::DiffAdded),
        _ => ('•', ThemeRole::Foreground),
    }
}

fn fg(c: Rgba) -> Style {
    Style::default().fg(c.to_ratatui())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untracked_uses_vscode_u_badge() {
        let (glyph, _) = status_glyph(StatusKind::Untracked);
        assert_eq!(glyph, 'U');
    }
}
