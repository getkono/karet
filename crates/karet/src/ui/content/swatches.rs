use super::*;

/// Swatch decorations for the color literals on the lines a viewport of
/// `height` rows starting at `first_line` can show (doubled for wrap slack).
pub(super) fn color_swatch_decorations(
    buffer: &TextBuffer,
    first_line: u32,
    height: u16,
) -> Vec<Decoration> {
    let mut out = Vec::new();
    let end = first_line.saturating_add(u32::from(height) * 2);
    for line in first_line..=end {
        let Some(text) = buffer.line(line as usize) else {
            break;
        };
        for (range, rgba) in karet_syntax::color::detect(&text) {
            out.push(Decoration {
                range: karet_core::Range {
                    start: karet_core::LineCol::new(line, u32::try_from(range.start).unwrap_or(0)),
                    end: karet_core::LineCol::new(
                        line,
                        u32::try_from(range.end).unwrap_or(u32::MAX),
                    ),
                },
                kind: karet_core::DecorationKind::ColorSwatch { rgba },
                role: None,
            });
        }
    }
    out
}
