//! Painting a buffer: styled runs, a selection, and a caret that costs no space.
//!
//! The multi-line [`TextArea`] paints cell by cell from [`layout::wrap_rows`],
//! advancing by each glyph's own width, and marks the caret by *reversing the
//! cell it sits on* — the way the editor does. Nothing is inserted into the text,
//! so no glyph moves when the caret does, and what the hit test computes is what
//! the paint put there.
//!
//! [`styled_text`] is the older model, kept for the single-line fields: it builds
//! a `Text` with a caret glyph spliced in at the cursor. That shifts everything
//! after the caret one cell right, which a one-line field with cell-offset
//! scrolling tolerates and a wrapped multi-line one does not.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use super::TextAreaState;
use super::layout;

/// The caret glyph [`styled_text`] splices in, one cell wide.
const CARET: &str = "▏";

/// The three styles a text area paints its content with.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextAreaStyle {
    /// Unselected text.
    pub normal: Style,
    /// Text inside the active selection.
    pub selection: Style,
    /// The insertion caret. [`TextArea`] paints it as this style *reversed* onto
    /// the cell the caret occupies; [`styled_text`] paints its caret glyph in it.
    pub caret: Style,
}

impl TextAreaStyle {
    /// A style set painting unselected text with `normal`, the run inside the
    /// active selection with `selection`, and the insertion caret with `caret`.
    #[must_use]
    pub fn new(normal: Style, selection: Style, caret: Style) -> Self {
        Self {
            normal,
            selection,
            caret,
        }
    }
}

/// Build the styled [`Text`] for a buffer: one line per `\n`, one span per
/// character, the selected run in [`TextAreaStyle::selection`], and a caret span
/// at `cursor` when it is `Some`.
///
/// Passing `cursor: None` (an unfocused field) paints no caret. This is the
/// renderer for the single-line [`crate::textfield`]; the multi-line
/// [`TextArea`] paints its own cells instead — see the [module docs](self).
#[must_use]
pub fn styled_text(
    text: &str,
    cursor: Option<usize>,
    selection: Option<Range<usize>>,
    style: TextAreaStyle,
) -> Text<'static> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    for (index, character) in text.char_indices() {
        if cursor == Some(index) {
            spans.push(Span::styled(CARET, style.caret));
        }
        if character == '\n' {
            lines.push(Line::from(std::mem::take(&mut spans)));
            continue;
        }
        let selected = selection
            .as_ref()
            .is_some_and(|range| range.start <= index && index < range.end);
        let painted = if selected {
            style.selection
        } else {
            style.normal
        };
        spans.push(Span::styled(character.to_string(), painted));
    }
    if cursor == Some(text.len()) {
        spans.push(Span::styled(CARET, style.caret));
    }
    lines.push(Line::from(spans));
    Text::from(lines)
}

/// A multi-line text area widget: paints a buffer with its selection and caret,
/// soft-wrapped to the area and scrolled to [`TextAreaState::scroll`].
///
/// The caller owns the frame around it (borders, title, focus styling) and calls
/// [`TextAreaState::ensure_cursor_visible`] before rendering.
pub struct TextArea<'a> {
    text: &'a str,
    state: &'a TextAreaState,
    style: TextAreaStyle,
    focused: bool,
    placeholder: Option<(&'a str, Style)>,
}

impl<'a> TextArea<'a> {
    /// A text area painting `text` through `state`.
    #[must_use]
    pub fn new(text: &'a str, state: &'a TextAreaState) -> Self {
        Self {
            text,
            state,
            style: TextAreaStyle::default(),
            focused: false,
            placeholder: None,
        }
    }

    /// Set the content styles.
    #[must_use]
    pub fn style(mut self, style: TextAreaStyle) -> Self {
        self.style = style;
        self
    }

    /// Whether the field has keyboard focus; an unfocused area paints no caret.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// A hint painted in place of the content while the buffer is empty **and**
    /// the field is unfocused.
    #[must_use]
    pub fn placeholder(mut self, text: &'a str, style: Style) -> Self {
        self.placeholder = Some((text, style));
        self
    }
}

impl Widget for TextArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if let Some((hint, style)) = self.placeholder
            && self.text.is_empty()
            && !self.focused
        {
            Paragraph::new(hint)
                .style(style)
                .wrap(Wrap { trim: false })
                .render(area, buf);
            return;
        }

        let selection = self.state.selection();
        let scroll = usize::from(self.state.scroll);
        let rows = layout::wrap_rows(self.text, area.width);
        for (offset, row) in rows
            .iter()
            .skip(scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let y = area
                .y
                .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX));
            let mut x = area.x;
            for (index, character) in self
                .text
                .get(row.start..row.end)
                .unwrap_or_default()
                .char_indices()
            {
                let advance = u16::try_from(layout::glyph_width(character)).unwrap_or(1);
                if x.saturating_add(advance) > area.right() {
                    break;
                }
                let byte = row.start + index;
                let selected = selection
                    .as_ref()
                    .is_some_and(|range| range.start <= byte && byte < range.end);
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(layout::glyph_symbol(character).encode_utf8(&mut [0u8; 4]));
                    cell.set_style(if selected {
                        self.style.selection
                    } else {
                        self.style.normal
                    });
                }
                x = x.saturating_add(advance);
            }
        }

        if !self.focused {
            return;
        }
        // The caret is a style flip on the cell it occupies, never a glyph of its
        // own: inserting one would move every character after it.
        let (row, column) = layout::caret_cell(self.text, self.state.cursor(), area.width);
        let Some(row) = usize::from(row).checked_sub(scroll) else {
            return;
        };
        if row >= usize::from(area.height) || column >= area.width {
            return;
        }
        let caret = Rect {
            x: area.x.saturating_add(column),
            y: area
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
            width: 1,
            height: 1,
        };
        buf.set_style(caret, self.style.caret.add_modifier(Modifier::REVERSED));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    fn at(text: &str, cursor: usize) -> TextAreaState {
        let mut state = TextAreaState::default();
        state.set_cursor(text, cursor, false);
        state
    }

    #[test]
    fn the_renderer_splits_lines_marks_the_caret_and_styles_the_selection() {
        let text = "subject\nbody";
        let mut state = at(text, 8);
        let display = styled_text(text, Some(state.cursor()), None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2);
        assert_eq!(display.lines[0].to_string(), "subject");
        assert_eq!(display.lines[1].to_string(), "▏body");

        state.move_word_right(text, true);
        let style = TextAreaStyle::new(
            Style::default().fg(Color::White),
            Style::default().fg(Color::White).bg(Color::Blue),
            Style::default().fg(Color::Red),
        );
        let display = styled_text(text, Some(state.cursor()), state.selection(), style);
        let spans = &display.lines[1].spans;
        assert_eq!(spans.last().map(|span| span.content.as_ref()), Some("▏"));
        assert!(
            spans[..4]
                .iter()
                .all(|span| span.style.bg == Some(Color::Blue)),
            "the selected run is painted"
        );

        // Unfocused: no caret at all.
        let display = styled_text(text, None, None, TextAreaStyle::default());
        assert_eq!(display.lines[1].to_string(), "body");
    }
    #[test]
    fn the_renderer_opens_a_line_after_a_trailing_newline() {
        let display = styled_text("a\n", Some(2), None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2, "the trailing newline opens a line");
        assert_eq!(display.lines[0].to_string(), "a");
        assert_eq!(display.lines[1].to_string(), "▏");
        assert_eq!(display.lines[1].spans.len(), 1, "the caret alone");

        // No caret at all still leaves the empty final line in place.
        let display = styled_text("a\n", None, None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2);
        assert!(display.lines[1].spans.is_empty());
    }
    #[test]
    fn the_renderer_splits_a_selection_around_a_caret_inside_it() {
        let style = TextAreaStyle::new(
            Style::default().fg(Color::White),
            Style::default().fg(Color::White).bg(Color::Blue),
            Style::default().fg(Color::Red),
        );
        let display = styled_text("abcd", Some(2), Some(1..3), style);
        let spans = &display.lines[0].spans;
        assert_eq!(display.lines[0].to_string(), "ab▏cd");
        assert_eq!(spans.len(), 5, "\"a\", \"b\", caret, \"c\", \"d\"");
        assert_eq!(spans[2].content.as_ref(), "▏");
        assert_eq!(
            spans[2].style.fg,
            Some(Color::Red),
            "the caret keeps its own style"
        );
        assert_eq!(spans[1].style.bg, Some(Color::Blue));
        assert_eq!(
            spans[3].style.bg,
            Some(Color::Blue),
            "the selection resumes past it"
        );
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[4].style.bg, None);
    }
    /// Every cell's symbol in `buf`, row by row.
    fn rows(buf: &Buffer, area: Rect) -> Vec<String> {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .filter_map(|x| buf.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect()
            })
            .collect()
    }

    fn painted(text: &str, state: &TextAreaState, focused: bool, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        TextArea::new(text, state)
            .focused(focused)
            .style(TextAreaStyle::new(
                Style::default().fg(Color::White),
                Style::default().fg(Color::White).bg(Color::Blue),
                Style::default().fg(Color::Red),
            ))
            .render(area, &mut buf);
        buf
    }

    #[test]
    fn the_caret_marks_its_cell_without_moving_a_single_glyph() {
        // The defect this renderer replaced: the caret was spliced into the text
        // as a glyph of its own, so every character after it shifted one cell
        // right and the box appeared to lose a column to the cursor.
        let area = Rect::new(0, 0, 10, 2);
        let text = "abc def";
        let focused = painted(text, &at(text, 0), true, area);
        let blurred = painted(text, &at(text, 0), false, area);
        assert_eq!(
            rows(&focused, area),
            rows(&blurred, area),
            "the caret must not displace any glyph"
        );
        assert_eq!(rows(&focused, area)[0], "abc def   ");

        // It is the cell that is marked, and only that cell.
        let reversed = |buf: &Buffer, x: u16| {
            buf.cell((x, 0))
                .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED))
        };
        assert!(reversed(&focused, 0), "the caret cell is reversed");
        assert!(!reversed(&focused, 1));
        assert!(
            (0..area.width).all(|x| !reversed(&blurred, x)),
            "an unfocused area paints no caret at all"
        );
    }

    #[test]
    fn the_widget_wraps_paints_the_selection_and_shows_the_placeholder() {
        let area = Rect::new(0, 0, 4, 3);
        let text = "abcdef";
        // "abcdef" wraps to "abcd" / "ef", and the caret at the end of an
        // exactly-full buffer opens the row below.
        let buf = painted(text, &at(text, 6), true, area);
        assert_eq!(rows(&buf, area), ["abcd", "ef  ", "    "]);
        assert!(
            buf.cell((2, 1))
                .is_some_and(|cell| cell.modifier.contains(Modifier::REVERSED)),
            "the caret sits just past the last glyph"
        );

        let mut state = at(text, 1);
        state.set_cursor(text, 3, true);
        let buf = painted(text, &state, true, area);
        let selected = |x: u16| buf.cell((x, 0)).map(|cell| cell.bg);
        assert_eq!(selected(0), Some(Color::Reset), "\"a\" is outside it");
        assert_eq!(selected(1), Some(Color::Blue));
        assert_eq!(selected(2), Some(Color::Blue));
        assert_eq!(selected(3), Some(Color::Reset), "the run ends before \"d\"");

        // The placeholder shows only while empty and unfocused.
        let state = TextAreaState::default();
        let mut buf = Buffer::empty(area);
        TextArea::new("", &state)
            .placeholder("hint", Style::default().fg(Color::Gray))
            .render(area, &mut buf);
        assert_eq!(rows(&buf, area)[0], "hint");
        let buf = painted("", &state, true, area);
        assert_eq!(rows(&buf, area)[0], "    ", "focused: the hint gives way");
    }

    #[test]
    fn a_control_character_paints_as_one_space_rather_than_desyncing_the_row() {
        let area = Rect::new(0, 0, 6, 1);
        let text = "a\tbc";
        let buf = painted(text, &at(text, 0), false, area);
        assert_eq!(rows(&buf, area)[0], "a bc  ", "the tab is one space");
    }

    #[test]
    fn the_widget_scrolls_to_the_state_viewport_and_skips_an_empty_area() {
        let text = "one\ntwo\nthree";
        let mut state = at(text, 0);
        state.scroll = 2;
        let area = Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        TextArea::new(text, &state).render(area, &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some("t".to_owned())
        );

        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        TextArea::new(text, &state).render(Rect::new(0, 0, 0, 0), &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some(" ".to_owned())
        );
    }
}
