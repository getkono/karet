use std::ops::RangeInclusive;

use super::hint::BlockIndex;
use super::hint::EolItem;
use super::hint::HintIndex;
use super::hint::LineHints;
use super::hint::place_eol;
use super::hint::record_virtual_text;
use super::hint::row_room;
use super::hint::text_width;
use super::hint::truncate;
use super::text::*;
use super::visual::*;
use super::*;

mod display;
mod virtual_paint;

pub use display::LineNumbers;
pub(crate) use display::gutter_width;

/// Chars read past a row's right edge, so the grapheme cluster that straddles
/// the edge is segmented whole — a flag or a ZWJ emoji sequence is several
/// chars — and keeps the width it has when the full line is measured.
const GRAPHEME_LOOKAHEAD: u32 = 32;

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
    virtual_text: &'a [VirtualText],
    virtual_lines: &'a [VirtualLine],
    folds: &'a [Fold],
    focused: bool,
    cell_caret: bool,
    read_only: bool,
    word_wrap: bool,
    tab_width: u16,
    unwrapped_lines: &'a [RangeInclusive<u32>],
    semantic_blocks: Option<&'a SemanticBlocks>,
    sticky_scroll: bool,
    line_numbers: LineNumbers,
    change_lane: bool,
    cursor_line: bool,
    rulers: &'a [u16],
    scroll_off: u16,
    undercurl: bool,
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
            virtual_text: &[],
            virtual_lines: &[],
            folds: &[],
            focused: false,
            cell_caret: true,
            read_only: false,
            word_wrap: false,
            tab_width: 4,
            unwrapped_lines: &[],
            semantic_blocks: None,
            sticky_scroll: false,
            line_numbers: LineNumbers::Absolute,
            change_lane: false,
            cursor_line: true,
            rulers: &[],
            scroll_off: 0,
            undercurl: false,
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

    /// Keep selected logical lines on one visual row even when soft wrapping is on.
    ///
    /// This is useful for source constructs such as Markdown tables whose columns
    /// lose their relationship when individual rows wrap independently.
    #[must_use]
    pub fn unwrapped_lines(mut self, lines: &'a [RangeInclusive<u32>]) -> Self {
        self.unwrapped_lines = lines;
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

    /// Supply virtual text: inlay hints (convert them with
    /// `VirtualText::from`), inline blame, dependency versions, and anything
    /// else painted on a line without being in the buffer.
    ///
    /// Inline text takes part in wrapping, caret placement, hit-testing and
    /// horizontal reveal; end-of-line text fills the cells a line's last row
    /// leaves free (see [`VirtualTextPlacement`](karet_core::VirtualTextPlacement)).
    /// Where each one was painted is reported by
    /// [`EditorState::virtual_text_rect`], keyed by its index in this slice.
    #[must_use]
    pub fn virtual_text(mut self, virtual_text: &'a [VirtualText]) -> Self {
        self.virtual_text = virtual_text;
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

    /// The swatch background + contrasting foreground for column `col` on line
    /// `l`, when a [`DecorationKind::ColorSwatch`] covers the cell. The
    /// foreground is whichever of black/white contrasts better, so the chip
    /// stays legible on any color; alpha is ignored (cells cannot blend).
    fn swatch_style(&self, l: u32, col: u32) -> Option<(Rgba, Rgba)> {
        for d in self.decorations {
            if let DecorationKind::ColorSwatch { rgba } = &d.kind
                && col_in_range(l, col, d.range)
            {
                let bg = Rgba::rgb(rgba[0], rgba[1], rgba[2]);
                let black = Rgba::rgb(0, 0, 0);
                let white = Rgba::rgb(255, 255, 255);
                let fg = if karet_theme::contrast_ratio(black, bg)
                    >= karet_theme::contrast_ratio(white, bg)
                {
                    black
                } else {
                    white
                };
                return Some((bg, fg));
            }
        }
        None
    }

    /// Append the syntax-colored content spans for line `l`, honoring horizontal
    /// scroll, active selections, and text-background decorations.
    ///
    /// `room` is the screen cells left on the row after the gutter: the walk
    /// stops once they are filled, since the frame clips anything past them.
    ///
    /// Returns whether the row ends with the selected line-break cell.
    #[allow(clippy::too_many_arguments)]
    // The already-resolved render palette and geometry for one row, plus the
    // row's inline text and the record of where it went. Bundling them would
    // duplicate the widget/state split for a single private helper.
    fn push_content_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        painted: &mut Vec<(usize, usize)>,
        l: u32,
        theme: &Theme,
        default_fg: Rgba,
        (range, room): (VisualRange, u32),
        selections: &[Range],
        hints: LineHints<'_>,
    ) -> bool {
        let Some(line_span) = self.buffer.line_to_byte_range(l as usize) else {
            return false;
        };
        let line_start = line_span.start.0;
        let line_len = line_len(self.buffer, l);
        // Only the chars up to the row's right edge are copied out and walked,
        // so a minified line costs its visible window (plus the scrolled-off
        // prefix, whose widths decide the tab stops), not its whole length. An
        // unwrapped row has no end column, so its window is bounded by the
        // room left on screen instead: every char a cell-taking glyph needs
        // at least one cell, so twice the room covers a row even when half
        // its chars are zero-width marks.
        let window_end = if range.end == u32::MAX {
            range.start.saturating_add(room.saturating_mul(2))
        } else {
            range.end
        };
        let chars = line_chars_prefix(
            self.buffer,
            l,
            window_end.saturating_add(GRAPHEME_LOOKAHEAD),
        );
        let hl: &[HighlightSpan] = self.highlights.map_or(&[], |h| h.spans_in(line_span));

        let mut run = String::new();
        let mut run_style: Option<Style> = None;
        let mut col: u32 = 0;
        let mut display_col = 0_u32;
        // Where on screen this row's first painted column sits, so the walk
        // stops once the row is full rather than at the end of the line.
        let mut row_origin = None;
        let mut boff = 0_usize;
        let widths = cluster_widths(&chars);
        let cells = |col: u32| widths.get(col as usize).copied().unwrap_or(0);
        // The shared horizontal scroll can start this row inside a cluster.
        // Its earlier chars took the cluster's cells off screen, so the chars
        // up to the next boundary are a fragment the width model counts as
        // zero cells: painting them would shift the rest of the row away from
        // the caret and hit-test columns.
        let paint_from = cluster_bounds(&chars)
            .into_iter()
            .find(|&bound| bound >= range.start)
            .unwrap_or(range.start);
        for &ch in &chars {
            let at = boff;
            boff = boff.saturating_add(ch.len_utf8());
            if col < range.start {
                // Hints before the row start still shift the running column,
                // and a tab's expansion is measured from it. Skipping their
                // width here would put every tab after the row start on a
                // different stop than `display_col` and `visual_ranges`
                // computed, so the caret and the painted text would disagree.
                display_col = display_col.saturating_add(hints.width_at(col));
                display_col = display_col.saturating_add(character_width(
                    ch,
                    cells(col),
                    display_col,
                    self.tab_width,
                ));
                col += 1;
                continue;
            }
            if col >= range.end {
                break;
            }
            let origin = *row_origin.get_or_insert(display_col);
            if display_col.saturating_sub(origin) >= room {
                break;
            }
            // The hint anchored here renders *before* this character but after
            // the caret slot of this column, so the caret stays against the
            // text the hint describes.
            if !hints.is_empty() {
                self.push_hint_spans(
                    spans,
                    painted,
                    (&mut run, &mut run_style),
                    hints,
                    col,
                    theme,
                );
                display_col = display_col.saturating_add(hints.width_at(col));
            }
            if col < paint_from {
                col += 1;
                continue;
            }
            let width = character_width(ch, cells(col), display_col, self.tab_width);
            let mut style = token_style(line_start + at, hl, theme, default_fg);
            // Diagnostics win an overlap: a squiggle carries more information than
            // "this is navigable", and losing it would hide a real problem. Theirs
            // is marked, so the frame's last pass can curl it.
            if let Some((color, mark)) = self
                .diagnostic_underline(l, col, theme)
                .map(|color| (color, diagnostic::DIAGNOSTIC_UNDERLINE))
                .or_else(|| {
                    let color = self.decoration_underline(l, col, theme, default_fg)?;
                    Some((color, Modifier::empty()))
                })
            {
                style = style
                    .underline_color(color.to_ratatui())
                    .add_modifier(Modifier::UNDERLINED | mark);
            }
            style = style.add_modifier(diagnostic::tag_modifiers(self.diagnostics, l, col));
            let bg = if in_any(selections, l, col) {
                Some(theme.role(ThemeRole::Selection))
            } else if let Some((swatch_bg, swatch_fg)) = self.swatch_style(l, col) {
                style = style.fg(swatch_fg.to_ratatui());
                Some(swatch_bg)
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
            spans.push(Span::styled(std::mem::take(&mut run), prev));
            run_style = None;
        }
        // A hint anchored one past the last character -- a trailing return type,
        // say -- has no character to precede, so it closes the row instead.
        // Anchored at the line's own length, never at `range.end`: an
        // unwrapped row carries `u32::MAX` there, which matches no hint at all.
        //
        // `range.start <= line_len` as well as `range.end >= line_len`: an
        // unwrapped row carries `u32::MAX` as its end, so the end test alone is
        // always true, and a line scrolled entirely off to the left would paint
        // its trailing hint at the left margin over nothing. `col >= line_len`
        // because a walk the right edge stopped never reached the line end, so
        // what follows it is off screen and must not close a full row.
        let ends_here = range.start <= line_len && range.end >= line_len && col >= line_len;
        if !hints.is_empty() && ends_here {
            self.push_hint_spans(
                spans,
                painted,
                (&mut run, &mut run_style),
                hints,
                line_len,
                theme,
            );
        }
        if let Some(prev) = run_style {
            spans.push(Span::styled(run, prev));
        }
        // A selected line break paints one cell past the line end, so a blank
        // line or a line end inside a selection shows what a delete removes.
        // Only the row holding the line end paints it, after any trailing hint,
        // so no hit-tested cell moves.
        let selected_break = ends_here && newline_in_any(selections, l, line_len);
        if selected_break {
            let style = Style::default()
                .fg(default_fg.to_ratatui())
                .bg(theme.role(ThemeRole::Selection).to_ratatui());
            spans.push(Span::styled(" ", style));
        }
        selected_break
    }

    /// Warning/error diagnostics use a colored underline while preserving syntax
    /// foreground and selection backgrounds. Terminals supporting styled underline
    /// rendering present this as their native diagnostic underline.
    fn diagnostic_underline(&self, line: u32, col: u32, theme: &Theme) -> Option<Rgba> {
        self.diagnostics
            .iter()
            .filter(|diagnostic| col_in_range(line, col, diagnostic.range))
            .min_by_key(|diagnostic| diagnostic.severity)
            .map(|diagnostic| theme.role(diagnostic::severity_role(diagnostic.severity)))
    }

    /// The underline color for column `col` on line `l`, from underline decorations.
    ///
    /// A decoration with no role underlines in the text's own foreground, so the
    /// treatment reads as an underline rather than as a recolouring.
    fn decoration_underline(
        &self,
        l: u32,
        col: u32,
        theme: &Theme,
        default_fg: Rgba,
    ) -> Option<Rgba> {
        self.decorations
            .iter()
            .find(|decoration| {
                matches!(decoration.kind, DecorationKind::Underline)
                    && col_in_range(l, col, decoration.range)
            })
            .map(|decoration| decoration.role.map_or(default_fg, |role| theme.role(role)))
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

    /// The headers that stick above a viewport starting at `anchor`: a
    /// block's header sticks once the top row is inside the block, past the
    /// header's own first text row (virtual lines above it are outside).
    fn sticky_blocks(&self, anchor: VisualAnchor, above: u32, height: u16) -> Vec<SemanticBlock> {
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
                        || (block.header_start == anchor.line && anchor.subrow > above))
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
        index: &HintIndex,
        rects: &mut Vec<(usize, Rect)>,
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
        let number_fg = theme.role(ThemeRole::LineNumberActive);
        let mut spans = self.gutter_spans(
            Some(line),
            state.cursor().line,
            digits,
            theme,
            (default_fg, number_fg),
        );
        let mut painted = Vec::new();
        let room = row_room(area, &spans);
        // Painted with its inline text, as `EditorState::pos_at` resolves a
        // click on it: the same mapping on both sides, so a click lands on
        // the character it is over.
        self.push_content_spans(
            &mut spans,
            &mut painted,
            line,
            theme,
            default_fg,
            (
                VisualRange {
                    start: state.scroll_col,
                    end: u32::MAX,
                },
                room,
            ),
            selections,
            index.line(line),
        );
        if state.scroll_col <= line_len(self.buffer, line) {
            // The multi-line header's `…` takes the last cell.
            let marker = u32::from(block.has_multiline_header());
            let room = row_room(area, &spans).saturating_sub(marker);
            self.push_eol_spans(&mut spans, &mut painted, index, line, room, theme);
        }
        record_virtual_text(rects, area, y, &spans, &painted);
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
        state.last_virtual_text.clear();
        state.last_virtual_lines.clear();
        state.last_fold_ellipses.clear();
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

        let gutter = gutter_width(line_count, self.line_numbers, self.change_lane);
        let content_width = area.width.saturating_sub(gutter).max(1);

        // Clamp scroll to the buffer and record horizontal geometry. The vertical
        // content height is resolved after semantic sticky rows are selected.
        state.last_content_width = content_width;
        state.last_line_numbers = self.line_numbers;
        state.last_change_lane = self.change_lane;
        state.last_scroll_off = self.scroll_off;
        state.last_line = line_count.saturating_sub(1);
        state.last_word_wrap = self.word_wrap;
        state.last_tab_width = self.tab_width;
        state.last_unwrapped_lines = self.unwrapped_lines.to_vec();
        state.scroll_line = state.scroll_line.min(line_count.saturating_sub(1));
        if self.word_wrap {
            state.scroll_col = 0;
        }

        let width = u32::from(content_width);
        // Indexed once per frame and handed to `state` after the row loop, so
        // a later mouse click resolves against the virtual text and lines
        // this frame painted. Held locally until then because `layout`
        // borrows them while the loop is still assigning scroll positions.
        let hint_index = HintIndex::new(self.virtual_text);
        let block_index = BlockIndex::new(self.virtual_lines);
        let layout = Layout {
            width,
            tab_width: self.tab_width,
            unwrapped_lines: self.unwrapped_lines,
            hints: &hint_index,
            blocks: &block_index,
            wrap: self.word_wrap,
        };
        if !self.word_wrap
            && state.follow_cursor
            && let Some((target, reveal)) = state.follow_col
        {
            // Re-resolved against the width this frame paints: the reveal's
            // own estimate used the last frame's, or had none to use.
            state.scroll_col = reveal_column(
                &line_chars(self.buffer, target.line),
                state.scroll_col,
                target.col,
                width,
                self.tab_width,
                layout.hints_on(target.line),
                reveal,
            );
        }
        let mut anchor = normalize_visual_anchor(
            self.buffer,
            self.folds,
            layout,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        // A wrap affinity outlives only the caret it was set for.
        if self.word_wrap {
            let heads = &state.cursors.selections;
            state
                .upstream
                .retain(|at| heads.iter().any(|s| s.head == *at));
        } else {
            state.upstream.clear();
        }
        let upstream = state.upstream.contains(&state.cursor());
        let mut sticky = self.sticky_blocks(anchor, layout.above(anchor.line), area.height);
        let off = state.reveal_margin();
        if state.follow_cursor {
            // Revealing moves the top, the top decides which headers stick,
            // and the headers decide how many rows are left for the caret. So
            // re-check until the caret is on a row the headers leave. A reveal
            // only moves down while the caret is below the last row, so this
            // settles; the bound is a backstop.
            for _ in 0..=area.height {
                let height = area
                    .height
                    .saturating_sub(u16::try_from(sticky.len()).unwrap_or(u16::MAX));
                let revealed = reveal_visual_anchor(
                    self.buffer,
                    self.folds,
                    layout,
                    height,
                    anchor,
                    state.cursor(),
                    upstream,
                    off,
                );
                if revealed == anchor {
                    break;
                }
                anchor = revealed;
                sticky = self.sticky_blocks(anchor, layout.above(anchor.line), area.height);
            }
        }
        state.scroll_line = anchor.line;
        state.scroll_subrow = anchor.subrow;
        state.follow_cursor = false;
        state.follow_col = None;
        state.last_folds.clear();
        state
            .last_folds
            .extend(self.folds.iter().copied().filter(|fold| fold.collapsed));

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

        let mut rects = Vec::new();
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
                &hint_index,
                &mut rects,
            );
        }
        state.last_virtual_text = rects;

        // Measured as the row loop walks anchors, so the scrollbar extents cost a
        // couple of comparisons per painted row rather than a second pass.
        let mut last_painted = None;
        let mut widest_visible = 0;
        for row in 0..state.last_height {
            if anchor.line >= line_count {
                break;
            }
            let l = anchor.line;
            last_painted = Some(l);
            let y = area
                .y
                .saturating_add(state.sticky_height)
                .saturating_add(row);
            let next = next_visual_anchor(self.buffer, self.folds, layout, anchor);
            let ranges = layout.text_rows(self.buffer, l, state.scroll_col);
            let (range_index, block) = match layout.kind(l, anchor.subrow, ranges.len() as u32) {
                RowKind::Text(index) => (index as usize, None),
                RowKind::Above(n) => (0, Some(block_index.above(l).get(n as usize))),
                RowKind::Below(n) => (0, Some(block_index.below(l).get(n as usize))),
            };
            if let Some(block) = block {
                if let Some(&(_, source)) = block {
                    let rects = &mut state.last_virtual_lines;
                    let col = state.scroll_col;
                    self.draw_virtual_line(buf, area, y, gutter, source, col, theme, rects);
                }
                if next == anchor {
                    break;
                }
                anchor = next;
                continue;
            }
            if !self.word_wrap {
                widest_visible = widest_visible.max(line_extent(
                    &mut state.extents,
                    self.buffer,
                    l,
                    width,
                    self.tab_width,
                    layout.hints_on(l),
                ));
            }
            // In read-only (pager) mode there is no active cursor line to emphasize.
            let is_cursor = !self.read_only && caret_lines.contains(&l);
            let row_bg = if is_cursor && self.cursor_line {
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

            let first_row = range_index == 0;
            let fold = self.fold_at(l);
            let number_color = if is_cursor {
                theme.role(ThemeRole::LineNumberActive)
            } else {
                theme.role(ThemeRole::LineNumber)
            };
            let mut spans = self.gutter_spans(
                first_row.then_some(l),
                state.cursor().line,
                digits,
                theme,
                (default_fg, number_color),
            );
            let range = ranges
                .get(range_index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            let mut painted = Vec::new();
            let room = row_room(area, &spans);
            let selected_break = self.push_content_spans(
                &mut spans,
                &mut painted,
                l,
                theme,
                default_fg,
                (range, room),
                &selections,
                layout.hints_on(l),
            );
            // A wrapped last row that exactly fills the content width pushes
            // the break cell one column past the area, where `set_line` clips
            // it. Paint the selection on the last content cell instead: the
            // end-of-line caret clamps to that same cell (`caret_cell`).
            let clipped_break = self.word_wrap
                && selected_break
                && spans
                    .iter()
                    .fold(0_usize, |used, span| used.saturating_add(span.width()))
                    > usize::from(area.width);
            // A collapsed header hints at the hidden lines it conceals.
            if fold.is_some_and(|f| f.collapsed) && range_index + 1 == ranges.len() {
                display::record_fold_ellipsis(&mut state.last_fold_ellipses, area, y, &spans, l);
                spans.push(Span::styled(
                    " \u{22ef}", // ⋯
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ));
            }
            // The end-of-line slot is on the last row, and only while the
            // line's end is not scrolled off to the left: past it, the slot
            // would float at the margin over nothing.
            if range_index + 1 == ranges.len() && range.start <= line_len(self.buffer, l) {
                let room = row_room(area, &spans);
                self.push_eol_spans(&mut spans, &mut painted, &hint_index, l, room, theme);
            }
            record_virtual_text(&mut state.last_virtual_text, area, y, &spans, &painted);
            buf.set_line(area.x, y, &Line::from(spans), area.width);
            if clipped_break && area.width > 0 {
                buf.set_style(
                    Rect {
                        x: area.right().saturating_sub(1),
                        y,
                        width: 1,
                        height: 1,
                    },
                    Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()),
                );
            }

            if next == anchor {
                break;
            }
            anchor = next;
        }
        state.last_hints = hint_index;
        state.last_blocks = block_index;
        state.last_visible_lines =
            last_painted.map_or(0, |last: u32| last.saturating_sub(state.scroll_line) + 1);
        if !self.word_wrap {
            // Raise the high-water mark by what this frame could see. The right-edge
            // term keeps the extent from ever claiming the caret has scrolled past
            // the end of the content.
            state.last_longest_col = state
                .last_longest_col
                .max(widest_visible)
                .max(state.scroll_col.saturating_add(u32::from(content_width)));
        }

        let content = Rect {
            x: area.x.saturating_add(gutter),
            width: area.width.saturating_sub(gutter),
            ..area
        };
        let plain = [background, cursor_line_bg];
        let ruler = theme.role(ThemeRole::IndentGuide);
        self.paint_rulers(buf, content, state.scroll_col, plain, ruler);
        diagnostic::finish_underlines(buf, area, self.undercurl);

        // Draw a reversed caret cell for every head when focused and editable.
        if self.focused && self.cell_caret && !self.read_only {
            for sel in &state.cursors().selections {
                self.draw_caret(buf, area, state, sel.head);
            }
        }
    }
}
