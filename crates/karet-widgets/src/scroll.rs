//! Two-axis scrollable text panels with overflow indicators.
//!
//! The shared render path for every read-only lines view (diffs, patches,
//! reports): clamp the offsets to the content, paint the paragraph, and overlay
//! a scrollbar on each axis whose content exceeds the viewport.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Scrollbar;
use ratatui::widgets::ScrollbarOrientation;
use ratatui::widgets::ScrollbarState;

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
