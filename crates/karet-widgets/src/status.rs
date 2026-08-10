//! The one-row status bar: a flexible left region and a right-aligned strip.
//!
//! The widget owns the bar's layout contract — the right strip takes exactly
//! its content width, the left line gets the rest, and both paint on the bar
//! style so the row reads as one surface. What the segments *say* (key hints,
//! cursor position, language badges) is the consumer's content, assembled
//! against the width this layout leaves it.

use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::scroll::line_width;

/// A one-row, two-region status bar.
pub struct StatusBar<'a> {
    /// The bar surface style (background + default foreground).
    pub bar: Style,
    /// The flexible left content (focus chip, hints, messages).
    pub left: Line<'a>,
    /// The right-aligned fixed strip (cursor, encoding, language).
    pub right: Line<'a>,
}

impl StatusBar<'_> {
    /// The columns the right strip will occupy, so the consumer can assemble
    /// its left content against the remaining width before drawing.
    #[must_use]
    pub fn right_width(&self) -> u16 {
        u16::try_from(line_width(&self.right)).unwrap_or(u16::MAX)
    }

    /// Paint the bar into `area`.
    pub fn draw(self, f: &mut Frame, area: Rect) {
        let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(self.right_width())])
            .split(area);
        f.render_widget(Paragraph::new(self.left).style(self.bar), cols[0]);
        f.render_widget(
            Paragraph::new(self.right)
                .style(self.bar)
                .alignment(Alignment::Right),
            cols[1],
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn right_strip_is_right_aligned_and_left_fills_the_rest() {
        let mut terminal = match Terminal::new(TestBackend::new(20, 1)) {
            Ok(terminal) => terminal,
            Err(_) => return,
        };
        let drawn = terminal.draw(|f| {
            StatusBar {
                bar: Style::default().bg(Color::Blue),
                left: Line::raw("LEFT"),
                right: Line::raw("R1"),
            }
            .draw(f, f.area());
        });
        assert!(drawn.is_ok());
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0u16, 0u16)].symbol(), "L");
        assert_eq!(buffer[(18u16, 0u16)].symbol(), "R");
        assert_eq!(buffer[(19u16, 0u16)].symbol(), "1");
        // The whole row carries the bar surface.
        assert_eq!(buffer[(10u16, 0u16)].bg, Color::Blue);
    }
}
