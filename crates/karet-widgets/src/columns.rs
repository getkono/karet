//! A cascading-column navigator: parallel lists where each column shows the children of
//! the selection to its left.
//!
//! The shape earns its place when the reader's question is "what is *under* this?" rather
//! than "where is this?". An indented tree answers the second well and the first badly:
//! by the time you have expanded four levels the ancestors have scrolled away, and the
//! siblings you were comparing are separated by everything you expanded in between.
//! Columns keep each level whole and adjacent, so descending never costs you the context
//! you descended from.
//!
//! It is also the shape that fails worst in a narrow terminal, so this widget does not
//! pretend otherwise: [`Columns::fits`] reports how many columns a width can actually
//! carry, and the consumer is expected to fall back to an indented tree below that. A
//! column squeezed to eight cells is not a degraded column, it is an unreadable one.
//!
//! Nothing here knows what it is listing. Rows arrive pre-composed — label, marker
//! glyphs, trailing count — so the same widget serves any hierarchy.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

/// The narrowest column that is still worth drawing.
///
/// Below this a label truncates to a few characters and the column stops carrying
/// information, so the consumer should switch to a different rendering instead.
pub const MIN_COLUMN_WIDTH: u16 = 18;

/// How a row is emphasized, independent of selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowEmphasis {
    /// An ordinary row.
    #[default]
    Normal,
    /// Present but not active — demoted by a filter, or excluded by a configuration.
    ///
    /// Demoted rather than hidden, so the shape of the tree never changes under the
    /// reader as they toggle a filter.
    Dimmed,
}

/// One row of a column, fully composed by the consumer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnRow {
    /// The row's text.
    pub label: String,
    /// Marker glyphs shown after the label, already resolved to the icon style.
    ///
    /// Characters rather than one string: each is painted into its own fixed-width slot,
    /// so a row carrying a glyph the font draws wide still lines its count up with the
    /// row above. A pre-composed string cannot be re-spaced — nothing downstream can see
    /// where one glyph ended and the next began.
    pub markers: Vec<char>,
    /// A right-aligned trailing value, such as a count.
    pub trailing: Option<String>,
    /// Whether descending into this row would show anything.
    pub has_children: bool,
    /// How the row is emphasized.
    pub emphasis: RowEmphasis,
}

impl ColumnRow {
    /// A plain row with just a label.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }

    /// This row with marker glyphs, one per slot.
    #[must_use]
    pub fn with_markers(mut self, markers: impl IntoIterator<Item = char>) -> Self {
        self.markers = markers.into_iter().collect();
        self
    }

    /// This row with a trailing value.
    #[must_use]
    pub fn with_trailing(mut self, trailing: impl Into<String>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// This row marked as having children.
    #[must_use]
    pub fn with_children(mut self, has_children: bool) -> Self {
        self.has_children = has_children;
        self
    }

    /// This row marked as demoted.
    #[must_use]
    pub fn dimmed(mut self) -> Self {
        self.emphasis = RowEmphasis::Dimmed;
        self
    }
}

/// One column: its rows, which is selected, and how far it has scrolled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Column {
    /// The rows, in order.
    pub rows: Vec<ColumnRow>,
    /// The selected row, when one is.
    pub selected: Option<usize>,
    /// The first visible row.
    pub offset: usize,
}

impl Column {
    /// A column over `rows` with nothing selected.
    #[must_use]
    pub fn new(rows: Vec<ColumnRow>) -> Self {
        Self {
            rows,
            selected: None,
            offset: 0,
        }
    }

    /// This column with `index` selected.
    #[must_use]
    pub fn selecting(mut self, index: usize) -> Self {
        self.selected = Some(index);
        self
    }

    /// Scroll so the selected row is visible in a viewport `height` rows tall.
    pub fn scroll_into_view(&mut self, height: u16) {
        let Some(selected) = self.selected else {
            return;
        };
        let height = usize::from(height).max(1);
        if selected < self.offset {
            self.offset = selected;
        } else if selected >= self.offset + height {
            self.offset = selected.saturating_sub(height - 1);
        }
    }
}

/// The styles a consumer supplies; the widget picks between them but chooses no colours.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColumnStyle {
    /// An ordinary row.
    pub normal: Style,
    /// The selected row in the focused column.
    pub selected: Style,
    /// The selected row in a column that is not focused, so the trail stays readable.
    pub trail: Style,
    /// A demoted row.
    pub dimmed: Style,
    /// Marker glyphs.
    pub marker: Style,
    /// The trailing value.
    pub trailing: Style,
    /// The vertical rule between columns.
    pub divider: Style,
}

/// A cascading-column navigator.
pub struct Columns<'a> {
    /// The columns, outermost first.
    pub columns: &'a [Column],
    /// Which column has focus.
    pub focused: usize,
    /// The styles to paint with.
    pub style: ColumnStyle,
    /// The glyph marking a row with children.
    pub child_marker: char,
    /// The cells each marker glyph is given.
    ///
    /// One by default, which is what a consumer composing its own single-width glyphs
    /// wants. A consumer resolving them through [`crate::glyph`] passes
    /// [`glyph_slot`](crate::glyph::glyph_slot) for its icon style instead, so a tier
    /// whose width the terminal may disagree about still paints an aligned run.
    pub marker_slot: u16,
}

impl<'a> Columns<'a> {
    /// A navigator over `columns`, focused on `focused`.
    #[must_use]
    pub fn new(columns: &'a [Column], focused: usize, style: ColumnStyle) -> Self {
        Self {
            columns,
            focused,
            style,
            child_marker: '>',
            marker_slot: 1,
        }
    }

    /// Use a different has-children glyph.
    #[must_use]
    pub fn child_marker(mut self, marker: char) -> Self {
        self.child_marker = marker;
        self
    }

    /// Reserve `cells` for every marker glyph, including the has-children one.
    #[must_use]
    pub fn marker_slot(mut self, cells: u16) -> Self {
        self.marker_slot = cells.max(1);
        self
    }

    /// How many columns `width` can carry at a readable width.
    ///
    /// Zero means the caller should render something else entirely rather than squeeze.
    #[must_use]
    pub fn fits(width: u16) -> usize {
        if width < MIN_COLUMN_WIDTH {
            return 0;
        }
        // Each column past the first also costs a divider.
        usize::from((width + 1) / (MIN_COLUMN_WIDTH + 1)).max(1)
    }

    /// The window of columns to show, keeping the focused one visible.
    ///
    /// The window is anchored to the *right* — at the deepest levels the reader is
    /// working in, with as much ancestry as still fits behind it.
    #[must_use]
    pub fn window(total: usize, focused: usize, capacity: usize) -> std::ops::Range<usize> {
        if capacity == 0 || total == 0 {
            return 0..0;
        }
        if total <= capacity {
            return 0..total;
        }
        let end = (focused + 1).max(capacity).min(total);
        end.saturating_sub(capacity)..end
    }

    /// Split `area` into one rect per column, with single-cell dividers between.
    #[must_use]
    pub fn layout(area: Rect, count: usize) -> Vec<Rect> {
        if count == 0 || area.width == 0 {
            return Vec::new();
        }
        let count_u16 = u16::try_from(count).unwrap_or(u16::MAX).max(1);
        let dividers = count_u16.saturating_sub(1);
        let usable = area.width.saturating_sub(dividers);
        let each = usable / count_u16;
        let mut extra = usable % count_u16;
        let mut rects = Vec::with_capacity(count);
        let mut x = area.x;
        for _ in 0..count_u16 {
            // Spread the remainder one cell at a time so columns differ by at most one.
            let mut width = each;
            if extra > 0 {
                width += 1;
                extra -= 1;
            }
            rects.push(Rect::new(x, area.y, width, area.height));
            x = x.saturating_add(width).saturating_add(1);
        }
        rects
    }

    /// The `(column, row)` at a screen position, when one is there.
    #[must_use]
    pub fn hit(&self, area: Rect, x: u16, y: u16) -> Option<(usize, usize)> {
        if !area.contains((x, y).into()) {
            return None;
        }
        let rects = Self::layout(area, self.columns.len());
        let (index, rect) = rects
            .iter()
            .enumerate()
            .find(|(_, rect)| x >= rect.x && x < rect.x.saturating_add(rect.width))?;
        let column = self.columns.get(index)?;
        let row = column.offset + usize::from(y.saturating_sub(rect.y));
        (row < column.rows.len()).then_some((index, row))
    }

    /// Paint the navigator into `area`.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rects = Self::layout(area, self.columns.len());
        for (index, (column, rect)) in self.columns.iter().zip(&rects).enumerate() {
            self.render_column(column, *rect, index == self.focused, buf);
            // The divider sits in the gap this column's rect leaves before the next.
            let divider_x = rect.x.saturating_add(rect.width);
            if index + 1 < rects.len() && divider_x < area.x.saturating_add(area.width) {
                for y in rect.y..rect.y.saturating_add(rect.height) {
                    if let Some(cell) = buf.cell_mut((divider_x, y)) {
                        cell.set_symbol("│").set_style(self.style.divider);
                    }
                }
            }
        }
    }

    /// Paint one column's visible rows.
    fn render_column(&self, column: &Column, rect: Rect, focused: bool, buf: &mut Buffer) {
        if rect.width == 0 {
            return;
        }
        for offset in 0..rect.height {
            let Some(row) = column.rows.get(column.offset + usize::from(offset)) else {
                break;
            };
            let selected = column.selected == Some(column.offset + usize::from(offset));
            let style = match (selected, focused, row.emphasis) {
                (true, true, _) => self.style.selected,
                // A selected row in an unfocused column is the trail the reader followed
                // to get here, so it stays legible rather than reverting to ordinary.
                (true, false, _) => self.style.trail,
                (false, _, RowEmphasis::Dimmed) => self.style.dimmed,
                (false, _, RowEmphasis::Normal) => self.style.normal,
            };
            self.render_row(
                row,
                Rect::new(rect.x, rect.y + offset, rect.width, 1),
                style,
                buf,
            );
        }
    }

    /// Paint one row: label, markers, and a right-aligned trailing value.
    fn render_row(&self, row: &ColumnRow, rect: Rect, style: Style, buf: &mut Buffer) {
        buf.set_style(rect, style);
        let width = usize::from(rect.width);

        // Reserve the right edge for the count and the child marker, so a long label
        // truncates rather than pushing them out of view.
        let slot = usize::from(self.marker_slot.max(1));
        let trailing = row.trailing.as_deref().unwrap_or("");
        let marker_width = if row.has_children { slot } else { 0 };
        let reserved = trailing.width() + marker_width + usize::from(!trailing.is_empty());
        let markers_width = row.markers.len().saturating_mul(slot);
        let label_room = width.saturating_sub(reserved + markers_width + 1);

        let mut x = rect.x;
        let label = truncate(&row.label, label_room);
        buf.set_stringn(x, rect.y, &label, label_room, style);
        x = x.saturating_add(u16::try_from(label.width()).unwrap_or(0));

        // One glyph per slot, advanced by the slot rather than by the glyph: a font that
        // draws a marker wide then overruns its own padding and nothing else.
        let markers_end = rect
            .x
            .saturating_add(rect.width)
            .saturating_sub(u16::try_from(reserved).unwrap_or(u16::MAX));
        let mut marker_x = x.saturating_add(1);
        for glyph in &row.markers {
            if marker_x >= markers_end {
                break;
            }
            if let Some(cell) = buf.cell_mut((marker_x, rect.y)) {
                cell.set_char(*glyph).set_style(self.style.marker);
            }
            marker_x = marker_x.saturating_add(self.marker_slot.max(1));
        }

        let mut right = rect.x.saturating_add(rect.width);
        if row.has_children {
            right = right.saturating_sub(self.marker_slot.max(1));
            if let Some(cell) = buf.cell_mut((right, rect.y)) {
                cell.set_char(self.child_marker).set_style(style);
            }
        }
        if !trailing.is_empty() {
            let value_width = u16::try_from(trailing.width()).unwrap_or(0);
            let value_x = right.saturating_sub(value_width);
            if value_x >= rect.x {
                buf.set_stringn(
                    value_x,
                    rect.y,
                    trailing,
                    trailing.width(),
                    self.style.trailing,
                );
            }
        }
    }
}

/// Shorten `text` to `room` display columns, marking the cut with an ellipsis.
fn truncate(text: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
    if text.width() <= room {
        return text.to_owned();
    }
    if room == 1 {
        return "…".to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = ch.to_string().width();
        if used + ch_width > room - 1 {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(labels: &[&str]) -> Vec<ColumnRow> {
        labels.iter().map(|l| ColumnRow::new(*l)).collect()
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.left()..area.right())
            .map(|x| {
                buf.cell((x, y))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
            })
            .collect()
    }

    fn render(columns: &[Column], focused: usize, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        Columns::new(columns, focused, ColumnStyle::default()).render(area, &mut buf);
        buf
    }

    #[test]
    fn a_width_too_narrow_for_one_column_carries_none() {
        // The consumer is expected to render something else entirely rather than squeeze.
        assert_eq!(Columns::fits(MIN_COLUMN_WIDTH - 1), 0);
        assert_eq!(Columns::fits(0), 0);
        assert_eq!(Columns::fits(MIN_COLUMN_WIDTH), 1);
    }

    #[test]
    fn wider_terminals_carry_proportionally_more_columns() {
        assert_eq!(Columns::fits(MIN_COLUMN_WIDTH * 2 + 1), 2);
        assert_eq!(Columns::fits(MIN_COLUMN_WIDTH * 3 + 2), 3);
        assert!(Columns::fits(200) >= 4);
    }

    #[test]
    fn the_window_is_anchored_to_the_deepest_visible_level() {
        // Everything fits.
        assert_eq!(Columns::window(3, 1, 4), 0..3);
        // Deeper than fits: keep the focused column and as much ancestry as remains.
        assert_eq!(Columns::window(6, 5, 3), 3..6);
        assert_eq!(Columns::window(6, 2, 3), 0..3);
        assert_eq!(Columns::window(0, 0, 3), 0..0);
        assert_eq!(Columns::window(4, 0, 0), 0..0);
    }

    #[test]
    fn layout_splits_evenly_and_leaves_room_for_dividers() {
        let rects = Columns::layout(Rect::new(0, 0, 31, 5), 3);
        assert_eq!(rects.len(), 3);
        // 31 cells minus 2 dividers is 29, split 10/10/9.
        let total: u16 = rects.iter().map(|r| r.width).sum();
        assert_eq!(total, 29);
        assert!(rects.windows(2).all(|w| w[1].x == w[0].x + w[0].width + 1));
        // Widths differ by at most one cell.
        let widest = rects.iter().map(|r| r.width).max().unwrap_or(0);
        let narrowest = rects.iter().map(|r| r.width).min().unwrap_or(0);
        assert!(widest - narrowest <= 1);
    }

    #[test]
    fn layout_of_nothing_is_nothing() {
        assert!(Columns::layout(Rect::new(0, 0, 40, 5), 0).is_empty());
        assert!(Columns::layout(Rect::new(0, 0, 0, 5), 3).is_empty());
    }

    #[test]
    fn rows_are_painted_into_their_own_column() {
        let columns = [
            Column::new(rows(&["alpha", "beta"])).selecting(0),
            Column::new(rows(&["gamma"])),
        ];
        let buf = render(&columns, 0, Rect::new(0, 0, 21, 3));
        let first = row_text(&buf, 0);
        assert!(first.starts_with("alpha"), "got {first:?}");
        assert!(first.contains("gamma"), "got {first:?}");
        assert!(row_text(&buf, 1).starts_with("beta"));
    }

    #[test]
    fn a_divider_separates_adjacent_columns() {
        let columns = [Column::new(rows(&["a"])), Column::new(rows(&["b"]))];
        let buf = render(&columns, 0, Rect::new(0, 0, 21, 1));
        assert!(
            row_text(&buf, 0).contains('│'),
            "got {:?}",
            row_text(&buf, 0)
        );
    }

    #[test]
    fn a_child_marker_and_count_sit_at_the_right_edge() {
        let columns = [Column::new(vec![
            ColumnRow::new("module")
                .with_children(true)
                .with_trailing("47"),
        ])];
        let buf = render(&columns, 0, Rect::new(0, 0, 20, 1));
        let text = row_text(&buf, 0);
        assert!(text.starts_with("module"), "got {text:?}");
        assert!(text.ends_with("47>"), "got {text:?}");
    }

    #[test]
    fn markers_follow_the_label() {
        let columns = [Column::new(vec![
            ColumnRow::new("Symbol").with_markers("*#".chars()),
        ])];
        let buf = render(&columns, 0, Rect::new(0, 0, 20, 1));
        let text = row_text(&buf, 0);
        assert!(text.contains("Symbol *#"), "got {text:?}");
    }

    #[test]
    fn each_marker_glyph_gets_its_own_slot() {
        // Rows with different marker counts must still agree about where the count sits.
        let columns = [Column::new(vec![
            ColumnRow::new("one")
                .with_markers("*".chars())
                .with_trailing("7"),
            ColumnRow::new("three")
                .with_markers("*#%".chars())
                .with_trailing("7"),
        ])];
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        Columns::new(&columns, 0, ColumnStyle::default())
            .marker_slot(2)
            .render(area, &mut buf);
        let first = row_text(&buf, 0);
        let second = row_text(&buf, 1);
        assert_eq!(first.find('7'), second.find('7'), "{first:?} vs {second:?}");
        // Two cells apart, whatever the glyphs measure.
        assert_eq!(
            second.find('*').map(|x| second.find('#').map(|y| y - x)),
            Some(Some(2))
        );
    }

    #[test]
    fn a_marker_wider_than_one_cell_does_not_shift_the_row() {
        let narrow = [Column::new(vec![
            ColumnRow::new("row")
                .with_markers("*".chars())
                .with_trailing("7"),
        ])];
        let wide = [Column::new(vec![
            ColumnRow::new("row")
                .with_markers("\u{65e5}".chars())
                .with_trailing("7"),
        ])];
        let area = Rect::new(0, 0, 20, 1);
        let paint = |columns: &[Column]| {
            let mut buf = Buffer::empty(area);
            Columns::new(columns, 0, ColumnStyle::default())
                .marker_slot(2)
                .render(area, &mut buf);
            row_text(&buf, 0)
        };
        // By cell, not by byte: a wide glyph is three bytes and would shift `find`.
        let column_of = |text: String, needle: char| text.chars().position(|c| c == needle);
        assert_eq!(column_of(paint(&narrow), '7'), column_of(paint(&wide), '7'));
    }

    #[test]
    fn a_child_marker_keeps_its_slot_clear_of_the_count() {
        let columns = [Column::new(vec![
            ColumnRow::new("module")
                .with_markers("*#".chars())
                .with_children(true)
                .with_trailing("47"),
        ])];
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        Columns::new(&columns, 0, ColumnStyle::default())
            .marker_slot(2)
            .render(area, &mut buf);
        let text = row_text(&buf, 0);
        // The count survives beside a two-cell child marker rather than being painted on.
        assert!(text.contains("47"), "got {text:?}");
        assert!(text.contains('>'), "got {text:?}");
    }

    #[test]
    fn a_long_label_truncates_rather_than_evicting_its_count() {
        let columns = [Column::new(vec![
            ColumnRow::new("an_extremely_long_identifier_name")
                .with_children(true)
                .with_trailing("12"),
        ])];
        let buf = render(&columns, 0, Rect::new(0, 0, 20, 1));
        let text = row_text(&buf, 0);
        assert!(text.contains('…'), "got {text:?}");
        assert!(text.ends_with("12>"), "the count must survive: {text:?}");
    }

    #[test]
    fn selection_style_distinguishes_the_focused_column_from_the_trail() {
        use ratatui::style::Color;
        let style = ColumnStyle {
            selected: Style::default().bg(Color::Blue),
            trail: Style::default().bg(Color::DarkGray),
            ..ColumnStyle::default()
        };
        let columns = [
            Column::new(rows(&["left"])).selecting(0),
            Column::new(rows(&["right"])).selecting(0),
        ];
        let area = Rect::new(0, 0, 21, 1);
        let mut buf = Buffer::empty(area);
        Columns::new(&columns, 1, style).render(area, &mut buf);

        // The unfocused column's selection is the trail the reader followed here.
        assert_eq!(buf.cell((0, 0)).map(|c| c.bg), Some(Color::DarkGray));
        let right_x = Columns::layout(area, 2).get(1).map_or(0, |r| r.x);
        assert_eq!(buf.cell((right_x, 0)).map(|c| c.bg), Some(Color::Blue));
    }

    #[test]
    fn a_dimmed_row_is_styled_apart_but_still_drawn() {
        use ratatui::style::Color;
        let style = ColumnStyle {
            dimmed: Style::default().fg(Color::DarkGray),
            ..ColumnStyle::default()
        };
        let columns = [Column::new(vec![ColumnRow::new("gated").dimmed()])];
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        Columns::new(&columns, 0, style).render(area, &mut buf);
        // Present but demoted — never removed, so the tree keeps its shape.
        assert!(row_text(&buf, 0).starts_with("gated"));
        assert_eq!(buf.cell((0, 0)).map(|c| c.fg), Some(Color::DarkGray));
    }

    #[test]
    fn scrolling_keeps_the_selection_in_view_from_both_directions() {
        let mut column = Column::new(rows(&["a", "b", "c", "d", "e", "f"]));
        column.selected = Some(5);
        column.scroll_into_view(3);
        assert_eq!(column.offset, 3);

        column.selected = Some(0);
        column.scroll_into_view(3);
        assert_eq!(column.offset, 0);
    }

    #[test]
    fn a_scrolled_column_paints_from_its_offset() {
        let mut column = Column::new(rows(&["a", "b", "c", "d"]));
        column.offset = 2;
        let buf = render(&[column], 0, Rect::new(0, 0, 20, 2));
        assert!(row_text(&buf, 0).starts_with('c'));
        assert!(row_text(&buf, 1).starts_with('d'));
    }

    #[test]
    fn hit_testing_maps_a_position_back_to_a_row() {
        let columns = [
            Column::new(rows(&["a", "b"])),
            Column::new(rows(&["c", "d", "e"])),
        ];
        let area = Rect::new(0, 0, 21, 3);
        let widget = Columns::new(&columns, 0, ColumnStyle::default());
        assert_eq!(widget.hit(area, 0, 1), Some((0, 1)));

        let second_x = Columns::layout(area, 2).get(1).map_or(0, |r| r.x);
        assert_eq!(widget.hit(area, second_x, 2), Some((1, 2)));
        // Past the last row of that column, and outside the area entirely.
        assert_eq!(widget.hit(area, 0, 2), None);
        assert_eq!(widget.hit(area, 99, 0), None);
    }

    #[test]
    fn hit_testing_honours_the_scroll_offset() {
        let mut column = Column::new(rows(&["a", "b", "c", "d"]));
        column.offset = 2;
        let columns = [column];
        let area = Rect::new(0, 0, 20, 2);
        let widget = Columns::new(&columns, 0, ColumnStyle::default());
        assert_eq!(widget.hit(area, 0, 0), Some((0, 2)));
    }

    #[test]
    fn rendering_into_nothing_does_not_panic() {
        let columns = [Column::new(rows(&["a"]))];
        let _ = render(&columns, 0, Rect::new(0, 0, 0, 0));
        let _ = render(&[], 0, Rect::new(0, 0, 20, 3));
    }

    #[test]
    fn truncation_respects_display_width() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 1), "…");
        assert_eq!(truncate("abc", 0), "");
    }
}
