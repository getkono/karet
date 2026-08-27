use super::*;

mod swatches;

/// Draw one pane's active tab into `area`. Returns the rect to reserve for a Kitty
/// image, if the active tab is an image on a Kitty terminal.
pub(super) fn draw_pane_content(
    f: &mut Frame,
    tabs: &mut [Tab],
    active: usize,
    ctx: &PaneCtx,
    area: Rect,
    hits: &mut ScrollHits,
) -> PaneContent {
    let theme = ctx.theme;
    let Some(tab) = tabs.get_mut(active) else {
        return PaneContent::default();
    };
    let word_wrap = crate::app::effective_word_wrap(tab, ctx.word_wrap);
    // Written by the image/PDF render arms; stays `None` (and non-`mut`) when neither
    // media feature is compiled in.
    #[cfg(any(feature = "images", feature = "pdf"))]
    let mut image_area = None;
    #[cfg(not(any(feature = "images", feature = "pdf")))]
    let image_area: Option<Rect> = None;
    let mut badge_rect = None;
    let mut file_hits = Vec::new();
    let mut collapse_hits = Vec::new();
    let mut blame_rect = None;
    let mut markdown_link_hits = Vec::new();
    let mut editor_rect = area;
    let mut markdown_preview_rect = Rect::default();
    match &mut tab.kind {
        TabKind::Welcome => draw_welcome(f, theme, area),
        TabKind::Seam(state) => super::seam::draw_seam(f, theme, area, state, ctx.icon_style),
        TabKind::LanguageServers(view) => draw_language_servers(f, theme, area, view, hits),
        TabKind::Github(view) => draw_github(f, theme, area, view, hits),
        TabKind::Code {
            path,
            doc,
            buffer,
            highlights,
            semantic_blocks,
            folds,
            folded,
            decos,
            search_decos,
            ..
        } => {
            if let Some(conflict) = tab.merge_conflict.as_mut() {
                let fold_lines = crate::app::resolve_folds(folds, folded);
                let version = buffer.version();
                if tab
                    .conflict_decorations
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != version)
                {
                    tab.conflict_decorations =
                        Some((version, karet_editor::conflict_decorations(&buffer.text())));
                }
                let conflict_decorations = tab
                    .conflict_decorations
                    .as_ref()
                    .map_or(&[][..], |(_, decorations)| decorations.as_slice());
                let combined: Vec<Decoration> = decos
                    .iter()
                    .chain(search_decos.iter())
                    .chain(conflict_decorations.iter())
                    .cloned()
                    .collect();
                let diagnostics = doc
                    .and_then(|doc| ctx.diagnostics.get(&doc))
                    .map_or(&[][..], Vec::as_slice);
                editor_rect = draw_merge_conflict(
                    f,
                    theme,
                    area,
                    word_wrap,
                    buffer,
                    highlights,
                    semantic_blocks,
                    &fold_lines,
                    &combined,
                    diagnostics,
                    &mut tab.editor,
                    conflict,
                    ctx,
                    hits,
                );
            } else {
                if tab.markdown_preview.is_some() && area.width >= 3 {
                    let columns = Layout::horizontal([
                        Constraint::Percentage(50),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .split(area);
                    editor_rect = columns[0];
                    markdown_preview_rect = columns[2];
                    f.render_widget(
                        Block::default()
                            .borders(Borders::LEFT)
                            .border_style(theme.style(ThemeRole::IndentGuide)),
                        columns[1],
                    );
                }
                let fold_lines = crate::app::resolve_folds(folds, folded);
                let version = buffer.version();
                if tab
                    .conflict_decorations
                    .as_ref()
                    .is_none_or(|(cached, _)| *cached != version)
                {
                    tab.conflict_decorations =
                        Some((version, karet_editor::conflict_decorations(&buffer.text())));
                }
                let conflict_decorations = tab
                    .conflict_decorations
                    .as_ref()
                    .map_or(&[][..], |(_, decorations)| decorations.as_slice());
                let table_lines = if karet_filetype::file_type_for_path(path).name() == "Markdown" {
                    if tab
                        .markdown_table_lines
                        .as_ref()
                        .is_none_or(|(cached, _)| *cached != version)
                    {
                        tab.markdown_table_lines =
                            Some((version, karet_markdown::table_line_ranges(&buffer.text())));
                    }
                    tab.markdown_table_lines
                        .as_ref()
                        .map_or(&[][..], |(_, ranges)| ranges.as_slice())
                } else {
                    &[]
                };
                // Local find and global search highlights are kept in separate
                // fields (so closing/rerunning one can't wipe the other) and
                // combined only here, at render time.
                let mut frame_decos =
                    swatches::frame_decorations(ctx, *doc, buffer, tab.editor.scroll_line, area);
                frame_decos.extend(swatches::debug_decorations(ctx, path));
                let combined: Vec<Decoration> = decos
                    .iter()
                    .chain(search_decos.iter())
                    .chain(conflict_decorations.iter())
                    .chain(ctx.blame.iter())
                    .chain(ctx.definition_underline.iter())
                    .chain(frame_decos.iter())
                    .cloned()
                    .collect();
                let diagnostics = doc
                    .and_then(|doc| ctx.diagnostics.get(&doc))
                    .map_or(&[][..], Vec::as_slice);
                let editor = Editor::new(buffer)
                    .highlights(highlights)
                    .semantic_blocks(semantic_blocks)
                    .theme(theme)
                    .decorations(&combined)
                    .diagnostics(diagnostics)
                    .folds(&fold_lines)
                    .focused(ctx.editor_focused)
                    .cell_caret(!ctx.graphical_cursor)
                    .word_wrap(word_wrap)
                    .tab_width(ctx.tab_width)
                    .unwrapped_lines(table_lines);
                let editor = editor.sticky_scroll(ctx.sticky_scroll);
                // Reserve the tracks before rendering, and keep `editor_rect` as the
                // reserved rect: it is the single value used both to paint the widget
                // and to map a click back to a caret, so shrinking it once keeps
                // render and hit-test in agreement. A soft-wrapped view has no
                // horizontal axis at all, so it reserves no bottom row.
                let (text_rect, tracks) = reserve_tracks(
                    editor_rect,
                    ScrollAxes {
                        vertical: true,
                        horizontal: !word_wrap,
                    },
                );
                editor_rect = text_rect;
                f.render_stateful_widget(editor, editor_rect, &mut tab.editor);
                // Read the extents back after the render: the widget clamps the
                // offsets and measures the viewport as it paints, so the bar is
                // correct on the very first frame.
                hits.record_both(
                    tracks.paint(
                        f.buffer_mut(),
                        ScrollbarStyles::from_theme(theme),
                        ScrollExtent::new(
                            buffer.line_count(),
                            tab.editor.scroll_line as usize,
                            tab.editor.visible_lines() as usize,
                        ),
                        ScrollExtent::new(
                            tab.editor.longest_col() as usize,
                            tab.editor.scroll_col as usize,
                            tab.editor.content_width().into(),
                        ),
                    ),
                    ScrollSurface::TabRows,
                    ScrollSurface::TabColumns,
                );
                if ctx.blame_clickable
                    && let Some(Decoration {
                        range,
                        kind:
                            karet_core::DecorationKind::InlineText {
                                text,
                                before: false,
                            },
                        ..
                    }) = ctx.blame.as_ref()
                {
                    let end = buffer
                        .line(range.start.line as usize)
                        .map_or(0, |line| line.chars().count() as u32);
                    if let Some((x, y)) = tab.editor.screen_cell(
                        editor_rect,
                        buffer,
                        &fold_lines,
                        karet_core::LineCol::new(range.start.line, end),
                    ) {
                        let width = u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX);
                        let visible = width.min(editor_rect.right().saturating_sub(x));
                        if visible > 0 {
                            blame_rect = Some(Rect::new(x, y, visible, 1));
                        }
                    }
                }
                if let Some(preview) = tab.markdown_preview.as_mut()
                    && markdown_preview_rect.width > 0
                {
                    markdown_link_hits = draw_markdown_preview(
                        f,
                        theme,
                        markdown_preview_rect,
                        MarkdownPreviewRender {
                            buffer,
                            wrapped: &mut preview.wrapped,
                            rendered: &mut preview.rendered,
                            scroll: &mut preview.scroll,
                            hover: ctx.markdown_link_hover,
                            source: path,
                            root: ctx.root,
                            source_scroll: Some(tab.editor.scroll_line as usize),
                            mermaid: ctx.mermaid,
                        },
                        hits,
                        ScrollSurface::EditorPreview,
                    );
                }
            }
        },
        TabKind::MarkdownPreview {
            path,
            buffer,
            wrapped,
            rendered,
            scroll,
            ..
        } => {
            markdown_link_hits = draw_markdown_preview(
                f,
                theme,
                area,
                MarkdownPreviewRender {
                    buffer,
                    wrapped,
                    rendered,
                    scroll,
                    hover: ctx.markdown_link_hover,
                    source: path,
                    root: ctx.root,
                    source_scroll: None,
                    mermaid: ctx.mermaid,
                },
                hits,
                ScrollSurface::TabRows,
            );
        },
        TabKind::Diff {
            file,
            loading_since,
            error,
            view,
            pager,
            ..
        } => match (file, &*error) {
            (Some(file), _) => draw_diff(
                f,
                theme,
                area,
                file,
                *view,
                &mut pager.scroll,
                &mut pager.column,
                hits,
            ),
            (None, Some(error)) => f.render_widget(
                Paragraph::new(error.as_str()).style(theme.style(ThemeRole::DiagnosticError)),
                area,
            ),
            // The diff is still being prepared: a stable, muted placeholder after
            // the shared reveal delay; nothing before it (fast paths never flash).
            (None, None) => {
                if loading_since.is_some_and(Pending::visible) {
                    f.render_widget(
                        Paragraph::new("Loading diff…").style(theme.style(ThemeRole::Muted)),
                        area,
                    );
                }
            },
        },
        TabKind::StashPreview { patch, pager, .. } => {
            let lines: Vec<Line> = patch
                .lines()
                .map(|line| Line::raw(line.to_string()))
                .collect();
            hits.record_both(
                draw_scrollable_lines(f, theme, area, lines, &mut pager.scroll, &mut pager.column),
                ScrollSurface::TabRows,
                ScrollSurface::TabColumns,
            );
        },
        TabKind::Graph { title, view, pager } => draw_graph(
            f,
            theme,
            area,
            title,
            view,
            &mut pager.scroll,
            &mut pager.column,
            hits,
        ),
        TabKind::LoadedConfig { report, pager } => {
            draw_loaded_config(
                f,
                theme,
                area,
                report,
                &mut pager.scroll,
                &mut pager.column,
                hits,
            );
        },
        TabKind::Commit {
            detail,
            files,
            explain_since,
            view,
        } => {
            let painted = draw_commit(f, theme, area, detail, files, *explain_since, view, hits);
            badge_rect = painted.badge_rect;
            file_hits = painted.file_hits;
            collapse_hits = painted.collapse_hits;
        },
        TabKind::CommitLoading {
            rev,
            loading_since,
            error,
            pager,
        } => draw_commit_loading(
            f,
            theme,
            area,
            rev,
            *loading_since,
            error.as_deref(),
            &mut pager.scroll,
            &mut pager.column,
            hits,
        ),
        TabKind::Compare {
            base_label,
            head_label,
            merge_base,
            files,
            view,
        } => {
            let painted = draw_compare(
                f,
                theme,
                area,
                base_label,
                head_label,
                *merge_base,
                files,
                view,
                hits,
            );
            file_hits = painted.file_hits;
            collapse_hits = painted.collapse_hits;
        },
        TabKind::CommitGraph {
            history_path,
            commits,
            rails,
            has_more,
            loading,
            loading_since,
            selected,
            compare_base: _,
            list_offset,
            column,
            list_rect,
        } => draw_commit_graph(
            f,
            theme,
            area,
            &CommitGraphInput {
                history_path: history_path.as_deref(),
                commits,
                rails,
                labels: ctx.ref_labels,
                repo_state: ctx.repo_state,
                has_more: *has_more,
                loading: *loading,
                loading_since: *loading_since,
                selected: *selected,
            },
            CommitGraphScroll {
                list_offset,
                column,
                list_rect,
            },
            hits,
        ),
        TabKind::Hex { bytes, scroll, .. } => {
            let rows = bytes.len().div_ceil(16);
            *scroll = (*scroll).min(rows.saturating_sub(1));
            let (dump, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
            f.render_widget(HexView::new(bytes).scroll(*scroll).theme(theme), dump);
            hits.record(
                tracks.paint(
                    f.buffer_mut(),
                    ScrollbarStyles::from_theme(theme),
                    ScrollExtent::new(rows, *scroll, dump.height.into()),
                    ScrollExtent::default(),
                ),
                ScrollSurface::TabRows,
            );
        },
        #[cfg(feature = "images")]
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
        #[cfg(feature = "pdf")]
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
                // Reserve a one-row footer for the page indicator, then a scroll track
                // out of what is left. The bar tracks the current page's position in
                // the document, so one page is the whole "viewport".
                let footer_h = u16::from(page_count > 1 && area.height > 3);
                let body = Rect {
                    height: area.height - footer_h,
                    ..area
                };
                let (content, tracks) = reserve_tracks(
                    body,
                    ScrollAxes {
                        vertical: page_count > 1,
                        horizontal: false,
                    },
                );
                if let Some((_, img)) = rendered.as_ref() {
                    // Reserve an aspect-fit sub-rect so the page is not stretched.
                    image_area = Some(fit_rect(content, img.width(), img.height()));
                } else {
                    // Parsed, but this page failed to rasterize — show a neutral note.
                    f.render_widget(Placeholder::new(path, FileKind::Pdf, None, 0), content);
                }
                hits.record(
                    tracks.paint(
                        f.buffer_mut(),
                        ScrollbarStyles::from_theme(theme),
                        ScrollExtent::new(page_count, idx, 1),
                        ScrollExtent::default(),
                    ),
                    ScrollSurface::TabRows,
                );
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
                        .style(theme.style(ThemeRole::LineNumber)),
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
        TabKind::LatexPreview {
            source,
            loading_since,
            error,
        } => {
            let message = error
                .as_deref()
                .or_else(|| loading_since.visible().then_some("Building LaTeX preview…"));
            if let Some(message) = message {
                let detail = source.file_name().map_or_else(
                    || source.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                f.render_widget(
                    Paragraph::new(format!("{message}\n{detail}"))
                        .alignment(Alignment::Center)
                        .style(theme.style(ThemeRole::Muted)),
                    area,
                );
            }
        },
    }
    PaneContent {
        editor_rect,
        markdown_preview_rect,
        image_area,
        badge_rect,
        file_hits,
        collapse_hits,
        blame_rect,
        markdown_link_hits,
    }
}

#[allow(clippy::too_many_arguments)] // the three editor panes share one render context
fn draw_merge_conflict(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    word_wrap: bool,
    merged: &TextBuffer,
    highlights: &Highlights,
    semantic_blocks: &SemanticBlocks,
    folds: &[karet_editor::Fold],
    decorations: &[Decoration],
    diagnostics: &[Diagnostic],
    merged_editor: &mut EditorState,
    conflict: &mut MergeConflictState,
    ctx: &PaneCtx,
    hits: &mut ScrollHits,
) -> Rect {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let columns = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Length(1),
        Constraint::Ratio(1, 3),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(rows[1]);
    let labels = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Length(1),
        Constraint::Ratio(1, 3),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(rows[0]);
    let muted = theme.style(ThemeRole::Muted).add_modifier(Modifier::BOLD);
    let active = theme
        .style(ThemeRole::Foreground)
        .bg(theme.role(ThemeRole::Selection).to_ratatui())
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(" CURRENT · read-only ").style(muted),
        labels[0],
    );
    f.render_widget(
        Paragraph::new(" MERGED · editable ").style(active),
        labels[2],
    );
    f.render_widget(
        Paragraph::new(" INCOMING · read-only ").style(muted),
        labels[4],
    );
    let divider_style = theme.style(ThemeRole::IndentGuide);
    f.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(divider_style),
        columns[1],
    );
    f.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(divider_style),
        columns[3],
    );

    conflict.current_editor.scroll_line = merged_editor.scroll_line;
    conflict.current_editor.scroll_col = merged_editor.scroll_col;
    conflict.incoming_editor.scroll_line = merged_editor.scroll_line;
    conflict.incoming_editor.scroll_col = merged_editor.scroll_col;
    draw_conflict_side(
        f,
        theme,
        columns[0],
        conflict.current.as_ref(),
        &mut conflict.current_editor,
        conflict.loading_since,
        conflict.error.as_deref(),
        word_wrap,
        ctx,
        hits,
    );
    draw_conflict_side(
        f,
        theme,
        columns[4],
        conflict.incoming.as_ref(),
        &mut conflict.incoming_editor,
        conflict.loading_since,
        conflict.error.as_deref(),
        word_wrap,
        ctx,
        hits,
    );
    let editor = Editor::new(merged)
        .highlights(highlights)
        .semantic_blocks(semantic_blocks)
        .theme(theme)
        .decorations(decorations)
        .diagnostics(diagnostics)
        .folds(folds)
        .focused(ctx.editor_focused)
        .cell_caret(!ctx.graphical_cursor)
        .word_wrap(word_wrap)
        .tab_width(ctx.tab_width)
        .sticky_scroll(ctx.sticky_scroll);
    // The three panes are already narrow and share one horizontal offset, so this
    // view reserves vertical tracks only. The merged rect is returned shrunk: it is
    // the caller's `editor_rect`, and so also its click-to-caret mapping.
    let (merged_rect, tracks) = reserve_tracks(columns[2], ScrollAxes::VERTICAL);
    f.render_stateful_widget(editor, merged_rect, merged_editor);
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(
                merged.line_count(),
                merged_editor.scroll_line as usize,
                merged_editor.visible_lines() as usize,
            ),
            ScrollExtent::default(),
        ),
        ScrollSurface::TabRows,
    );
    merged_rect
}

#[allow(clippy::too_many_arguments)] // presentation state and delayed-load state are independent
fn draw_conflict_side(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    buffer: Option<&TextBuffer>,
    editor: &mut EditorState,
    loading_since: Pending,
    error: Option<&str>,
    word_wrap: bool,
    ctx: &PaneCtx,
    hits: &mut ScrollHits,
) {
    if let Some(buffer) = buffer {
        let (text_rect, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        f.render_stateful_widget(
            Editor::new(buffer)
                .theme(theme)
                .focused(false)
                .read_only(true)
                .word_wrap(word_wrap)
                .tab_width(ctx.tab_width),
            text_rect,
            editor,
        );
        // The side panes copy the merged editor's offset every frame, so their bars
        // grab the same thing the middle one does.
        hits.record(
            tracks.paint(
                f.buffer_mut(),
                ScrollbarStyles::from_theme(theme),
                ScrollExtent::new(
                    buffer.line_count(),
                    editor.scroll_line as usize,
                    editor.visible_lines() as usize,
                ),
                ScrollExtent::default(),
            ),
            ScrollSurface::TabRows,
        );
    } else if let Some(error) = error {
        f.render_widget(
            Paragraph::new(error).style(theme.style(ThemeRole::DiagnosticError)),
            area,
        );
    } else if loading_since.visible() {
        f.render_widget(
            Paragraph::new("Loading conflict side…").style(theme.style(ThemeRole::Muted)),
            area,
        );
    }
}

#[allow(clippy::too_many_arguments)] // diff model, layout mode, scroll offsets and the track sink are independent
pub(super) fn draw_diff(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    file: &render::FileView,
    view: ViewMode,
    scroll: &mut u16,
    column: &mut u16,
    hits: &mut ScrollHits,
) {
    match view {
        ViewMode::Unified => {
            let mut lines = render::unified_lines(file, theme);
            render::pad_diff_lines(&mut lines, area.width);
            hits.record_both(
                draw_scrollable_lines(f, theme, area, lines, scroll, column),
                ScrollSurface::TabRows,
                ScrollSurface::TabColumns,
            );
        },
        ViewMode::SideBySide => {
            // The panes scroll together, so the view carries one vertical track for
            // the pair — but each pane has its own content width, so the horizontal
            // track is split to match the panes above it.
            let (body, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
            let (mut left, mut right) = render::side_by_side_lines(file, theme);
            let height = left.len().max(right.len());
            let max = u16::try_from(height)
                .unwrap_or(u16::MAX)
                .saturating_sub(body.height);
            *scroll = (*scroll).min(max);
            let constraints = [
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Min(0),
            ];
            let panes = Layout::horizontal(constraints).split(body);
            let left_width = left.iter().map(line_width).max().unwrap_or_default();
            let right_width = right.iter().map(line_width).max().unwrap_or_default();
            let content_width = left_width.max(right_width);
            let pane_width = panes[0].width.min(panes[2].width);
            let max_column = content_width.saturating_sub(usize::from(pane_width));
            *column = (*column).min(u16::try_from(max_column).unwrap_or(u16::MAX));
            render::pad_diff_lines(&mut left, panes[0].width);
            render::pad_diff_lines(&mut right, panes[2].width);
            f.render_widget(Paragraph::new(left).scroll((*scroll, *column)), panes[0]);
            f.render_widget(Block::new().borders(Borders::LEFT), panes[1]);
            f.render_widget(Paragraph::new(right).scroll((*scroll, *column)), panes[2]);
            let styles = ScrollbarStyles::from_theme(theme);
            if let Some(track) = tracks.vertical {
                let extent = ScrollExtent::new(height, usize::from(*scroll), body.height.into());
                f.render_widget(ScrollBar::vertical(extent, styles), track);
                hits.record_track(
                    Some(ScrollTrack::new(track, ScrollAxis::Vertical, extent)),
                    ScrollSurface::TabRows,
                );
            }
            if let Some(track) = tracks.horizontal {
                let halves = Layout::horizontal(constraints).split(track);
                let offset = usize::from(*column);
                for (half, width, pane) in [
                    (halves[0], left_width, panes[0]),
                    (halves[2], right_width, panes[2]),
                ] {
                    let extent = ScrollExtent::new(width, offset, pane.width.into());
                    f.render_widget(ScrollBar::horizontal(extent, styles), half);
                    // Both halves drive the one shared column offset, each at its own
                    // pane's scale — so a drag on either is measured against the text
                    // actually under it.
                    hits.record_track(
                        Some(ScrollTrack::new(half, ScrollAxis::Horizontal, extent)),
                        ScrollSurface::TabColumns,
                    );
                }
            }
        },
    }
}
