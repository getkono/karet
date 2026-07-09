//! The optional ratatui renderer (the `view` feature).
//!
//! Turns a [`WrappedDocument`] into ratatui [`Line`]s by resolving each span's
//! [`TokenId`](karet_core::TokenId) through a [`Theme`] — color *and* emphasis, so a
//! heading renders bold and `*emphasis*` renders italic.

use karet_theme::Theme;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

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
}
