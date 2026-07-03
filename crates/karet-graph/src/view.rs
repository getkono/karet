//! The ratatui rail renderer (behind the `view` feature).
//!
//! [`render_rail`] turns a [`RailRow`] into a [`Line`] whose glyphs are coloured by
//! lane. It takes a `lane_style` closure (index → [`Style`]) instead of depending on a
//! theme crate, so the caller maps lane colours onto whatever palette it uses.

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::RailRow;

/// Render `row`'s glyph gutter as a coloured [`Line`], mapping each glyph's lane colour
/// index through `lane_style`. Contiguous same-style glyphs are coalesced into one
/// [`Span`] so the line stays cheap.
#[must_use]
pub fn render_rail<'a>(row: &RailRow, lane_style: impl Fn(u8) -> Style) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut run = String::new();
    let mut run_color: Option<u8> = None;

    for (ch, &color) in row.gutter.chars().zip(row.colors.iter()) {
        if let Some(prev) = run_color
            && prev != color
        {
            spans.push(Span::styled(std::mem::take(&mut run), lane_style(prev)));
        }
        run_color = Some(color);
        run.push(ch);
    }
    if let Some(color) = run_color {
        spans.push(Span::styled(run, lane_style(color)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;
    use crate::LaneInput;
    use crate::assign_lanes;

    #[test]
    fn renders_a_line_with_the_gutter_glyphs() {
        let rows = assign_lanes(&[
            LaneInput {
                id: "b".into(),
                parents: vec!["a".into()],
                head: true,
            },
            LaneInput::new("a", vec![]),
        ]);
        let line = render_rail(&rows[0], |_| Style::default().fg(Color::Red));
        // The rendered line reproduces the gutter text.
        let text: String = line.spans.iter().flat_map(|s| s.content.chars()).collect();
        assert_eq!(text, rows[0].gutter);
    }

    #[test]
    fn different_lane_colors_split_into_spans() {
        // A merge row has at least two lane colours → at least two spans.
        let rows = assign_lanes(&[
            LaneInput::new("d", vec!["c".into(), "b".into()]),
            LaneInput::new("c", vec!["a".into()]),
            LaneInput::new("b", vec!["a".into()]),
            LaneInput::new("a", vec![]),
        ]);
        let line = render_rail(&rows[0], |c| {
            Style::default().fg(if c == 0 { Color::Red } else { Color::Blue })
        });
        assert!(
            line.spans.len() >= 2,
            "merge row spans multiple lane colours"
        );
    }
}
