use super::*;
use crate::app::LOADING_REVEAL_DELAY;

/// Draw one pane's active tab into `area`. Returns the rect to reserve for a Kitty
/// image, if the active tab is an image on a Kitty terminal.
pub(super) fn draw_pane_content(
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
    let word_wrap = crate::app::effective_word_wrap(tab, ctx.word_wrap);
    // Written by the image/PDF render arms; stays `None` (and non-`mut`) when neither
    // media feature is compiled in.
    #[cfg(any(feature = "images", feature = "pdf"))]
    let mut image_area = None;
    #[cfg(not(any(feature = "images", feature = "pdf")))]
    let image_area: Option<Rect> = None;
    let mut badge_rect = None;
    let mut file_hits = Vec::new();
    let mut blame_rect = None;
    let mut markdown_link_hits = Vec::new();
    let mut editor_rect = area;
    let mut markdown_preview_rect = Rect::default();
    match &mut tab.kind {
        TabKind::Welcome => draw_welcome(f, theme, area),
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
                    Block::default().borders(Borders::LEFT).border_style(
                        Style::default().fg(theme.role(ThemeRole::IndentGuide).to_ratatui()),
                    ),
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
            let combined: Vec<Decoration> = decos
                .iter()
                .chain(search_decos.iter())
                .chain(conflict_decorations.iter())
                .chain(ctx.blame.iter())
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
            f.render_stateful_widget(editor, editor_rect, &mut tab.editor);
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
                    },
                );
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
                },
            );
        },
        TabKind::Diff { file, view, scroll } => draw_diff(f, theme, area, file, *view, scroll),
        TabKind::StashPreview { patch, scroll, .. } => {
            let lines: Vec<Line> = patch
                .lines()
                .map(|line| Line::raw(line.to_string()))
                .collect();
            let max = lines.len().saturating_sub(area.height as usize);
            *scroll = (*scroll).min(u16::try_from(max).unwrap_or(u16::MAX));
            f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
        },
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
            view,
        } => {
            let painted = draw_commit(
                f,
                theme,
                area,
                detail,
                files,
                file_load_status(*files_loading_since, files_error.as_deref()),
                verification.as_ref(),
                *explain_since,
                view,
            );
            badge_rect = painted.badge_rect;
            file_hits = painted.file_hits;
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
            );
            file_hits = painted.file_hits;
        },
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
        TabKind::LatexPreview {
            source,
            loading_since,
            error,
        } => {
            let message = error.as_deref().or_else(|| {
                (loading_since.elapsed() >= LOADING_REVEAL_DELAY)
                    .then_some("Building LaTeX preview…")
            });
            if let Some(message) = message {
                let detail = source.file_name().map_or_else(
                    || source.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                f.render_widget(
                    Paragraph::new(format!("{message}\n{detail}"))
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui())),
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
        blame_rect,
        markdown_link_hits,
    }
}

pub(super) fn draw_diff(
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
