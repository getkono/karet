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
    pub fn paint(
        self,
        buf: &mut Buffer,
        styles: ScrollbarStyles,
        vertical: ScrollExtent,
        horizontal: ScrollExtent,
    ) {
        if let Some(track) = self.vertical {
            ScrollBar::vertical(vertical, styles).render(track, buf);
        }
        if let Some(track) = self.horizontal {
            ScrollBar::horizontal(horizontal, styles).render(track, buf);
        }
    }
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

/// Render a two-axis scrollable paragraph and overlay indicators for axes whose
/// content exceeds the viewport.
pub fn draw_scrollable_lines(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: &mut u16,
    column: &mut u16,
) {
    let content_height = lines.len();
    let content_width = lines.iter().map(line_width).max().unwrap_or_default();
    clamp_viewport(area, content_height, content_width, scroll, column);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, *column)), area);
    draw_scroll_indicators(
        f,
        theme,
        area,
        content_height,
        content_width,
        *scroll,
        *column,
    );
}

/// Render content whose vertical wheel is reserved for surrounding navigation,
/// while still exposing horizontal overflow.
pub fn draw_horizontally_scrollable_lines(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    lines: Vec<Line<'static>>,
    column: &mut u16,
) {
    let content_width = lines.iter().map(line_width).max().unwrap_or_default();
    let max_column = content_width.saturating_sub(usize::from(area.width));
    *column = (*column).min(u16::try_from(max_column).unwrap_or(u16::MAX));
    f.render_widget(Paragraph::new(lines).scroll((0, *column)), area);
    draw_scroll_indicators(
        f,
        theme,
        area,
        usize::from(area.height),
        content_width,
        0,
        *column,
    );
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

/// Overlay a scrollbar on each axis whose content exceeds the viewport.
#[allow(clippy::too_many_arguments)] // content extents and both offsets are independent render inputs
pub fn draw_scroll_indicators(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    content_height: usize,
    content_width: usize,
    scroll: u16,
    column: u16,
) {
    let track = theme.style(ThemeRole::IndentGuide);
    let thumb = theme.style(ThemeRole::Foreground);
    if content_height > usize::from(area.height) && area.height > 2 {
        let mut state = ScrollbarState::new(content_height)
            .position(usize::from(scroll))
            .viewport_content_length(usize::from(area.height));
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(track)
                .thumb_style(thumb),
            area,
            &mut state,
        );
    }
    if content_width > usize::from(area.width) && area.width > 2 {
        let mut state = ScrollbarState::new(content_width)
            .position(usize::from(column))
            .viewport_content_length(usize::from(area.width));
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(track)
                .thumb_style(thumb),
            area,
            &mut state,
        );
    }
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

    #[test]
    fn themed_styles_resolve_the_scrollbar_roles() {
        let theme = Theme::dark();
        let resolved = ScrollbarStyles::from_theme(&theme);

        assert_eq!(resolved.track, theme.style(ThemeRole::ScrollbarTrack));
        assert_eq!(resolved.thumb, theme.style(ThemeRole::ScrollbarThumb));
        assert_ne!(resolved.track, resolved.thumb);
    }
}
