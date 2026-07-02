//! Ratatui layout and drawing for the IDE shell: a tab strip, an optional
//! breadcrumb, a switchable sidebar (explorer / search / source-control), the main
//! content area (the active tab), and a status bar.

use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use karet_core::ThemeRole;
use karet_editor::Editor;
use karet_filetype::FileKind;
use karet_fileview::HexView;
use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::ImageWidget;
use karet_fileview::viewer::Placeholder;
use karet_theme::Theme;
use karet_vcs::StatusKind;
use karet_widgets::Corner;
use karet_widgets::FileTree;
use karet_widgets::Toasts;
use karet_widgets::UiIcon;
use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

use crate::app::App;
use crate::app::FindState;
use crate::app::TabHit;
use crate::app::ToastHit;
use crate::command::Command;
use crate::keymap::ChordStyle;
use crate::keymap::Context;
use crate::keymap::Focus;
use crate::keymap::SidebarPanel;
use crate::keymap::{self};
use crate::overlay::Overlay;
use crate::render::Section;
use crate::render::{self};
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;

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

    // Toasts float above everything, including the modal overlay.
    draw_toasts(f, app, &theme, area);
}

/// Draw the notification toast stack (bottom-right) and record each card's clickable
/// region for dismissal hit-testing. A no-op when there are no active notifications.
fn draw_toasts(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.toast_hits.clear();
    if app.notifications.is_empty() {
        return;
    }
    let notes = app.notifications.active();
    let toasts = Toasts {
        notifications: &notes,
        theme,
        corner: Corner::BottomRight,
    };
    for slot in toasts.layout(area) {
        app.toast_hits.push(ToastHit {
            rect: slot.rect,
            id: slot.id,
        });
    }
    f.render_widget(toasts, area);
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
        // The next/close chords are derived from the keymap's Find layer.
        let next = keymap::hint_for(Command::FindNext, ChordStyle::Verbose).unwrap_or_default();
        let close = keymap::hint_for(Command::FindCancel, ChordStyle::Verbose).unwrap_or_default();
        format!(
            " Find: {}   {}/{}   ({next} next · {close} close) ",
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
            },
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
        // A pre-allocated 1-cell status slot keeps the layout stable: `●` for
        // unsaved changes (a spinner frame while a slow save writes), else blank.
        let mark = save_mark(tab);
        let label = format!(" {mark} {} ", tab.title);
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

/// The 1-cell tab status mark: `●` for unsaved changes, else blank.
fn save_mark(tab: &Tab) -> char {
    if tab.dirty { '\u{25cf}' } else { ' ' }
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
            let icon_style = app.icon_style;
            // Highlight files open in tabs, with the active one emphasized.
            let open: Vec<PathBuf> = app
                .tabs
                .iter()
                .filter_map(|t| t.path().map(Path::to_path_buf))
                .collect();
            let active = app
                .tabs
                .get(app.active)
                .and_then(Tab::path)
                .map(Path::to_path_buf);
            let hover = app.hovered_explorer_row();
            f.render_stateful_widget(
                FileTree::new(&root)
                    .theme(theme)
                    .icons(icon_style)
                    .open(&open)
                    .active(active.as_deref())
                    .hover(hover),
                rows[1],
                &mut app.explorer,
            );
        },
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

    // The activity-bar switcher: an icon per panel. Each glyph occupies one cell
    // plus the space after it (2 cells), so the hit regions march in twos.
    let switch = cols[1];
    let active = app.sidebar_panel;
    let icon_style = app.icon_style;
    app.panel_hits = vec![
        (switch.x, switch.x + 2, SidebarPanel::Explorer),
        (switch.x + 2, switch.x + 4, SidebarPanel::Search),
        (switch.x + 4, switch.x + 6, SidebarPanel::SourceControl),
    ];
    let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let on = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let icon = |ui: UiIcon, panel: SidebarPanel| {
        Span::styled(
            format!("{} ", ui.glyph(icon_style)),
            if active == panel { on } else { hint },
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            icon(UiIcon::Explorer, SidebarPanel::Explorer),
            icon(UiIcon::Search, SidebarPanel::Search),
            icon(UiIcon::SourceControl, SidebarPanel::SourceControl),
        ])),
        switch,
    );
}

fn draw_scm(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    // Reserve a top row for the commit-message input while it is open.
    let list_area = if app.commit_input.is_some() {
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        draw_commit_input(f, app, theme, rows[0]);
        rows[1]
    } else {
        area
    };

    let selection_bg = theme.role(ThemeRole::Selection).to_ratatui();
    let hover_bg = theme.role(ThemeRole::HoverHighlight).to_ratatui();
    let hovered = app.hovered_scm_change();
    let cursor = app.scm.selection.cursor();
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
        if i == cursor {
            selected_row = items.len();
        }
        let (glyph, role) = status_glyph(change.status);
        let mut item = ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {glyph} "),
                Style::default().fg(theme.role(role).to_ratatui()),
            ),
            Span::raw(change.path.to_string_lossy().into_owned()),
        ]));
        // Every selected row (a contiguous range or a scattered toggle-set) gets the
        // selection background; the cursor row additionally gets the bold highlight
        // below. A hovered-but-unselected row gets the secondary hover accent.
        if app.scm.selection.is_selected(i) {
            item = item.style(Style::default().bg(selection_bg));
        } else if hovered == Some(i) {
            item = item.style(Style::default().bg(hover_bg));
        }
        items.push(item);
        row_map.push(Some(i));
    }

    // The commit-history log (lazily loaded), below the change sections. Its rows
    // are not selectable (row_map None); only the "load more" affordance is clickable.
    app.scm_more_row = None;
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    if !app.scm.log.is_empty() || app.scm.log_has_more {
        if !app.scm.changes.is_empty() {
            items.push(ListItem::new(Line::raw("")));
            row_map.push(None);
        }
        items.push(ListItem::new(Line::styled(" COMMITS", header_style)));
        row_map.push(None);
        let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
        for commit in &app.scm.log {
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", commit.short_hash), hash_style),
                Span::raw(commit.summary.clone()),
                Span::styled(format!("  {}", relative_time(commit.time)), dim),
            ])));
            row_map.push(None);
        }
        if app.scm.log_has_more {
            app.scm_more_row = Some(items.len());
            let label = if app.scm.log_loading {
                " loading…"
            } else {
                " ⋯ load more"
            };
            items.push(ListItem::new(Line::styled(label, dim)));
            row_map.push(None);
        }
    }

    if items.is_empty() {
        app.scm_row_map = Vec::new();
        app.scm_offset = 0;
        f.render_widget(Paragraph::new(Line::raw(" no changes")), list_area);
        return;
    }
    let mut state = ListState::default();
    // Highlight the selected change (auto-scrolls to it); with no changes, the log
    // is shown without a highlighted row.
    state.select((!app.scm.changes.is_empty()).then_some(selected_row));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(selection_bg)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, list_area, &mut state);
    app.scm_row_map = row_map;
    app.scm_offset = state.offset();
}

/// A terse `git log`-style relative time (e.g. `3d ago`) for a Unix timestamp.
fn relative_time(secs: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    let delta = now - secs;
    if delta < 0 {
        return "just now".to_string();
    }
    let (n, unit) = if delta < 60 {
        (delta, "s")
    } else if delta < 3600 {
        (delta / 60, "m")
    } else if delta < 86_400 {
        (delta / 3600, "h")
    } else if delta < 86_400 * 7 {
        (delta / 86_400, "d")
    } else if delta < 86_400 * 30 {
        (delta / (86_400 * 7), "w")
    } else if delta < 86_400 * 365 {
        (delta / (86_400 * 30), "mo")
    } else {
        (delta / (86_400 * 365), "y")
    };
    format!("{n}{unit} ago")
}

/// Draw the one-line commit-message input shown above the change list.
fn draw_commit_input(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let message = app.commit_input.as_deref().unwrap_or("");
    let line = Line::from(vec![
        Span::styled(
            " commit ",
            Style::default()
                .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(message.to_string()),
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    f.render_widget(Paragraph::new(line), area);
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
        },
        TabKind::Diff { file, view, scroll } => draw_diff(f, theme, area, file, *view, scroll),
        TabKind::Blame { groups, scroll, .. } => draw_blame(f, theme, area, groups, scroll),
        TabKind::Hex { bytes, scroll, .. } => {
            let rows = bytes.len().div_ceil(16);
            *scroll = (*scroll).min(rows.saturating_sub(1));
            f.render_widget(HexView::new(bytes).scroll(*scroll).theme(theme), area);
        },
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
        },
        TabKind::Placeholder {
            path,
            kind,
            dims,
            len,
        } => {
            let mut widget = Placeholder::new(path, *kind, *dims, *len);
            // A too-large file can be opened anyway; surface the override right on
            // the placeholder, with the chord read from the keymap so it can't drift.
            if matches!(kind, FileKind::TooLarge { .. })
                && let Some(chord) = keymap::hint_for(Command::OpenAnyway, ChordStyle::Verbose)
            {
                widget = widget.hint(format!("Press {chord} to open anyway"));
            }
            f.render_widget(widget, area);
        },
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
        },
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
        },
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

/// The commands shown on the empty-editor welcome screen, with descriptions. As in
/// the footer, only this selection and the prose are presentation — each chord is
/// derived from the keymap so the cheat-sheet can't drift from a rebinding.
const WELCOME_HINTS: &[(Command, &str)] = &[
    (Command::OpenQuickOpen, "go to file"),
    (Command::OpenCommandPalette, "command palette"),
    (Command::ToggleSidebar, "toggle sidebar"),
    (Command::OpenGlobalSearch, "search the workspace"),
    (Command::Copy, "copy selection"),
    (Command::ToggleFocus, "switch focus"),
    (Command::Quit, "quit"),
];

fn draw_welcome(f: &mut Frame, theme: &Theme, area: Rect) {
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let title = Style::default()
        .fg(theme.role(ThemeRole::Foreground).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let mut text = vec![Line::raw(""), Line::styled("  karet", title), Line::raw("")];
    for &(cmd, desc) in WELCOME_HINTS {
        let chord = keymap::hint_for(cmd, ChordStyle::Verbose).unwrap_or_default();
        text.push(Line::styled(format!("  {chord:<14}{desc}"), dim));
    }
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

/// The separator drawn between adjacent hints in the status bar.
const HINT_SEP: &str = " · ";

/// A single hint's segment text (`"^S save"`), used for both measuring and drawing.
fn hint_segment(hint: &keymap::Hint) -> String {
    format!("{} {}", hint.chord, hint.verb)
}

/// The terminal-cell width of `s` (display width, wide/combining aware — unlike a
/// raw `chars().count()`), via ratatui's own measurement so no extra dependency is
/// pulled in.
fn cell_width(s: &str) -> u16 {
    u16::try_from(Span::raw(s).width()).unwrap_or(u16::MAX)
}

/// How many leading `hints` fit in `avail` columns when joined by [`HINT_SEP`].
/// When some don't fit, room is reserved for a trailing ` +N` overflow marker (a
/// hint is dropped if the marker wouldn't otherwise fit). Pure, so it is unit-tested.
fn pack_hints(hints: &[keymap::Hint], avail: u16) -> usize {
    let sep = cell_width(HINT_SEP);
    let mut used = 0u16;
    let mut shown = 0usize;
    for (i, hint) in hints.iter().enumerate() {
        let seg = cell_width(&hint_segment(hint)) + if i == 0 { 0 } else { sep };
        if used + seg > avail {
            break;
        }
        used += seg;
        shown += 1;
    }
    // Reserve room for the ` +N` marker by dropping trailing hints until it fits.
    while shown < hints.len() && shown > 0 {
        let marker = cell_width(&format!(" +{}", hints.len() - shown));
        if used + marker <= avail {
            break;
        }
        shown -= 1;
        let seg = cell_width(&hint_segment(&hints[shown]));
        used -= seg + if shown == 0 { 0 } else { sep };
    }
    shown
}

/// Render `hints` into `spans` starting at column `*x`, packing what fits in `avail`
/// and appending a clickable ` +N` marker (opens the palette) for the rest. Each
/// shown hint records a clickable `(start, end, command)` region in `hits`.
fn render_hints(
    hints: &[keymap::Hint],
    spans: &mut Vec<Span<'static>>,
    hits: &mut Vec<(u16, u16, Command)>,
    x: &mut u16,
    avail: u16,
    bar: Style,
    key: Style,
) {
    let shown = pack_hints(hints, avail);
    for (i, hint) in hints.iter().take(shown).enumerate() {
        if i > 0 {
            spans.push(Span::styled(HINT_SEP.to_string(), bar));
            *x += cell_width(HINT_SEP);
        }
        let start = *x;
        spans.push(Span::styled(hint.chord.clone(), key));
        spans.push(Span::styled(format!(" {}", hint.verb), bar));
        *x += cell_width(&hint.chord) + 1 + cell_width(hint.verb);
        hits.push((start, *x, hint.command));
    }
    if shown < hints.len() {
        let marker = format!(" +{}", hints.len() - shown);
        let start = *x;
        *x += cell_width(&marker);
        spans.push(Span::styled(marker, bar));
        // The overflow marker opens the palette, so the hidden commands stay reachable.
        hits.push((start, *x, Command::OpenCommandPalette));
    }
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
    let key = bar.add_modifier(Modifier::BOLD);

    // The language label is a fixed-width right column; the hints get everything else.
    let language = app.tabs.get(app.active).map_or("", Tab::language);
    let right = format!(" {language} ");
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(cell_width(&right))])
        .split(area);
    let left = cols[0];

    // The focus chip, then a gutter, then the responsive hint region.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut x = left.x;
    let focus_text = format!(" {focus} ");
    let fw = cell_width(&focus_text);
    spans.push(Span::styled(focus_text, key));
    app.status_hits.push((x, x + fw, Command::ToggleFocus));
    x += fw;
    let gutter = "   ";
    spans.push(Span::styled(gutter.to_string(), bar));
    x += cell_width(gutter);
    let avail = left.width.saturating_sub(x - left.x);

    // Priority for the remaining space: an in-progress chord's completions, then any
    // transient message, then the active context's key hints — all keymap-derived.
    if !app.pending.is_empty() {
        let ctx = Context::focus(app.focus_target());
        let prefix = app
            .pending
            .iter()
            .map(|c| c.display(ChordStyle::Caret))
            .collect::<Vec<_>>()
            .join(" ");
        let comps = keymap::completions_for(ctx, &app.pending, ChordStyle::Caret);
        spans.push(Span::styled(prefix.clone(), key));
        spans.push(Span::styled(" → ".to_string(), bar));
        x += cell_width(&prefix) + cell_width(" → ");
        let rest = avail.saturating_sub(cell_width(&prefix) + cell_width(" → "));
        render_hints(
            &comps,
            &mut spans,
            &mut app.status_hits,
            &mut x,
            rest,
            bar,
            key,
        );
    } else if let Some(msg) = app.status.clone() {
        spans.push(Span::styled(format!("{msg} "), bar));
    } else {
        let hints = keymap::hints_for(app.input_context(), ChordStyle::Caret);
        render_hints(
            &hints,
            &mut spans,
            &mut app.status_hits,
            &mut x,
            avail,
            bar,
            key,
        );
    }

    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), left);
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
        StatusKind::Conflicted => ('!', ThemeRole::DiagnosticError),
        _ => ('•', ThemeRole::Foreground),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_hints_are_all_bound() {
        // Every welcome-screen command must resolve a chord from the keymap;
        // otherwise the cheat-sheet would silently drop it. The status bar's hints
        // are now enumerated from the keymap directly, so they can't drift.
        for &(cmd, _) in WELCOME_HINTS {
            assert!(
                keymap::hint_for(cmd, ChordStyle::Verbose).is_some(),
                "welcome command {cmd:?} has no keymap binding"
            );
        }
    }

    #[test]
    fn hint_bar_is_context_aware() {
        use crate::keymap::FocusTarget;
        let cmds = |ctx| {
            keymap::hints_for(ctx, ChordStyle::Caret)
                .iter()
                .map(|h| h.command)
                .collect::<Vec<_>>()
        };
        let editor = cmds(Context::focus(FocusTarget::Editor));
        let scm = cmds(Context::focus(FocusTarget::SourceControl));
        // The bar's command set follows the focused pane.
        assert!(editor.contains(&Command::Save));
        assert!(!editor.contains(&Command::ScmStage));
        assert!(scm.contains(&Command::ScmStage));
        assert!(!scm.contains(&Command::Save));
    }

    #[test]
    fn pack_hints_respects_width() {
        let hint = |chord: &str, command, verb| keymap::Hint {
            chord: chord.to_string(),
            command,
            verb,
        };
        let hints = vec![
            hint("^S", Command::Save, "save"),
            hint("^Z", Command::Undo, "undo"),
            hint("^C", Command::Copy, "copy"),
        ];
        // A wide bar shows everything; a zero-width bar shows nothing.
        assert_eq!(pack_hints(&hints, 100), 3);
        assert_eq!(pack_hints(&hints, 0), 0);
        // A narrow bar drops trailing hints (leaving room for the ` +N` marker).
        assert!(pack_hints(&hints, 12) < hints.len());
    }
}
