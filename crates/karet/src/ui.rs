//! Ratatui layout and drawing for the IDE shell: a tab strip, an optional
//! breadcrumb, a switchable sidebar (explorer / search / source-control), the main
//! content area (the active tab), and a status bar.

pub(crate) mod commit;
mod content;
mod github;
mod language_servers;
mod panes;
mod scm;
mod seam;
mod secondary;
mod sidebar;
mod status;
mod view_chrome;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

pub(crate) use commit::relative_time;
use commit::*;
use content::*;
use github::*;
use karet_core::Decoration;
use karet_core::Diagnostic;
use karet_core::Severity;
use karet_core::ThemeRole;
use karet_editor::Editor;
use karet_editor::EditorState;
use karet_filetype::FileKind;
use karet_fileview::HexView;
use karet_fileview::image::GraphicsProtocol;
#[cfg(feature = "pdf")]
use karet_fileview::image::Image;
#[cfg(feature = "images")]
use karet_fileview::image::ImageWidget;
#[cfg(feature = "pdf")]
use karet_fileview::image::fit_rect;
use karet_fileview::viewer::Placeholder;
use karet_markdown::WrappedDocument;
use karet_markdown::view::MarkdownView;
use karet_markdown::view::MarkdownViewState;
use karet_session::ConfigLayerStatus;
use karet_session::DocumentId;
use karet_session::LoadedConfig;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_vcs::StatusKind;
use karet_widgets::CompletionPopup;
use karet_widgets::Corner;
use karet_widgets::FileTree;
use karet_widgets::Spinner;
use karet_widgets::SplitAxis;
use karet_widgets::Toasts;
use karet_widgets::UiIcon;
use language_servers::*;
use panes::*;
use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Margin;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::ListState;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use scm::draw_scm;
use secondary::*;
use sidebar::*;
use status::*;
use unicode_width::UnicodeWidthStr;
use view_chrome::*;

use crate::app::App;
use crate::app::MIN_SCM_REGION;
use crate::app::OperationBlocker;
use crate::app::Pending;
use crate::app::SIDEBAR_MIN_WIDTH;
use crate::app::TabDrag;
use crate::app::TabHit;
use crate::app::TextFieldState;
use crate::app::ToastHit;
use crate::command::Command;
use crate::keymap::ChordStyle;
use crate::keymap::Context;
use crate::keymap::Focus;
use crate::tab::CommitFiles;
use crate::view::View;

/// Render single-line text-field content with a highlighted selection and an
/// insertion caret, through the shared `karet-widgets` text-area renderer.
pub(super) fn text_field_text(
    text: &str,
    edit: &TextFieldState,
    focused: bool,
    normal: Style,
    selection: Style,
    caret: Style,
) -> Text<'static> {
    karet_widgets::textarea::styled_text(
        text,
        focused.then(|| edit.cursor()),
        edit.selection(),
        karet_widgets::textarea::TextAreaStyle::new(normal, selection, caret),
    )
}

pub(crate) use commit::responsive::document_row as commit_document_row;
pub(crate) use karet_widgets::scroll::ScrollAxes;
pub(crate) use karet_widgets::scroll::ScrollAxis;
pub(crate) use karet_widgets::scroll::ScrollBar;
pub(crate) use karet_widgets::scroll::ScrollExtent;
pub(crate) use karet_widgets::scroll::ScrollTrack;
pub(crate) use karet_widgets::scroll::ScrollbarStyles;
pub(crate) use karet_widgets::scroll::draw_scrollable_lines;
pub(crate) use karet_widgets::scroll::line_width;
pub(crate) use karet_widgets::scroll::reserve_tracks;
pub(crate) use panes::FindBarRects;

use crate::app::ContextMenuEntryExt;
pub(crate) use crate::app::ScrollHits;
pub(crate) use crate::app::ScrollSurface;
use crate::keymap::SidebarPanel;
use crate::keymap::{self};
use crate::overlay::Overlay;
use crate::render::{self};
use crate::tab::FindState;
use crate::tab::MergeConflictState;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;

/// Draw one frame of the shell.
pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = app.theme.clone();
    let area = f.area();
    // Collected into a local and handed over at the end rather than cleared on `app`
    // up front: the panel draws already borrow `app` mutably, so a field sink would
    // not compile. One assignment is also one clear — no half-rebuilt registry can
    // survive a frame that returns early.
    let mut hits = ScrollHits::default();

    // Top level: a one-row view switcher, the body (sidebar + panes) it sits above,
    // and a one-row status bar. Tab strips and breadcrumbs live *inside* each pane.
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    draw_view_chrome(f, app, &theme, rows[0]);
    let body = rows[1];

    let sidebar = if app.sidebar_visible && app.view.shows_sidebar() {
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
        draw_sidebar(f, app, &theme, rect, &mut hits);
        draw_sidebar_divider(f, &theme, divider, app.sidebar_resizing);
    }

    // On wide terminals the fixed-width outline reserves space. On narrow ones it
    // temporarily overlays the right edge and dismisses as soon as the editor is used.
    let mut outline_area = None;
    let mut outline_divider = None;
    app.outline.overlay = false;
    let outline_shown = app.outline.visible && app.view == View::Editor;
    if outline_shown && app.main_rect.width > app.outline.width + 8 {
        let region = app.main_rect;
        let cols = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(app.outline.width),
        ])
        .split(region);
        app.main_rect = cols[0];
        app.outline.rect = cols[2];
        outline_area = Some(cols[2]);
        outline_divider = Some(cols[1]);
    } else if outline_shown && app.main_rect.width > 0 {
        let width = app.outline.width.min(app.main_rect.width);
        let rect = Rect::new(
            app.main_rect.right().saturating_sub(width),
            app.main_rect.y,
            width,
            app.main_rect.height,
        );
        app.outline.rect = rect;
        app.outline.overlay = true;
        outline_area = Some(rect);
    } else {
        app.outline.rect = Rect::default();
        app.outline.content_rect = Rect::default();
    }

    match app.view {
        View::Editor => draw_panes(f, app, &theme, app.main_rect, &mut hits),
        view => draw_view_placeholder(f, &theme, app.main_rect, view, app.icon_style),
    }
    if let Some(divider) = outline_divider {
        draw_sidebar_divider(f, &theme, divider, false);
    }
    if let Some(area) = outline_area {
        draw_outline(f, app, &theme, area, &mut hits);
    }
    draw_drop_preview(f, app, &theme);
    draw_status(f, app, &theme, rows[2]);

    // The completion popup floats over the editor, anchored at the caret; it
    // sits under modal overlays and toasts.
    draw_completion(f, app, &theme, &mut hits);
    // The hover popup floats the same way (they never overlap in practice —
    // one follows typing, the other an explicit keystroke).
    draw_hover(f, app, &theme);
    draw_diagnostic_view(f, app, &theme, area);

    if let Some(overlay) = &app.overlay {
        draw_overlay(f, overlay, &theme, area);
    }
    if let Some(rev) = &app.rev_input {
        draw_rev_input(f, rev, &theme, area);
    }
    draw_context_menu(f, app, &theme, area);
    if let Some(blocker) = &app.operation_blocker {
        draw_operation_blocker(f, blocker, &theme, area);
    }

    // Toasts float above everything, including the modal overlay.
    draw_toasts(f, app, &theme, area);

    app.scroll_hits = hits;
}

/// Draw the modal explaining why a destructive operation is delaying shutdown.
fn draw_operation_blocker(f: &mut Frame, blocker: &OperationBlocker, theme: &Theme, area: Rect) {
    let rect = centered(area, 62, 7);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Finishing source control operation")
        .border_style(theme.style(ThemeRole::DiagnosticWarning))
        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let remaining = blocker
        .deadline
        .saturating_duration_since(Instant::now())
        .as_secs();
    let text = vec![
        Line::raw(format!("{} must finish before karet exits.", blocker.label)),
        Line::raw(""),
        Line::raw(format!("Waiting up to {remaining}s · Esc cancels quit")),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);
}

/// Draw the scrollable diagnostic detail view (Ctrl+K Ctrl+M): a centered
/// modal rendering the composed markdown through `MarkdownView`, so fenced
/// TypeScript types come out highlighted and long errors scroll.
fn draw_diagnostic_view(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(view) = app.diagnostic_view.as_mut() else {
        return;
    };
    let width = area.width.saturating_sub(8).clamp(24, 100);
    let height = area.height.saturating_sub(4).clamp(5, 28);
    let rect = centered(area, width, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Diagnostics · ↑↓ scroll · Esc closes")
        .border_style(theme.style(ThemeRole::LineNumberActive))
        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    if view.wrapped.as_ref().map(|(w, _)| *w) != Some(inner.width) {
        let doc = karet_markdown::parse(&view.markdown);
        view.wrapped = Some((inner.width, doc.wrap(inner.width)));
    }
    let Some((_, wrapped)) = view.wrapped.as_ref() else {
        return;
    };
    let mut state = MarkdownViewState {
        scroll: view.scroll,
    };
    f.render_stateful_widget(MarkdownView::new(wrapped, theme), inner, &mut state);
    // The widget clamps the scroll to the document; keep the clamped value.
    view.scroll = state.scroll;
}

/// Draw the centered go-to-commit (revision) input prompt.
fn draw_rev_input(f: &mut Frame, rev: &str, theme: &Theme, area: Rect) {
    let width = area.width.clamp(20, 60);
    let rect = centered(area, width, 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Go to commit")
        .border_style(theme.style(ThemeRole::LineNumberActive))
        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui()));
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let line = Line::from(vec![
        Span::styled("› ", theme.style(ThemeRole::LineNumber)),
        Span::styled(rev.to_string(), theme.style(ThemeRole::Foreground)),
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

/// Immutable per-pane render inputs, bundled to keep the render helpers' signatures
/// small.
struct PaneCtx<'a> {
    theme: &'a Theme,
    root: &'a Path,
    icon_style: karet_filetype::IconStyle,
    graphics: GraphicsProtocol,
    /// Whether this pane holds the window focus (affects tab-strip styling).
    pane_focused: bool,
    /// Whether the editor should draw its caret as focused.
    editor_focused: bool,
    /// Whether the app will draw a Kitty graphics caret after this frame.
    graphical_cursor: bool,
    /// Language-resolved word-wrap override; `None` delegates to the active file type.
    word_wrap: Option<bool>,
    /// Language-resolved semantic sticky-scroll setting.
    sticky_scroll: bool,
    /// Per-document hard-tab display width.
    tab_width: u16,
    /// Complete diagnostic sets keyed by backend document.
    diagnostics: &'a HashMap<DocumentId, Vec<Diagnostic>>,
    /// The find bar to draw atop this pane's content, if any (focused pane only).
    /// Owned (not borrowed): it now lives on the active `Tab` itself, and
    /// `render_pane` needs a mutable borrow of the tabs slice at the same time.
    find: Option<FindState>,
    /// Stale-checked virtual text for the focused editor.
    blame: Option<Decoration>,
    /// The Ctrl-hovered symbol to underline as navigable (focused pane only).
    definition_underline: Option<Decoration>,
    /// Whether the blame decoration represents an attributed commit.
    blame_clickable: bool,
    /// Mouse position over a link in this pane, used for hover emphasis.
    markdown_link_hover: Option<(u16, u16)>,
    /// The live pointer selection to paint over this pane's rows (focused pane
    /// only) — read-only surfaces have no selection state of their own.
    selection: Option<crate::app::SurfaceSelection>,
    /// Mouse position over a format-specific pane action.
    pane_action_hover: Option<(u16, u16)>,
    /// Fence languages the markdown preview renders as mermaid diagrams
    /// (`None` = rendering disabled or compiled out).
    mermaid: Option<&'a [String]>,
    /// Whether to tint visible color literals with their own color.
    color_highlight: bool,
    /// Dependency-freshness hints per open manifest.
    manifest_hints: &'a HashMap<DocumentId, (u64, Vec<karet_session::ManifestHint>)>,
    /// Armed breakpoints per absolute file path: line → verified.
    breakpoints: &'a HashMap<PathBuf, std::collections::BTreeMap<u32, bool>>,
    /// The debuggee's stop location, for the stopped-line tint.
    debug_stopped: Option<&'a (PathBuf, u32)>,
    /// Every ref per commit hash, for log-row decorations.
    ref_labels: &'a HashMap<String, Vec<karet_vcs::RefLabel>>,
    /// Branch, upstream and divergence, for the commit-graph header.
    repo_state: Option<&'a karet_vcs::RepositoryState>,
}

/// What a rendered pane reported back for hit-testing and image placement.
struct RenderedPane {
    tabstrip_rect: Rect,
    tab_hits: Vec<TabHit>,
    action_hits: Vec<(u16, u16, Command)>,
    breadcrumb_rect: Rect,
    breadcrumb_hits: Vec<crate::app::BreadcrumbHit>,
    content_rect: Rect,
    editor_rect: Rect,
    markdown_preview_rect: Rect,
    image_area: Option<Rect>,
    commit_badge_rect: Option<Rect>,
    commit_file_hits: Vec<crate::app::CommitFileHit>,
    commit_collapse_hits: Vec<crate::app::CommitCollapseHit>,
    blame_rect: Option<Rect>,
    markdown_link_hits: Vec<crate::app::MarkdownLinkHit>,
    select_regions: Vec<crate::app::SelectRegion>,
    find_rects: FindBarRects,
}

/// Geometry a tab's content reported for post-draw use: a reserved Kitty image rect
/// and the commit view's signature-badge rect (for double-click hit-testing).
#[derive(Default)]
struct PaneContent {
    editor_rect: Rect,
    markdown_preview_rect: Rect,
    image_area: Option<Rect>,
    badge_rect: Option<Rect>,
    file_hits: Vec<crate::app::CommitFileHit>,
    collapse_hits: Vec<crate::app::CommitCollapseHit>,
    blame_rect: Option<Rect>,
    markdown_link_hits: Vec<crate::app::MarkdownLinkHit>,
    select_regions: Vec<crate::app::SelectRegion>,
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
    let style = theme.style(role);
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
    ctx: &PaneCtx<'_>,
    area: Rect,
) -> (Rect, Vec<TabHit>, Vec<(u16, u16, Command)>) {
    let mut hits = Vec::new();
    let mut spans = Vec::new();
    let mut x = area.x;
    let titles = tab_display_titles(tabs, ctx.root, ctx.icon_style);
    for (i, tab) in tabs.iter().enumerate() {
        let style = tab_text_style(ctx.theme, i == active, ctx.pane_focused, tab.is_preview);
        // A pre-allocated 1-cell status slot keeps the layout stable: `●` for
        // unsaved changes (a spinner frame while a slow save writes), else blank.
        let mark = save_mark(tab, ctx.icon_style);
        let title = &titles[i];
        let label_w = 4u16
            .saturating_add(cell_width(&title.prefix))
            .saturating_add(cell_width(&title.name));
        let start = x;
        spans.push(Span::styled(format!(" {mark} "), style));
        if !title.prefix.is_empty() {
            spans.push(Span::styled(
                title.prefix.clone(),
                tab_prefix_style(ctx.theme, style, i == active, ctx.pane_focused),
            ));
        }
        spans.push(Span::styled(title.name.clone(), style));
        spans.push(Span::styled(" ", style));
        let pinned = tab.is_github_dashboard();
        spans.push(Span::styled(if pinned { " " } else { "\u{00d7}" }, style));
        spans.push(Span::styled(" ", style));
        let close = start + label_w;
        x = close + 2;
        hits.push(TabHit {
            start,
            end: x,
            close: if pinned { u16::MAX } else { close },
        });
    }
    let bar = Style::default().bg(ctx.theme.role(ThemeRole::Background).to_ratatui());
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
    let actions = tabs.get(active).map_or_else(Vec::new, pane_actions);
    let visible = usize::from(area.width / 3).min(actions.len());
    let start = area
        .right()
        .saturating_sub(u16::try_from(visible.saturating_mul(3)).unwrap_or(u16::MAX));
    let mut action_hits = Vec::with_capacity(visible);
    for (index, (icon, command, active)) in actions.into_iter().take(visible).enumerate() {
        let x = start.saturating_add(u16::try_from(index.saturating_mul(3)).unwrap_or(u16::MAX));
        let hovered = ctx
            .pane_action_hover
            .is_some_and(|(col, row)| row == area.y && col >= x && col < x.saturating_add(3));
        let state = match (active, hovered) {
            (true, true) => ChromeButtonState::ActiveHovered,
            (true, false) => ChromeButtonState::Active,
            (false, true) => ChromeButtonState::Hovered,
            (false, false) => ChromeButtonState::Normal,
        };
        f.buffer_mut().set_string(
            x,
            area.y,
            format!(" {} ", icon.glyph(ctx.icon_style)),
            chrome_button_style(ctx.theme, state)
                .bg(ctx.theme.role(ThemeRole::Background).to_ratatui()),
        );
        action_hits.push((x, x.saturating_add(3), command));
    }
    (area, hits, action_hits)
}

fn pane_actions(tab: &Tab) -> Vec<(UiIcon, Command, bool)> {
    match &tab.kind {
        TabKind::Code { path, .. }
            if karet_filetype::file_type_for_path(path).name() == "Markdown" =>
        {
            vec![
                (
                    UiIcon::Preview,
                    Command::MarkdownPreviewSide,
                    tab.markdown_preview.is_some(),
                ),
                (UiIcon::FormatTable, Command::FormatMarkdownTables, false),
            ]
        },
        TabKind::Code { path, .. }
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tex")) =>
        {
            vec![(UiIcon::Preview, Command::LatexBuildPreview, false)]
        },
        _ => Vec::new(),
    }
}

fn tab_text_style(theme: &Theme, active: bool, pane_focused: bool, preview: bool) -> Style {
    let mut style = if active && pane_focused {
        theme
            .style(ThemeRole::Foreground)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else if active {
        // Active tab of an unfocused pane: a distinct accent keeps pane ownership
        // visible without competing with the reversed tab in the focused pane.
        theme
            .style(ThemeRole::DiagnosticInfo)
            .add_modifier(Modifier::BOLD)
    } else {
        theme.style(ThemeRole::LineNumber)
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

fn tab_display_titles(
    tabs: &[Tab],
    root: &Path,
    icon_style: karet_filetype::IconStyle,
) -> Vec<TabDisplayTitle> {
    tabs.iter()
        .map(|tab| {
            let raw_name = tab_name(tab);
            let duplicate = tabs
                .iter()
                .filter(|other| tab_name(other) == raw_name)
                .count()
                > 1;
            let prefix = if duplicate {
                tab.path().and_then(|path| tab_parent_prefix(path, root))
            } else {
                None
            }
            .unwrap_or_default();
            let name = if tab.is_symlink {
                format!("{raw_name} {}", UiIcon::Symlink.glyph(icon_style))
            } else {
                raw_name
            };
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

/// How long a save must run before its spinner appears. This is the save-specific
/// reveal policy — deliberately slower than the shared `LOADING_REVEAL_DELAY`,
/// since most saves finish imperceptibly — and it lives here rather than in the
/// [`Spinner`] widget, which owns only the frame cycle.
const SPINNER_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// The scale at which document (PDF) pages are rasterized. Larger than a typical
/// pane so the Kitty protocol downscales (sharp) rather than upscales into the
/// reserved cell box; 2.0 ≈ 144 DPI for a native 72-DPI page.
#[cfg(feature = "pdf")]
const DOC_RENDER_SCALE: f32 = 2.0;

/// The 1-cell tab status mark: a spinner while a slow save writes, `●` for unsaved
/// changes, else blank. The slot is always one cell so the layout never shifts.
fn save_mark(tab: &Tab, icon_style: karet_filetype::IconStyle) -> char {
    if let Some(since) = tab.saving_since {
        let elapsed = since.elapsed();
        if elapsed >= SPINNER_DELAY {
            return Spinner::new(icon_style).frame(elapsed);
        }
    }
    if tab.dirty { '\u{25cf}' } else { ' ' }
}

/// Draw the pane's breadcrumb (the active tab's path components joined by `›`)
/// via the shared widget, and return the clickable segment regions.
fn draw_pane_breadcrumb(
    f: &mut Frame,
    tab: Option<&Tab>,
    theme: &Theme,
    root: &Path,
    icon_style: karet_filetype::IconStyle,
    area: Rect,
) -> Vec<crate::app::BreadcrumbHit> {
    let Some(path) = tab.and_then(Tab::path) else {
        return Vec::new();
    };
    karet_widgets::breadcrumbs::Breadcrumbs {
        path,
        root,
        is_symlink: tab.is_some_and(|tab| tab.is_symlink),
        icon_style,
    }
    .draw(f, theme, area)
}
