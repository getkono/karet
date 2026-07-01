//! `karet-editor` — the composable code-editor widget for karet.
//!
//! Combines the text engines (`karet-text`, `karet-syntax`, `karet-theme`) into a
//! ratatui editor widget. By design it depends on **none** of the feature
//! producers (`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`): diagnostics,
//! git markers, breakpoints, inlay hints and code lenses arrive as `karet-core`
//! decorations supplied by the application from the backend's event stream.
//!
//! This is the implementation *skeleton*: the [`Editor`] builder (the data joint)
//! and [`EditorState`] are defined; the painting/input logic is filled in
//! separately.

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::ThemeRole;
use karet_syntax::FoldRegions;
use karet_syntax::HighlightSpan;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_theme::Rgba;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;

/// The persistent, per-view editor state: scroll position and cursor.
///
/// The viewport height is cached at each render so motions and
/// [`scroll_to`](Self::scroll_to) know how far a page is without re-deriving it.
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    /// The first visible buffer line (top of the viewport).
    pub scroll_line: u32,
    /// The first visible column (horizontal scroll, counted in `char`s).
    pub scroll_col: u32,
    /// The cursor position.
    pub cursor: LineCol,
    /// The fixed end of the selection, if a selection is active. The moving end is
    /// the [`cursor`](Self::cursor).
    pub selection_anchor: Option<LineCol>,
    /// The viewport height captured at the last render.
    last_height: u16,
}

impl EditorState {
    /// Create a fresh editor state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll vertically so that `pos` is within the viewport.
    pub fn scroll_to(&mut self, pos: LineCol) {
        let height = u32::from(self.last_height.max(1));
        if pos.line < self.scroll_line {
            self.scroll_line = pos.line;
        } else if pos.line >= self.scroll_line + height {
            self.scroll_line = pos.line + 1 - height;
        }
    }

    /// The currently-visible line range `[top, top + height)`.
    #[must_use]
    pub fn viewport(&self) -> Range {
        let height = u32::from(self.last_height.max(1));
        Range {
            start: LineCol::new(self.scroll_line, 0),
            end: LineCol::new(self.scroll_line + height, 0),
        }
    }

    /// Scroll so `line` sits at the vertical center of the viewport. Handy for a
    /// read-only viewer centering a search match. The scroll is clamped to the
    /// buffer at render time.
    pub fn center_on(&mut self, line: u32) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = line.saturating_sub(height / 2);
    }

    /// Scroll the viewport down one page **without moving the cursor** — read-only
    /// paging for a pager/viewer. The scroll is clamped to the buffer at render
    /// time.
    pub fn scroll_page_down(&mut self) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = self.scroll_line.saturating_add(height);
    }

    /// Scroll the viewport up one page without moving the cursor (read-only paging).
    pub fn scroll_page_up(&mut self) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = self.scroll_line.saturating_sub(height);
    }

    /// Move the cursor down one line, clamping to the buffer and keeping it in view.
    pub fn move_down(&mut self, buffer: &TextBuffer) {
        self.cursor.line = (self.cursor.line + 1).min(last_line(buffer));
        self.clamp_col(buffer);
        self.scroll_to(self.cursor);
    }

    /// Move the cursor up one line.
    pub fn move_up(&mut self, buffer: &TextBuffer) {
        self.cursor.line = self.cursor.line.saturating_sub(1);
        self.clamp_col(buffer);
        self.scroll_to(self.cursor);
    }

    /// Move the cursor left one column, wrapping to the previous line's end.
    pub fn move_left(&mut self, buffer: &TextBuffer) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = line_len(buffer, self.cursor.line);
        }
        self.scroll_to(self.cursor);
    }

    /// Move the cursor right one column, wrapping to the next line's start.
    pub fn move_right(&mut self, buffer: &TextBuffer) {
        if self.cursor.col < line_len(buffer, self.cursor.line) {
            self.cursor.col += 1;
        } else if self.cursor.line < last_line(buffer) {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
        self.scroll_to(self.cursor);
    }

    /// Move the cursor down one page.
    pub fn page_down(&mut self, buffer: &TextBuffer) {
        let height = u32::from(self.last_height.max(1));
        self.cursor.line = (self.cursor.line + height).min(last_line(buffer));
        self.clamp_col(buffer);
        self.scroll_to(self.cursor);
    }

    /// Move the cursor up one page.
    pub fn page_up(&mut self, buffer: &TextBuffer) {
        let height = u32::from(self.last_height.max(1));
        self.cursor.line = self.cursor.line.saturating_sub(height);
        self.clamp_col(buffer);
        self.scroll_to(self.cursor);
    }

    /// Move the cursor to `pos`, clamped to the buffer, keeping it in view.
    pub fn goto(&mut self, buffer: &TextBuffer, pos: LineCol) {
        self.cursor.line = pos.line.min(last_line(buffer));
        self.cursor.col = pos.col.min(line_len(buffer, self.cursor.line));
        self.scroll_to(self.cursor);
    }

    /// Clear any active selection (leaving the cursor where it is).
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// Start a selection at the cursor if none is active (for shift-extension).
    pub fn ensure_anchor(&mut self) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor);
        }
    }

    /// The active selection as a normalized range, or `None` when empty.
    #[must_use]
    pub fn selection_range(&self) -> Option<Range> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        let (start, end) = if (anchor.line, anchor.col) <= (self.cursor.line, self.cursor.col) {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        Some(Range { start, end })
    }

    /// Place the caret at `pos` (clamped), clearing any selection.
    pub fn set_caret(&mut self, buffer: &TextBuffer, pos: LineCol) {
        self.clear_selection();
        self.goto(buffer, pos);
    }

    /// Extend the selection so its moving end is `pos`, anchoring at the current
    /// cursor if no selection is active yet.
    pub fn extend_to(&mut self, buffer: &TextBuffer, pos: LineCol) {
        self.ensure_anchor();
        self.goto(buffer, pos);
    }

    /// Set the selection to span `anchor`..`head` (both clamped), placing the
    /// caret at `head`.
    pub fn set_selection(&mut self, buffer: &TextBuffer, anchor: LineCol, head: LineCol) {
        let anchor = LineCol::new(
            anchor.line.min(last_line(buffer)),
            anchor
                .col
                .min(line_len(buffer, anchor.line.min(last_line(buffer)))),
        );
        self.selection_anchor = Some(anchor);
        self.goto(buffer, head);
    }

    /// The buffer position under the screen cell `(col, row)`, given the editor's
    /// render `area`. Accounts for the gutter width and the scroll offsets.
    #[must_use]
    pub fn pos_at(&self, area: Rect, buffer: &TextBuffer, col: u16, row: u16) -> LineCol {
        let line_count = buffer.line_count().max(1) as u32;
        let rel_row = u32::from(row.saturating_sub(area.y));
        let line = (self.scroll_line + rel_row).min(line_count - 1);
        let gutter = 1 + digit_count(line_count) as u16 + 1;
        let content_x = area.x.saturating_add(gutter);
        let rel_col = u32::from(col.saturating_sub(content_x));
        let want = self.scroll_col + rel_col;
        LineCol::new(line, want.min(line_len(buffer, line)))
    }

    /// Clamp the cursor column to the current line's length.
    fn clamp_col(&mut self, buffer: &TextBuffer) {
        self.cursor.col = self.cursor.col.min(line_len(buffer, self.cursor.line));
    }
}

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
    folds: Option<&'a FoldRegions>,
    selection: Option<Range>,
    focused: bool,
    read_only: bool,
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
            folds: None,
            selection: None,
            focused: false,
            read_only: false,
        }
    }

    /// Supply the active selection range to paint.
    #[must_use]
    pub fn selection(mut self, selection: Option<Range>) -> Self {
        self.selection = selection;
        self
    }

    /// Mark the editor focused, so the caret cell is drawn.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
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

    /// Supply fold regions.
    #[must_use]
    pub fn folds(mut self, folds: &'a FoldRegions) -> Self {
        self.folds = Some(folds);
        self
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

    /// Whether the cell at line `l`, column `col` lies within the active selection.
    fn in_selection(&self, l: u32, col: u32) -> bool {
        self.selection.is_some_and(|r| col_in_range(l, col, r))
    }

    /// Append the syntax-colored content spans for line `l`, honoring horizontal
    /// scroll and text-background decorations.
    fn push_content_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        l: u32,
        theme: &Theme,
        default_fg: Rgba,
        scroll_col: u32,
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
        for (boff, ch) in content.char_indices() {
            if col < scroll_col {
                col += 1;
                continue;
            }
            let fg = fg_for(line_start + boff, hl, theme, default_fg);
            let mut style = Style::default().fg(fg.to_ratatui());
            let bg = if self.in_selection(l, col) {
                Some(theme.role(ThemeRole::Selection))
            } else {
                self.decoration_bg(l, col, theme)
            };
            if let Some(bg) = bg {
                style = style.bg(bg.to_ratatui());
            }
            if run_style == Some(style) {
                run.push(ch);
            } else {
                if let Some(prev) = run_style {
                    spans.push(Span::styled(std::mem::take(&mut run), prev));
                }
                run.push(ch);
                run_style = Some(style);
            }
            col += 1;
        }
        if let Some(prev) = run_style {
            spans.push(Span::styled(run, prev));
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

        // Clamp scroll to the buffer and record the viewport height for motions.
        state.last_height = area.height;
        state.scroll_line = state.scroll_line.min(line_count.saturating_sub(1));

        // Base background for the whole editor area (covers rows past end-of-file).
        buf.set_style(area, Style::default().bg(background.to_ratatui()));

        for row in 0..area.height {
            let l = state.scroll_line + u32::from(row);
            if l >= line_count {
                break;
            }
            let y = area.y + row;
            // In read-only (pager) mode there is no active cursor line to emphasize.
            let is_cursor = !self.read_only && l == state.cursor.line;
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

            let (marker_ch, marker_color) = self
                .gutter_marker(l, theme, default_fg)
                .unwrap_or((' ', default_fg));
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
                    format!("{:>width$} ", l + 1, width = digits),
                    Style::default().fg(number_color.to_ratatui()),
                ),
            ];
            self.push_content_spans(&mut spans, l, theme, default_fg, state.scroll_col);
            buf.set_line(area.x, y, &Line::from(spans), area.width);
        }

        // Draw the caret as a reversed cell when the editor is focused and editable.
        if self.focused && !self.read_only {
            let cl = state.cursor.line;
            let top = state.scroll_line;
            if cl >= top
                && cl < top + u32::from(area.height)
                && state.cursor.col >= state.scroll_col
            {
                let gutter = 1 + digits as u16 + 1;
                let cx = area.x
                    + gutter
                    + u16::try_from(state.cursor.col - state.scroll_col).unwrap_or(u16::MAX);
                let cy = area.y + u16::try_from(cl - top).unwrap_or(0);
                if cx < area.right() && cy < area.bottom() {
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
            }
        }
    }
}

/// The index of the last line in `buffer` (0 for an empty buffer).
fn last_line(buffer: &TextBuffer) -> u32 {
    (buffer.line_count().max(1) - 1) as u32
}

/// The length (in `char`s) of line `line` in `buffer`.
fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    buffer
        .line(line as usize)
        .map_or(0, |s| s.chars().count() as u32)
}

/// The number of decimal digits needed to print `n`.
fn digit_count(n: u32) -> usize {
    if n < 10 { 1 } else { (n.ilog10() + 1) as usize }
}

/// Whether line `l` falls within the line span of `range`.
fn line_in_range(l: u32, range: Range) -> bool {
    l >= range.start.line && l <= range.end.line
}

/// Whether column `col` on line `l` falls within `range`.
fn col_in_range(l: u32, col: u32, range: Range) -> bool {
    if !line_in_range(l, range) {
        return false;
    }
    let lo = if l == range.start.line {
        range.start.col
    } else {
        0
    };
    let hi = if l == range.end.line {
        range.end.col
    } else {
        u32::MAX
    };
    col >= lo && col < hi
}

/// The foreground color for the char at absolute byte `abs` from the highlight
/// spans, falling back to `default_fg`.
fn fg_for(abs: usize, hl: &[HighlightSpan], theme: &Theme, default_fg: Rgba) -> Rgba {
    for s in hl {
        if s.span.start.0 <= abs && abs < s.span.end.0 {
            return theme.color(s.token);
        }
    }
    default_fg
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;
    use karet_core::TokenId;

    use super::*;

    #[test]
    fn editor_builder_collects_layers() {
        let buffer = TextBuffer::from_text("fn main() {}");
        let _editor = Editor::new(&buffer).diagnostics(&[]).decorations(&[]);
        assert_eq!(EditorState::new().scroll_line, 0);
    }

    #[test]
    fn fg_for_uses_highlight_then_default() {
        let theme = Theme::dark();
        let default_fg = theme.role(ThemeRole::Foreground);
        let hl = [HighlightSpan {
            span: karet_core::Span {
                start: BytePos(0),
                end: BytePos(2),
            },
            token: TokenId(0),
        }];
        assert_eq!(fg_for(1, &hl, &theme, default_fg), theme.color(TokenId(0)));
        assert_eq!(fg_for(5, &hl, &theme, default_fg), default_fg);
    }

    #[test]
    fn scroll_to_keeps_cursor_in_view() {
        let mut state = EditorState::new();
        state.last_height = 10;
        state.scroll_to(LineCol::new(25, 0));
        let vp = state.viewport();
        assert!(vp.start.line <= 25 && 25 < vp.end.line);
        state.scroll_to(LineCol::new(0, 0));
        assert_eq!(state.scroll_line, 0);
    }

    #[test]
    fn motions_clamp_to_buffer() {
        let buffer = TextBuffer::from_text("ab\ncde\nf");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.move_down(&buffer);
        state.move_down(&buffer);
        state.move_down(&buffer); // past the end clamps to the last line
        assert_eq!(state.cursor.line, 2);
        state.goto(&buffer, LineCol::new(1, 99)); // col clamps to the line length
        assert_eq!(state.cursor, LineCol::new(1, 3));
    }

    #[test]
    fn pos_at_accounts_for_gutter_and_scroll() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let mut state = EditorState::new();
        state.last_height = 3;
        let area = Rect::new(0, 0, 20, 3);
        // gutter = marker(1) + 1 digit + space = 3; column 5 -> content col 2.
        assert_eq!(state.pos_at(area, &buffer, 5, 0), LineCol::new(0, 2));
        // A click past the line end clamps to the line length.
        assert_eq!(state.pos_at(area, &buffer, 100, 0), LineCol::new(0, 5));
        // Vertical scroll shifts the mapped line.
        state.scroll_line = 1;
        assert_eq!(state.pos_at(area, &buffer, 3, 0), LineCol::new(1, 0));
    }

    #[test]
    fn selection_range_normalizes_and_clears() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let mut state = EditorState::new();
        state.last_height = 3;
        assert_eq!(state.selection_range(), None);
        state.set_caret(&buffer, LineCol::new(0, 2));
        assert_eq!(
            state.selection_range(),
            None,
            "a bare caret is not a selection"
        );
        state.extend_to(&buffer, LineCol::new(1, 1));
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 2),
                end: LineCol::new(1, 1),
            })
        );
        // Dragging back above the anchor normalizes start <= end.
        state.extend_to(&buffer, LineCol::new(0, 0));
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 2),
            })
        );
        state.clear_selection();
        assert_eq!(state.selection_range(), None);
    }

    #[test]
    fn render_draws_gutter_and_cursor_line() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let theme = Theme::dark();
        let mut state = EditorState::new();
        state.cursor = LineCol::new(1, 0);
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .render(area, &mut buf, &mut state);

        let row0: String = (0..20)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row0.contains('1'), "gutter line number missing: {row0:?}");
        assert!(row0.contains("alpha"), "content missing: {row0:?}");
        // The cursor row (line 1) carries the cursor-line background.
        assert_eq!(
            buf[(0, 1)].bg,
            theme.role(ThemeRole::CursorLine).to_ratatui()
        );
        // A non-cursor row uses the editor background.
        assert_eq!(
            buf[(0, 0)].bg,
            theme.role(ThemeRole::Background).to_ratatui()
        );
    }

    #[test]
    fn read_only_suppresses_cursor_line_and_caret() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let theme = Theme::dark();
        let mut state = EditorState::new();
        state.cursor = LineCol::new(1, 0);
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .focused(true) // focused, but read-only must still hide the caret
            .read_only(true)
            .render(area, &mut buf, &mut state);

        // The cursor's line carries the plain background, not the cursor-line color.
        assert_eq!(
            buf[(0, 1)].bg,
            theme.role(ThemeRole::Background).to_ratatui(),
            "read-only mode must not highlight the cursor line"
        );
        // No caret cell is drawn anywhere.
        let any_caret = (0..area.width)
            .any(|x| (0..area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED)));
        assert!(!any_caret, "read-only mode must not draw a caret");
    }

    #[test]
    fn center_on_and_scroll_paging_move_viewport_only() {
        let mut state = EditorState::new();
        state.last_height = 10;
        state.center_on(50);
        assert_eq!(state.scroll_line, 45, "line centered in a 10-row viewport");
        // Scroll-only paging moves the viewport without touching the cursor.
        state.scroll_page_up();
        assert_eq!(state.scroll_line, 35);
        state.scroll_page_down();
        assert_eq!(state.scroll_line, 45);
        assert_eq!(state.cursor.line, 0, "paging never moves the cursor");
        // Centering near the top saturates at 0.
        state.center_on(2);
        assert_eq!(state.scroll_line, 0);
    }
}
