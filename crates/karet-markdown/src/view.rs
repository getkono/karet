//! The optional ratatui renderer (the `view` feature).
//!
//! Turns a [`WrappedDocument`] into ratatui [`Line`]s by resolving each span's
//! [`TokenId`](karet_core::TokenId) through a [`Theme`] — color *and* emphasis, so a
//! heading renders bold and `*emphasis*` renders italic.
//!
//! [`MarkdownView`] paints those lines into a scrollable viewport; callers that want the
//! lines themselves (to lay out a popup, say) use [`to_ratatui`] directly.

use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;

use crate::WrappedDocument;
use crate::WrappedLine;

/// Style one wrapped line with `theme`.
#[must_use]
pub fn line_to_ratatui(line: &WrappedLine, theme: &Theme) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|s| {
            let style = match s.token {
                Some(token) => Style::default()
                    .fg(theme.color(token).to_ratatui())
                    .add_modifier(theme.emphasis(token).to_ratatui()),
                None => Style::default(),
            };
            Span::styled(s.text.clone(), style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Style every line of `doc` with `theme`.
#[must_use]
pub fn to_ratatui(doc: &WrappedDocument, theme: &Theme) -> Vec<Line<'static>> {
    doc.lines
        .iter()
        .map(|line| line_to_ratatui(line, theme))
        .collect()
}

/// The scroll position of a [`MarkdownView`], persisted across frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownViewState {
    /// The first visible wrapped line. Clamped to the document when rendered, so a
    /// caller may drive it freely (e.g. from a synchronized source pane).
    pub scroll: u16,
}

impl MarkdownViewState {
    /// A fresh state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// A scrollable, themed view of a [`WrappedDocument`].
///
/// Wrap the document to the viewport's width before rendering — the widget paints the
/// lines it is given and does not re-wrap them.
#[derive(Clone, Copy, Debug)]
pub struct MarkdownView<'a> {
    doc: &'a WrappedDocument,
    theme: &'a Theme,
}

impl<'a> MarkdownView<'a> {
    /// View `doc`, resolving its span tokens through `theme`.
    #[must_use]
    pub fn new(doc: &'a WrappedDocument, theme: &'a Theme) -> Self {
        Self { doc, theme }
    }
}

impl StatefulWidget for MarkdownView<'_> {
    type State = MarkdownViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Clamp before painting, and write it back: a scroll driven from outside — a
        // synced source pane, or a document that shrank under an edit — must not be able
        // to leave the viewport stranded past the end.
        let last = u16::try_from(self.doc.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
        state.scroll = state.scroll.min(last);

        let rows = self.doc.lines.iter().skip(usize::from(state.scroll));
        for (row, line) in rows.take(usize::from(area.height)).enumerate() {
            let offset = u16::try_from(row).unwrap_or(u16::MAX);
            let styled = line_to_ratatui(line, self.theme);
            buf.set_line(area.x, area.y.saturating_add(offset), &styled, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use karet_core::StandardToken;
    use ratatui::style::Modifier;

    use super::*;

    #[test]
    fn heading_renders_bold_in_the_theme_color() {
        let theme = Theme::dark();
        let doc = crate::parse("# Title\n").wrap(20);
        let lines = to_ratatui(&doc, &theme);
        let Some(first) = lines.first().and_then(|l| l.spans.first()) else {
            return;
        };
        assert!(first.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            first.style.fg,
            Some(theme.color(StandardToken::MarkupHeading.id()).to_ratatui())
        );
    }

    #[test]
    fn emphasis_renders_italic() {
        let theme = Theme::dark();
        let doc = crate::parse("plain *slanted*\n").wrap(40);
        let lines = to_ratatui(&doc, &theme);
        let italic = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
        assert!(italic, "emphasis should reach the terminal as italic");
    }

    #[test]
    fn untokened_text_carries_no_style() {
        let theme = Theme::dark();
        let doc = crate::parse("plain text\n").wrap(40);
        let lines = to_ratatui(&doc, &theme);
        let Some(first) = lines.first().and_then(|l| l.spans.first()) else {
            return;
        };
        assert_eq!(first.style.fg, None);
        assert!(first.style.add_modifier.is_empty());
    }

    /// Render `source` into a `width`x`height` buffer at `scroll`, returning the painted
    /// rows and the (clamped) scroll the widget settled on.
    fn render(source: &str, width: u16, height: u16, scroll: u16) -> (Vec<String>, u16) {
        let theme = Theme::dark();
        let doc = crate::parse(source).wrap(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let mut state = MarkdownViewState { scroll };
        MarkdownView::new(&doc, &theme).render(area, &mut buf, &mut state);
        let rows = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        (rows, state.scroll)
    }

    const DOC: &str = "# Title\n\nalpha\n\nbravo\n\ncharlie\n";

    #[test]
    fn renders_from_the_top_by_default() {
        let (rows, scroll) = render(DOC, 12, 3, 0);
        assert_eq!(scroll, 0);
        assert_eq!(rows, vec!["# Title", "", "alpha"]);
    }

    #[test]
    fn scroll_offsets_the_first_visible_line() {
        let (rows, _) = render(DOC, 12, 3, 2);
        assert_eq!(rows, vec!["alpha", "", "bravo"]);
    }

    #[test]
    fn scrolling_past_the_end_clamps_to_the_last_line() {
        let doc = crate::parse(DOC).wrap(12);
        let last = u16::try_from(doc.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
        let (rows, scroll) = render(DOC, 12, 3, u16::MAX);
        assert_eq!(scroll, last, "the widget writes the clamped scroll back");
        assert_eq!(rows.first().map(String::as_str), Some("charlie"));
    }

    #[test]
    fn a_heading_reaches_the_buffer_bold_and_colored() {
        let theme = Theme::dark();
        let doc = crate::parse("# Title\n").wrap(12);
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        MarkdownView::new(&doc, &theme).render(area, &mut buf, &mut MarkdownViewState::new());
        let cell = &buf[(0, 0)];
        assert_eq!(cell.symbol(), "#");
        assert!(cell.modifier.contains(Modifier::BOLD));
        assert_eq!(
            cell.fg,
            theme.color(StandardToken::MarkupHeading.id()).to_ratatui()
        );
    }

    #[test]
    fn a_degenerate_area_paints_nothing_and_does_not_panic() {
        let theme = Theme::dark();
        let doc = crate::parse(DOC).wrap(12);
        for area in [Rect::new(0, 0, 0, 4), Rect::new(0, 0, 4, 0)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
            let mut state = MarkdownViewState { scroll: 3 };
            MarkdownView::new(&doc, &theme).render(area, &mut buf, &mut state);
            assert_eq!(state.scroll, 3, "a zero area must not touch the scroll");
        }
    }

    #[test]
    fn an_empty_document_renders_nothing() {
        let (rows, scroll) = render("", 8, 2, 5);
        assert_eq!(scroll, 0);
        assert_eq!(rows, vec!["", ""]);
    }
}
