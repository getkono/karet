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

use crate::app::{App, FindState, TabHit};
use crate::command::Command;
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
    let hints = overlay.row_hints();
    let selected = overlay.selected();
    let list_h = rows[1].height as usize;
    let width = rows[1].width as usize;
    let offset = selected.saturating_sub(list_h.saturating_sub(1));
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let items: Vec<ListItem> = labels
        .iter()
        .zip(hints.iter())
        .skip(offset)
        .take(list_h.max(1))
        .map(|(label, hint)| match hint {
            Some(h) => {
                let used = label.chars().count() + h.chars().count();
                let pad = width.saturating_sub(used).max(1);
                ListItem::new(Line::from(vec![
                    Span::raw((*label).to_string()),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(h.clone(), dim),
                ]))
            }
            None => ListItem::new(Line::raw((*label).to_string())),
        })
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

fn draw_tabs(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.tabstrip_rect = area;
    app.tab_hits.clear();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, tab) in app.tabs.iter().enumerate() {
        let style = if i == app.active {
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui())
        };
        let label = format!(" {} ", tab.title);
        let label_w = label.chars().count() as u16;
        let start = x;
        spans.push(Span::styled(label, style));
        spans.push(Span::styled("\u{00d7}", style)); // × close glyph
        spans.push(Span::styled(" ", style));
        let close = start + label_w;
        x = close + 2;
        app.tab_hits.push(TabHit {
            start,
            end: x,
            close,
        });
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
    app.sidebar_content_rect = rows[1];
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
        SidebarPanel::Search => draw_search_panel(f, app, theme, rows[1]),
    }
}

fn draw_sidebar_header(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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

    // The "1 2 3" panel switcher: each digit occupies its cell + the space after.
    let switch = cols[1];
    let active = app.sidebar_panel;
    app.panel_hits = vec![
        (switch.x, switch.x + 2, SidebarPanel::Explorer),
        (switch.x + 2, switch.x + 4, SidebarPanel::Search),
        (switch.x + 4, switch.x + 6, SidebarPanel::SourceControl),
    ];
    let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let on = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let digit = |d: &str, panel: SidebarPanel| {
        Span::styled(format!("{d} "), if active == panel { on } else { hint })
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            digit("1", SidebarPanel::Explorer),
            digit("2", SidebarPanel::Search),
            digit("3", SidebarPanel::SourceControl),
        ])),
        switch,
    );
}

fn draw_scm(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let mut items: Vec<ListItem> = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();
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
            row_map.push(None);
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
        row_map.push(Some(i));
    }
    if items.is_empty() {
        app.scm_row_map = Vec::new();
        app.scm_offset = 0;
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
    app.scm_row_map = row_map;
    app.scm_offset = state.offset();
}

fn draw_search_panel(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    app.search_results_rect = rows[1];
    app.search_offset = 0;
    let search = &app.search;

    // Query line: prefixed with a caret, highlighted while the input is active.
    let cursor = if search.input { "_" } else { "" };
    let query_style = if search.input {
        Style::default()
            .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui())
    };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!(" › {}{cursor}", search.query),
            query_style,
        )),
        rows[0],
    );

    if search.results.is_empty() {
        let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
        let msg = if search.query.is_empty() {
            "  type a query, Enter to search"
        } else {
            "  no results"
        };
        f.render_widget(Paragraph::new(Line::styled(msg, hint)), rows[1]);
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
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(search.selected));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, rows[1], &mut state);
    app.search_offset = state.offset();
}

fn draw_main(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.image_area = None;
    app.editor_rect = Rect::default();
    let active = app.active;
    let graphics = app.graphics;
    let focused = app.focus == Focus::Editor;
    if matches!(
        app.tabs.get(active).map(|t| &t.kind),
        Some(TabKind::Code { .. })
    ) {
        app.editor_rect = area;
    }
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
            let selection = tab.editor.selection_range();
            let editor = Editor::new(buffer)
                .highlights(highlights)
                .theme(theme)
                .decorations(decos)
                .selection(selection)
                .focused(focused);
            f.render_stateful_widget(editor, area, &mut tab.editor);
        }
        TabKind::Diff { file, view, scroll } => draw_diff(f, theme, area, file, *view, scroll),
        TabKind::Blame { groups, scroll, .. } => draw_blame(f, theme, area, groups, scroll),
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

/// Render the semantic-blame view: each commit group as a header (line range, short
/// hash, author, date) followed by its full commit message — the "why".
fn draw_blame(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    groups: &[blameline::BlameGroup],
    scroll: &mut u16,
) {
    let accent = Style::default().fg(theme.role(ThemeRole::DiffModified).to_ratatui());
    let range_style = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let body = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let subject = body.add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for group in groups {
        let range = if group.lines.start == group.lines.end {
            format!("line {}", group.lines.start)
        } else {
            format!("lines {}\u{2013}{}", group.lines.start, group.lines.end)
        };
        let date = group
            .date
            .split('T')
            .next()
            .unwrap_or(&group.date)
            .to_string();
        lines.push(Line::from(vec![
            Span::styled("\u{258c} ", accent),
            Span::styled(format!("{range:<13}"), range_style),
            Span::styled(format!("{}  ", group.short_hash()), accent),
            Span::styled(format!("{}  ", group.author), body),
            Span::styled(date, dim),
        ]));
        for (i, message_line) in group.message.lines().enumerate() {
            let style = if i == 0 { subject } else { dim };
            lines.push(Line::from(vec![
                Span::styled("\u{258c}   ", accent),
                Span::styled(message_line.to_string(), style),
            ]));
        }
        lines.push(Line::from(""));
    }

    let max = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(area.height);
    *scroll = (*scroll).min(max);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
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
        Line::styled("  Ctrl+C        copy selection", dim),
        Line::styled("  Tab switch focus     Ctrl+Q quit", dim),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

fn draw_status(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.status_rect = area;
    app.status_hits.clear();

    let focus = match app.focus {
        Focus::Sidebar => "SIDEBAR",
        Focus::Editor => "EDITOR",
    };
    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());

    // Build the left side as discrete, clickable segments, tracking columns.
    let mut spans = Vec::new();
    let mut x = area.x;
    let focus_text = format!(" {focus} ");
    let fw = focus_text.chars().count() as u16;
    spans.push(Span::styled(focus_text, bar.add_modifier(Modifier::BOLD)));
    app.status_hits.push((x, x + fw, Command::ToggleFocus));
    x += fw;

    if let Some(msg) = &app.status {
        spans.push(Span::styled(format!("  {msg} "), bar));
    } else {
        let segments = [
            ("^P open", Command::OpenQuickOpen),
            ("^F find", Command::OpenFind),
            ("^C copy", Command::Copy),
            ("^Q quit", Command::Quit),
        ];
        spans.push(Span::styled("   ".to_string(), bar));
        x += 3;
        for (i, (label, cmd)) in segments.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ".to_string(), bar));
                x += 3;
            }
            let w = label.chars().count() as u16;
            spans.push(Span::styled((*label).to_string(), bar));
            app.status_hits.push((x, x + w, *cmd));
            x += w;
        }
    }

    let language = app.tabs.get(app.active).map_or("", Tab::language);
    let right = format!(" {language} ");
    let cols = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(u16::try_from(right.len()).unwrap_or(0)),
    ])
    .split(area);
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), cols[0]);
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
