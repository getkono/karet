use super::*;

/// Draw every pane (tab strip + breadcrumb + content) tiled across `area`, recording
/// each pane's clickable regions for mouse routing.
pub(super) fn draw_panes(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.pane_frames.clear();
    app.image_area = None;
    app.editor_rect = Rect::default();
    app.markdown_preview_rect = Rect::default();
    app.blame_rect = None;
    app.markdown_link_hits.clear();
    app.commit_badge_rect = None;
    let focused = app.focus_pane();
    let editor_focused = app.focus == Focus::Editor;
    let graphics = app.graphics;
    let graphical_cursor = app.graphical_cursor_enabled();
    for (pane, rect) in app.layout.layout(area) {
        let is_focused = pane == focused;
        let stored_tab_width = (!is_focused)
            .then(|| app.stored.get(&pane))
            .flatten()
            .and_then(|stored| stored.tabs.get(stored.active))
            .map(|tab| app.tab_width_for(tab));
        let rendered = if is_focused {
            let resolved = app.tabs.get(app.active).map(|tab| {
                app.settings
                    .editor
                    .for_language(crate::app::tab_language(tab))
            });
            let word_wrap = resolved.map_or(app.settings.editor.word_wrap, |r| r.word_wrap());
            let sticky_scroll =
                resolved.map_or(app.settings.editor.sticky_scroll, |r| r.sticky_scroll());
            let tab_width = app
                .tabs
                .get(app.active)
                .map_or(4, |tab| app.tab_width_for(tab));
            let blame = app
                .live_blame
                .as_ref()
                .and_then(crate::app::LiveBlame::decoration);
            let ctx = PaneCtx {
                theme,
                root: &app.root,
                icon_style: app.icon_style,
                graphics,
                pane_focused: true,
                editor_focused,
                graphical_cursor,
                word_wrap,
                sticky_scroll,
                tab_width,
                diagnostics: &app.document_diagnostics,
                find: app
                    .find_open
                    .then(|| app.tabs.get(app.active))
                    .flatten()
                    .and_then(|t| t.find.clone()),
                blame,
                blame_clickable: app
                    .live_blame
                    .as_ref()
                    .and_then(crate::app::LiveBlame::commit_hash)
                    .is_some(),
                markdown_link_hover: app.markdown_link_hover,
                pane_action_hover: app.pane_action_hover,
            };
            render_pane(f, &mut app.tabs, app.active, rect, &ctx)
        } else if let Some(stored) = app.stored.get_mut(&pane) {
            let resolved = stored.tabs.get(stored.active).map(|tab| {
                app.settings
                    .editor
                    .for_language(crate::app::tab_language(tab))
            });
            let word_wrap = resolved.map_or(app.settings.editor.word_wrap, |r| r.word_wrap());
            let sticky_scroll =
                resolved.map_or(app.settings.editor.sticky_scroll, |r| r.sticky_scroll());
            let tab_width = stored_tab_width.unwrap_or(4);
            let ctx = PaneCtx {
                theme,
                root: &app.root,
                icon_style: app.icon_style,
                graphics,
                pane_focused: false,
                editor_focused: false,
                graphical_cursor: false,
                word_wrap,
                sticky_scroll,
                tab_width,
                diagnostics: &app.document_diagnostics,
                find: None,
                blame: None,
                blame_clickable: false,
                markdown_link_hover: None,
                pane_action_hover: app.pane_action_hover,
            };
            render_pane(f, &mut stored.tabs, stored.active, rect, &ctx)
        } else {
            continue;
        };
        if is_focused {
            app.editor_rect = rendered.editor_rect;
            app.markdown_preview_rect = rendered.markdown_preview_rect;
            app.image_area = rendered.image_area;
            app.blame_rect = rendered.blame_rect;
            app.markdown_link_hits = rendered.markdown_link_hits;
            app.commit_badge_rect = rendered.commit_badge_rect;
        }
        app.pane_frames.push(crate::app::PaneFrame {
            pane,
            tabstrip_rect: rendered.tabstrip_rect,
            tab_hits: rendered.tab_hits,
            action_hits: rendered.action_hits,
            breadcrumb_rect: rendered.breadcrumb_rect,
            breadcrumb_hits: rendered.breadcrumb_hits,
            content_rect: rendered.content_rect,
            commit_file_hits: rendered.commit_file_hits,
        });
    }
    app.pane_dividers = app.layout.dividers(area);
    for divider in app.pane_dividers.iter().copied() {
        let emphasized = app.pane_divider_hover == Some(divider)
            || app.pane_resize.is_some_and(|resize| {
                resize.divider.axis == divider.axis
                    && resize.divider.before == divider.before
                    && resize.divider.after == divider.after
            });
        let role = if emphasized {
            ThemeRole::LineNumberActive
        } else {
            ThemeRole::IndentGuide
        };
        let style = Style::default().fg(theme.role(role).to_ratatui());
        match divider.axis {
            SplitAxis::Cols => {
                for y in divider.start..divider.end {
                    f.buffer_mut().set_string(divider.position, y, "│", style);
                }
            },
            SplitAxis::Rows => {
                let style = if emphasized {
                    style.bg(theme.role(ThemeRole::HoverHighlight).to_ratatui())
                } else {
                    style.add_modifier(Modifier::UNDERLINED)
                };
                f.buffer_mut().set_style(
                    Rect::new(
                        divider.start,
                        divider.position,
                        divider.end.saturating_sub(divider.start),
                        1,
                    ),
                    style,
                );
            },
        }
    }
}

/// Render one pane into `area`: its tab strip, optional breadcrumb, an optional find
/// bar (focused pane), and the active tab's content.
pub(super) fn render_pane(
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
    let (tabstrip_rect, tab_hits, action_hits) = draw_pane_tabs(f, tabs, active, ctx, parts[0]);
    let (breadcrumb_rect, breadcrumb_hits) = if bc == 1 {
        let hits = draw_pane_breadcrumb(
            f,
            tabs.get(active),
            ctx.theme,
            ctx.root,
            ctx.icon_style,
            parts[1],
        );
        (parts[1], hits)
    } else {
        (Rect::default(), Vec::new())
    };
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
        action_hits,
        breadcrumb_rect,
        breadcrumb_hits,
        content_rect: content,
        editor_rect: painted.editor_rect,
        markdown_preview_rect: painted.markdown_preview_rect,
        image_area: painted.image_area,
        commit_badge_rect: painted.badge_rect,
        commit_file_hits: painted.file_hits,
        blame_rect: painted.blame_rect,
        markdown_link_hits: painted.markdown_link_hits,
    }
}

/// While dragging a tab over another pane, tint the region it would land in (a half
/// for an edge split, the whole pane for a center move) — VS Code's drop preview.
pub(super) fn draw_drop_preview(f: &mut Frame, app: &App, theme: &Theme) {
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
/// Draw the completion popup anchored at the caret: aligned with the start of
/// the word being completed, below the caret's line, or above it when there is
/// not enough room below. Sized so the widest label+detail row is fully
/// visible within the editor area (the widget truncates details with an
/// ellipsis when the editor itself is too narrow).
pub(super) fn draw_completion(f: &mut Frame, app: &mut App, theme: &Theme) {
    // The filter doubles as the "popup still applies" check.
    let Some(filter) = app.completion_filter() else {
        return;
    };
    let editor = app.editor_rect;
    if editor.width == 0 || editor.height == 0 {
        return;
    }
    let caret_cell = {
        let Some(tab) = app.tabs.get(app.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            return;
        };
        let fold_lines = crate::app::resolve_folds(folds, folded);
        tab.editor.primary_caret_cell(editor, buffer, &fold_lines)
    };
    let Some((caret_x, caret_y)) = caret_cell else {
        return; // caret scrolled out of view
    };
    // Align the popup with the completed word's start (identifier characters
    // are single-column, so char count is column count here).
    let prefix_cols = u16::try_from(filter.chars().count()).unwrap_or(0);
    let x = caret_x
        .saturating_sub(prefix_cols)
        .clamp(editor.left(), editor.right().saturating_sub(1));

    let Some(ui) = app.completion.as_mut() else {
        return;
    };
    if filter != ui.last_filter {
        ui.list.reset();
        ui.last_filter.clone_from(&filter);
    }
    let crate::completion::CompletionUi { items, list, .. } = ui;
    let mut popup = CompletionPopup::new(items, &mut app.completion_matcher, &filter, theme);
    let avail_w = editor.right().saturating_sub(x).max(1);
    let (width, rows) = popup.desired_size((avail_w, karet_widgets::completion::MAX_VISIBLE_ROWS));
    if width == 0 || rows == 0 {
        return; // nothing matches the filter
    }
    let below = editor.bottom().saturating_sub(caret_y.saturating_add(1));
    let above = caret_y.saturating_sub(editor.top());
    let (y, height) = if below >= rows {
        (caret_y + 1, rows)
    } else if above >= rows {
        (caret_y - rows, rows)
    } else if below >= above {
        (caret_y + 1, below)
    } else {
        (caret_y.saturating_sub(above), above)
    };
    if height == 0 {
        return;
    }
    let rect = Rect::new(x, y, width, height);
    f.render_widget(Clear, rect);
    f.render_stateful_widget(popup, rect, list);
}

pub(super) fn draw_toasts(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
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
pub(super) fn draw_find_bar(f: &mut Frame, find: &FindState, theme: &Theme, area: Rect) {
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
pub(super) fn draw_overlay(f: &mut Frame, overlay: &Overlay, theme: &Theme, area: Rect) {
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
