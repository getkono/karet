//! Overview markers: coloured ranges painted onto a scrollbar track, so the track
//! doubles as a map of where things are in content too long to see at once —
//! diagnostics, changed lines, search matches, selections.
//!
//! A marker is a range in the same unit as the track's [`ScrollExtent`], and it is
//! mapped onto the track with the same proportion the thumb is: a marker on the
//! first visible unit lands on the thumb's first cell. Markers are an overlay on a
//! bar that was already painted, so a track whose content fits — and so painted no
//! bar — paints no markers either: everything they would point at is on screen.

use ratatui::buffer::Buffer;
use ratatui::style::Style;

use super::PaintedTracks;
use super::ScrollAxis;
use super::ScrollExtent;
use super::ScrollTrack;
use super::ScrollTracks;
use super::ScrollbarStyles;

/// The glyph a marker paints on the groove, a short rule across the track, so a
/// marker reads as a tick rather than as more thumb.
const VERTICAL_TICK: &str = "━";

/// The groove glyph for a horizontal track's marker, a short upright rule.
const HORIZONTAL_TICK: &str = "┃";

/// One coloured range on a scrollbar track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollMarker {
    /// The first unit the marker covers, in the track's extent unit.
    pub start: usize,
    /// One past the last unit it covers. An `end` at or before `start` covers
    /// `start` alone, so a caller can mark a single unit with `start == end`.
    pub end: usize,
    /// The style patched onto the marked cells; its foreground is the marker's
    /// colour.
    pub style: Style,
}

impl ScrollMarker {
    /// A marker over `start..end` in `style`.
    #[must_use]
    pub const fn new(start: usize, end: usize, style: Style) -> Self {
        Self { start, end, style }
    }

    /// The half-open run of cells `(first, end)` this marker covers along a track
    /// of `track_len` cells over `extent`: the cells a marked unit starts in, at
    /// least one, never past the end.
    ///
    /// `None` for an empty track, for content that fits (no bar to mark), and for
    /// a marker that starts past the end of the content.
    #[must_use]
    pub const fn cells(self, extent: ScrollExtent, track_len: u16) -> Option<(u16, u16)> {
        if track_len == 0 || !extent.overflows() || self.start >= extent.content {
            return None;
        }
        // Widened, so a document of any length maps without saturating.
        let len = track_len as u128;
        let content = extent.content as u128;
        let start = self.start as u128;
        let end = if self.end > self.start {
            self.end as u128
        } else {
            start + 1
        };
        let end = if end > content { content } else { end };
        let first = start * len / content;
        let first = if first >= len { len - 1 } else { first };
        // A cell is marked when a marked unit starts in it, or when the range
        // covers the whole cell (content shorter than the track). Taking the
        // cell a unit merely *ends* in would smear a one-line marker across two
        // cells whenever that line straddles a cell boundary.
        let starts = (end - 1) * len / content + 1;
        let covered = end * len / content;
        let last = if covered > starts { covered } else { starts };
        let last = if last > len { len } else { last };
        let last = if last <= first { first + 1 } else { last };
        Some((first as u16, last as u16))
    }
}

impl ScrollTrack {
    /// Paint `markers` over this already-painted track, in order, so a later
    /// marker wins a cell an earlier one also covers: pass them least important
    /// first.
    ///
    /// A cell on the thumb keeps the thumb glyph and takes the marker's colour, so
    /// the thumb's extent stays readable; a cell on the groove becomes a tick.
    pub fn paint_markers(self, buf: &mut Buffer, markers: &[ScrollMarker]) {
        let len = self.length();
        let Some((thumb_start, thumb_len)) = self.thumb_span() else {
            return;
        };
        let thumb_end = thumb_start.saturating_add(thumb_len);
        let rect = self.rect();
        let tick = match self.axis() {
            ScrollAxis::Vertical => VERTICAL_TICK,
            ScrollAxis::Horizontal => HORIZONTAL_TICK,
        };
        for marker in markers {
            let Some((first, end)) = marker.cells(self.extent(), len) else {
                continue;
            };
            for along in first..end {
                let position = match self.axis() {
                    ScrollAxis::Vertical => (rect.x, rect.y + along),
                    ScrollAxis::Horizontal => (rect.x + along, rect.y),
                };
                let Some(cell) = buf.cell_mut(position) else {
                    continue;
                };
                if along < thumb_start || along >= thumb_end {
                    cell.set_symbol(tick);
                }
                cell.set_style(marker.style);
            }
        }
    }
}

impl ScrollTracks {
    /// [`paint`](Self::paint) both axes, then lay `markers` over the vertical bar
    /// as [`ScrollTrack::paint_markers`] does — the overview a long document's
    /// scrollbar gives of where its problems, changes and matches are.
    pub fn paint_marked(
        self,
        buf: &mut Buffer,
        styles: ScrollbarStyles,
        vertical: ScrollExtent,
        horizontal: ScrollExtent,
        markers: &[ScrollMarker],
    ) -> PaintedTracks {
        let painted = self.paint(buf, styles, vertical, horizontal);
        if let Some(track) = painted.vertical {
            track.paint_markers(buf, markers);
        }
        painted
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::*;

    const THUMB: &str = "█";

    fn styles() -> ScrollbarStyles {
        ScrollbarStyles {
            track: Style::default().fg(Color::Blue),
            thumb: Style::default().fg(Color::White),
        }
    }

    fn red() -> Style {
        Style::default().fg(Color::Red)
    }

    /// `(symbol, fg)` of each cell down column `x` of `rect`.
    fn column(buf: &Buffer, rect: Rect) -> Vec<(String, Option<Color>)> {
        (rect.y..rect.bottom())
            .map(|y| {
                let cell = &buf[(rect.x, y)];
                (cell.symbol().to_owned(), cell.style().fg)
            })
            .collect()
    }

    #[test]
    fn a_marker_maps_onto_the_track_in_proportion_to_the_content() {
        // 100 lines on a 10-cell track: ten lines per cell.
        let extent = ScrollExtent::new(100, 0, 10);
        let cells = |start, end| ScrollMarker::new(start, end, red()).cells(extent, 10);

        assert_eq!(cells(0, 1), Some((0, 1)));
        assert_eq!(cells(55, 55), Some((5, 6)), "an empty range marks its line");
        assert_eq!(
            cells(19, 21),
            Some((1, 3)),
            "a range crossing a cell boundary marks both cells"
        );
        assert_eq!(
            cells(99, 100),
            Some((9, 10)),
            "the last line reaches the end"
        );
        assert_eq!(cells(90, 500), Some((9, 10)), "a range past the end clamps");
        assert_eq!(
            cells(100, 101),
            None,
            "a marker past the content paints nowhere"
        );
    }

    #[test]
    fn a_line_straddling_a_cell_boundary_marks_only_the_cell_it_starts_in() {
        // 1001 lines on 20 cells: line 900 runs from cell 17.98 to 18.002.
        let extent = ScrollExtent::new(1001, 0, 20);

        assert_eq!(
            ScrollMarker::new(900, 901, red()).cells(extent, 20),
            Some((17, 18))
        );
    }

    #[test]
    fn a_short_document_squeezes_no_marker_below_one_cell() {
        // Two lines over a 10-cell track: each line spans five cells.
        let extent = ScrollExtent::new(2, 0, 1);
        let marker = ScrollMarker::new(1, 2, red());

        assert_eq!(marker.cells(extent, 10), Some((5, 10)));
    }

    #[test]
    fn content_that_fits_gets_no_markers() {
        let extent = ScrollExtent::new(5, 0, 10);

        assert_eq!(ScrollMarker::new(0, 1, red()).cells(extent, 10), None);
        assert_eq!(
            ScrollMarker::new(0, 1, red()).cells(ScrollExtent::new(9, 0, 1), 0),
            None
        );
    }

    #[test]
    fn a_huge_document_does_not_overflow_the_mapping() {
        let extent = ScrollExtent::new(usize::MAX, 0, 10);
        let marker = ScrollMarker::new(usize::MAX - 1, usize::MAX, red());

        assert_eq!(marker.cells(extent, 10).map(|(_, end)| end), Some(10));
    }

    #[test]
    fn markers_tick_the_groove_and_tint_the_thumb() {
        let area = Rect::new(0, 0, 1, 10);
        let mut buf = Buffer::empty(area);
        let tracks = ScrollTracks {
            vertical: Some(area),
            horizontal: None,
        };
        // Thumb over cells 0..1; markers at line 5 (thumb) and line 50 (groove).
        let painted = tracks.paint_marked(
            &mut buf,
            styles(),
            ScrollExtent::new(100, 0, 10),
            ScrollExtent::default(),
            &[
                ScrollMarker::new(5, 6, red()),
                ScrollMarker::new(50, 51, Style::default().fg(Color::Yellow)),
            ],
        );

        assert!(painted.vertical.is_some());
        let cells = column(&buf, area);
        assert_eq!(cells[0], (THUMB.to_owned(), Some(Color::Red)));
        assert_eq!(cells[5], (VERTICAL_TICK.to_owned(), Some(Color::Yellow)));
        assert_eq!(
            cells[3].1,
            Some(Color::Blue),
            "unmarked groove keeps its style"
        );
    }

    #[test]
    fn a_later_marker_wins_a_shared_cell() {
        let area = Rect::new(0, 0, 1, 10);
        let mut buf = Buffer::empty(area);
        let track = ScrollTrack::new(area, ScrollAxis::Vertical, ScrollExtent::new(100, 0, 10));
        track.paint_markers(
            &mut buf,
            &[
                ScrollMarker::new(70, 71, Style::default().fg(Color::Yellow)),
                ScrollMarker::new(72, 73, red()),
            ],
        );

        assert_eq!(column(&buf, area)[7].1, Some(Color::Red));
    }

    #[test]
    fn a_track_whose_content_fits_paints_no_markers() {
        let area = Rect::new(0, 0, 1, 10);
        let mut buf = Buffer::empty(area);
        let tracks = ScrollTracks {
            vertical: Some(area),
            horizontal: None,
        };
        tracks.paint_marked(
            &mut buf,
            styles(),
            ScrollExtent::new(5, 0, 10),
            ScrollExtent::default(),
            &[ScrollMarker::new(2, 3, red())],
        );

        assert_eq!(buf, Buffer::empty(area));
    }

    #[test]
    fn a_horizontal_track_marks_along_its_row() {
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        let track = ScrollTrack::new(area, ScrollAxis::Horizontal, ScrollExtent::new(100, 0, 10));
        track.paint_markers(&mut buf, &[ScrollMarker::new(80, 81, red())]);

        assert_eq!(buf[(8, 0)].symbol(), HORIZONTAL_TICK);
        assert_eq!(buf[(8, 0)].style().fg, Some(Color::Red));
    }
}
