use super::text::*;
use super::visual::*;
use super::*;

/// The editor widget: a builder over the buffer and the (borrowed) data layers
/// the application supplies. Render it as a ratatui [`StatefulWidget`] with an
/// [`EditorState`].
///
/// [`StatefulWidget`]: ratatui::widgets::StatefulWidget
pub struct Editor<'a> {
    buffer: &'a TextBuffer,
    highlights: Option<&'a Highlights>,
    theme: Option<&'a Theme>,
    decorations: &'a [Decoration],
    diagnostics: &'a [Diagnostic],
    inlay_hints: &'a [InlayHint],
    folds: &'a [Fold],
    focused: bool,
    cell_caret: bool,
    read_only: bool,
    word_wrap: bool,
    tab_width: u16,
    semantic_blocks: Option<&'a SemanticBlocks>,
    sticky_scroll: bool,
}

impl<'a> Editor<'a> {
    /// Start building an editor over `buffer`.
    #[must_use]
    pub fn new(buffer: &'a TextBuffer) -> Self {
        Self {
            buffer,
            highlights: None,
            theme: None,
            decorations: &[],
            diagnostics: &[],
            inlay_hints: &[],
            folds: &[],
            focused: false,
            cell_caret: true,
            read_only: false,
            word_wrap: false,
            tab_width: 4,
            semantic_blocks: None,
            sticky_scroll: false,
        }
    }

    /// Mark the editor focused, so the caret cell is drawn.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Choose whether the focused editor paints its built-in reversed-cell caret.
    ///
    /// Applications that draw a separate terminal-native or pixel-graphics caret can
    /// disable this while keeping focus-dependent cursor-line highlighting.
    #[must_use]
    pub fn cell_caret(mut self, visible: bool) -> Self {
        self.cell_caret = visible;
        self
    }

    /// Render in read-only (pager) mode: never draw the caret and don't highlight
    /// the cursor's line, even when [`focused`](Self::focused). Pair with
    /// [`EditorState::scroll_page_down`]/[`center_on`](EditorState::center_on) to
    /// page through a document without an editable cursor.
    #[must_use]
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Soft-wrap long buffer lines to the available content width.
    #[must_use]
    pub fn word_wrap(mut self, word_wrap: bool) -> Self {
        self.word_wrap = word_wrap;
        self
    }

    /// Set the display width between hard-tab stops (clamped to at least one).
    #[must_use]
    pub fn tab_width(mut self, width: u16) -> Self {
        self.tab_width = width.max(1);
        self
    }

    /// Supply semantic source blocks used by sticky scroll.
    #[must_use]
    pub fn semantic_blocks(mut self, blocks: &'a SemanticBlocks) -> Self {
        self.semantic_blocks = Some(blocks);
        self
    }

    /// Keep active semantic block headers pinned above the scrolling document.
    #[must_use]
    pub fn sticky_scroll(mut self, enabled: bool) -> Self {
        self.sticky_scroll = enabled;
        self
    }

    /// Supply syntax highlight spans.
    #[must_use]
    pub fn highlights(mut self, highlights: &'a Highlights) -> Self {
        self.highlights = Some(highlights);
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Supply decorations (VCS markers, breakpoints, search highlights, …).
    #[must_use]
    pub fn decorations(mut self, decorations: &'a [Decoration]) -> Self {
        self.decorations = decorations;
        self
    }

    /// Supply diagnostics (from LSP, spell-check, …).
    #[must_use]
    pub fn diagnostics(mut self, diagnostics: &'a [Diagnostic]) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// Supply inlay hints.
    #[must_use]
    pub fn inlay_hints(mut self, inlay_hints: &'a [InlayHint]) -> Self {
        self.inlay_hints = inlay_hints;
        self
    }

    /// Supply the resolved fold regions to render (collapsed folds hide their
    /// interior lines and mark their header).
    #[must_use]
    pub fn folds(mut self, folds: &'a [Fold]) -> Self {
        self.folds = folds;
        self
    }

    /// The fold whose header is line `l`, if any.
    fn fold_at(&self, l: u32) -> Option<Fold> {
        self.folds.iter().copied().find(|f| f.start == l)
    }
}

impl Editor<'_> {
    /// The gutter marker glyph + color for line `l`, if a decoration places one.
    fn gutter_marker(&self, l: u32, theme: &Theme, default_fg: Rgba) -> Option<(char, Rgba)> {
        for d in self.decorations {
            if let DecorationKind::GutterMarker { glyph } = &d.kind
                && line_in_range(l, d.range)
            {
                let color = d.role.map_or(default_fg, |r| theme.role(r));
                return Some((*glyph, color));
            }
        }
        None
    }

    /// The background color for column `col` on line `l`, from line/text-background
    /// decorations (e.g. search matches).
    fn decoration_bg(&self, l: u32, col: u32, theme: &Theme) -> Option<Rgba> {
        for d in self.decorations {
            let Some(role) = d.role else { continue };
            let covers = match &d.kind {
                DecorationKind::LineBackground => line_in_range(l, d.range),
                DecorationKind::TextBackground => col_in_range(l, col, d.range),
                _ => false,
            };
            if covers {
                return Some(theme.role(role));
            }
        }
        None
    }

    /// Inline virtual text attached to `l`, filtered by whether it belongs before or
    /// after the decorated range.
    fn push_inline_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        l: u32,
        before: bool,
        theme: &Theme,
    ) {
        for decoration in self.decorations {
            if let DecorationKind::InlineText {
                text,
                before: decoration_before,
            } = &decoration.kind
                && *decoration_before == before
                && line_in_range(l, decoration.range)
            {
                let color = decoration
                    .role
                    .map_or_else(|| theme.role(ThemeRole::Muted), |role| theme.role(role));
                spans.push(Span::styled(
                    text.clone(),
                    Style::default().fg(color.to_ratatui()),
                ));
            }
        }
    }

    /// Append the syntax-colored content spans for line `l`, honoring horizontal
    /// scroll, active selections, and text-background decorations.
    fn push_content_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        l: u32,
        theme: &Theme,
        default_fg: Rgba,
        range: VisualRange,
        selections: &[Range],
    ) {
        let Some(content) = self.buffer.line(l as usize) else {
            return;
        };
        let Some(line_span) = self.buffer.line_to_byte_range(l as usize) else {
            return;
        };
        let line_start = line_span.start.0;
        let hl: &[HighlightSpan] = self.highlights.map_or(&[], |h| h.spans_in(line_span));

        let mut run = String::new();
        let mut run_style: Option<Style> = None;
        let mut col: u32 = 0;
        let mut display_col = 0_u32;
        for (boff, ch) in content.char_indices() {
            let width = character_width(ch, display_col, self.tab_width);
            if col < range.start {
                display_col = display_col.saturating_add(width);
                col += 1;
                continue;
            }
            if col >= range.end {
                break;
            }
            let mut style = token_style(line_start + boff, hl, theme, default_fg);
            let bg = if in_any(selections, l, col) {
                Some(theme.role(ThemeRole::Selection))
            } else {
                self.decoration_bg(l, col, theme)
            };
            if let Some(bg) = bg {
                style = style.bg(bg.to_ratatui());
            }
            if run_style == Some(style) {
                if ch == '\t' {
                    run.push_str(&" ".repeat(width as usize));
                } else {
                    run.push(ch);
                }
            } else {
                if let Some(prev) = run_style {
                    spans.push(Span::styled(std::mem::take(&mut run), prev));
                }
                if ch == '\t' {
                    run.push_str(&" ".repeat(width as usize));
                } else {
                    run.push(ch);
                }
                run_style = Some(style);
            }
            display_col = display_col.saturating_add(width);
            col += 1;
        }
        if let Some(prev) = run_style {
            spans.push(Span::styled(run, prev));
        }
    }

    /// Draw the caret at buffer position `at` as a reversed cell, when it falls within
    /// the visible, non-folded region of `area`. Called once per caret so multi-cursor
    /// renders every head.
    fn draw_caret(&self, buf: &mut Buffer, area: Rect, state: &EditorState, at: LineCol) {
        let Some((cx, cy)) = caret_cell(area, self.buffer, self.folds, state, at) else {
            return;
        };
        buf.set_style(
            Rect {
                x: cx,
                y: cy,
                width: 1,
                height: 1,
            },
            Style::default().add_modifier(Modifier::REVERSED),
        );
    }

    fn sticky_blocks(&self, anchor: VisualAnchor, height: u16) -> Vec<SemanticBlock> {
        if !self.sticky_scroll {
            return Vec::new();
        }
        let budget = usize::from(height.saturating_sub(1));
        if budget == 0 {
            return Vec::new();
        }
        let mut active: Vec<SemanticBlock> = self
            .semantic_blocks
            .map_or(&[][..], SemanticBlocks::blocks)
            .iter()
            .copied()
            .filter(|block| {
                block.scope_end >= anchor.line
                    && (block.header_start < anchor.line
                        || (block.header_start == anchor.line && anchor.subrow > 0))
            })
            .collect();
        if active.len() > budget {
            active.drain(..active.len() - budget);
        }
        active
    }

    #[allow(clippy::too_many_arguments)]
    // The arguments are the already-resolved render palette/geometry shared with the
    // normal row painter; bundling them would obscure the one-row operation.
    fn draw_sticky_row(
        &self,
        buf: &mut Buffer,
        area: Rect,
        y: u16,
        block: SemanticBlock,
        state: &EditorState,
        theme: &Theme,
        default_fg: Rgba,
        digits: usize,
        selections: &[Range],
    ) {
        let line = block.header_start;
        let background = theme.role(ThemeRole::CursorLine);
        buf.set_style(
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            Style::default().bg(background.to_ratatui()),
        );
        let fold = self.fold_at(line);
        let (marker, marker_color) = fold.map_or_else(
            || {
                self.gutter_marker(line, theme, default_fg)
                    .unwrap_or((' ', default_fg))
            },
            |fold| {
                (
                    if fold.collapsed {
                        '\u{25b8}'
                    } else {
                        '\u{25be}'
                    },
                    theme.role(ThemeRole::LineNumberActive),
                )
            },
        );
        let mut spans = vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(marker_color.to_ratatui()),
            ),
            Span::styled(
                format!("{:>width$} ", line + 1, width = digits),
                Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui()),
            ),
        ];
        self.push_content_spans(
            &mut spans,
            line,
            theme,
            default_fg,
            VisualRange {
                start: state.scroll_col,
                end: u32::MAX,
            },
            selections,
        );
        buf.set_line(area.x, y, &Line::from(spans), area.width);
        if block.has_multiline_header() && area.width > 0 {
            buf.set_string(
                area.right().saturating_sub(1),
                y,
                "\u{2026}",
                Style::default()
                    .fg(theme.role(ThemeRole::LineNumber).to_ratatui())
                    .bg(background.to_ratatui()),
            );
        }
    }
}

impl StatefulWidget for Editor<'_> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut EditorState) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let fallback;
        let theme = match self.theme {
            Some(theme) => theme,
            None => {
                fallback = Theme::dark();
                &fallback
            },
        };

        let line_count = self.buffer.line_count() as u32;
        let background = theme.role(ThemeRole::Background);
        let cursor_line_bg = theme.role(ThemeRole::CursorLine);
        let default_fg = theme.role(ThemeRole::Foreground);
        let digits = digit_count(line_count.max(1));

        let gutter = 1 + digits as u16 + 1;
        let content_width = area.width.saturating_sub(gutter).max(1);

        // Clamp scroll to the buffer and record horizontal geometry. The vertical
        // content height is resolved after semantic sticky rows are selected.
        state.last_content_width = content_width;
        state.last_word_wrap = self.word_wrap;
        state.last_tab_width = self.tab_width;
        state.scroll_line = state.scroll_line.min(line_count.saturating_sub(1));
        if self.word_wrap {
            state.scroll_col = 0;
        } else {
            state.scroll_subrow = 0;
        }

        let width = u32::from(content_width);
        let mut anchor = normalize_visual_anchor(
            self.buffer,
            self.folds,
            width,
            self.tab_width,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        let initial_sticky = self.sticky_blocks(anchor, area.height);
        let initial_content_height = area
            .height
            .saturating_sub(u16::try_from(initial_sticky.len()).unwrap_or(u16::MAX));
        if self.word_wrap && state.follow_cursor {
            anchor = reveal_visual_anchor(
                self.buffer,
                self.folds,
                width,
                self.tab_width,
                initial_content_height,
                anchor,
                state.cursor(),
            );
        }
        state.scroll_line = anchor.line;
        state.scroll_subrow = if self.word_wrap { anchor.subrow } else { 0 };
        state.follow_cursor = false;

        let sticky = self.sticky_blocks(anchor, area.height);
        state.sticky_rows = sticky.iter().map(|block| block.header_start).collect();
        state.sticky_height = u16::try_from(sticky.len()).unwrap_or(u16::MAX);
        state.last_height = area.height.saturating_sub(state.sticky_height);

        // Snapshot the cursor set for painting: every non-empty selection's range and
        // the line of every caret (each caret's line gets the cursor-line emphasis).
        let selections = state.selection_ranges();
        let caret_lines: Vec<u32> = state
            .cursors()
            .selections
            .iter()
            .map(|s| s.head.line)
            .collect();

        // Base background for the whole editor area (covers rows past end-of-file).
        buf.set_style(area, Style::default().bg(background.to_ratatui()));

        for (row, block) in sticky.iter().copied().enumerate() {
            let y = area
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            self.draw_sticky_row(
                buf,
                area,
                y,
                block,
                state,
                theme,
                default_fg,
                digits,
                &selections,
            );
        }

        for row in 0..state.last_height {
            if anchor.line >= line_count {
                break;
            }
            let l = anchor.line;
            let y = area
                .y
                .saturating_add(state.sticky_height)
                .saturating_add(row);
            // In read-only (pager) mode there is no active cursor line to emphasize.
            let is_cursor = !self.read_only && caret_lines.contains(&l);
            let row_bg = if is_cursor {
                cursor_line_bg
            } else {
                background
            };
            buf.set_style(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                Style::default().bg(row_bg.to_ratatui()),
            );

            // A fold header shows a collapse/expand chevron in the marker column;
            // other lines show their usual decoration marker (git/diagnostic).
            let first_row = !self.word_wrap || anchor.subrow == 0;
            let fold = self.fold_at(l);
            let (marker_ch, marker_color) = match (first_row, fold) {
                (true, Some(f)) => (
                    if f.collapsed { '\u{25b8}' } else { '\u{25be}' },
                    theme.role(ThemeRole::LineNumberActive),
                ),
                (true, None) => self
                    .gutter_marker(l, theme, default_fg)
                    .unwrap_or((' ', default_fg)),
                (false, _) => (' ', default_fg),
            };
            let number_color = if is_cursor {
                theme.role(ThemeRole::LineNumberActive)
            } else {
                theme.role(ThemeRole::LineNumber)
            };
            let mut spans = vec![
                Span::styled(
                    marker_ch.to_string(),
                    Style::default().fg(marker_color.to_ratatui()),
                ),
                Span::styled(
                    if first_row {
                        format!("{:>width$} ", l + 1, width = digits)
                    } else {
                        " ".repeat(digits + 1)
                    },
                    Style::default().fg(number_color.to_ratatui()),
                ),
            ];
            let ranges = if self.word_wrap {
                visual_ranges(self.buffer, l, width, self.tab_width)
            } else {
                vec![VisualRange {
                    start: state.scroll_col,
                    end: u32::MAX,
                }]
            };
            let range_index = if self.word_wrap {
                (anchor.subrow as usize).min(ranges.len().saturating_sub(1))
            } else {
                0
            };
            let range = ranges
                .get(range_index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            if range_index == 0 {
                self.push_inline_spans(&mut spans, l, true, theme);
            }
            self.push_content_spans(&mut spans, l, theme, default_fg, range, &selections);
            // A collapsed header hints at the hidden lines it conceals.
            if fold.is_some_and(|f| f.collapsed) && range_index + 1 == ranges.len() {
                spans.push(Span::styled(
                    " \u{22ef}", // ⋯
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ));
            }
            if range_index + 1 == ranges.len() {
                self.push_inline_spans(&mut spans, l, false, theme);
            }
            buf.set_line(area.x, y, &Line::from(spans), area.width);

            let next = if self.word_wrap {
                next_visual_anchor(self.buffer, self.folds, width, self.tab_width, anchor)
            } else {
                next_line_anchor(self.folds, line_count, anchor)
            };
            if next == anchor {
                break;
            }
            anchor = next;
        }

        // Draw a reversed caret cell for every head when focused and editable.
        if self.focused && self.cell_caret && !self.read_only {
            for sel in &state.cursors().selections {
                self.draw_caret(buf, area, state, sel.head);
            }
        }
    }
}
