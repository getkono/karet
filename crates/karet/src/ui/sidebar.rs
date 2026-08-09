use super::*;

/// Draw the right-side outline panel: a header over the active tab's navigation
/// outline (a depth-indented, selectable list). Records the content rect and syncs
/// the selection length for keyboard navigation and mouse hit-testing.
pub(super) fn draw_outline(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.request_active_outline();
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
            theme
                .style(ThemeRole::LineNumber)
                .add_modifier(Modifier::BOLD),
        ),
        header,
    );

    let entries = app.active_outline_rows();
    app.outline_sel.set_len(entries.len());
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
        .style(theme.style(ThemeRole::Foreground))
        .highlight_style(Style::default().bg(theme.role(sel_bg).to_ratatui()));
    let mut state = ListState::default();
    *state.offset_mut() = app.outline_scroll;
    state.select(Some(cursor));
    f.render_stateful_widget(list, content, &mut state);
    // Remember where the list settled so a click maps to the right entry next frame.
    app.outline_scroll = state.offset();
}

pub(super) fn draw_sidebar(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
                rows[1],
                &mut app.explorer,
            );
        },
        SidebarPanel::SourceControl => draw_scm(f, app, theme, rows[1]),
        SidebarPanel::Search => draw_search_panel(f, app, theme, rows[1]),
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

pub(super) fn draw_search_panel(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
    let find_prefix = Rect {
        width: find_cols[0].width.min(3),
        ..find_cols[0]
    };
    let find_field = Rect {
        x: find_prefix.right(),
        width: find_cols[0].width.saturating_sub(find_prefix.width),
        ..find_cols[0]
    };
    app.search_query_rect = find_field;
    app.search
        .query_edit
        .ensure_cursor_visible(&app.search.query, find_field.width);
    f.render_widget(Paragraph::new(Line::styled(" › ", find_style)), find_prefix);
    let selection = theme.role(ThemeRole::Selection).to_ratatui();
    f.render_widget(
        Paragraph::new(text_field_text(
            &app.search.query,
            &app.search.query_edit,
            editing_find,
            find_style,
            find_style.bg(selection),
            Style::default().fg(accent),
        ))
        .scroll((0, app.search.query_edit.scroll)),
        find_field,
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
        let rep_prefix = Rect {
            width: rep_cols[0].width.min(3),
            ..rep_cols[0]
        };
        let rep_field = Rect {
            x: rep_prefix.right(),
            width: rep_cols[0].width.saturating_sub(rep_prefix.width),
            ..rep_cols[0]
        };
        app.search_replace_rect = Some(rep_field);
        app.search
            .replace_edit
            .ensure_cursor_visible(&app.search.replace, rep_field.width);
        f.render_widget(Paragraph::new(Line::styled(" ⇄ ", rep_style)), rep_prefix);
        f.render_widget(
            Paragraph::new(text_field_text(
                &app.search.replace,
                &app.search.replace_edit,
                editing_replace,
                rep_style,
                rep_style.bg(selection),
                Style::default().fg(accent),
            ))
            .scroll((0, app.search.replace_edit.scroll)),
            rep_field,
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
    } else {
        app.search_replace_rect = None;
    }

    let search = &app.search;
    if search.results.is_empty() {
        let hint = theme.style(ThemeRole::LineNumber);
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
                    theme.style(ThemeRole::LineNumber),
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
