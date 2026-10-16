//! Painting what is not in the buffer: inline and end-of-line virtual text
//! inside a line's rows, and virtual lines on rows of their own.

use super::*;

impl<'a> Editor<'a> {
    /// Supply virtual lines: whole rows of text above or below a buffer
    /// line (code lenses, multi-line inline diagnostics, inline diff hunks,
    /// multi-line ghost text).
    ///
    /// Each takes a row of its own that scrolling, cursor reveal and
    /// hit-testing all count, and is hidden with its line when a collapsed
    /// fold hides that line. It is painted from the content's left edge,
    /// scrolled horizontally as a buffer line holding the same text would
    /// be, cut short with `…` at the right edge, and never wrapped. A click
    /// on one is reported by [`EditorState::virtual_line_at`], keyed by its
    /// index in this slice.
    #[must_use]
    pub fn virtual_lines(mut self, virtual_lines: &'a [VirtualLine]) -> Self {
        self.virtual_lines = virtual_lines;
        self
    }
}

impl Editor<'_> {
    /// The style the virtual text at `source` paints in: its role's color over
    /// the row's own background.
    pub(super) fn virtual_style(&self, source: usize, theme: &Theme) -> Style {
        let role = self
            .virtual_text
            .get(source)
            .map_or(ThemeRole::Muted, |text| text.role);
        Style::default().fg(theme.role(role).to_ratatui())
    }

    /// Fit line `l`'s end-of-line virtual text into the `room` cells its last
    /// row leaves free and append it, noting in `painted` the index and span
    /// of each text that got any cells.
    pub(super) fn push_eol_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        painted: &mut Vec<(usize, usize)>,
        index: &HintIndex,
        l: u32,
        room: u32,
        theme: &Theme,
    ) {
        let items: Vec<EolItem<'_>> = index
            .eol_on(l)
            .filter_map(|source| {
                let text = self.virtual_text.get(source)?;
                match text.placement {
                    VirtualTextPlacement::EndOfLine { align, priority } => Some(EolItem {
                        source,
                        text: &text.text,
                        align,
                        priority,
                    }),
                    _ => None,
                }
            })
            .collect();
        let mut at = 0_u32;
        for placed in place_eol(&items, room) {
            if placed.offset > at {
                spans.push(Span::raw(" ".repeat((placed.offset - at) as usize)));
            }
            at = placed.offset.saturating_add(text_width(&placed.text));
            painted.push((placed.source, spans.len()));
            let style = self.virtual_style(placed.source, theme);
            spans.push(Span::styled(placed.text, style));
        }
    }

    /// Flush the pending run and emit the inline text anchored at `col`,
    /// noting each one's index and span in `painted`.
    ///
    /// Threading the in-progress run through is what keeps inline text from
    /// splitting a styled run it does not belong to.
    pub(super) fn push_hint_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        painted: &mut Vec<(usize, usize)>,
        (run, run_style): (&mut String, &mut Option<Style>),
        hints: LineHints<'_>,
        col: u32,
        theme: &Theme,
    ) {
        let mut first = true;
        for index in hints.at(col) {
            let Some(text) = self.virtual_text.get(index) else {
                continue;
            };
            if first {
                if let Some(prev) = run_style.take() {
                    spans.push(Span::styled(std::mem::take(run), prev));
                }
                first = false;
            }
            painted.push((index, spans.len()));
            spans.push(Span::styled(
                text.text.clone(),
                self.virtual_style(index, theme),
            ));
        }
    }

    /// Paint row `y` of `area` as the virtual line at `source`, behind a
    /// blank gutter `gutter` cells wide, and record the row's content cells
    /// in `rects` for [`EditorState::virtual_line_at`].
    ///
    /// Its first `scroll_col` characters are scrolled off, as a buffer
    /// line's would be, and what is left is cut to the content width.
    #[allow(clippy::too_many_arguments)]
    // The row's resolved geometry and palette, as `draw_sticky_row` takes
    // them; bundling them would add a type for one private painter.
    pub(super) fn draw_virtual_line(
        &self,
        buf: &mut Buffer,
        area: Rect,
        y: u16,
        gutter: u16,
        source: usize,
        scroll_col: u32,
        theme: &Theme,
        rects: &mut Vec<(usize, Rect)>,
    ) {
        let Some(line) = self.virtual_lines.get(source) else {
            return;
        };
        let x = area.x.saturating_add(gutter);
        let width = area.right().saturating_sub(x);
        if width == 0 {
            return;
        }
        let shown: String = line.text.chars().skip(scroll_col as usize).collect();
        let text = truncate(&shown, u32::from(width));
        let style = Style::default().fg(theme.role(line.role).to_ratatui());
        buf.set_stringn(x, y, text, usize::from(width), style);
        rects.push((source, Rect::new(x, y, width, 1)));
    }
}
