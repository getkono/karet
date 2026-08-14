//! Scrollbars, and the two-axis scrollable text panel built on them.
//!
//! A scrollbar never paints over content. A caller first *reserves* a one-cell
//! track out of its area with [`reserve_tracks`], draws into the rect that is left,
//! and then paints a [`ScrollBar`] into the track.
//!
//! Reservation depends only on the area and on which axes the view scrolls — never
//! on the content extent or the current offset. That is load-bearing: a
//! content-dependent reservation feeds back on itself, because reserving a column
//! narrows the wrap width, which produces more visual rows, which overflows the
//! viewport, which reserves a column. It also means text never reflows as you
//! scroll. When the content fits, the bar is suppressed but the column stays
//! reserved.
//!
//! [`ScrollBar`] is a plain [`Widget`] rather than a function taking a [`Frame`],
//! so `Frame::render_widget` and `bar.render(track, buf)` are the same code path —
//! which is what lets `StatefulWidget`-based views (the editor, the file tree) and
//! `Frame`-drawing views share one implementation.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Scrollbar;
use ratatui::widgets::ScrollbarOrientation;
use ratatui::widgets::ScrollbarState;
use ratatui::widgets::Widget;

/// The two style slots a scrollbar paints with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarStyles {
    /// The empty part of the track — the groove.
    pub track: Style,
    /// The thumb standing for the visible slice of the content.
    pub thumb: Style,
}

impl ScrollbarStyles {
    /// The themed slots, from the dedicated scrollbar roles.
    #[must_use]
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            track: theme.style(ThemeRole::ScrollbarTrack),
            thumb: theme.style(ThemeRole::ScrollbarThumb),
        }
    }
}

/// One axis's scroll extent, in whatever unit that view scrolls by — visual rows,
/// buffer lines, list items or terminal columns.
///
/// The unit only has to be consistent across the three fields. A view with variable
/// row heights therefore measures in items rather than rows, and a soft-wrapped
/// editor measures in buffer lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollExtent {
    /// Total content along the axis.
    pub content: usize,
    /// The first visible unit — the scroll offset.
    pub position: usize,
    /// How much of the content the viewport shows.
    pub viewport: usize,
}

impl ScrollExtent {
    /// An extent from its three parts.
    #[must_use]
    pub const fn new(content: usize, position: usize, viewport: usize) -> Self {
        Self {
            content,
            position,
            viewport,
        }
    }

    /// Whether the content outruns the viewport, i.e. whether a thumb carries any
    /// information.
    ///
    /// A bar for content that fits is suppressed rather than drawn full-length:
    /// ratatui sizes the thumb as `viewport / (content - 1 + viewport)`, which draws
    /// a *half*-length thumb for content that exactly fits.
    #[must_use]
    pub const fn overflows(self) -> bool {
        self.content > self.viewport
    }

    /// The largest valid scroll position — the offset that rests the last unit of
    /// content against the end of the viewport.
    #[must_use]
    pub const fn max_position(self) -> usize {
        self.content.saturating_sub(self.viewport)
    }

    /// `position` clamped into the valid range, so a mapped-back pointer position can
    /// never scroll past the end.
    #[must_use]
    pub const fn clamp_position(self, position: usize) -> usize {
        clamp_usize(position, 0, self.max_position())
    }

    /// The denominator both thumb ends divide by, and the viewport length that goes
    /// with it — ratatui's `max_viewport_position` and `viewport_length`.
    ///
    /// A zero viewport is ratatui's "measure me against the track" signal, so it is
    /// resolved here rather than at each use.
    const fn span(self, track_len: u16) -> (usize, usize) {
        let viewport = if self.viewport == 0 {
            track_len as usize
        } else {
            self.viewport
        };
        (self.max_position().saturating_add(viewport), viewport)
    }

    /// The thumb's `(start, length)` in cells along a track of `track_len` cells.
    ///
    /// This mirrors ratatui's `Scrollbar::part_lengths` under the arguments
    /// [`ScrollBar`] passes it (no begin/end symbols, so the track is the whole rect;
    /// `content_length` = the number of scroll positions; `viewport_content_length` =
    /// the viewport). A render test pins it to the cells ratatui actually paints.
    #[must_use]
    pub const fn thumb(self, track_len: u16) -> (u16, u16) {
        if track_len == 0 {
            return (0, 0);
        }
        let len = track_len as usize;
        let (span, viewport) = self.span(track_len);
        if span == 0 {
            return (0, track_len);
        }
        let thumb = clamp_usize(round_div(viewport.saturating_mul(len), span), 1, len);
        let position = self.clamp_position(self.position);
        let start = clamp_usize(
            round_div(position.saturating_mul(len), span),
            0,
            len - thumb,
        );
        (start as u16, thumb as u16)
    }

    /// Which part of the track the cell `cell` cells into it falls on.
    #[must_use]
    pub const fn hit(self, track_len: u16, cell: u16) -> TrackHit {
        let (start, thumb) = self.thumb(track_len);
        if cell < start {
            TrackHit::Before
        } else if cell < start.saturating_add(thumb) {
            TrackHit::Thumb
        } else {
            TrackHit::After
        }
    }

    /// The position a drag lands on: `origin`, the position when the thumb was
    /// grabbed, moved by the pointer's travel of `cells` along the track.
    ///
    /// A drag is anchored on the grab rather than mapping the pointer's absolute cell
    /// back to a position, because that map is many-to-one: on a 100 000-line file in
    /// a 40-cell track each cell stands for some 2 500 lines, so an absolute inverse
    /// would fling the view up to a thousand lines the instant the button went down,
    /// before the pointer had moved at all. Anchoring makes a press-without-motion
    /// exactly inert.
    ///
    /// The travel is scaled by the thumb's own range of movement — the
    /// `track_len - thumb` cells it can occupy — so dragging the thumb from one end of
    /// the track to the other covers exactly the whole content, no more and no less.
    #[must_use]
    pub const fn position_after_drag(self, track_len: u16, origin: usize, cells: i32) -> usize {
        let (_, thumb) = self.thumb(track_len);
        let travel = track_len.saturating_sub(thumb) as usize;
        if travel == 0 || cells == 0 {
            // A thumb that fills its track has nowhere to go, so the drag is inert
            // rather than snapping the view to an end.
            return self.clamp_position(origin);
        }
        let moved = round_div(
            (cells.unsigned_abs() as usize).saturating_mul(self.max_position()),
            travel,
        );
        if cells < 0 {
            self.clamp_position(origin.saturating_sub(moved))
        } else {
            self.clamp_position(origin.saturating_add(moved))
        }
    }

    /// The position one viewport back — a click on the groove before the thumb.
    #[must_use]
    pub const fn page_back(self) -> usize {
        self.clamp_position(self.position.saturating_sub(self.viewport))
    }

    /// The position one viewport forward — a click on the groove after the thumb.
    #[must_use]
    pub const fn page_forward(self) -> usize {
        self.clamp_position(self.position.saturating_add(self.viewport))
    }

    /// The position `delta` units away — the one-unit-per-notch wheel over a track.
    #[must_use]
    pub const fn step(self, delta: i32) -> usize {
        if delta < 0 {
            self.clamp_position(self.position.saturating_sub(delta.unsigned_abs() as usize))
        } else {
            self.clamp_position(self.position.saturating_add(delta as usize))
        }
    }
}

/// A scrollbar track that was painted, and the extent it was painted from —
/// everything needed to turn a pointer position back into a scroll position.
///
/// A caller records these from a frame and resolves the next frame's mouse events
/// against them, the way every other last-frame hit region works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollTrack {
    rect: Rect,
    axis: ScrollAxis,
    extent: ScrollExtent,
}

impl ScrollTrack {
    /// A track over `rect`, running along `axis`, painted from `extent`.
    #[must_use]
    pub const fn new(rect: Rect, axis: ScrollAxis, extent: ScrollExtent) -> Self {
        Self { rect, axis, extent }
    }

    /// The cells the track occupies, in screen coordinates.
    #[must_use]
    pub const fn rect(self) -> Rect {
        self.rect
    }

    /// Which way the track runs.
    #[must_use]
    pub const fn axis(self) -> ScrollAxis {
        self.axis
    }

    /// The extent the bar was painted from.
    #[must_use]
    pub const fn extent(self) -> ScrollExtent {
        self.extent
    }

    /// The track's length in cells along its own axis.
    #[must_use]
    pub const fn length(self) -> u16 {
        match self.axis {
            ScrollAxis::Vertical => self.rect.height,
            ScrollAxis::Horizontal => self.rect.width,
        }
    }

    /// Whether `(x, y)` lands on the track.
    #[must_use]
    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.rect.x
            && x < self.rect.x + self.rect.width
            && y >= self.rect.y
            && y < self.rect.y + self.rect.height
    }

    /// How many cells along the track `(x, y)` sits, clamped to its ends so a drag
    /// that runs off either end pins the thumb there rather than wrapping.
    #[must_use]
    pub const fn along(self, x: u16, y: u16) -> u16 {
        let (coordinate, origin) = match self.axis {
            ScrollAxis::Vertical => (y, self.rect.y),
            ScrollAxis::Horizontal => (x, self.rect.x),
        };
        let last = self.length().saturating_sub(1);
        if coordinate <= origin {
            0
        } else if coordinate - origin > last {
            last
        } else {
            coordinate - origin
        }
    }

    /// The thumb's `(start, length)` in cells from the track's origin, or `None` when
    /// no thumb is painted — the column stays reserved for content that fits, but
    /// there is nothing on it to grab.
    #[must_use]
    pub const fn thumb_span(self) -> Option<(u16, u16)> {
        if !self.extent.overflows() || self.length() == 0 {
            return None;
        }
        Some(self.extent.thumb(self.length()))
    }

    /// What `(x, y)` hit, or `None` when it is off the track or the track is inert.
    #[must_use]
    pub const fn hit(self, x: u16, y: u16) -> Option<TrackHit> {
        if !self.contains(x, y) || self.thumb_span().is_none() {
            return None;
        }
        Some(self.extent.hit(self.length(), self.along(x, y)))
    }
}

/// Which way a track runs, so a hit test knows which coordinate to measure along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis {
    /// A track down the right-hand column.
    Vertical,
    /// A track along the bottom row.
    Horizontal,
}

/// Where a press on a track landed, relative to the thumb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackHit {
    /// Before the thumb — a page towards the start.
    Before,
    /// On the thumb — the start of a drag.
    Thumb,
    /// After the thumb — a page towards the end.
    After,
}

/// Divide, rounding to nearest.
///
/// This is ratatui's own `rounding_divide`, reproduced because the thumb geometry has
/// to agree with what ratatui paints cell for cell — a pointer that grabs a cell the
/// bar did not draw a thumb into would scroll to somewhere else entirely.
const fn round_div(numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }
    numerator.saturating_add(denominator / 2) / denominator
}

/// `value` clamped to `lo..=hi`, as a `const fn` (`Ord::clamp` is not one).
const fn clamp_usize(value: usize, lo: usize, hi: usize) -> usize {
    if value < lo {
        lo
    } else if value > hi {
        hi
    } else {
        value
    }
}

/// Which axes a view reserves a track for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollAxes {
    /// Reserve the rightmost column for a vertical bar.
    pub vertical: bool,
    /// Reserve the bottom row for a horizontal bar.
    pub horizontal: bool,
}

impl ScrollAxes {
    /// Vertical only — the common case (lists, trees, popups).
    pub const VERTICAL: Self = Self {
        vertical: true,
        horizontal: false,
    };
    /// Horizontal only — a view whose vertical wheel belongs to its container.
    pub const HORIZONTAL: Self = Self {
        vertical: false,
        horizontal: true,
    };
    /// Both axes.
    pub const BOTH: Self = Self {
        vertical: true,
        horizontal: true,
    };
}

/// The tracks reserved out of an area; `None` where the area was too small to
/// spare a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollTracks {
    /// The rightmost column, spanning the content's height.
    pub vertical: Option<Rect>,
    /// The bottom row, spanning the content's width.
    pub horizontal: Option<Rect>,
}

impl ScrollTracks {
    /// Paint both axes. An extent whose axis reserved no track is ignored, so a
    /// caller may pass [`ScrollExtent::default`] for an axis it does not scroll.
    ///
    /// Returns what was painted. A caller that wants the bars to be draggable records
    /// that; one that only wants them drawn ignores it, which is why this carries no
    /// `#[must_use]`.
    pub fn paint(
        self,
        buf: &mut Buffer,
        styles: ScrollbarStyles,
        vertical: ScrollExtent,
        horizontal: ScrollExtent,
    ) -> PaintedTracks {
        if let Some(track) = self.vertical {
            ScrollBar::vertical(vertical, styles).render(track, buf);
        }
        if let Some(track) = self.horizontal {
            ScrollBar::horizontal(horizontal, styles).render(track, buf);
        }
        PaintedTracks {
            vertical: self
                .vertical
                .map(|rect| ScrollTrack::new(rect, ScrollAxis::Vertical, vertical)),
            horizontal: self
                .horizontal
                .map(|rect| ScrollTrack::new(rect, ScrollAxis::Horizontal, horizontal)),
        }
    }
}

/// What a paint pass put on screen: the tracks it reserved, each carrying the extent
/// it was painted from.
///
/// Returned rather than asked for, so a caller gets everything a hit test needs by
/// capturing the return value instead of restating the extents it just passed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintedTracks {
    /// The vertical bar, if one was reserved.
    pub vertical: Option<ScrollTrack>,
    /// The horizontal bar, if one was reserved.
    pub horizontal: Option<ScrollTrack>,
}

/// Split `area` into the content rect and the tracks reserved for `axes`.
///
/// A track is reserved only when the area can both spare the cell and show a bar
/// worth reading; below that the caller keeps the whole area and gets `None`. The
/// corner cell where two tracks would meet belongs to neither.
#[must_use]
pub fn reserve_tracks(area: Rect, axes: ScrollAxes) -> (Rect, ScrollTracks) {
    let vertical = axes.vertical && area.height > 2 && area.width >= 2;
    let horizontal = axes.horizontal && area.width > 2 && area.height >= 2;
    let mut content = area;
    content.width -= u16::from(vertical);
    content.height -= u16::from(horizontal);
    let tracks = ScrollTracks {
        vertical: vertical.then(|| Rect {
            x: area.right() - 1,
            y: area.y,
            width: 1,
            height: content.height,
        }),
        horizontal: horizontal.then(|| Rect {
            x: area.x,
            y: area.bottom() - 1,
            width: content.width,
            height: 1,
        }),
    };
    (content, tracks)
}

/// A one-axis scrollbar, painted into a track reserved by [`reserve_tracks`].
///
/// Rendering is a no-op when the extent does not overflow: the reserved cells are
/// simply left as the frame found them (ratatui clears the buffer between frames,
/// so nothing stale survives).
#[derive(Clone, Debug)]
pub struct ScrollBar {
    orientation: ScrollbarOrientation,
    extent: ScrollExtent,
    styles: ScrollbarStyles,
}

impl ScrollBar {
    /// A vertical bar.
    #[must_use]
    pub const fn vertical(extent: ScrollExtent, styles: ScrollbarStyles) -> Self {
        Self {
            orientation: ScrollbarOrientation::VerticalRight,
            extent,
            styles,
        }
    }

    /// A horizontal bar.
    #[must_use]
    pub const fn horizontal(extent: ScrollExtent, styles: ScrollbarStyles) -> Self {
        Self {
            orientation: ScrollbarOrientation::HorizontalBottom,
            extent,
            styles,
        }
    }
}

impl Widget for ScrollBar {
    fn render(self, track: Rect, buf: &mut Buffer) {
        if !self.extent.overflows() || track.is_empty() {
            return;
        }
        // ratatui's `content_length` counts scroll *positions*, not content units:
        // it places the thumb at `position * track / (content_length - 1 + viewport)`.
        // Handing it the total would leave the thumb short of the bottom at maximum
        // scroll, so pass the number of distinct offsets instead — then offset 0 puts
        // the thumb at the top and the last offset puts it against the bottom.
        let positions = self.extent.content.saturating_sub(self.extent.viewport) + 1;
        let mut state = ScrollbarState::new(positions)
            .position(self.extent.position)
            .viewport_content_length(self.extent.viewport);
        ratatui::widgets::StatefulWidget::render(
            Scrollbar::new(self.orientation)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(self.styles.track)
                .thumb_style(self.styles.thumb),
            track,
            buf,
            &mut state,
        );
    }
}

/// Render a two-axis scrollable paragraph, reserving a track for each axis whose
/// content can exceed the viewport.
pub fn draw_scrollable_lines(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: &mut u16,
    column: &mut u16,
) -> PaintedTracks {
    let (area, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
    let content_height = lines.len();
    let content_width = lines.iter().map(line_width).max().unwrap_or_default();
    clamp_viewport(area, content_height, content_width, scroll, column);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, *column)), area);
    tracks.paint(
        f.buffer_mut(),
        ScrollbarStyles::from_theme(theme),
        ScrollExtent::new(
            content_height,
            usize::from(*scroll),
            usize::from(area.height),
        ),
        ScrollExtent::new(content_width, usize::from(*column), usize::from(area.width)),
    )
}

/// Render content whose vertical wheel is reserved for surrounding navigation,
/// while still exposing horizontal overflow.
pub fn draw_horizontally_scrollable_lines(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    lines: Vec<Line<'static>>,
    column: &mut u16,
) -> PaintedTracks {
    let (area, tracks) = reserve_tracks(area, ScrollAxes::HORIZONTAL);
    let content_width = lines.iter().map(line_width).max().unwrap_or_default();
    let max_column = content_width.saturating_sub(usize::from(area.width));
    *column = (*column).min(u16::try_from(max_column).unwrap_or(u16::MAX));
    f.render_widget(Paragraph::new(lines).scroll((0, *column)), area);
    tracks.paint(
        f.buffer_mut(),
        ScrollbarStyles::from_theme(theme),
        ScrollExtent::default(),
        ScrollExtent::new(content_width, usize::from(*column), usize::from(area.width)),
    )
}

/// The display width of a styled line in terminal cells.
#[must_use]
pub fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(Span::width).sum()
}

/// Clamp both scroll offsets so the viewport never runs past the content.
pub fn clamp_viewport(
    area: Rect,
    content_height: usize,
    content_width: usize,
    scroll: &mut u16,
    column: &mut u16,
) {
    let max_scroll = content_height.saturating_sub(usize::from(area.height));
    let max_column = content_width.saturating_sub(usize::from(area.width));
    *scroll = (*scroll).min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    *column = (*column).min(u16::try_from(max_column).unwrap_or(u16::MAX));
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    /// The thumb glyph ratatui paints, so a test can tell thumb from track.
    const THUMB: &str = "█";

    fn styles() -> ScrollbarStyles {
        ScrollbarStyles {
            track: Style::default().fg(Color::Blue),
            thumb: Style::default().fg(Color::White),
        }
    }

    /// The symbols of a rect's column, top to bottom.
    fn column(buf: &Buffer, x: u16, rect: Rect) -> Vec<String> {
        (rect.y..rect.bottom())
            .map(|y| buf[(x, y)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn a_reserved_track_takes_the_edge_and_leaves_the_corner_to_neither_axis() {
        let area = Rect::new(0, 0, 10, 6);
        let (content, tracks) = reserve_tracks(area, ScrollAxes::BOTH);

        assert_eq!(content, Rect::new(0, 0, 9, 5));
        assert_eq!(tracks.vertical, Some(Rect::new(9, 0, 1, 5)));
        assert_eq!(tracks.horizontal, Some(Rect::new(0, 5, 9, 1)));
        // The corner cell (9, 5) belongs to neither track.
        let vertical = tracks.vertical.unwrap_or_default();
        let horizontal = tracks.horizontal.unwrap_or_default();
        assert!(!vertical.contains((9, 5).into()));
        assert!(!horizontal.contains((9, 5).into()));
    }

    #[test]
    fn an_area_too_small_for_a_bar_keeps_all_of_its_cells() {
        let area = Rect::new(0, 0, 2, 2);
        let (content, tracks) = reserve_tracks(area, ScrollAxes::BOTH);

        assert_eq!(content, area);
        assert_eq!(tracks, ScrollTracks::default());
    }

    #[test]
    fn reservation_ignores_the_content_it_will_measure() {
        // Load-bearing: a content-dependent reservation would feed back through the
        // wrap width (narrower content -> more rows -> overflow -> reserve), and
        // would reflow the text as the reader scrolls.
        let area = Rect::new(0, 0, 20, 10);
        let (content, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        let mut buf = Buffer::empty(area);

        ScrollBar::vertical(ScrollExtent::new(1, 0, 10), styles())
            .render(tracks.vertical.unwrap_or_default(), &mut buf);
        let empty = column(&buf, 19, content);
        ScrollBar::vertical(ScrollExtent::new(10_000, 0, 10), styles())
            .render(tracks.vertical.unwrap_or_default(), &mut buf);

        assert_eq!(reserve_tracks(area, ScrollAxes::VERTICAL).0, content);
        // Same reservation either way; only what is painted into it differs.
        assert!(empty.iter().all(|symbol| symbol != THUMB));
        assert!(column(&buf, 19, content).contains(&THUMB.to_owned()));
    }

    #[test]
    fn a_bar_for_content_that_fits_paints_nothing() {
        // Not merely cosmetic: ratatui would draw a half-length thumb for content
        // that exactly fits, which reads as "you are halfway down" on a full view.
        let area = Rect::new(0, 0, 6, 5);
        let (_, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        let mut buf = Buffer::empty(area);

        ScrollBar::vertical(ScrollExtent::new(5, 0, 5), styles())
            .render(tracks.vertical.unwrap_or_default(), &mut buf);

        assert!(column(&buf, 5, area).iter().all(|symbol| symbol == " "));
    }

    #[test]
    fn the_thumb_sits_at_the_top_at_rest_and_reaches_the_bottom_at_the_end() {
        let area = Rect::new(0, 0, 6, 6);
        let (content, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        let track = tracks.vertical.unwrap_or_default();
        let viewport = usize::from(content.height);
        let extent = |position| ScrollExtent::new(30, position, viewport);

        let mut top = Buffer::empty(area);
        ScrollBar::vertical(extent(0), styles()).render(track, &mut top);
        let mut bottom = Buffer::empty(area);
        ScrollBar::vertical(extent(30 - viewport), styles()).render(track, &mut bottom);

        assert_eq!(
            column(&top, 5, content).first().map(String::as_str),
            Some(THUMB)
        );
        assert_eq!(
            column(&bottom, 5, content).last().map(String::as_str),
            Some(THUMB)
        );
    }

    #[test]
    fn a_horizontal_bar_paints_along_the_reserved_row() {
        let area = Rect::new(0, 0, 10, 4);
        let (content, tracks) = reserve_tracks(area, ScrollAxes::HORIZONTAL);
        let mut buf = Buffer::empty(area);

        tracks.paint(
            &mut buf,
            styles(),
            ScrollExtent::default(),
            ScrollExtent::new(40, 0, usize::from(content.width)),
        );

        let row: String = (0..content.width)
            .map(|x| buf[(x, 3)].symbol().to_owned())
            .collect();
        assert!(row.contains(THUMB), "row was {row:?}");
    }

    /// The `(start, len)` of the run of thumb cells ratatui actually painted.
    fn painted_thumb(extent: ScrollExtent, track_len: u16) -> Option<(u16, u16)> {
        let area = Rect::new(0, 0, 1, track_len);
        let mut buf = Buffer::empty(area);
        ScrollBar::vertical(extent, styles()).render(area, &mut buf);
        let cells: Vec<bool> = (0..track_len)
            .map(|y| buf[(0, y)].symbol() == THUMB)
            .collect();
        let start = cells.iter().position(|&on| on)?;
        let len = cells[start..].iter().take_while(|&&on| on).count();
        Some((
            u16::try_from(start).unwrap_or(u16::MAX),
            u16::try_from(len).unwrap_or(u16::MAX),
        ))
    }

    #[test]
    fn the_computed_thumb_matches_the_cells_ratatui_paints() {
        // The load-bearing test of the whole drag scheme: `thumb` reproduces ratatui's
        // private layout maths so the pointer can be told which cells are grabbable.
        // If a ratatui bump ever changes that rounding, this fails loudly instead of
        // letting a drag scroll to somewhere the thumb is not.
        for track_len in [3_u16, 5, 8, 20, 41] {
            for content in [7_usize, 30, 199, 5_000] {
                for viewport in [1_usize, 3, 19] {
                    let extent = |position| ScrollExtent::new(content, position, viewport);
                    if !extent(0).overflows() {
                        continue;
                    }
                    let last = extent(0).max_position();
                    for position in [0, 1, last / 3, last / 2, last - 1, last] {
                        let extent = extent(position);
                        assert_eq!(
                            painted_thumb(extent, track_len),
                            Some(extent.thumb(track_len)),
                            "extent {extent:?} on a {track_len}-cell track"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_press_that_never_moves_the_pointer_never_moves_the_content() {
        // The reason a drag is anchored on the grab instead of inverting the cell the
        // pointer is over: that map is many-to-one, so an absolute inverse would fling
        // a long file hundreds of lines before the pointer had moved at all.
        for track_len in [3_u16, 8, 40] {
            for content in [30_usize, 5_000, 100_000] {
                let extent = |position| ScrollExtent::new(content, position, 20);
                for position in [0, 1, extent(0).max_position() / 2, extent(0).max_position()] {
                    assert_eq!(
                        extent(position).position_after_drag(track_len, position, 0),
                        position,
                        "a still pointer moved {content} units of content on a \
                         {track_len}-cell track"
                    );
                }
            }
        }
    }

    #[test]
    fn dragging_the_thumb_end_to_end_covers_exactly_the_whole_content() {
        // The off-by-one this is most likely to hit: several positions crowd onto the
        // last cell, so dragging to the bottom has to land on the last offset exactly
        // — a bar you cannot drag to the end of is worse than no bar.
        for (content, viewport, track_len) in
            [(5_000_usize, 40_usize, 40_u16), (97, 9, 17), (30, 6, 6)]
        {
            let extent = ScrollExtent::new(content, 0, viewport);
            let (_, thumb) = extent.thumb(track_len);
            let travel = i32::from(track_len - thumb);

            assert_eq!(
                extent.position_after_drag(track_len, 0, travel),
                extent.max_position(),
                "dragging to the end of a {track_len}-cell track fell short"
            );
            let bottom = ScrollExtent::new(content, extent.max_position(), viewport);
            assert_eq!(
                bottom.position_after_drag(track_len, bottom.position, -travel),
                0
            );
            // Dragging beyond either end pins, never wraps.
            assert_eq!(
                extent.position_after_drag(track_len, 0, travel * 3),
                extent.max_position()
            );
            assert_eq!(
                bottom.position_after_drag(track_len, bottom.position, -travel * 3),
                0
            );
        }
    }

    #[test]
    fn a_wheel_notch_over_a_track_moves_a_single_unit() {
        // The line-by-line workaround: a track cell stands for many lines on a long
        // file, so the wheel is the only gesture that can still step one at a time.
        let extent = ScrollExtent::new(100_000, 500, 40);

        assert_eq!(extent.step(1), 501);
        assert_eq!(extent.step(-1), 499);
        assert_eq!(ScrollExtent::new(100_000, 0, 40).step(-1), 0);
        assert_eq!(
            ScrollExtent::new(100_000, 99_960, 40).step(1),
            99_960,
            "stepping past the end should hold at the end"
        );
    }

    #[test]
    fn a_click_on_the_groove_pages_by_a_viewport_and_stops_at_the_ends() {
        let extent = ScrollExtent::new(200, 50, 20);

        assert_eq!(extent.page_back(), 30);
        assert_eq!(extent.page_forward(), 70);
        assert_eq!(ScrollExtent::new(200, 5, 20).page_back(), 0);
        assert_eq!(ScrollExtent::new(200, 175, 20).page_forward(), 180);
    }

    #[test]
    fn a_track_resolves_a_pointer_against_the_bar_it_painted() {
        let extent = ScrollExtent::new(40, 10, 10);
        let track = ScrollTrack::new(Rect::new(9, 2, 1, 20), ScrollAxis::Vertical, extent);
        let (start, len) = track.thumb_span().unwrap_or_default();

        assert_eq!(track.length(), 20);
        assert!(track.contains(9, 2) && !track.contains(8, 2) && !track.contains(9, 22));
        assert_eq!(track.hit(9, 2 + start - 1), Some(TrackHit::Before));
        assert_eq!(track.hit(9, 2 + start), Some(TrackHit::Thumb));
        assert_eq!(track.hit(9, 2 + start + len), Some(TrackHit::After));
        assert_eq!(track.hit(8, 2 + start), None, "off the track");
        // A drag that runs off the end pins to it rather than wrapping around.
        assert_eq!(track.along(9, 0), 0);
        assert_eq!(track.along(9, 99), 19);
    }

    #[test]
    fn a_track_whose_content_fits_has_nothing_to_grab() {
        // The reserved column stays even when the bar is suppressed, so the hit test
        // runs over cells that show no thumb. They must be inert, not a jump to zero.
        let track = ScrollTrack::new(
            Rect::new(5, 0, 1, 10),
            ScrollAxis::Vertical,
            ScrollExtent::new(4, 0, 10),
        );

        assert_eq!(track.thumb_span(), None);
        assert_eq!(track.hit(5, 3), None);
        assert!(track.contains(5, 3), "still on the track, just inert");
    }

    #[test]
    fn a_horizontal_track_measures_along_its_own_axis() {
        let extent = ScrollExtent::new(100, 20, 10);
        let track = ScrollTrack::new(Rect::new(4, 7, 20, 1), ScrollAxis::Horizontal, extent);

        assert_eq!(track.length(), 20);
        assert_eq!(track.along(4, 7), 0);
        assert_eq!(track.along(10, 7), 6);
        assert!(track.hit(10, 7).is_some());
        assert_eq!(track.hit(10, 8), None, "one row below the track");
    }

    #[test]
    fn a_press_is_classified_against_the_thumb_it_can_see() {
        let extent = ScrollExtent::new(40, 10, 10);
        let track_len = 20;
        let (start, len) = extent.thumb(track_len);
        assert!(
            start > 0 && start + len < track_len,
            "need a thumb in the middle"
        );

        assert_eq!(extent.hit(track_len, start - 1), TrackHit::Before);
        assert_eq!(extent.hit(track_len, start), TrackHit::Thumb);
        assert_eq!(extent.hit(track_len, start + len - 1), TrackHit::Thumb);
        assert_eq!(extent.hit(track_len, start + len), TrackHit::After);
    }

    #[test]
    fn the_geometry_survives_the_degenerate_extents() {
        // A bar is suppressed rather than drawn when the content fits, but the column
        // stays reserved, so the hit test still runs over it — it must not divide by
        // zero or hand back a position the view cannot hold.
        let empty = ScrollExtent::default();
        assert_eq!(empty.thumb(10), (0, 10));
        assert_eq!(empty.position_after_drag(10, 0, 5), 0);
        assert_eq!(empty.max_position(), 0);

        let fits = ScrollExtent::new(4, 0, 10);
        assert_eq!(fits.max_position(), 0);
        assert_eq!(fits.position_after_drag(10, 0, 9), 0);

        assert_eq!(ScrollExtent::new(100, 0, 10).thumb(0), (0, 0));
        assert_eq!(
            ScrollExtent::new(100, 50, 10).position_after_drag(0, 50, 3),
            50
        );

        // A thumb that fills its track has nowhere to travel, so a drag holds still
        // rather than snapping the view to an end.
        let filled = ScrollExtent::new(41, 1, 40);
        assert_eq!(filled.thumb(20), (0, 20));
        assert_eq!(filled.position_after_drag(20, 1, 7), 1);
    }

    #[test]
    fn a_huge_content_does_not_overflow_the_mapping() {
        // `position * track_len` leaves `u32` long before a file this size is absurd.
        let huge = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
        let extent = ScrollExtent::new(huge, huge / 2, 50);
        let (start, len) = extent.thumb(60);

        assert!(start + len <= 60);
        assert!(extent.position_after_drag(60, extent.position, 30) <= extent.max_position());
        assert!(extent.position_after_drag(60, extent.position, -30) <= extent.max_position());
    }

    #[test]
    fn a_painted_panel_reports_the_tracks_it_reserved() {
        // The helper reserves and paints internally, so its caller only learns the
        // track rect if the helper hands it back — that is what lets the app register
        // these bars for the mouse without threading its own state into the widget.
        let area = Rect::new(0, 0, 12, 6);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .expect("test terminal");
        let theme = Theme::dark();
        let lines: Vec<Line<'static>> = (0..40).map(|i| Line::raw(format!("line {i}"))).collect();
        let (mut scroll, mut column) = (3_u16, 0_u16);
        let mut painted = PaintedTracks::default();

        let _ = terminal.draw(|f| {
            painted = draw_scrollable_lines(f, &theme, area, lines, &mut scroll, &mut column);
        });

        let (content, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
        let vertical = painted.vertical.unwrap_or(ScrollTrack::new(
            Rect::default(),
            ScrollAxis::Vertical,
            ScrollExtent::default(),
        ));
        assert_eq!(Some(vertical.rect()), tracks.vertical);
        assert_eq!(
            vertical.extent(),
            ScrollExtent::new(40, 3, usize::from(content.height))
        );
        assert!(vertical.thumb_span().is_some());
        assert_eq!(
            painted.horizontal.map(ScrollTrack::rect),
            tracks.horizontal,
            "both axes are reported, so a caller can register either"
        );
    }

    #[test]
    fn themed_styles_resolve_the_scrollbar_roles() {
        let theme = Theme::dark();
        let resolved = ScrollbarStyles::from_theme(&theme);

        assert_eq!(resolved.track, theme.style(ThemeRole::ScrollbarTrack));
        assert_eq!(resolved.thumb, theme.style(ThemeRole::ScrollbarThumb));
        assert_ne!(resolved.track, resolved.thumb);
    }
}
