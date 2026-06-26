//! Ratatui layout and drawing for the IDE shell: a tab strip, an optional
//! breadcrumb, a switchable sidebar (explorer / search / source-control), the main
//! content area (the active tab), and a status bar.

use karet_core::ThemeRole;
use karet_editor::Editor;
use karet_theme::Theme;
use karet_vcs::StatusKind;
use karet_widgets::image::{GraphicsProtocol, ImageWidget};
use karet_widgets::viewer::Placeholder;
use karet_widgets::{FileTree, HexView};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, FindState};
use crate::keymap::{Focus, SidebarPanel};
use crate::overlay::Overlay;
use crate::render::{self, Section};
use crate::tab::{Tab, TabKind, ViewMode};

/// Draw one frame of the shell.
pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = app.theme.clone();
    let area = f.area();

    let has_path = app.tabs.get(app.active).is_some_and(|t| t.path().is_some());
    let breadcrumb_h = u16::from(has_path);
    let rows = Layout::vertical([
        Constraint::Length(1),            // tab strip
        Constraint::Length(breadcrumb_h), // breadcrumb (collapses when no path)
        Constraint::Min(0),               // body
        Constraint::Length(1),            // status bar
    ])
    .split(area);

    let sidebar = if app.sidebar_visible {
        let width = sidebar_width(area.width);
        let cols =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).split(rows[2]);
        app.sidebar_rect = cols[0];
        app.main_rect = cols[1];
        Some(cols[0])
    } else {
        app.sidebar_rect = Rect::default();
        app.main_rect = rows[2];
        None
    };

    draw_tabs(f, app, &theme, rows[0]);
    if breadcrumb_h == 1 {
        draw_breadcrumb(f, app, &theme, rows[1]);
    }
    if let Some(rect) = sidebar {
        draw_sidebar(f, app, &theme, rect);
    }
    let mut main = app.main_rect;
    if let Some(find) = &app.find {
        let bar = Rect {
            height: 1.min(main.height),
            ..main
        };
        draw_find_bar(f, find, &theme, bar);
        main = Rect {
            y: main.y.saturating_add(1),
            height: main.height.saturating_sub(1),
            ..main
        };
    }
    draw_main(f, app, &theme, main);
    draw_status(f, app, &theme, rows[3]);

    if let Some(overlay) = &app.overlay {
        draw_overlay(f, overlay, &theme, area);
    }
}

/// Draw the one-line find-in-file bar.
fn draw_find_bar(f: &mut Frame, find: &FindState, theme: &Theme, area: Rect) {
    let style = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let label = if find.query.is_empty() {
        " Find: ".to_string()
    } else if find.count == 0 {
        format!(" Find: {}   no results ", find.query)
    } else {
        format!(
            " Find: {}   {}/{}   (Enter/Ctrl+G next · Esc close) ",
            find.query,
            find.current + 1,
            find.count
        )
    };
    f.render_widget(Paragraph::new(label).style(style), area);
}

/// Draw a centered modal overlay (quick-open / command palette).
fn draw_overlay(f: &mut Frame, overlay: &Overlay, theme: &Theme, area: Rect) {
    let width = (u32::from(area.width) * 7 / 10).clamp(20, 80) as u16;
    let height = (u32::from(area.height) * 6 / 10).clamp(6, 18) as u16;
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let style = Style::default()
        .bg(theme.role(ThemeRole::Background).to_ratatui())
        .fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let block = Block::default()
        .borders(Borders::ALL)
        .title(overlay.title().to_string())
        .style(style);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let query = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    f.render_widget(
        Paragraph::new(Line::styled(format!("› {}", overlay.query()), query)),
        rows[0],
    );

    let labels = overlay.rows();
    let selected = overlay.selected();
    let list_h = rows[1].height as usize;
    let offset = selected.saturating_sub(list_h.saturating_sub(1));
    let items: Vec<ListItem> = labels
        .iter()
        .skip(offset)
        .take(list_h.max(1))
        .map(|l| ListItem::new(Line::raw((*l).to_string())))
        .collect();
    let mut state = ListState::default();
    state.select(Some(selected.saturating_sub(offset)));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, rows[1], &mut state);
}

/// A `width`×`height` rect centered within `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The sidebar width: 30 columns, capped at ~40% of a narrow terminal.
fn sidebar_width(total: u16) -> u16 {
    let cap = (total * 2 / 5).max(12);
    30.min(cap)
}

fn draw_tabs(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut spans = Vec::new();
    for (i, tab) in app.tabs.iter().enumerate() {
        let style = if i == app.active {
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui())
        };
        spans.push(Span::styled(format!(" {} ", tab.title), style));
    }
    let bar = Style::default().bg(theme.role(ThemeRole::Background).to_ratatui());
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

fn draw_breadcrumb(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let crumbs = app
        .tabs
        .get(app.active)
        .and_then(Tab::path)
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("  ›  ")
        })
        .unwrap_or_default();
    let style = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    f.render_widget(Paragraph::new(Line::styled(crumbs, style)), area);
}

fn draw_sidebar(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    draw_sidebar_header(f, app, theme, rows[0]);
    match app.sidebar_panel {
        SidebarPanel::Explorer => {
            let root = app.root.clone();
            f.render_stateful_widget(
                FileTree::new(&root).theme(theme),
                rows[1],
                &mut app.explorer,
            );
        }
        SidebarPanel::SourceControl => draw_scm(f, app, theme, rows[1]),
        SidebarPanel::Search => draw_search_panel(f, theme, rows[1]),
    }
}

fn draw_sidebar_header(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let name = match app.sidebar_panel {
        SidebarPanel::Explorer => "EXPLORER",
        SidebarPanel::Search => "SEARCH",
        SidebarPanel::SourceControl => "SOURCE CONTROL",
    };
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(7)]).split(area);
    let title = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {name}"), title)),
        cols[0],
    );
    let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    f.render_widget(Paragraph::new(Line::styled("1 2 3 ", hint)), cols[1]);
}

fn draw_scm(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row = 0;
    let mut last: Option<Section> = None;
    for (i, change) in app.scm.changes.iter().enumerate() {
        let section = if i < app.scm.staged_count {
            Section::Staged
        } else {
            Section::Working
        };
        if last != Some(section) {
            let label = match section {
                Section::Staged => "STAGED CHANGES",
                Section::Working => "CHANGES",
            };
            items.push(ListItem::new(Line::styled(
                format!(" {label}"),
                Style::default()
                    .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
                    .add_modifier(Modifier::BOLD),
            )));
            last = Some(section);
        }
        if i == app.scm.selected {
            selected_row = items.len();
        }
        let (glyph, role) = status_glyph(change.status);
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {glyph} "),
                Style::default().fg(theme.role(role).to_ratatui()),
            ),
            Span::raw(change.path.to_string_lossy().into_owned()),
        ])));
    }
    if items.is_empty() {
        f.render_widget(Paragraph::new(Line::raw(" no changes")), area);
        return;
    }
    let mut state = ListState::default();
    state.select(Some(selected_row));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_search_panel(f: &mut Frame, theme: &Theme, area: Rect) {
    let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled("  Press Ctrl+Shift+F to search", hint),
        ]),
        area,
    );
}

fn draw_main(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.image_area = None;
    let active = app.active;
    let graphics = app.graphics;
    let Some(tab) = app.tabs.get_mut(active) else {
        return;
    };
    match &mut tab.kind {
        TabKind::Welcome => draw_welcome(f, theme, area),
        TabKind::Code {
            buffer,
            highlights,
            decos,
            ..
        } => {
            let editor = Editor::new(buffer)
                .highlights(highlights)
                .theme(theme)
                .decorations(decos);
            f.render_stateful_widget(editor, area, &mut tab.editor);
        }
        TabKind::Diff { file, view, scroll } => draw_diff(f, theme, area, file, *view, scroll),
        TabKind::Hex { bytes, scroll, .. } => {
            let rows = bytes.len().div_ceil(16);
            *scroll = (*scroll).min(rows.saturating_sub(1));
            f.render_widget(HexView::new(bytes).scroll(*scroll).theme(theme), area);
        }
        TabKind::Image { image, .. } => {
            if graphics == GraphicsProtocol::Kitty {
                // Reserve the area; the app flushes the Kitty escape after drawing.
                f.render_widget(
                    Block::default()
                        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
                    area,
                );
                app.image_area = Some(area);
            } else {
                f.render_widget(ImageWidget::new(image), area);
            }
        }
        TabKind::Placeholder {
            path,
            kind,
            dims,
            len,
        } => {
            f.render_widget(Placeholder::new(path, *kind, *dims, *len), area);
        }
    }
}

fn draw_diff(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    file: &render::FileView,
    view: ViewMode,
    scroll: &mut u16,
) {
    match view {
        ViewMode::Unified => {
            let lines = render::unified_lines(file, theme);
            let max = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_sub(area.height);
            *scroll = (*scroll).min(max);
            f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
        }
        ViewMode::SideBySide => {
            let (left, right) = render::side_by_side_lines(file, theme);
            let height = left.len().max(right.len());
            let max = u16::try_from(height)
                .unwrap_or(u16::MAX)
                .saturating_sub(area.height);
            *scroll = (*scroll).min(max);
            let panes = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);
            f.render_widget(Paragraph::new(left).scroll((*scroll, 0)), panes[0]);
            f.render_widget(Block::new().borders(Borders::LEFT), panes[1]);
            f.render_widget(Paragraph::new(right).scroll((*scroll, 0)), panes[2]);
        }
    }
}

fn draw_welcome(f: &mut Frame, theme: &Theme, area: Rect) {
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let title = Style::default()
        .fg(theme.role(ThemeRole::Foreground).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let text = vec![
        Line::raw(""),
        Line::styled("  karet", title),
        Line::raw(""),
        Line::styled("  Ctrl+P        go to file", dim),
        Line::styled("  Ctrl+B        toggle sidebar", dim),
        Line::styled("  Ctrl+1/2/3    explorer · search · source control", dim),
        Line::styled("  Ctrl+Shift+F  search the workspace", dim),
        Line::styled("  Tab switch focus     q quit", dim),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

fn draw_status(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let focus = match app.focus {
        Focus::Sidebar => "SIDEBAR",
        Focus::Editor => "EDITOR",
    };
    let left = if let Some(msg) = &app.status {
        format!(" {focus}  {msg} ")
    } else {
        format!(" {focus}   ^P open · ^F find · ^B sidebar · q quit ")
    };
    let language = app.tabs.get(app.active).map_or("", Tab::language);
    let right = format!(" {language} ");

    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let cols = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(right.len()).unwrap_or(0)),
    ])
    .split(area);
    f.render_widget(Paragraph::new(left).style(bar), cols[0]);
    f.render_widget(
        Paragraph::new(right).style(bar).alignment(Alignment::Right),
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
