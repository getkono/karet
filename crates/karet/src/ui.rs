//! Ratatui layout and drawing for the IDE shell: a tab strip, an optional
//! breadcrumb, a switchable sidebar (explorer / search / source-control), the main
//! content area (the active tab), and a status bar.

use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use karet_core::Decoration;
use karet_core::Severity;
use karet_core::ThemeRole;
use karet_editor::Editor;
use karet_filetype::FileKind;
use karet_fileview::HexView;
use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::Image;
use karet_fileview::image::ImageWidget;
use karet_fileview::image::fit_rect;
use karet_fileview::viewer::Placeholder;
use karet_graph::LaneInput;
use karet_graph::assign_lanes;
use karet_graph::view::render_rail;
use karet_markdown::WrappedDocument;
use karet_markdown::view::MarkdownView;
use karet_markdown::view::MarkdownViewState;
use karet_session::ConfigLayerStatus;
use karet_session::LoadedConfig;
use karet_text::TextBuffer;
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
use ratatui::style::Color;
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
use ratatui::widgets::Scrollbar;
use ratatui::widgets::ScrollbarOrientation;
use ratatui::widgets::ScrollbarState;
use ratatui::widgets::Wrap;

use crate::app::App;
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
use crate::tab::FindState;
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

    // Reserve a right-side column for the outline panel, carved from the main region.
    // Skipped on a terminal too narrow to keep the editor usable.
    if app.outline_visible && app.main_rect.width > app.outline_width + 8 {
        let region = app.main_rect;
        let cols = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(app.outline_width),
        ])
        .split(region);
        app.main_rect = cols[0];
        app.outline_rect = cols[2];
        draw_sidebar_divider(f, &theme, cols[1], false);
        draw_outline(f, app, &theme, cols[2]);
    } else {
        app.outline_rect = Rect::default();
        app.outline_content_rect = Rect::default();
    }

    draw_panes(f, app, &theme, app.main_rect);
    draw_drop_preview(f, app, &theme);
    draw_status(f, app, &theme, rows[1]);

    if let Some(overlay) = &app.overlay {
        draw_overlay(f, overlay, &theme, area);
    }
    if let Some(rev) = &app.rev_input {
        draw_rev_input(f, rev, &theme, area);
    }
    draw_explorer_context_menu(f, app, &theme, area);

    // Toasts float above everything, including the modal overlay.
    draw_toasts(f, app, &theme, area);
}

/// Draw the centered go-to-commit (revision) input prompt.
fn draw_rev_input(f: &mut Frame, rev: &str, theme: &Theme, area: Rect) {
    let width = area.width.clamp(20, 60);
    let rect = centered(area, width, 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Go to commit")
        .border_style(Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui()))
        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let line = Line::from(vec![
        Span::styled(
            "› ",
            Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
        ),
        Span::styled(
            rev.to_string(),
            Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui()),
        ),
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

/// Immutable per-pane render inputs, bundled to keep the render helpers' signatures
/// small.
struct PaneCtx<'a> {
    theme: &'a Theme,
    root: &'a Path,
    graphics: GraphicsProtocol,
    /// Whether this pane holds the window focus (affects tab-strip styling).
    pane_focused: bool,
    /// Whether the editor should draw its caret as focused.
    editor_focused: bool,
    /// Whether the app will draw a Kitty graphics caret after this frame.
    graphical_cursor: bool,
    /// The find bar to draw atop this pane's content, if any (focused pane only).
    /// Owned (not borrowed): it now lives on the active `Tab` itself, and
    /// `render_pane` needs a mutable borrow of the tabs slice at the same time.
    find: Option<FindState>,
}

/// What a rendered pane reported back for hit-testing and image placement.
struct RenderedPane {
    tabstrip_rect: Rect,
    tab_hits: Vec<TabHit>,
    content_rect: Rect,
    image_area: Option<Rect>,
    commit_badge_rect: Option<Rect>,
}

/// Geometry a tab's content reported for post-draw use: a reserved Kitty image rect
/// and the commit view's signature-badge rect (for double-click hit-testing).
#[derive(Default)]
struct PaneContent {
    image_area: Option<Rect>,
    badge_rect: Option<Rect>,
}

/// Draw every pane (tab strip + breadcrumb + content) tiled across `area`, recording
/// each pane's clickable regions for mouse routing.
fn draw_panes(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.pane_frames.clear();
    app.image_area = None;
    app.editor_rect = Rect::default();
    app.commit_badge_rect = None;
    let focused = app.focus_pane();
    let editor_focused = app.focus == Focus::Editor;
    let graphics = app.graphics;
    let graphical_cursor = app.graphical_cursor_enabled();
    for (pane, rect) in app.layout.layout(area) {
        let is_focused = pane == focused;
        let rendered = if is_focused {
            let ctx = PaneCtx {
                theme,
                root: &app.root,
                graphics,
                pane_focused: true,
                editor_focused,
                graphical_cursor,
                find: app
                    .find_open
                    .then(|| app.tabs.get(app.active))
                    .flatten()
                    .and_then(|t| t.find.clone()),
            };
            render_pane(f, &mut app.tabs, app.active, rect, &ctx)
        } else if let Some(stored) = app.stored.get_mut(&pane) {
            let ctx = PaneCtx {
                theme,
                root: &app.root,
                graphics,
                pane_focused: false,
                editor_focused: false,
                graphical_cursor: false,
                find: None,
            };
            render_pane(f, &mut stored.tabs, stored.active, rect, &ctx)
        } else {
            continue;
        };
        if is_focused {
            app.editor_rect = rendered.content_rect;
            app.image_area = rendered.image_area;
            app.commit_badge_rect = rendered.commit_badge_rect;
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
    let (tabstrip_rect, tab_hits) = draw_pane_tabs(
        f,
        tabs,
        active,
        ctx.pane_focused,
        ctx.theme,
        ctx.root,
        parts[0],
    );
    if bc == 1 {
        draw_pane_breadcrumb(f, tabs.get(active), ctx.theme, parts[1]);
    }
    let mut content = parts[2];
    if let Some(find) = ctx.find.as_ref() {
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
    let painted = draw_pane_content(f, tabs, active, ctx, content);
    RenderedPane {
        tabstrip_rect,
        tab_hits,
        content_rect: content,
        image_area: painted.image_area,
        commit_badge_rect: painted.badge_rect,
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
    use crate::tab::SearchField;

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
    root: &Path,
    area: Rect,
) -> (Rect, Vec<TabHit>) {
    let mut hits = Vec::new();
    let mut spans = Vec::new();
    let mut x = area.x;
    let titles = tab_display_titles(tabs, root);
    for (i, tab) in tabs.iter().enumerate() {
        let style = tab_text_style(theme, i == active, pane_focused, tab.is_preview);
        // A pre-allocated 1-cell status slot keeps the layout stable: `●` for
        // unsaved changes (a spinner frame while a slow save writes), else blank.
        let mark = save_mark(tab);
        let title = &titles[i];
        let label_w = (4 + title.prefix.chars().count() + title.name.chars().count()) as u16;
        let start = x;
        spans.push(Span::styled(format!(" {mark} "), style));
        if !title.prefix.is_empty() {
            spans.push(Span::styled(
                title.prefix.clone(),
                tab_prefix_style(theme, style, i == active, pane_focused),
            ));
        }
        spans.push(Span::styled(title.name.clone(), style));
        spans.push(Span::styled(" ", style));
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

fn tab_text_style(theme: &Theme, active: bool, pane_focused: bool, preview: bool) -> Style {
    let mut style = if active && pane_focused {
        Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if active {
        // Active tab of an unfocused pane: emphasized but not reversed.
        Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui())
    };
    // The preview tab (VS Code-style single-reused-slot tab) renders italicized so
    // it reads as provisional until edited or promoted.
    if preview {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

fn tab_prefix_style(theme: &Theme, base: Style, active: bool, pane_focused: bool) -> Style {
    let muted = theme.role(ThemeRole::Muted).to_ratatui();
    if active && pane_focused {
        base.remove_modifier(Modifier::REVERSED)
            .fg(muted)
            .bg(theme.role(ThemeRole::Foreground).to_ratatui())
    } else {
        base.fg(muted)
    }
}

struct TabDisplayTitle {
    prefix: String,
    name: String,
}

fn tab_display_titles(tabs: &[Tab], root: &Path) -> Vec<TabDisplayTitle> {
    tabs.iter()
        .map(|tab| {
            let name = tab_name(tab);
            let duplicate = tabs.iter().filter(|other| tab_name(other) == name).count() > 1;
            let prefix = if duplicate {
                tab.path().and_then(|path| tab_parent_prefix(path, root))
            } else {
                None
            }
            .unwrap_or_default();
            TabDisplayTitle { prefix, name }
        })
        .collect()
}

fn tab_name(tab: &Tab) -> String {
    tab.path()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map_or_else(|| tab.title.clone(), str::to_string)
}

fn tab_parent_prefix(path: &Path, root: &Path) -> Option<String> {
    let display = path.strip_prefix(root).unwrap_or(path);
    let parent = display.parent()?;
    let prefix = parent.to_string_lossy();
    (!prefix.is_empty()).then(|| format!("{prefix}/"))
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

/// The scale at which document (PDF) pages are rasterized. Larger than a typical
/// pane so the Kitty protocol downscales (sharp) rather than upscales into the
/// reserved cell box; 2.0 ≈ 144 DPI for a native 72-DPI page.
const DOC_RENDER_SCALE: f32 = 2.0;

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

/// Draw the right-side outline panel: a header over the active tab's navigation
/// outline (a depth-indented, selectable list). Records the content rect and syncs
/// the selection length for keyboard navigation and mouse hit-testing.
fn draw_outline(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let header = rows[0];
    let content = rows[1];
    app.outline_content_rect = content;

    f.render_widget(
        Block::default().style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
        area,
    );
    f.render_widget(
        Paragraph::new(" OUTLINE").style(
            Style::default()
                .fg(theme.role(ThemeRole::LineNumber).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        header,
    );

    let entries = app.active_outline_rows();
    app.outline_sel.set_len(entries.len());
    if entries.is_empty() {
        f.render_widget(
            Paragraph::new(" No outline")
                .style(Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())),
            content,
        );
        return;
    }

    let focused = app.focus == Focus::Outline;
    let cursor = app.outline_sel.cursor();
    let sel_bg = if focused {
        ThemeRole::Selection
    } else {
        ThemeRole::HoverHighlight
    };
    let items: Vec<ListItem> = entries
        .iter()
        .map(|row| {
            let indent = "  ".repeat(row.depth);
            ListItem::new(format!(" {indent}{}", row.label))
        })
        .collect();
    let list = List::new(items)
        .style(Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui()))
        .highlight_style(Style::default().bg(theme.role(sel_bg).to_ratatui()));
    let mut state = ListState::default();
    *state.offset_mut() = app.outline_scroll;
    state.select(Some(cursor));
    f.render_stateful_widget(list, content, &mut state);
    // Remember where the list settled so a click maps to the right entry next frame.
    app.outline_scroll = state.offset();
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
            let cut_paths = app.explorer_cut_paths().to_vec();
            f.render_stateful_widget(
                FileTree::new(&root)
                    .theme(theme)
                    .icons(icon_style)
                    .visible(&visible)
                    .active(active.as_deref())
                    .cut_paths(&cut_paths)
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

fn draw_explorer_context_menu(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(menu) = app.explorer_context_menu.as_mut() else {
        return;
    };
    if menu.items.is_empty() {
        menu.rect = Rect::default();
        return;
    }
    let hints: Vec<Option<String>> = menu
        .items
        .iter()
        .map(|cmd| keymap::hint_for(*cmd, ChordStyle::Verbose))
        .collect();
    let labels: Vec<&str> = menu
        .items
        .iter()
        .map(|cmd| context_menu_label(*cmd))
        .collect();
    let label_w = labels
        .iter()
        .map(|label| cell_width(label))
        .max()
        .unwrap_or(0);
    let hint_w = hints
        .iter()
        .flatten()
        .map(|hint| cell_width(hint))
        .max()
        .unwrap_or(0);
    let width = (label_w + hint_w + 6).clamp(18, 46).min(area.width.max(1));
    let height = (menu.items.len() as u16 + 2).min(area.height.max(1));
    let x = menu.x.min(area.right().saturating_sub(width));
    let y = menu.y.min(area.bottom().saturating_sub(height));
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    menu.rect = rect;
    f.render_widget(Clear, rect);
    let style = Style::default()
        .bg(theme.role(ThemeRole::Background).to_ratatui())
        .fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let block = Block::default()
        .borders(Borders::ALL)
        .style(style)
        .border_style(Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let items: Vec<ListItem> = labels
        .iter()
        .zip(hints.iter())
        .map(|(label, hint)| match hint {
            Some(hint) => {
                let used = cell_width(label) + cell_width(hint);
                let pad = inner.width.saturating_sub(used).max(1);
                ListItem::new(Line::from(vec![
                    Span::raw((*label).to_string()),
                    Span::raw(" ".repeat(pad as usize)),
                    Span::styled(hint.clone(), dim),
                ]))
            },
            None => ListItem::new(Line::raw((*label).to_string())),
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(menu.selected));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, inner, &mut state);
}

fn context_menu_label(command: Command) -> &'static str {
    match command {
        Command::SidebarActivate => "Open",
        Command::ExplorerNewFile => "New File",
        Command::ExplorerNewFolder => "New Folder",
        Command::ExplorerRename => "Rename",
        Command::ExplorerCopy => "Copy",
        Command::ExplorerCut => "Cut",
        Command::ExplorerPaste => "Paste",
        Command::ExplorerDuplicate => "Duplicate",
        Command::ExplorerDelete => "Delete",
        Command::ExplorerCopyPath => "Copy Path",
        Command::ExplorerCopyRelativePath => "Copy Relative Path",
        Command::ExplorerRefresh => "Refresh",
        Command::ExplorerCollapseAll => "Collapse All",
        _ => command.label(),
    }
}

fn draw_sidebar_header(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let name = match app.sidebar_panel {
        SidebarPanel::Explorer => "EXPLORER",
        SidebarPanel::Search => "SEARCH",
        SidebarPanel::SourceControl => "SOURCE CONTROL",
    };
    // Header columns: a compact workspace-root label, the panel title, an
    // optional Explorer toolbar, then the activity-bar switcher (7 cells). The
    // toolbar (Explorer only) and then the root are dropped on a narrow sidebar so
    // the title and switcher always fit.
    const ROOT_MAX_W: u16 = 24;
    const ACTIONS_W: u16 = 8; // four buttons × 2 cells
    let icon_style = app.icon_style;
    let explorer = app.sidebar_panel == SidebarPanel::Explorer;
    let actions_w = if explorer && area.width >= 9 + ACTIONS_W + 7 {
        ACTIONS_W
    } else {
        0
    };
    let min_title_w = 9;
    let root_avail = area
        .width
        .saturating_sub(min_title_w + actions_w + 7)
        .min(ROOT_MAX_W);
    let root_label = root_header_label(&app.root, root_avail.saturating_sub(1));
    let show_root = root_avail > 6 && !root_label.is_empty();
    let root_w = if show_root {
        cell_width(&root_label).saturating_add(1).min(root_avail)
    } else {
        0
    };
    let cols = Layout::horizontal([
        Constraint::Length(root_w),
        Constraint::Min(0),
        Constraint::Length(actions_w),
        Constraint::Length(7),
    ])
    .split(area);
    if show_root {
        let root_style = Style::default()
            .fg(theme.role(ThemeRole::Muted).to_ratatui())
            .add_modifier(Modifier::BOLD);
        f.render_widget(
            Paragraph::new(Line::styled(format!(" {root_label}"), root_style)),
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
            let hovered = header_hovered(app, x, x + 2);
            let state = if hovered {
                ChromeButtonState::Hovered
            } else {
                ChromeButtonState::Normal
            };
            spans.push(Span::styled(
                format!("{} ", ui_icon.glyph(icon_style)),
                chrome_button_style(theme, state),
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
    let icon = |ui: UiIcon, panel: SidebarPanel| {
        let hovered = app
            .panel_hits
            .iter()
            .any(|&(start, end, p)| p == panel && header_hovered(app, start, end));
        let state = match (active == panel, hovered) {
            (true, true) => ChromeButtonState::ActiveHovered,
            (true, false) => ChromeButtonState::Active,
            (false, true) => ChromeButtonState::Hovered,
            (false, false) => ChromeButtonState::Normal,
        };
        Span::styled(
            format!("{} ", ui.glyph(icon_style)),
            chrome_button_style(theme, state),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChromeButtonState {
    Normal,
    Hovered,
    Active,
    ActiveHovered,
}

fn chrome_button_style(theme: &Theme, state: ChromeButtonState) -> Style {
    match state {
        ChromeButtonState::Normal => {
            Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui())
        },
        ChromeButtonState::Hovered => {
            Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        },
        ChromeButtonState::Active => Style::default()
            .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
            .add_modifier(Modifier::BOLD),
        ChromeButtonState::ActiveHovered => Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .add_modifier(Modifier::BOLD),
    }
}

fn header_hovered(app: &App, start: u16, end: u16) -> bool {
    app.sidebar_header_hover
        .is_some_and(|(col, row)| row == app.sidebar_rect.y && col >= start && col < end)
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

fn root_header_label(root: &Path, max_width: u16) -> String {
    if max_width == 0 {
        return String::new();
    }
    let full = root.to_string_lossy();
    if cell_width(&full) <= max_width {
        return full.into_owned();
    }
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| full.as_ref());
    let compact = format!(".../{name}");
    if cell_width(&compact) <= max_width {
        return compact;
    }
    truncate_left(&full, max_width)
}

fn truncate_left(text: &str, max_width: u16) -> String {
    if max_width <= 3 {
        return ".".repeat(max_width as usize);
    }
    let suffix_width = max_width - 3;
    let mut suffix = String::new();
    let mut used = 0;
    for ch in text.chars().rev() {
        let mut buf = [0; 4];
        let w = cell_width(ch.encode_utf8(&mut buf));
        if used + w > suffix_width {
            break;
        }
        suffix.insert(0, ch);
        used += w;
    }
    format!("...{suffix}")
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
    // A small cycle of distinct colours so adjacent branch lanes read apart. Like
    // other git tools, lane colour is decorative, so it uses fixed terminal colours
    // rather than theme tokens.
    const LANE_COLORS: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
        Color::Red,
    ];
    let lane_style = |lane: u8| Style::default().fg(LANE_COLORS[lane as usize % LANE_COLORS.len()]);
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(" COMMITS", header_style))];

    // Lay the loaded commits out as a DAG: one rail gutter per row, drawn to the left
    // of the hash/summary/age columns. The newest loaded commit (row 0, page 0) is the
    // current tip. Parents beyond the loaded window simply leave their lane open.
    let inputs: Vec<LaneInput> = app
        .scm
        .log
        .iter()
        .enumerate()
        .map(|(i, c)| LaneInput {
            id: c.hash.clone(),
            parents: c.parents.clone(),
            head: i == 0 && app.scm_commits_offset == 0,
        })
        .collect();
    let rails = assign_lanes(&inputs);
    for (commit, rail) in app.scm.log.iter().zip(rails.iter()) {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(render_rail(rail, lane_style).spans);
        spans.push(Span::styled(format!(" {} ", commit.short_hash), hash_style));
        spans.push(Span::raw(commit.summary.clone()));
        spans.push(Span::styled(
            format!("  {}", relative_time(commit.time)),
            dim,
        ));
        items.push(ListItem::new(Line::from(spans)));
    }
    if app.scm.log_has_more {
        // The "load more" display row is relative to the commit region's top.
        app.scm_more_row = Some(items.len());
        let label = if app.scm.log_loading_since.is_some_and(loading_visible) {
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
    use crate::tab::SearchField;

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
) -> PaneContent {
    let theme = ctx.theme;
    let Some(tab) = tabs.get_mut(active) else {
        return PaneContent::default();
    };
    let mut image_area = None;
    let mut badge_rect = None;
    match &mut tab.kind {
        TabKind::Welcome => draw_welcome(f, theme, area),
        TabKind::Code {
            buffer,
            highlights,
            folds,
            folded,
            decos,
            search_decos,
            ..
        } => {
            let fold_lines = crate::app::resolve_folds(folds, folded);
            // Local find and global search highlights are kept in separate
            // fields (so closing/rerunning one can't wipe the other) and
            // combined only here, at render time.
            let combined: Vec<Decoration> =
                decos.iter().chain(search_decos.iter()).cloned().collect();
            let editor = Editor::new(buffer)
                .highlights(highlights)
                .theme(theme)
                .decorations(&combined)
                .folds(&fold_lines)
                .focused(ctx.editor_focused)
                .cell_caret(!ctx.graphical_cursor);
            f.render_stateful_widget(editor, area, &mut tab.editor);
        },
        TabKind::MarkdownPreview {
            buffer,
            wrapped,
            rendered,
            scroll,
            ..
        } => draw_markdown_preview(f, theme, area, buffer, wrapped, rendered, scroll),
        TabKind::Diff { file, view, scroll } => draw_diff(f, theme, area, file, *view, scroll),
        TabKind::Blame { groups, scroll, .. } => draw_blame(f, theme, area, groups, scroll),
        TabKind::Graph {
            title,
            view,
            scroll,
        } => draw_graph(f, theme, area, title, view, scroll),
        TabKind::LoadedConfig { report, scroll } => {
            draw_loaded_config(f, theme, area, report, scroll);
        },
        TabKind::Commit {
            detail,
            files,
            files_loading_since,
            files_error,
            verification,
            explain_since,
            scroll,
        } => {
            badge_rect = draw_commit(
                f,
                theme,
                area,
                detail,
                files,
                file_load_status(*files_loading_since, files_error.as_deref()),
                verification.as_ref(),
                *explain_since,
                scroll,
            );
        },
        TabKind::CommitLoading {
            rev,
            loading_since,
            error,
            scroll,
        } => draw_commit_loading(
            f,
            theme,
            area,
            rev,
            *loading_since,
            error.as_deref(),
            scroll,
        ),
        TabKind::Compare {
            base_label,
            head_label,
            merge_base,
            files,
            scroll,
        } => draw_compare(
            f,
            theme,
            area,
            base_label,
            head_label,
            *merge_base,
            files,
            scroll,
        ),
        TabKind::CommitGraph {
            history_path: _,
            commits,
            has_more,
            loading,
            loading_since,
            selected,
            detail_loading_since,
            detail,
            files,
            files_loading_since,
            files_error,
            verification,
            compare_base: _,
            list_offset,
        } => draw_commit_graph(
            f,
            theme,
            area,
            commits,
            *has_more,
            *loading,
            *loading_since,
            *selected,
            *detail_loading_since,
            detail.as_deref(),
            files,
            file_load_status(*files_loading_since, files_error.as_deref()),
            verification.as_ref(),
            list_offset,
        ),
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
        TabKind::Document {
            path,
            doc,
            page_count,
            page,
            rendered,
            ..
        } => {
            let page_count = (*page_count).max(1);
            let idx = (*page).min(page_count - 1);
            *page = idx;
            if ctx.graphics == GraphicsProtocol::Kitty {
                // Rasterize the current page unless it is already cached.
                if !matches!(rendered.as_ref(), Some((i, _)) if *i == idx) {
                    *rendered = doc.render_page(idx, DOC_RENDER_SCALE).ok().map(|p| {
                        let (w, h) = (p.width(), p.height());
                        (idx, Image::from_rgba(p.into_rgba(), w, h))
                    });
                }
                // Paint the pane background so nothing shows through the page margins.
                f.render_widget(
                    Block::default()
                        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
                    area,
                );
                // Reserve a one-row footer for the page indicator and a one-column
                // vertical scroll bar (page position), each only when there is room.
                let footer_h = u16::from(page_count > 1 && area.height > 3);
                let scrollbar_w = u16::from(page_count > 1 && area.width > 3);
                let content = Rect {
                    width: area.width - scrollbar_w,
                    height: area.height - footer_h,
                    ..area
                };
                if let Some((_, img)) = rendered.as_ref() {
                    // Reserve an aspect-fit sub-rect so the page is not stretched.
                    image_area = Some(fit_rect(content, img.width(), img.height()));
                } else {
                    // Parsed, but this page failed to rasterize — show a neutral note.
                    f.render_widget(Placeholder::new(path, FileKind::Pdf, None, 0), content);
                }
                if scrollbar_w == 1 {
                    // The scroll bar tracks the current page's position in the document.
                    let track = Rect {
                        x: area.x + area.width - 1,
                        y: area.y,
                        width: 1,
                        height: area.height - footer_h,
                    };
                    let mut sb = ScrollbarState::new(page_count).position(idx);
                    f.render_stateful_widget(
                        Scrollbar::new(ScrollbarOrientation::VerticalRight)
                            .begin_symbol(None)
                            .end_symbol(None)
                            .track_style(
                                Style::default()
                                    .fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
                            )
                            .thumb_style(
                                Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui()),
                            ),
                        track,
                        &mut sb,
                    );
                }
                if footer_h == 1 {
                    let footer = Rect {
                        y: area.y + area.height - 1,
                        height: 1,
                        ..area
                    };
                    f.render_widget(
                        Paragraph::new(format!(
                            "Page {} / {}   ·   PgDn / PgUp",
                            idx + 1,
                            page_count
                        ))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui())),
                        footer,
                    );
                }
            } else {
                // No Kitty graphics: attribute the limitation to the terminal.
                f.render_widget(Placeholder::requires_kitty(path), area);
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
    PaneContent {
        image_area,
        badge_rect,
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

/// Draw the commit view (metadata header + changed-file list + per-file diffs) as one
/// scrollable paragraph.
#[allow(clippy::too_many_arguments)] // a commit view has several independent inputs
fn draw_commit(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    detail: &karet_vcs::CommitDetail,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    verification: Option<&karet_session::GithubVerification>,
    explain_since: Option<Instant>,
    scroll: &mut u16,
) -> Option<Rect> {
    let reveal = explain_since.is_some_and(|t| t.elapsed() < crate::app::COMMIT_REVEAL);
    let (lines, badge) = commit_detail_lines(
        theme,
        detail,
        files,
        file_status,
        verification,
        reveal,
        area.width,
    );
    let max = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(area.height);
    *scroll = (*scroll).min(max);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
    // Translate the badge's content-row into a screen rect for click hit-testing,
    // dropping it when scrolled out of view.
    badge.and_then(|b| {
        let row = b.line.checked_sub(*scroll)?;
        (row < area.height).then_some(Rect {
            x: area.x.saturating_add(b.col),
            y: area.y.saturating_add(row),
            width: b.width,
            height: 1,
        })
    })
}

fn draw_commit_loading(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    rev: &str,
    loading_since: Instant,
    error: Option<&str>,
    scroll: &mut u16,
) {
    *scroll = 0;
    if error.is_none() && !loading_visible(loading_since) {
        f.render_widget(
            Block::default()
                .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
            area,
        );
        return;
    }
    let title = Style::default()
        .fg(theme.role(ThemeRole::Foreground).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let error_style = Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui());
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let short = rev.chars().take(12).collect::<String>();
    let lines = if let Some(error) = error {
        vec![
            Line::styled(" Could not load commit", title),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(short, hash_style),
                Span::styled("  ", muted),
                Span::styled(error.to_string(), error_style),
            ]),
        ]
    } else {
        vec![
            Line::styled(" Loading commit", title),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(short, hash_style),
                Span::styled(" details and file changes…", muted),
            ]),
        ]
    };
    f.render_widget(Paragraph::new(lines), area);
}

/// Draw the compare view (a range header + changed-file cards) as one scrollable
/// paragraph, reusing the commit view's file rendering.
#[allow(clippy::too_many_arguments)] // a range view has several independent inputs
fn draw_compare(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
    files: &[render::FileView],
    scroll: &mut u16,
) {
    let lines = compare_lines(theme, base_label, head_label, merge_base, files, area.width);
    let max = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(area.height);
    *scroll = (*scroll).min(max);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
}

/// Build the compare view's scrollable lines: a range header, then the shared
/// changed-files block ([`changed_files_lines`]).
fn compare_lines(
    theme: &Theme,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
    files: &[render::FileView],
    width: u16,
) -> Vec<Line<'static>> {
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(" Comparing ", fg.add_modifier(Modifier::BOLD)),
        Span::styled(base_label.to_string(), hash_style),
        Span::styled(if merge_base { " \u{2026} " } else { " .. " }, muted),
        Span::styled(head_label.to_string(), hash_style),
    ]));
    lines.push(Line::styled(
        format!(
            "  {}",
            if merge_base {
                "changes since the two diverged (merge base)"
            } else {
                "changes from the first to the second"
            }
        ),
        label,
    ));
    lines.extend(changed_files_lines(theme, files, width));
    lines
}

/// Where the signature badge sits within the commit view's line list, so a click can
/// be hit-tested against it: its row index and horizontal column span.
#[derive(Clone, Copy)]
struct BadgeHit {
    /// Row index into the commit view's line list (before scrolling).
    line: u16,
    /// First column of the badge glyph/label, relative to the render area's left.
    col: u16,
    /// The badge's width in columns (glyph + label).
    width: u16,
}

/// A short, plain-language explanation of what the signature badge means, keyed on the
/// same four states as [`verified_badge`]. Revealed under the badge on a double-click.
fn badge_explanation(
    verification: Option<&karet_session::GithubVerification>,
    signature: Option<&karet_vcs::CommitSignature>,
) -> &'static [&'static str] {
    match verification {
        Some(v) if v.verified => &[
            "Verified \u{2014} a key the forge trusts for this author signed the",
            "commit and the forge confirmed it, proving who wrote it.",
        ],
        Some(_) => &[
            "Unverified \u{2014} this commit is signed, but the forge could not",
            "confirm the signature (see the reason on the signature line below).",
        ],
        None if signature.is_some() => &[
            "Signed \u{2014} this commit carries a cryptographic signature, but it",
            "has not been checked with the forge, so its authenticity is unconfirmed.",
        ],
        None => &[
            "Unsigned \u{2014} no signature is attached, so the author cannot be",
            "cryptographically confirmed beyond the recorded name and email.",
        ],
    }
}

/// The commit's signature badge as `(glyph, label, role)`. Prefers the forge's verdict
/// once fetched; otherwise reports only what the local object records ("Signed" /
/// "Unsigned"), never claiming a verification result the tool did not compute.
fn verified_badge(
    verification: Option<&karet_session::GithubVerification>,
    signature: Option<&karet_vcs::CommitSignature>,
) -> (&'static str, &'static str, ThemeRole) {
    match verification {
        Some(v) if v.verified => ("\u{2714}", "Verified", ThemeRole::VcsVerified),
        Some(_) => ("\u{26a0}", "Unverified", ThemeRole::VcsUnverified),
        None if signature.is_some() => ("\u{25cf}", "Signed", ThemeRole::Foreground),
        None => ("", "Unsigned", ThemeRole::Muted),
    }
}

/// Format a Unix timestamp (with its timezone `offset` in seconds) as
/// `YYYY-MM-DD HH:MM`, without pulling in a date library (civil-from-days).
fn format_datetime(secs: i64, offset: i32) -> String {
    let t = secs + i64::from(offset);
    let days = t.div_euclid(86_400);
    let tod = t.rem_euclid(86_400);
    let (hour, minute) = (tod / 3600, (tod % 3600) / 60);
    // Howard Hinnant's civil_from_days: days since 1970-01-01 -> (y, m, d).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Build the commit view's scrollable lines. Shared by the standalone [`TabKind::Commit`]
/// tab and the graph browser's detail pane.
/// When `reveal` is set, the signature badge's explanation is inserted under the badge
/// (a transient tooltip). The returned [`BadgeHit`], if any, locates the badge for
/// click hit-testing.
#[derive(Clone, Copy)]
enum CommitFileStatus<'a> {
    Ready,
    Loading(Instant),
    Failed(&'a str),
}

fn file_load_status(loading_since: Option<Instant>, error: Option<&str>) -> CommitFileStatus<'_> {
    if let Some(error) = error {
        CommitFileStatus::Failed(error)
    } else if let Some(since) = loading_since {
        CommitFileStatus::Loading(since)
    } else {
        CommitFileStatus::Ready
    }
}

#[allow(clippy::too_many_arguments)] // commit metadata, file state, badge state, and width are independent
fn commit_detail_lines(
    theme: &Theme,
    detail: &karet_vcs::CommitDetail,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    verification: Option<&karet_session::GithubVerification>,
    reveal: bool,
    width: u16,
) -> (Vec<Line<'static>>, Option<BadgeHit>) {
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let subject = fg.add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let accent = Style::default().fg(theme.role(ThemeRole::DiffModified).to_ratatui());
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let bar = || Span::styled("\u{258c} ", accent);

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Subject + body.
    lines.push(Line::styled(format!(" {}", detail.summary), subject));
    if !detail.body.is_empty() {
        lines.push(Line::raw(""));
        for l in detail.body.lines() {
            lines.push(Line::styled(format!(" {l}"), muted));
        }
    }
    lines.push(Line::raw(""));

    // Commit hash + verified badge.
    let (glyph, badge, badge_role) = verified_badge(verification, detail.signature.as_ref());
    let badge_style = Style::default()
        .fg(theme.role(badge_role).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let mut hash_spans = vec![
        bar(),
        Span::styled(format!("{:<10} ", "commit"), label),
        Span::styled(detail.hash.clone(), hash_style),
        Span::raw("   "),
    ];
    // The badge's row and column span, derived from the spans already on the line so
    // hit-testing can't drift from the layout. The badge starts after everything built
    // above (bar + label + hash + gap); its width is the glyph (with a space) + label.
    let badge_col: usize = hash_spans.iter().map(|s| s.content.chars().count()).sum();
    let badge_width = if glyph.is_empty() {
        0
    } else {
        glyph.chars().count() + 1
    } + badge.chars().count();
    let badge_hit = BadgeHit {
        line: u16::try_from(lines.len()).unwrap_or(u16::MAX),
        col: u16::try_from(badge_col).unwrap_or(u16::MAX),
        width: u16::try_from(badge_width).unwrap_or(u16::MAX),
    };
    if !glyph.is_empty() {
        hash_spans.push(Span::styled(format!("{glyph} "), badge_style));
    }
    hash_spans.push(Span::styled(badge, badge_style));
    lines.push(Line::from(hash_spans));

    // On a double-click of the badge, reveal its meaning right beneath it.
    if reveal {
        for text in badge_explanation(verification, detail.signature.as_ref()) {
            lines.push(Line::from(vec![
                bar(),
                Span::styled((*text).to_string(), muted),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Author, and committer only when it differs.
    let ident_line = |role_label: &str, id: &karet_vcs::Identity, verb: &str| {
        Line::from(vec![
            bar(),
            Span::styled(format!("{role_label:<10} "), label),
            Span::styled(format!("{} <{}>", id.name, id.email), fg),
            Span::styled(
                format!("   {verb} {}", format_datetime(id.time, id.offset)),
                dim,
            ),
        ])
    };
    lines.push(ident_line("author", &detail.author, "authored"));
    if detail.committer.name != detail.author.name
        || detail.committer.email != detail.author.email
        || detail.committer.time != detail.author.time
    {
        lines.push(ident_line("committer", &detail.committer, "committed"));
    }

    // Parents.
    if !detail.parents.is_empty() {
        let mut spans = vec![bar(), Span::styled(format!("{:<10} ", "parents"), label)];
        for (i, p) in detail.parents.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                p.chars().take(7).collect::<String>(),
                hash_style,
            ));
        }
        lines.push(Line::from(spans));
    }

    // Signature detail (type · key, plus the forge reason once known).
    if let Some(sig) = &detail.signature {
        let kind = match sig.kind {
            karet_vcs::SignatureKind::Ssh => "SSH",
            karet_vcs::SignatureKind::OpenPgp => "GPG",
            karet_vcs::SignatureKind::X509 => "X.509",
            _ => "signature",
        };
        let mut text = kind.to_string();
        if let Some(key) = &sig.signer_key {
            text.push_str(&format!(" \u{b7} {key}"));
        }
        if let Some(v) = verification {
            if v.reason != "valid" {
                text.push_str(&format!("  ({})", v.reason));
            }
            if let Some(s) = &v.signer {
                text.push_str(&format!("  {s}"));
            }
        }
        lines.push(Line::from(vec![
            bar(),
            Span::styled(format!("{:<10} ", "signature"), label),
            Span::styled(text, muted),
        ]));
    }

    // The summary, the changed-file table of contents, and the per-file diff cards.
    // Metadata can arrive before file extraction; keep this lower block stable while
    // the heavier work finishes.
    match file_status {
        CommitFileStatus::Ready => lines.extend(changed_files_lines(theme, files, width)),
        CommitFileStatus::Loading(since) => {
            lines.push(Line::raw(""));
            if loading_visible(since) {
                lines.push(Line::styled(" loading changed files\u{2026}", muted));
            }
        },
        CommitFileStatus::Failed(error) => {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(" changed files unavailable", label),
                Span::raw("   "),
                Span::styled(error.to_string(), muted),
            ]));
        },
    }
    (lines, Some(badge_hit))
}

/// Build the shared "changed files" block: a `N files changed +a −b` summary, a
/// changed-file table of contents, then one boxed diff card per file. Used by both the
/// commit view ([`commit_detail_lines`]) and the compare view so the two render files
/// identically. `width` is the render width, used to size the card rules.
fn changed_files_lines(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
) -> Vec<Line<'static>> {
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let add_fg = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let rem_fg = Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui());

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Summary: N files changed, +added −removed.
    let (mut added, mut removed) = (0usize, 0usize);
    for file in files {
        let (a, r) = file.line_stats();
        added += a;
        removed += r;
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            format!(
                " {} file{} changed",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            label,
        ),
        Span::raw("   "),
        Span::styled(format!("+{added}"), add_fg),
        Span::raw(" "),
        Span::styled(format!("\u{2212}{removed}"), rem_fg),
    ]));

    // Changed-file table of contents.
    for file in files {
        let (a, r) = file.line_stats();
        let (g, role) = status_glyph(file.change.status);
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {g}  "),
                Style::default().fg(theme.role(role).to_ratatui()),
            ),
            Span::styled(file.change.path.to_string_lossy().into_owned(), fg),
            Span::styled(format!("   +{a}"), add_fg),
            Span::styled(format!(" \u{2212}{r}"), rem_fg),
        ]));
    }

    // Per-file diff cards.
    for file in files {
        lines.push(Line::raw(""));
        lines.extend(file_card(theme, file, width));
    }
    lines
}

/// Render one file's diff as a boxed "card": a top rule carrying the status glyph, the
/// path (and the old path for renames), and the `+a −b` stats; each diff line prefixed
/// with a left rail; then a bottom rule. `width` sizes the rules (a small floor keeps a
/// narrow pane from producing a degenerate box).
fn file_card(theme: &Theme, file: &render::FileView, width: u16) -> Vec<Line<'static>> {
    let border = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let add_fg = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let rem_fg = Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui());
    let (glyph, role) = status_glyph(file.change.status);
    let glyph_style = Style::default().fg(theme.role(role).to_ratatui());
    let (a, r) = file.line_stats();

    // A small floor keeps a narrow pane from underflowing into a degenerate rule.
    let w = usize::from(width).max(24);
    let mut path = file.change.path.to_string_lossy().into_owned();
    if let Some(old) = &file.change.old_path {
        path.push_str(&format!(" \u{2190} {}", old.to_string_lossy()));
    }
    let (add, rem) = (format!("+{a}"), format!("\u{2212}{r}"));

    // Top rule: "╭─ {g} path " + dashes + " +a −b ─╮", padded so the row is `w` columns.
    // Column counts assume 1-wide cells (paths are ~ASCII; box/±/− glyphs are 1 wide).
    let fixed =
        3 + 2 + path.chars().count() + 1 + 1 + add.chars().count() + 1 + rem.chars().count() + 3;
    let dashes = w.saturating_sub(fixed).max(1);
    let top: Vec<Span<'static>> = vec![
        Span::styled("\u{256d}\u{2500} ", border),
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(path, fg.add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", "\u{2500}".repeat(dashes)), border),
        Span::styled(add, add_fg),
        Span::raw(" "),
        Span::styled(rem, rem_fg),
        Span::styled(" \u{2500}\u{256e}", border),
    ];

    let mut out = vec![Line::from(top)];
    // Body: each diff line behind a left rail.
    for line in render::unified_lines(file, theme) {
        let mut spans = vec![Span::styled("\u{2502} ", border)];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    // Bottom rule: "╰" + dashes + "╯", `w` columns wide.
    out.push(Line::styled(
        format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(w.saturating_sub(2))),
        border,
    ));
    out
}

/// Draw the full-screen commit graph browser: a DAG commit list on the left and the
/// selected commit's detail on the right.
#[allow(clippy::too_many_arguments)] // a browser pane genuinely has many independent inputs
fn draw_commit_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    commits: &[karet_vcs::Commit],
    has_more: bool,
    loading: bool,
    loading_since: Option<Instant>,
    selected: usize,
    detail_loading_since: Option<Instant>,
    detail: Option<&karet_vcs::CommitDetail>,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    verification: Option<&karet_session::GithubVerification>,
    list_offset: &mut u16,
) {
    let cols = Layout::horizontal([
        Constraint::Percentage(42),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);
    let (list_area, detail_area) = (cols[0], cols[2]);
    f.render_widget(Block::new().borders(Borders::LEFT), cols[1]);

    // Left: the DAG commit list (same rail palette as the SCM sidebar log).
    const LANE_COLORS: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
        Color::Red,
    ];
    let lane_style = |lane: u8| Style::default().fg(LANE_COLORS[lane as usize % LANE_COLORS.len()]);
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let sel_bg = theme.role(ThemeRole::Selection).to_ratatui();

    let inputs: Vec<LaneInput> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| LaneInput {
            id: c.hash.clone(),
            parents: c.parents.clone(),
            head: i == 0,
        })
        .collect();
    let rails = assign_lanes(&inputs);
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(" COMMITS", header_style))];
    for (i, (commit, rail)) in commits.iter().zip(rails.iter()).enumerate() {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(render_rail(rail, lane_style).spans);
        spans.push(Span::styled(format!(" {} ", commit.short_hash), hash_style));
        spans.push(Span::raw(commit.summary.clone()));
        spans.push(Span::styled(
            format!("  {}", relative_time(commit.time)),
            dim,
        ));
        let mut line = Line::from(spans);
        if i == selected {
            line = line.style(Style::default().bg(sel_bg));
        }
        items.push(ListItem::new(line));
    }
    if loading && commits.is_empty() && loading_since.is_some_and(loading_visible) {
        items.push(ListItem::new(Line::styled(" loading\u{2026}", dim)));
    } else if has_more {
        items.push(ListItem::new(Line::styled(" \u{22ef} more", dim)));
    }

    // Keep the selected row (offset by the header) visible.
    let height = list_area.height as usize;
    let sel_row = selected + 1;
    let mut off = *list_offset as usize;
    if sel_row < off {
        off = sel_row;
    } else if height > 0 && sel_row >= off + height {
        off = sel_row + 1 - height;
    }
    *list_offset = u16::try_from(off).unwrap_or(u16::MAX);
    let mut state = ListState::default();
    *state.offset_mut() = off;
    f.render_stateful_widget(List::new(items), list_area, &mut state);

    // Right: the selected commit's detail (once its fetch answers).
    let sel_hash = commits.get(selected).map(|c| c.hash.as_str());
    match detail {
        Some(d) if Some(d.hash.as_str()) == sel_hash => {
            f.render_widget(
                Paragraph::new(
                    commit_detail_lines(
                        theme,
                        d,
                        files,
                        file_status,
                        verification,
                        false,
                        detail_area.width,
                    )
                    .0,
                ),
                detail_area,
            );
        },
        _ => {
            let pending_since = if commits.is_empty() {
                loading_since
            } else {
                detail_loading_since
            };
            if pending_since.is_some_and(loading_visible) {
                let msg = if commits.is_empty() {
                    "loading commits\u{2026}"
                } else {
                    "loading commit details\u{2026}"
                };
                f.render_widget(
                    Paragraph::new(Line::styled(format!("  {msg}"), dim)),
                    detail_area,
                );
            } else {
                f.render_widget(
                    Block::default()
                        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
                    detail_area,
                );
            }
        },
    }
}

fn loading_visible(since: Instant) -> bool {
    since.elapsed() >= crate::app::LOADING_REVEAL_DELAY
}

/// Render the semantic-blame view: each commit group as a header (line range, short
/// hash, author, date) followed by its full commit message — the "why".
/// Draw a code-visualization graph as a scrollable indented tree: a DFS from the
/// graph's roots along dependency edges, with box-drawing depth guides. Cycles and
/// already-expanded nodes are shown once and marked `⟲` rather than re-expanded.
fn draw_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    view: &karet_core::GraphView,
    scroll: &mut u16,
) {
    use karet_core::GraphEdgeKind;

    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let guide = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let name_style = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let badge_style = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let revisit_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());

    // Flatten the graph to indented rows (DFS from roots, cycle-safe).
    let mut rows: Vec<Line> = vec![Line::styled(
        format!(" ⧉ {title} — dependency graph"),
        header_style,
    )];
    let mut expanded: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut stack: Vec<(&str, usize)> = view
        .roots
        .iter()
        .rev()
        .map(|r| (r.as_str(), 0usize))
        .collect();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = view.nodes.iter().find(|n| n.id == id) else {
            continue;
        };
        let first_visit = expanded.insert(id);
        let children = view.successors(id, GraphEdgeKind::Dependency);
        let mut spans = vec![Span::raw(" ")];
        for _ in 0..depth {
            spans.push(Span::styled("\u{2502} ", guide));
        }
        spans.push(Span::styled("\u{25CF} ", guide));
        spans.push(Span::styled(node.label.clone(), name_style));
        if let Some(badge) = &node.badge {
            spans.push(Span::styled(format!("  {badge}"), badge_style));
        }
        if !first_visit && !children.is_empty() {
            // Already expanded elsewhere (or a cycle): show but don't recurse again.
            spans.push(Span::styled("  \u{27F2}", revisit_style));
        }
        rows.push(Line::from(spans));
        if first_visit {
            for child in children.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }

    let height = area.height as usize;
    let max_scroll = rows.len().saturating_sub(height);
    *scroll = (*scroll).min(max_scroll as u16);
    let para = Paragraph::new(rows).scroll((*scroll, 0));
    f.render_widget(para, area);
}

/// Draw the loaded settings and provenance as a scrollable read-only report.
fn draw_loaded_config(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    report: &LoadedConfig,
    scroll: &mut u16,
) {
    let header = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let explicit = fg.add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let badge = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let warning = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());

    let mut lines = Vec::new();
    lines.push(Line::styled(" Loaded Settings", header));
    lines.push(Line::raw(""));
    lines.push(Line::styled(" Layers", header));

    let mut layers = report.layers.clone();
    layers.sort_by_key(|row| std::cmp::Reverse(row.layer));
    if layers.is_empty() {
        lines.push(Line::styled("  no layer provenance captured", muted));
    }
    for row in layers {
        let (status, style) = match &row.status {
            ConfigLayerStatus::Loaded => ("loaded".to_string(), fg),
            ConfigLayerStatus::Missing => ("missing".to_string(), muted),
            ConfigLayerStatus::Invalid(_) => ("invalid".to_string(), warning),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<8}", row.layer.label()), style),
            Span::styled(format!("{status:<9}"), style),
            Span::styled(row.path.to_string_lossy().into_owned(), style),
        ]));
        if let ConfigLayerStatus::Invalid(message) = row.status {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(message, warning),
            ]));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(" Diagnostics", header));
    if report.diagnostics.is_empty() {
        lines.push(Line::styled("  none", muted));
    } else {
        for diag in &report.diagnostics {
            let style = severity_style(theme, diag.severity);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<8}", severity_label(diag.severity)), style),
                Span::styled(format!("{}  ", diag.path.to_string_lossy()), muted),
                Span::styled(diag.message.clone(), style),
            ]));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(" Values", header));
    match serde_json::to_value(&report.settings) {
        Ok(serde_json::Value::Object(sections)) => {
            for (section, value) in sections {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(section.clone(), header),
                ]));
                flatten_setting_lines(
                    &mut lines, report, &section, "", &value, explicit, muted, badge,
                );
            }
        },
        _ => lines.push(Line::styled("  settings could not be serialized", warning)),
    }

    let height = area.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    *scroll = (*scroll).min(max_scroll as u16);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
}

#[allow(clippy::too_many_arguments)]
fn flatten_setting_lines(
    lines: &mut Vec<Line<'static>>,
    report: &LoadedConfig,
    section: &str,
    prefix: &str,
    value: &serde_json::Value,
    explicit: Style,
    muted: Style,
    badge: Style,
) {
    if let serde_json::Value::Object(obj) = value {
        for (key, child) in obj {
            let next = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            flatten_setting_lines(lines, report, section, &next, child, explicit, muted, badge);
        }
        return;
    }

    let full_path = format!("{section}.{prefix}");
    let source = report.explicit.get(&full_path);
    let style = if source.is_some() { explicit } else { muted };
    let source_label = source.map_or("default", |layer| layer.label());
    let value_text = serde_json::to_string(value).unwrap_or_else(|_| "<value>".to_string());
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(format!("{prefix:<24}"), style),
        Span::styled(format!("{value_text:<26}"), style),
        Span::styled(source_label.to_string(), source.map_or(muted, |_| badge)),
    ]));
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
        _ => "info",
    }
}

fn severity_style(theme: &Theme, severity: Severity) -> Style {
    let role = match severity {
        Severity::Error => ThemeRole::DiagnosticError,
        Severity::Warning => ThemeRole::DiagnosticWarning,
        Severity::Information => ThemeRole::DiagnosticInfo,
        Severity::Hint => ThemeRole::DiagnosticHint,
        _ => ThemeRole::DiagnosticInfo,
    };
    Style::default().fg(theme.role(role).to_ratatui())
}

/// Paint a markdown preview, re-parsing and re-wrapping only when the document version or
/// the pane width has moved since the last frame.
///
/// Caching here rather than on every snapshot is what keeps typing cheap: a burst of
/// keystrokes lands many snapshots but only one draw, so it costs one re-parse.
fn draw_markdown_preview(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    buffer: &TextBuffer,
    wrapped: &mut WrappedDocument,
    rendered: &mut Option<(u64, u16)>,
    scroll: &mut u16,
) {
    let key = (buffer.version(), area.width);
    if *rendered != Some(key) {
        *wrapped = karet_markdown::parse(&buffer.text()).wrap(area.width);
        *rendered = Some(key);
    }
    let mut state = MarkdownViewState { scroll: *scroll };
    f.render_stateful_widget(MarkdownView::new(wrapped, theme), area, &mut state);
    // The widget clamps the scroll to the document; keep the clamped value so a
    // shrinking document doesn't leave the tab scrolled past the end.
    *scroll = state.scroll;
}

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
        Focus::Outline => "OUTLINE",
    };
    let bar = Style::default()
        .bg(theme.role(ThemeRole::StatusBarBackground).to_ratatui())
        .fg(theme.role(ThemeRole::StatusBarForeground).to_ratatui());
    let key = bar.add_modifier(Modifier::BOLD);

    // The right column is a fixed-width strip: cursor position (code tabs only),
    // encoding/EOL, then the language/kind label — the hints get everything else.
    let language = app.tabs.get(app.active).map_or("", Tab::language);
    let right = match app.tabs.get(app.active) {
        Some(
            tab @ Tab {
                kind: TabKind::Code { .. },
                ..
            },
        ) => {
            let cursor_label = cursor_status_label(tab);
            match tab.encoding_label() {
                Some(enc) => format!(" {cursor_label} · {enc} · {language} "),
                None => format!(" {cursor_label} · {language} "),
            }
        },
        _ => format!(" {language} "),
    };
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

/// The status bar's cursor-position label for a code tab: `"Ln {line}, Col
/// {col}"` (1-based), with a `"(N selected)"` / `"(N lines selected)"` suffix
/// when the primary selection is non-empty.
fn cursor_status_label(tab: &Tab) -> String {
    let primary = tab.editor.cursors().primary();
    let head = primary.head;
    let mut label = format!("Ln {}, Col {}", head.line + 1, head.col + 1);
    let range = primary.range();
    if range.start != range.end {
        if range.start.line == range.end.line {
            let n = range.end.col.saturating_sub(range.start.col);
            label.push_str(&format!(" ({n} selected)"));
        } else {
            let lines = range.end.line - range.start.line + 1;
            label.push_str(&format!(" ({lines} lines selected)"));
        }
    }
    label
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

    fn test_code_tab(path: &str) -> Tab {
        use karet_text::TextBuffer;

        let buffer = TextBuffer::from_text("");
        Tab::new(
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path),
            TabKind::Code {
                path: PathBuf::from(path),
                language: "plaintext",
                doc: None,
                next_version: 0,
                buffer,
                text: String::new(),
                highlights: karet_syntax::Highlights::default(),
                folds: karet_syntax::FoldRegions::default(),
                folded: std::collections::BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
            },
        )
    }

    #[test]
    fn tab_titles_disambiguate_duplicate_file_names() {
        let root = Path::new("/repo");
        let tabs = vec![
            test_code_tab("/repo/src/view/mod.rs"),
            test_code_tab("/repo/tests/view/mod.rs"),
            test_code_tab("/repo/src/lib.rs"),
        ];

        let titles = tab_display_titles(&tabs, root);

        assert_eq!(titles[0].prefix, "src/view/");
        assert_eq!(titles[0].name, "mod.rs");
        assert_eq!(titles[1].prefix, "tests/view/");
        assert_eq!(titles[1].name, "mod.rs");
        assert_eq!(titles[2].prefix, "");
        assert_eq!(titles[2].name, "lib.rs");
    }

    #[test]
    fn active_tab_prefix_keeps_active_fill() {
        let theme = Theme::dark();
        let base = tab_text_style(&theme, true, true, false);

        let prefix = tab_prefix_style(&theme, base, true, true);

        assert_eq!(prefix.fg, Some(theme.role(ThemeRole::Muted).to_ratatui()));
        assert_eq!(
            prefix.bg,
            Some(theme.role(ThemeRole::Foreground).to_ratatui())
        );
        assert!(!prefix.add_modifier.contains(Modifier::REVERSED));
        assert!(prefix.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn unfocused_active_tab_prefix_stays_muted_without_fill() {
        let theme = Theme::dark();
        let base = tab_text_style(&theme, true, false, false);

        let prefix = tab_prefix_style(&theme, base, true, false);

        assert_eq!(prefix.fg, Some(theme.role(ThemeRole::Muted).to_ratatui()));
        assert_eq!(prefix.bg, None);
        assert!(prefix.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn chrome_button_hover_changes_foreground_without_background() {
        let theme = Theme::dark();
        let hover = chrome_button_style(&theme, ChromeButtonState::Hovered);
        assert_eq!(
            hover.fg,
            Some(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        );
        assert_eq!(hover.bg, None);

        let active_hover = chrome_button_style(&theme, ChromeButtonState::ActiveHovered);
        assert_eq!(
            active_hover.fg,
            Some(theme.role(ThemeRole::Foreground).to_ratatui())
        );
        assert_eq!(active_hover.bg, None);
        assert!(active_hover.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn format_datetime_is_correct_and_applies_offset() {
        assert_eq!(format_datetime(0, 0), "1970-01-01 00:00");
        assert_eq!(format_datetime(0, 3600), "1970-01-01 01:00");
        // 1_700_000_000 = 2023-11-14 22:13:20 UTC.
        assert_eq!(format_datetime(1_700_000_000, 0), "2023-11-14 22:13");
    }

    #[test]
    fn verified_badge_reflects_forge_and_signature() {
        use karet_vcs::CommitSignature;
        use karet_vcs::SignatureKind;
        let verified = karet_session::GithubVerification {
            verified: true,
            reason: "valid".to_string(),
            signer: None,
        };
        let unverified = karet_session::GithubVerification {
            verified: false,
            reason: "unsigned".to_string(),
            signer: None,
        };
        let sig = CommitSignature {
            kind: SignatureKind::Ssh,
            signer_key: None,
            raw: String::new(),
        };
        assert_eq!(verified_badge(Some(&verified), None).1, "Verified");
        assert_eq!(verified_badge(Some(&unverified), None).1, "Unverified");
        assert_eq!(verified_badge(None, Some(&sig)).1, "Signed");
        assert_eq!(verified_badge(None, None).1, "Unsigned");
    }

    #[test]
    fn file_cards_are_boxed_and_width_sized() {
        use karet_vcs::FileChange;
        use karet_vcs::StatusKind;
        let change = FileChange {
            path: std::path::PathBuf::from("src/main.rs"),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: "fn a() {}\n".to_string(),
            new: "fn b() {}\n".to_string(),
        };
        let files = vec![render::FileView::new(
            change,
            render::Section::Staged,
            false,
        )];
        let width = 60u16;
        let lines = changed_files_lines(&Theme::dark(), &files, width);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // A rounded top rule (corners) and a bottom rule bound the card.
        let top = text
            .iter()
            .find(|t| t.starts_with('\u{256d}'))
            .expect("a top rule");
        assert!(top.contains("src/main.rs"), "top rule carries the path");
        assert!(top.ends_with('\u{256e}'), "top rule closes with a corner");
        assert_eq!(
            top.chars().count(),
            usize::from(width),
            "the top rule spans the pane width"
        );
        let bottom = text
            .iter()
            .find(|t| t.starts_with('\u{2570}') && t.ends_with('\u{256f}'))
            .expect("a bottom rule");
        assert_eq!(bottom.chars().count(), usize::from(width));
        // Diff body lines sit behind a left rail.
        assert!(
            text.iter().any(|t| t.starts_with("\u{2502} ")),
            "diff lines are railed"
        );
    }

    #[test]
    fn badge_hit_spans_the_badge_and_reveal_explains_it() {
        use karet_vcs::CommitDetail;
        use karet_vcs::CommitSignature;
        use karet_vcs::Identity;
        use karet_vcs::SignatureKind;

        let id = || Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        let detail = CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: id(),
            committer: id(),
            parents: Vec::new(),
            signature: Some(CommitSignature {
                kind: SignatureKind::Ssh,
                signer_key: None,
                raw: String::new(),
            }),
        };
        let files: Vec<render::FileView> = Vec::new();
        let flat = |l: &Line| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() };

        // Without a forge verdict, a signed commit reads "Signed"; the reported hit
        // must land exactly on that badge text within its line.
        let (lines, hit) = commit_detail_lines(
            &Theme::dark(),
            &detail,
            &files,
            CommitFileStatus::Ready,
            None,
            false,
            80,
        );
        let hit = hit.expect("a signed commit has a badge");
        let chars: Vec<char> = flat(&lines[hit.line as usize]).chars().collect();
        let span: String = chars[hit.col as usize..(hit.col + hit.width) as usize]
            .iter()
            .collect();
        assert!(
            span.contains("Signed"),
            "the hit span covers the badge: {span:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| flat(l).contains("cryptographic signature")),
            "no explanation is shown until revealed"
        );

        // Revealing inserts the badge's plain-language meaning.
        let (revealed, _) = commit_detail_lines(
            &Theme::dark(),
            &detail,
            &files,
            CommitFileStatus::Ready,
            None,
            true,
            80,
        );
        assert!(
            revealed
                .iter()
                .any(|l| flat(l).contains("cryptographic signature")),
            "the reveal explains what Signed means"
        );
    }

    #[test]
    fn cursor_status_label_reports_position_and_selection_extent() {
        use karet_core::LineCol;
        use karet_text::TextBuffer;

        let buffer = TextBuffer::from_text("hello\nworld\n");
        let mut tab = Tab::new(
            "a.txt",
            TabKind::Code {
                path: std::path::PathBuf::from("/x/a.txt"),
                language: "plaintext",
                doc: None,
                next_version: 0,
                buffer: buffer.clone(),
                text: "hello\nworld\n".to_string(),
                highlights: karet_syntax::Highlights::default(),
                folds: karet_syntax::FoldRegions::default(),
                folded: std::collections::BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
            },
        );

        tab.editor.place_caret(LineCol::new(1, 2));
        assert_eq!(cursor_status_label(&tab), "Ln 2, Col 3");

        // A same-line selection reports the selected character count.
        tab.editor
            .set_selection(&buffer, LineCol::new(0, 1), LineCol::new(0, 4));
        assert_eq!(cursor_status_label(&tab), "Ln 1, Col 5 (3 selected)");

        // A multi-line selection reports the line count instead.
        tab.editor
            .set_selection(&buffer, LineCol::new(0, 0), LineCol::new(1, 2));
        assert_eq!(cursor_status_label(&tab), "Ln 2, Col 3 (2 lines selected)");
    }

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
