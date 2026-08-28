mod debug;
mod search;

use super::*;

/// Draw the right-side outline panel: a header over the active tab's navigation
/// outline (a depth-indented, selectable list). Records the content rect and syncs
/// the selection length for keyboard navigation and mouse hit-testing.
pub(super) fn draw_outline(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    app.request_active_outline();
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let header = rows[0];
    let content = rows[1];
    app.outline.content_rect = content;

    f.render_widget(
        Block::default().style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
        area,
    );
    f.render_widget(
        Paragraph::new(" OUTLINE").style(
            theme
                .style(ThemeRole::LineNumber)
                .add_modifier(Modifier::BOLD),
        ),
        header,
    );

    let entries = app.active_outline_rows();
    app.outline.sel.set_len(entries.len());
    if entries.is_empty() {
        let pending = app.active_outline_loading();
        let label = if pending.is_some_and(crate::app::Pending::visible) {
            " Loading…"
        } else if pending.is_some() {
            ""
        } else {
            " No outline"
        };
        f.render_widget(
            Paragraph::new(label).style(theme.style(ThemeRole::Muted)),
            content,
        );
        return;
    }

    let focused = app.focus == Focus::Outline;
    let cursor = app.outline.sel.cursor();
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
        .style(theme.style(ThemeRole::Foreground))
        .highlight_style(Style::default().bg(theme.role(sel_bg).to_ratatui()));
    let mut state = ListState::default();
    *state.offset_mut() = app.outline.scroll;
    state.select(Some(cursor));
    let (rows, tracks) = reserve_tracks(content, ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, rows, &mut state);
    // Remember where the list settled so a click maps to the right entry next frame.
    // Reading it back is also what makes the bar agree with the list ratatui drew,
    // which may have scrolled further to keep the cursor visible.
    app.outline.scroll = state.offset();
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(entries.len(), app.outline.scroll, rows.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::Outline,
    );
}

pub(super) fn draw_sidebar(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
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
            app.request_nested_repository_statuses();
            let repository_badges = app.nested_repository_badges(Instant::now());
            // The tree's rect is also its click target, so narrowing it here is what
            // makes a click on the track column fall outside the tree instead of
            // selecting whatever row it happens to sit beside.
            let (tree, tracks) = reserve_tracks(rows[1], ScrollAxes::VERTICAL);
            app.sidebar_content_rect = tree;
            f.render_stateful_widget(
                FileTree::new(&root)
                    .theme(theme)
                    .icons(icon_style)
                    .visible(&visible)
                    .active(active.as_deref())
                    .cut_paths(&cut_paths)
                    .explorer_focused(explorer_focused)
                    .hover(hover)
                    .badges(&repository_badges),
                tree,
                &mut app.explorer,
            );
            hits.record(
                tracks.paint(
                    f.buffer_mut(),
                    ScrollbarStyles::from_theme(theme),
                    ScrollExtent::new(
                        app.explorer.row_count(),
                        app.explorer.offset(),
                        tree.height.into(),
                    ),
                    ScrollExtent::default(),
                ),
                ScrollSurface::Explorer,
            );
        },
        SidebarPanel::SourceControl => draw_scm(f, app, theme, rows[1], hits),
        SidebarPanel::Search => search::draw_search_panel(f, app, theme, rows[1], hits),
        SidebarPanel::Spelling => draw_spelling_panel(f, app, theme, rows[1], hits),
        SidebarPanel::Todos => draw_todos_panel(f, app, theme, rows[1], hits),
        SidebarPanel::Debug => debug::draw_debug_panel(f, app, theme, rows[1], hits),
    }
}

pub(super) fn draw_context_menu(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let Some(menu) = app.context_menu.as_mut() else {
        return;
    };
    // Resolve the app-specific row texts (default command labels + key hints);
    // the shared widget owns placement, clamping, and painting.
    let hints: Vec<Option<String>> = menu
        .entries
        .iter()
        .map(|entry| {
            entry
                .command()
                .and_then(|command| keymap::hint_for(command, ChordStyle::Verbose))
        })
        .collect();
    let labels: Vec<String> = menu
        .entries
        .iter()
        .map(|entry| {
            entry.label.clone().unwrap_or_else(|| {
                entry
                    .command()
                    .map(context_menu_label)
                    .unwrap_or_default()
                    .to_string()
            })
        })
        .collect();
    menu.draw(f, theme, area, &labels, &hints);
}

pub(super) fn context_menu_label(command: Command) -> &'static str {
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
        Command::CopyPath => "Copy Path",
        Command::CopyRelativePath => "Copy Relative Path",
        Command::RevealActiveInExplorer => "Show File in Explorer",
        Command::CopyRemoteFileUrl => "Copy Remote File URL",
        Command::CopyGithubPermalink => "Copy GitHub Permalink",
        Command::CopyGithubHeadLink => "Copy GitHub Head Link",
        _ => command.label(),
    }
}

pub(super) fn draw_sidebar_header(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let name = match app.sidebar_panel {
        SidebarPanel::Explorer => "EXPLORER",
        SidebarPanel::Search => "SEARCH",
        SidebarPanel::SourceControl => "SOURCE CONTROL",
        SidebarPanel::Spelling => "SPELLING",
        SidebarPanel::Todos => "TODOS",
        SidebarPanel::Debug => "DEBUG",
    };
    // Header columns: a compact workspace-root label, the panel title, an
    // optional Explorer toolbar, then the activity-bar switcher (7 cells). The
    // toolbar (Explorer only) and then the root are dropped on a narrow sidebar so
    // the title and switcher always fit.
    const ROOT_MAX_W: u16 = 24;
    const ACTIONS_W: u16 = 8; // four buttons × 2 cells
    // The activity-bar switcher: one 2-cell button per shown panel, plus a
    // trailing cell. Spelling is offered only while spell check is on, Todos
    // only while codetag highlighting is.
    let spelling = app.spelling_available();
    let todos_shown = app.todos_available();
    // 3 always-on panels (2 cells each) + Debug + a trailing spacer cell.
    let switcher_w: u16 = 9 + if spelling { 2 } else { 0 } + if todos_shown { 2 } else { 0 };
    let icon_style = app.icon_style;
    let explorer = app.sidebar_panel == SidebarPanel::Explorer;
    let actions_w = if explorer && area.width >= 9 + ACTIONS_W + switcher_w {
        ACTIONS_W
    } else {
        0
    };
    let min_title_w = 9;
    let root_avail = area
        .width
        .saturating_sub(min_title_w + actions_w + switcher_w)
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
        Constraint::Length(switcher_w),
    ])
    .split(area);
    if show_root {
        let root_style = theme.style(ThemeRole::Muted).add_modifier(Modifier::BOLD);
        f.render_widget(
            Paragraph::new(Line::styled(format!(" {root_label}"), root_style)),
            cols[0],
        );
    }
    let title = theme
        .style(ThemeRole::LineNumberActive)
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
    let mut next_hit = switch.x + 6;
    if spelling {
        app.panel_hits
            .push((next_hit, next_hit + 2, SidebarPanel::Spelling));
        next_hit += 2;
    }
    let todos = app.todos_available();
    if todos {
        app.panel_hits
            .push((next_hit, next_hit + 2, SidebarPanel::Todos));
        next_hit += 2;
    }
    app.panel_hits
        .push((next_hit, next_hit + 2, SidebarPanel::Debug));
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
    let mut icons = vec![
        icon(UiIcon::Explorer, SidebarPanel::Explorer),
        icon(UiIcon::Search, SidebarPanel::Search),
        icon(UiIcon::SourceControl, SidebarPanel::SourceControl),
    ];
    if spelling {
        icons.push(icon(UiIcon::Spelling, SidebarPanel::Spelling));
    }
    if todos {
        icons.push(icon(UiIcon::Todos, SidebarPanel::Todos));
    }
    icons.push(icon(UiIcon::Debug, SidebarPanel::Debug));
    f.render_widget(Paragraph::new(Line::from(icons)), switch);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ChromeButtonState {
    Normal,
    Hovered,
    Active,
    ActiveHovered,
}

pub(super) fn chrome_button_style(theme: &Theme, state: ChromeButtonState) -> Style {
    match state {
        ChromeButtonState::Normal => theme.style(ThemeRole::LineNumber),
        ChromeButtonState::Hovered => theme.style(ThemeRole::LineNumberActive),
        ChromeButtonState::Active => theme
            .style(ThemeRole::LineNumberActive)
            .add_modifier(Modifier::BOLD),
        ChromeButtonState::ActiveHovered => theme
            .style(ThemeRole::Foreground)
            .add_modifier(Modifier::BOLD),
    }
}

pub(super) fn header_hovered(app: &App, start: u16, end: u16) -> bool {
    app.sidebar_header_hover
        .is_some_and(|(col, row)| row == app.sidebar_rect.y && col >= start && col < end)
}

pub(super) fn root_header_label(root: &Path, max_width: u16) -> String {
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

pub(super) fn truncate_left(text: &str, max_width: u16) -> String {
    karet_widgets::text::fit_start(text, usize::from(max_width))
}

pub(super) fn draw_spelling_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    use crate::app::SpellingRow;

    // A one-line toolbar (scan status on the left, the ⟳ re-scan action on the
    // right) over the results list.
    const ACTION_W: u16 = 8;
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let cols =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(ACTION_W)]).split(rows[0]);
    app.spelling_ui.results_rect = rows[1];
    app.spelling_ui.offset = 0;
    app.spelling_ui.action_hits = vec![(
        cols[1].x,
        cols[1].x + ACTION_W,
        rows[0].y,
        Command::SpellingScan,
    )];

    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let spelling = &app.spelling;

    let status = if spelling.scanning.is_some() {
        format!(" scanning… {} files", spelling.files_scanned)
    } else if spelling.truncated {
        format!(" {} shown (limit reached)", spelling.hits.len())
    } else if spelling.scanned {
        format!(
            " {} in {} files",
            spelling.hits.len(),
            spelling.files_scanned
        )
    } else {
        String::new()
    };
    f.render_widget(Paragraph::new(Line::styled(status, muted)), cols[0]);
    f.render_widget(
        Paragraph::new(Line::styled(" ⟳ scan", Style::default().fg(accent))),
        cols[1],
    );

    if spelling.rows.is_empty() {
        // No "spell check is off" case: the panel is only reachable while
        // `spellcheck.enabled` is on.
        let message = if spelling.scanning.is_some() {
            "  scanning the workspace…"
        } else if spelling.scanned {
            "  no misspellings"
        } else {
            "  press ⟳ to scan the workspace"
        };
        f.render_widget(Paragraph::new(Line::styled(message, dim)), rows[1]);
        return;
    }

    let items: Vec<ListItem> = spelling
        .rows
        .iter()
        .map(|row| match *row {
            SpellingRow::File { hit, count } => {
                let name = spelling.hits[hit]
                    .path
                    .strip_prefix(&app.root)
                    .unwrap_or(&spelling.hits[hit].path)
                    .to_string_lossy()
                    .into_owned();
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {name} ")),
                    Span::styled(format!("({count})"), dim),
                ]))
            },
            // The word leads (it is what the eye scans for), with its line as
            // dimmed context and the 1-based line number where a reader expects it.
            SpellingRow::Word { hit } => {
                let hit = &spelling.hits[hit];
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("   {} ", hit.word),
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{}: ", hit.range.start.line.saturating_add(1)), dim),
                    Span::styled(hit.line_text.clone(), muted),
                ]))
            },
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(spelling.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = spelling.rows.len();
    let (results, tracks) = reserve_tracks(rows[1], ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.spelling_ui.offset = state.offset();
    app.spelling_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, app.spelling_ui.offset, results.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::SpellingResults,
    );
}

/// Draw the Todos panel: a status/action toolbar over the grouped codetag list.
pub(super) fn draw_todos_panel(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    use crate::app::TodoRow;

    const ACTION_W: u16 = 16;
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let cols =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(ACTION_W)]).split(rows[0]);
    app.todos_ui.results_rect = rows[1];
    app.todos_ui.offset = 0;
    app.todos_ui.action_hits = vec![
        (cols[1].x, cols[1].x + 8, rows[0].y, Command::TodoScan),
        (
            cols[1].x + 8,
            cols[1].x + ACTION_W,
            rows[0].y,
            Command::TodoToggleGrouping,
        ),
    ];

    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let todos = &app.todos;

    let status = if todos.scanning.is_some() {
        format!(" scanning… {} files", todos.files_scanned)
    } else if todos.truncated {
        format!(" {} shown (limit reached)", todos.hits.len())
    } else if todos.scanned {
        format!(" {} in {} files", todos.hits.len(), todos.files_scanned)
    } else {
        String::new()
    };
    f.render_widget(Paragraph::new(Line::styled(status, muted)), cols[0]);
    let grouping = if todos.by_tag { "by tag" } else { "by file" };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ⟳ scan ", Style::default().fg(accent)),
            Span::styled(grouping.to_owned(), dim),
        ])),
        cols[1],
    );

    if todos.rows.is_empty() {
        let message = if todos.scanning.is_some() {
            "  scanning the workspace…"
        } else if todos.scanned {
            "  no codetags"
        } else {
            "  press ⟳ to scan the workspace"
        };
        f.render_widget(Paragraph::new(Line::styled(message, dim)), rows[1]);
        return;
    }

    // The list reserves a scrollbar track, so the text budget is one cell short
    // of the pane (`reserve_tracks` below carves the same column off).
    let list_width = rows[1].width.saturating_sub(1);
    let items: Vec<ListItem> = todos
        .rows
        .iter()
        .map(|row| match *row {
            TodoRow::Group { hit, count } => {
                let first = &todos.hits[todos.order[hit]];
                let name = if todos.by_tag {
                    first.tag.clone()
                } else {
                    first
                        .path
                        .strip_prefix(&app.root)
                        .unwrap_or(&first.path)
                        .to_string_lossy()
                        .into_owned()
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {name} ")),
                    Span::styled(format!("({count})"), dim),
                ]))
            },
            // The tag leads in accent, then the message, then the location.
            //
            // Grouped by tag, the tag is dropped: the group header above already
            // names it, and the sidebar is narrow enough that repeating it costs
            // the `file:line` its place — the one part telling the rows apart.
            TodoRow::Item { hit } => {
                let hit = &todos.hits[todos.order[hit]];
                let location = if todos.by_tag {
                    let file = hit
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    format!("{file}:{}", hit.line.saturating_add(1))
                } else {
                    format!("{}", hit.line.saturating_add(1))
                };
                let lead = if todos.by_tag {
                    "   ".to_owned()
                } else {
                    format!("   {} ", hit.tag)
                };
                // Budget the row so the location survives a long message: it is
                // what tells two rows apart, so the message yields to it rather
                // than pushing it off the edge.
                let spent =
                    karet_widgets::text::width(&lead) + karet_widgets::text::width(&location);
                let message = karet_widgets::text::fit_end(
                    &hit.message,
                    usize::from(list_width).saturating_sub(spent + 1),
                );
                let mut spans = Vec::with_capacity(3);
                if todos.by_tag {
                    spans.push(Span::raw(lead));
                } else {
                    spans.push(Span::styled(
                        lead,
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    ));
                }
                spans.push(Span::styled(format!("{message} "), muted));
                spans.push(Span::styled(location, dim));
                ListItem::new(Line::from(spans))
            },
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(todos.selection.cursor()));
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(theme.role(ThemeRole::Selection).to_ratatui())
            .add_modifier(Modifier::BOLD),
    );
    let total = todos.rows.len();
    let (results, tracks) = reserve_tracks(rows[1], ScrollAxes::VERTICAL);
    f.render_stateful_widget(list, results, &mut state);
    app.todos_ui.offset = state.offset();
    app.todos_ui.results_rect = results;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, state.offset(), results.height.into()),
            ScrollExtent::default(),
        ),
        ScrollSurface::TodoResults,
    );
}
