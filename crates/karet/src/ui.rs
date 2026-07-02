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
use crate::app::MIN_SCM_REGION;
use crate::app::SIDEBAR_MIN_WIDTH;
use crate::app::TabDrag;
use crate::app::TabHit;
use crate::app::ToastHit;
use crate::command::Command;
use crate::keymap::ChordStyle;
use crate::keymap::Context;
use crate::keymap::Focus;
use crate::keymap::SidebarPanel;
use crate::keymap::{self};
use crate::overlay::Overlay;
use crate::render::{self};
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;

/// Draw one frame of the shell.
pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = app.theme.clone();
    let area = f.area();

    // Top level: the body (sidebar + panes) over a one-row status bar. Tab strips
    // and breadcrumbs now live *inside* each pane rather than spanning the top.
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let body = rows[0];

    let sidebar = if app.sidebar_visible {
        // Responsive clamp: the sidebar can grow to nearly the full width, but always
        // leaves one column for the drag divider. The stored width is written back so
        // a subsequent drag starts from what's actually shown after a terminal resize.
        let max = body.width.saturating_sub(1).max(1);
        let lo = SIDEBAR_MIN_WIDTH.min(max);
        let width = app.sidebar_width.clamp(lo, max);
        app.sidebar_width = width;
        let cols = Layout::horizontal([
            Constraint::Length(width),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(body);
        app.sidebar_rect = cols[0];
        app.sidebar_divider_x = cols[1].x;
        app.main_rect = cols[2];
        Some((cols[0], cols[1]))
    } else {
        app.sidebar_rect = Rect::default();
        app.sidebar_divider_x = 0;
        app.main_rect = body;
        None
    };

    if let Some((rect, divider)) = sidebar {
        draw_sidebar(f, app, &theme, rect);
        draw_sidebar_divider(f, &theme, divider, app.sidebar_resizing);
    }
    draw_panes(f, app, &theme, app.main_rect);
    draw_drop_preview(f, app, &theme);
    draw_status(f, app, &theme, rows[1]);

    if let Some(overlay) = &app.overlay {
        draw_overlay(f, overlay, &theme, area);
    }

    // Toasts float above everything, including the modal overlay.
    draw_toasts(f, app, &theme, area);
}

/// Immutable per-pane render inputs, bundled to keep the render helpers' signatures
/// small.
struct PaneCtx<'a> {
    theme: &'a Theme,
    graphics: GraphicsProtocol,
    /// Whether this pane holds the window focus (affects tab-strip styling).
    pane_focused: bool,
    /// Whether the editor should draw its caret as focused.
    editor_focused: bool,
    /// The find bar to draw atop this pane's content, if any (focused pane only).
    find: Option<&'a FindState>,
}

/// What a rendered pane reported back for hit-testing and image placement.
struct RenderedPane {
    tabstrip_rect: Rect,
    tab_hits: Vec<TabHit>,
    content_rect: Rect,
    image_area: Option<Rect>,
}

/// Draw every pane (tab strip + breadcrumb + content) tiled across `area`, recording
/// each pane's clickable regions for mouse routing.
fn draw_panes(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.pane_frames.clear();
    app.image_area = None;
    app.editor_rect = Rect::default();
    let focused = app.focus_pane();
    let editor_focused = app.focus == Focus::Editor;
    let graphics = app.graphics;
    for (pane, rect) in app.layout.layout(area) {
        let is_focused = pane == focused;
        let rendered = if is_focused {
            let ctx = PaneCtx {
                theme,
                graphics,
                pane_focused: true,
                editor_focused,
                find: app.find.as_ref(),
            };
            render_pane(f, &mut app.tabs, app.active, rect, &ctx)
        } else if let Some(stored) = app.stored.get_mut(&pane) {
            let ctx = PaneCtx {
                theme,
                graphics,
                pane_focused: false,
                editor_focused: false,
                find: None,
            };
            render_pane(f, &mut stored.tabs, stored.active, rect, &ctx)
        } else {
            continue;
        };
        if is_focused {
            app.editor_rect = rendered.content_rect;
            app.image_area = rendered.image_area;
        }
        app.pane_frames.push(crate::app::PaneFrame {
            pane,
            tabstrip_rect: rendered.tabstrip_rect,
            tab_hits: rendered.tab_hits,
            content_rect: rendered.content_rect,
        });
    }
}

/// Render one pane into `area`: its tab strip, optional breadcrumb, an optional find
/// bar (focused pane), and the active tab's content.
fn render_pane(
    f: &mut Frame,
    tabs: &mut [Tab],
    active: usize,
    area: Rect,
    ctx: &PaneCtx,
) -> RenderedPane {
    let has_path = tabs.get(active).is_some_and(|t| t.path().is_some());
    let bc = u16::from(has_path);
    let parts = Layout::vertical([
        Constraint::Length(1),  // tab strip
        Constraint::Length(bc), // breadcrumb (collapses when no path)
        Constraint::Min(0),     // content
    ])
    .split(area);
    let (tabstrip_rect, tab_hits) =
        draw_pane_tabs(f, tabs, active, ctx.pane_focused, ctx.theme, parts[0]);
    if bc == 1 {
        draw_pane_breadcrumb(f, tabs.get(active), ctx.theme, parts[1]);
    }
    let mut content = parts[2];
    if let Some(find) = ctx.find {
        // One row for find; a second when the replace field is shown.
        let want = if find.replace_visible { 2 } else { 1 };
        let h = want.min(content.height);
        let bar = Rect {
            height: h,
            ..content
        };
        draw_find_bar(f, find, ctx.theme, bar);
        content = Rect {
            y: content.y.saturating_add(h),
            height: content.height.saturating_sub(h),
            ..content
        };
    }
    let image_area = draw_pane_content(f, tabs, active, ctx, content);
    RenderedPane {
        tabstrip_rect,
        tab_hits,
        content_rect: content,
        image_area,
    }
}

/// While dragging a tab over another pane, tint the region it would land in (a half
/// for an edge split, the whole pane for a center move) — VS Code's drop preview.
fn draw_drop_preview(f: &mut Frame, app: &App, theme: &Theme) {
    let Some(TabDrag {
        hover: Some((pane, zone)),
        ..
    }) = app.tab_drag
    else {
        return;
    };
    let Some(frame) = app.pane_frames.iter().find(|fr| fr.pane == pane) else {
        return;
    };
    let preview = karet_widgets::drop_preview_rect(frame.content_rect, zone);
    f.render_widget(Clear, preview);
    f.render_widget(
        Block::default()
            .style(Style::default().bg(theme.role(ThemeRole::HoverHighlight).to_ratatui())),
        preview,
    );
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

/// Draw the find-in-file bar: a find row (query, match count, option toggles) and,
/// when the replace field is shown, a replace row. Mirrors the workspace Search
/// panel's model on the status-bar strip for a consistent find/replace experience.
fn draw_find_bar(f: &mut Frame, find: &FindState, theme: &Theme, area: Rect) {
    use crate::app::SearchField;

    let base = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.role(ThemeRole::LineNumber).to_ratatui();
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);

    // Find row.
    let editing_find = find.field == SearchField::Find;
    let count = if find.query.is_empty() {
        String::new()
    } else if find.count == 0 {
        "  no results".to_string()
    } else {
        format!("  {}/{}", find.current + 1, find.count)
    };
    let toggle = |label: &'static str, on: bool| {
        Span::styled(
            label,
            if on {
                base.fg(accent).add_modifier(Modifier::BOLD)
            } else {
                base.fg(dim)
            },
        )
    };
    let find_spans = vec![
        Span::styled(" Find: ", base),
        Span::styled(
            find.query.clone(),
            if editing_find { base.fg(accent) } else { base },
        ),
        Span::styled(if editing_find { "_" } else { "" }, base),
        Span::styled(count, base.fg(dim)),
        Span::styled("   ", base),
        toggle(".*", find.regex),
        Span::styled(" ", base),
        toggle("Aa", find.case_sensitive),
        Span::styled(" ", base),
        toggle("\\b", find.whole_word),
    ];
    f.render_widget(Paragraph::new(Line::from(find_spans)).style(base), rows[0]);

    // Replace row (only when shown and there is room).
    if find.replace_visible && rows[1].height >= 1 {
        let editing_replace = find.field == SearchField::Replace;
        let replace_spans = vec![
            Span::styled(" Repl: ", base),
            Span::styled(
                find.replace.clone(),
                if editing_replace {
                    base.fg(accent)
                } else {
                    base
                },
            ),
            Span::styled(if editing_replace { "_" } else { "" }, base),
            Span::styled(
                "   (Enter replace · Alt+Enter all · Tab find)",
                base.fg(dim),
            ),
        ];
        f.render_widget(
            Paragraph::new(Line::from(replace_spans)).style(base),
            rows[1],
        );
    }
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

/// Draw the vertical divider between the sidebar and the main area. It doubles as
/// the drag handle for resizing the sidebar; it brightens while a resize is active.
fn draw_sidebar_divider(f: &mut Frame, theme: &Theme, area: Rect, active: bool) {
    let role = if active {
        ThemeRole::LineNumberActive
    } else {
        ThemeRole::IndentGuide
    };
    let style = Style::default().fg(theme.role(role).to_ratatui());
    let buf = f.buffer_mut();
    for y in area.y..area.bottom() {
        buf.set_string(area.x, y, "\u{2502}", style); // │
    }
}

/// Draw a pane's tab strip and return its rect plus per-tab clickable regions. The
/// active tab is emphasized when the pane is focused, and muted when it is not, so
/// the focused pane reads clearly.
fn draw_pane_tabs(
    f: &mut Frame,
    tabs: &[Tab],
    active: usize,
    pane_focused: bool,
    theme: &Theme,
    area: Rect,
) -> (Rect, Vec<TabHit>) {
    let mut hits = Vec::new();
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, tab) in tabs.iter().enumerate() {
        let style = if i == active && pane_focused {
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if i == active {
            // Active tab of an unfocused pane: emphasized but not reversed.
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
                .add_modifier(Modifier::BOLD)
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
        hits.push(TabHit {
            start,
            end: x,
            close,
        });
    }
    let bar = Style::default().bg(theme.role(ThemeRole::Background).to_ratatui());
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
    (area, hits)
}

/// Braille spinner frames for a slow save (each is a single display cell).
const SPINNER: &[char] = &[
    '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280f}',
];
/// How long a save must run before its spinner appears.
const SPINNER_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
/// Milliseconds per spinner frame.
const SPINNER_FRAME_MS: u128 = 100;

/// The 1-cell tab status mark: a spinner while a slow save writes, `●` for unsaved
/// changes, else blank. The slot is always one cell so the layout never shifts.
fn save_mark(tab: &Tab) -> char {
    if let Some(since) = tab.saving_since {
        let elapsed = since.elapsed();
        if elapsed >= SPINNER_DELAY {
            let frame = (elapsed.as_millis() / SPINNER_FRAME_MS) as usize % SPINNER.len();
            return SPINNER[frame];
        }
    }
    if tab.dirty { '\u{25cf}' } else { ' ' }
}

fn draw_pane_breadcrumb(f: &mut Frame, tab: Option<&Tab>, theme: &Theme, area: Rect) {
    let crumbs = tab
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
            // The explorer highlight tracks which editors *show* a file, not which
            // are merely open: the focused pane's active tab is the strong "active"
            // marker; every other pane's active tab is a weaker "visible" marker.
            let active = app
                .tabs
                .get(app.active)
                .and_then(Tab::path)
                .map(Path::to_path_buf);
            let visible: Vec<PathBuf> = app
                .stored
                .values()
                .filter_map(|p| {
                    p.tabs
                        .get(p.active)
                        .and_then(Tab::path)
                        .map(Path::to_path_buf)
                })
                .collect();
            let explorer_focused =
                app.focus == Focus::Sidebar && app.sidebar_panel == SidebarPanel::Explorer;
            let hover = app.hovered_explorer_row();
            f.render_stateful_widget(
                FileTree::new(&root)
                    .theme(theme)
                    .icons(icon_style)
                    .visible(&visible)
                    .active(active.as_deref())
                    .explorer_focused(explorer_focused)
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
    // Header columns: a small "karet" brand at the far left, the panel title, an
    // optional Explorer toolbar, then the activity-bar switcher (7 cells). The
    // toolbar (Explorer only) and then the brand are dropped on a narrow sidebar so
    // the title and switcher always fit.
    const BRAND: &str = "karet";
    const BRAND_W: u16 = BRAND.len() as u16 + 1; // one leading space
    const ACTIONS_W: u16 = 8; // four buttons × 2 cells
    let icon_style = app.icon_style;
    let explorer = app.sidebar_panel == SidebarPanel::Explorer;
    let actions_w = if explorer && area.width >= 9 + ACTIONS_W + 7 {
        ACTIONS_W
    } else {
        0
    };
    let show_brand = area.width >= BRAND_W + 9 + actions_w + 7;
    let brand_w = if show_brand { BRAND_W } else { 0 };
    let cols = Layout::horizontal([
        Constraint::Length(brand_w),
        Constraint::Min(0),
        Constraint::Length(actions_w),
        Constraint::Length(7),
    ])
    .split(area);
    if show_brand {
        let brand_style = Style::default()
            .fg(theme.role(ThemeRole::Muted).to_ratatui())
            .add_modifier(Modifier::BOLD);
        f.render_widget(
            Paragraph::new(Line::styled(format!(" {BRAND}"), brand_style)),
            cols[0],
        );
    }
    let title = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::styled(format!(" {name}"), title)),
        cols[1],
    );

    // The Explorer toolbar (new file / new folder / refresh / collapse all), each
    // glyph occupying 2 cells; hit regions march in twos like the switcher.
    app.header_action_hits = Vec::new();
    if actions_w > 0 {
        let a = cols[2];
        let action_style = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
        let actions = [
            (UiIcon::NewFile, Command::ExplorerNewFile),
            (UiIcon::NewFolder, Command::ExplorerNewFolder),
            (UiIcon::Refresh, Command::ExplorerRefresh),
            (UiIcon::CollapseAll, Command::ExplorerCollapseAll),
        ];
        let mut spans = Vec::with_capacity(actions.len());
        for (i, (ui_icon, cmd)) in actions.into_iter().enumerate() {
            let x = a.x + i as u16 * 2;
            app.header_action_hits.push((x, x + 2, cmd));
            spans.push(Span::styled(
                format!("{} ", ui_icon.glyph(icon_style)),
                action_style,
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), a);
    }

    // The activity-bar switcher: an icon per panel. Each glyph occupies one cell
    // plus the space after it (2 cells), so the hit regions march in twos.
    let switch = cols[3];
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

    // A scrollable changes region on top; when there is commit history and room for
    // it, a resizable commit-log region pinned to the bottom with a drag divider.
    let has_log = !app.scm.log.is_empty() || app.scm.log_has_more;
    let (changes_area, commits_area) = if has_log && list_area.height > MIN_SCM_REGION * 2 + 1 {
        let commits_h = app.scm_commits_h.clamp(
            MIN_SCM_REGION,
            list_area.height.saturating_sub(MIN_SCM_REGION + 1),
        );
        let parts = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(commits_h),
        ])
        .split(list_area);
        app.scm_divider_y = parts[1].y;
        draw_scm_divider(f, theme, parts[1], app.scm_resizing);
        (parts[0], Some(parts[2]))
    } else {
        app.scm_divider_y = 0;
        (list_area, None)
    };

    draw_scm_changes(f, app, theme, changes_area);
    if let Some(commits_area) = commits_area {
        draw_scm_commits(f, app, theme, commits_area);
    } else {
        // No pinned region this frame: clear its state so stale hit-testing can't fire.
        app.scm_commits_rect = Rect::default();
        app.scm_commits_total = 0;
        app.scm_more_row = None;
    }
}

/// Draw the horizontal drag divider between the changes and commit-log regions. It
/// brightens while a resize is active (mirrors the sidebar-width divider).
fn draw_scm_divider(f: &mut Frame, theme: &Theme, area: Rect, active: bool) {
    let role = if active {
        ThemeRole::LineNumberActive
    } else {
        ThemeRole::IndentGuide
    };
    let style = Style::default().fg(theme.role(role).to_ratatui());
    let rule = "\u{2500}".repeat(area.width as usize); // ─
    f.render_widget(Paragraph::new(Line::styled(rule, style)), area);
}

/// Draw the changes region. Both the staged and working sections are always shown;
/// an empty section renders a greyed placeholder line rather than collapsing, so the
/// layout stays stable as files move between them.
fn draw_scm_changes(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let selection_bg = theme.role(ThemeRole::Selection).to_ratatui();
    let hover_bg = theme.role(ThemeRole::HoverHighlight).to_ratatui();
    let hovered = app.hovered_scm_change();
    let cursor = app.scm.selection.cursor();
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let placeholder_style = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let mut items: Vec<ListItem> = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();

    // Both sections are always drawn, in order. Each reserves at least one line — a
    // greyed placeholder when empty — so staging a single file (moving it between the
    // two sections) never makes a header appear or disappear and shift the layout.
    let staged = app.scm.staged_count;
    let total_changes = app.scm.changes.len();
    let sections = [
        ("STAGED CHANGES", "No staged changes", 0..staged),
        ("CHANGES", "No changes", staged..total_changes),
    ];
    for (label, empty_hint, range) in sections {
        items.push(ListItem::new(Line::styled(
            format!(" {label}"),
            header_style,
        )));
        row_map.push(None);
        if range.is_empty() {
            items.push(ListItem::new(Line::styled(
                format!("   {empty_hint}"),
                placeholder_style,
            )));
            row_map.push(None);
            continue;
        }
        for i in range {
            let change = &app.scm.changes[i];
            let (glyph, role) = status_glyph(change.status);
            // Filename front and centre; the parent directory trails in dim grey and
            // is omitted entirely for files at the repo root.
            let name = change.path.file_name().map_or_else(
                || change.path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            let parent = change
                .path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|p| !p.is_empty());
            let mut spans = vec![
                Span::styled(
                    format!(" {glyph} "),
                    Style::default().fg(theme.role(role).to_ratatui()),
                ),
                Span::raw(name),
            ];
            if let Some(parent) = parent {
                spans.push(Span::styled(
                    format!("  {parent}"),
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ));
            }
            let item = ListItem::new(Line::from(spans));
            // Every selected row (a contiguous range or a scattered toggle-set) gets
            // the selection background; the cursor row additionally gets a bold
            // highlight. A hovered-but-unselected row gets the secondary hover accent.
            let mut style = Style::default();
            if app.scm.selection.is_selected(i) {
                style = style.bg(selection_bg);
            } else if hovered == Some(i) {
                style = style.bg(hover_bg);
            }
            if i == cursor {
                style = style.add_modifier(Modifier::BOLD);
            }
            items.push(item.style(style));
            row_map.push(Some(i));
        }
    }

    app.scm_changes_rect = area;
    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_row_map = row_map;
    app.scm_offset = state.offset();
    app.scm_total_rows = total;
}

/// Draw the pinned commit-log region (header, lazily-loaded commits, "load more").
/// Its rows aren't selectable; only the "load more" affordance is clickable.
fn draw_scm_commits(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.scm_more_row = None;
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(" COMMITS", header_style))];
    for commit in &app.scm.log {
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!(" {} ", commit.short_hash), hash_style),
            Span::raw(commit.summary.clone()),
            Span::styled(format!("  {}", relative_time(commit.time)), dim),
        ])));
    }
    if app.scm.log_has_more {
        // The "load more" display row is relative to the commit region's top.
        app.scm_more_row = Some(items.len());
        let label = if app.scm.log_loading {
            " loading…"
        } else {
            " ⋯ load more"
        };
        items.push(ListItem::new(Line::styled(label, dim)));
    }

    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_commits_offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_commits_offset = state.offset();
    app.scm_commits_total = total;
    app.scm_commits_rect = area;
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
    use crate::app::SearchField;

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
    app.search_results_rect = rows[2];
    app.search_offset = 0;
    app.search_query_row = rows[0].y;
    app.search_replace_row = replace_visible.then_some(rows[1].y);
    app.search_action_hits = Vec::new();

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
    let find_cursor = if editing_find { "_" } else { "" };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!(" › {}{find_cursor}", app.search.query),
            find_style,
        )),
        find_cols[0],
    );
    let toggles = [
        (".*", app.search.regex, Command::SearchToggleRegex),
        ("Aa", app.search.case_sensitive, Command::SearchToggleCase),
        ("\\b", app.search.whole_word, Command::SearchToggleWord),
    ];
    let mut toggle_spans = Vec::with_capacity(toggles.len());
    for (i, (label, on, cmd)) in toggles.into_iter().enumerate() {
        let x = find_cols[1].x + i as u16 * 3;
        app.search_action_hits.push((x, x + 2, rows[0].y, cmd));
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
        let rep_cursor = if editing_replace { "_" } else { "" };
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(" ⇄ {}{rep_cursor}", app.search.replace),
                rep_style,
            )),
            rep_cols[0],
        );
        // "replace all" button, active only when there are results to replace.
        let has_results = !app.search.results.is_empty();
        let btn_style = if has_results {
            Style::default().fg(accent)
        } else {
            Style::default().fg(dim)
        };
        app.search_action_hits.push((
            rep_cols[1].x,
            rep_cols[1].x + SLOT_W,
            rows[1].y,
            Command::SearchReplaceAll,
        ));
        f.render_widget(
            Paragraph::new(Line::styled(" ⟳ all", btn_style)),
            rep_cols[1],
        );
    }

    let search = &app.search;
    if search.results.is_empty() {
        let hint = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
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
    f.render_stateful_widget(list, rows[2], &mut state);
    app.search_offset = state.offset();
}

/// Draw one pane's active tab into `area`. Returns the rect to reserve for a Kitty
/// image, if the active tab is an image on a Kitty terminal.
fn draw_pane_content(
    f: &mut Frame,
    tabs: &mut [Tab],
    active: usize,
    ctx: &PaneCtx,
    area: Rect,
) -> Option<Rect> {
    let theme = ctx.theme;
    let tab = tabs.get_mut(active)?;
    let mut image_area = None;
    match &mut tab.kind {
        TabKind::Welcome => draw_welcome(f, theme, area),
        TabKind::Code {
            buffer,
            highlights,
            folds,
            folded,
            decos,
            ..
        } => {
            let fold_lines = crate::app::resolve_folds(folds, folded);
            let editor = Editor::new(buffer)
                .highlights(highlights)
                .theme(theme)
                .decorations(decos)
                .folds(&fold_lines)
                .focused(ctx.editor_focused);
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
            if ctx.graphics == GraphicsProtocol::Kitty {
                // Reserve the area; the app flushes the Kitty escape after drawing.
                f.render_widget(
                    Block::default()
                        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
                    area,
                );
                image_area = Some(area);
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
    image_area
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
