//! Ratatui layout and drawing: file list, diff panel (unified or side-by-side),
//! and the status bar (with the detected-language indicator in the corner).

use karet_core::ThemeRole;
use karet_theme::{Rgba, Theme};
use karet_vcs::{Selection, StatusKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, ViewMode};
use crate::render;

/// Draw one frame: file list (left), diff (right), status bar (bottom).
pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = app.theme.clone();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(f.area());
    let cols = Layout::horizontal([Constraint::Percentage(22), Constraint::Min(0)]).split(rows[0]);

    draw_file_list(f, app, &theme, cols[0]);
    draw_diff(f, app, &theme, cols[1]);
    draw_status(f, app, &theme, rows[1]);
}

fn draw_file_list(f: &mut Frame, app: &App, theme: &Theme, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .files
        .iter()
        .map(|fv| {
            let (glyph, role) = status_glyph(fv.change.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {glyph} "), fg(theme.role(role))),
                Span::raw(fv.change.path.to_string_lossy().into_owned()),
            ]))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.current));
    let list = List::new(items)
        .block(Block::new().borders(Borders::RIGHT).title("Changes"))
        .highlight_style(
            Style::default()
                .bg(theme.role(ThemeRole::Selection).to_ratatui())
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_diff(f: &mut Frame, app: &mut App, theme: &Theme, area: ratatui::layout::Rect) {
    let Some(file) = app.files.get(app.current) else {
        return;
    };
    match app.view {
        ViewMode::Unified => {
            let lines = render::unified_lines(file, theme);
            let max = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_sub(area.height);
            app.scroll = app.scroll.min(max);
            f.render_widget(Paragraph::new(lines).scroll((app.scroll, 0)), area);
        }
        ViewMode::SideBySide => {
            let (left, right) = render::side_by_side_lines(file, theme);
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
            f.render_widget(Paragraph::new(left).scroll((app.scroll, 0)), panes[0]);
            f.render_widget(Block::new().borders(Borders::LEFT), panes[1]);
            f.render_widget(Paragraph::new(right).scroll((app.scroll, 0)), panes[2]);
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, theme: &Theme, area: ratatui::layout::Rect) {
    let selection = match app.selection {
        Selection::Staged => "staged",
        Selection::Unstaged => "unstaged",
        _ => "changes",
    };
    let view = match app.view {
        ViewMode::Unified => "unified",
        ViewMode::SideBySide => "side-by-side",
    };
    let left = format!(
        " {selection}  {}/{}  {view}   q quit · Tab layout · j/k scroll · h/l file ",
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
        StatusKind::Untracked => ('?', ThemeRole::LineNumberActive),
        _ => ('•', ThemeRole::Foreground),
    }
}

fn fg(c: Rgba) -> Style {
    Style::default().fg(c.to_ratatui())
}
