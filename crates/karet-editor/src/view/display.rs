//! The editor's display options — gutter numbering, the cursor-line
//! highlight, rulers and the scroll margin — and the gutter and ruler
//! geometry they drive.

use super::*;

impl<'a> Editor<'a> {
    /// Choose how the gutter numbers lines; [`LineNumbers::Off`] narrows the
    /// gutter to its marker column. Hit-testing follows the gutter this sets.
    #[must_use]
    pub fn line_numbers(mut self, numbers: LineNumbers) -> Self {
        self.line_numbers = numbers;
        self
    }

    /// Highlight the background of every caret's line (on by default). The
    /// caret lines' numbers stay emphasized either way.
    #[must_use]
    pub fn cursor_line(mut self, highlight: bool) -> Self {
        self.cursor_line = highlight;
        self
    }

    /// Draw a vertical ruler after each of these display columns (none by
    /// default), as a tint on the cells it crosses.
    #[must_use]
    pub fn rulers(mut self, columns: &'a [u16]) -> Self {
        self.rulers = columns;
        self
    }

    /// Keep at least `lines` rows between the caret and the top or bottom
    /// edge when a keyboard move reveals it (0, the default, keeps none).
    ///
    /// Capped at the rows the viewport can spare, and never scrolling past
    /// either end of the buffer. Pointer-originated moves keep no margin, so
    /// a click never scrolls the text under it.
    #[must_use]
    pub fn scroll_off(mut self, lines: u16) -> Self {
        self.scroll_off = lines;
        self
    }

    /// Draw a diagnostic's underline curly (SGR `4:3`) rather than straight
    /// (off by default).
    ///
    /// Turn it on only for a terminal known to draw a curly underline: one
    /// that does not may read `4:3` as two attributes and paint the text
    /// italic. The underline color and every other style are the same either
    /// way, and a navigable-symbol underline stays straight.
    #[must_use]
    pub fn undercurl(mut self, curly: bool) -> Self {
        self.undercurl = curly;
        self
    }
}

/// How the gutter numbers lines (see [`Editor::line_numbers`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineNumbers {
    /// Each line shows its own 1-based number.
    #[default]
    Absolute,
    /// Each line shows its distance in buffer lines from the primary caret's
    /// line, which shows its own absolute number.
    Relative,
    /// No numbers: the gutter keeps only its marker column.
    Off,
}

/// The gutter's width in cells for a buffer of `line_count` lines: the marker
/// column, the right-aligned numbers when they are shown, and the fold column
/// (see [`GutterLane`](crate::GutterLane)).
pub(crate) fn gutter_width(line_count: u32, numbers: LineNumbers) -> u16 {
    let digits = match numbers {
        LineNumbers::Off => 0,
        LineNumbers::Absolute | LineNumbers::Relative => {
            u16::try_from(digit_count(line_count.max(1))).unwrap_or(u16::MAX)
        },
    };
    digits.saturating_add(2)
}

/// The number column of a row: `line` is the row's buffer line, or `None` on
/// a soft-wrap continuation row, which shows no number; `caret` is the
/// primary caret's line.
fn number_label(numbers: LineNumbers, line: Option<u32>, caret: u32, digits: usize) -> String {
    match (numbers, line) {
        (LineNumbers::Off, _) => String::new(),
        (_, None) => " ".repeat(digits),
        (LineNumbers::Relative, Some(l)) if l != caret => {
            format!("{:>digits$}", l.abs_diff(caret))
        },
        (LineNumbers::Absolute | LineNumbers::Relative, Some(l)) => {
            format!("{:>digits$}", l + 1)
        },
    }
}

/// Record where a collapsed fold header's ` ⋯` lands on row `y` of `area`:
/// right after the cells `spans` already fill, clipped to the area, so a
/// click on it resolves against what this frame painted.
pub(super) fn record_fold_ellipsis(
    out: &mut Vec<(Rect, u32)>,
    area: Rect,
    y: u16,
    spans: &[Span<'_>],
    line: u32,
) {
    let used = spans
        .iter()
        .fold(0_usize, |used, span| used.saturating_add(span.width()));
    let Ok(used) = u16::try_from(used) else {
        return;
    };
    let width = area.width.saturating_sub(used).min(2);
    if width > 0 {
        out.push((Rect::new(area.x.saturating_add(used), y, width, 1), line));
    }
}

impl Editor<'_> {
    /// The gutter marker glyph + color for line `l`, if a decoration places one.
    fn gutter_marker(&self, l: u32, theme: &Theme, default_fg: Rgba) -> Option<(char, Rgba)> {
        for d in self.decorations {
            if let DecorationKind::GutterMarker { glyph } = &d.kind
                && line_in_range(l, d.range)
            {
                let color = d.role.map_or(default_fg, |r| theme.role(r));
                return Some((*glyph, color));
            }
        }
        None
    }

    /// A row's three gutter spans, one per [`GutterLane`](crate::GutterLane):
    /// the marker column, the number column, then the fold column.
    ///
    /// `line` is the row's buffer line, or `None` on a soft-wrap continuation
    /// row, which shows none of them. The marker column shows the line's
    /// decoration marker (breakpoint/diagnostic/git); a fold header shows its
    /// collapse/expand chevron in the fold column, so neither hides the
    /// other. `caret` is the primary caret's line, which relative numbers
    /// count from.
    pub(super) fn gutter_spans(
        &self,
        line: Option<u32>,
        caret: u32,
        digits: usize,
        theme: &Theme,
        (default_fg, number_fg): (Rgba, Rgba),
    ) -> Vec<Span<'static>> {
        let (marker, marker_fg) = line
            .and_then(|l| self.gutter_marker(l, theme, default_fg))
            .unwrap_or((' ', default_fg));
        let chevron = match line.and_then(|l| self.fold_at(l)) {
            Some(fold) if fold.collapsed => '\u{25b8}',
            Some(_) => '\u{25be}',
            None => ' ',
        };
        vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(marker_fg.to_ratatui()),
            ),
            Span::styled(
                number_label(self.line_numbers, line, caret, digits),
                Style::default().fg(number_fg.to_ratatui()),
            ),
            Span::styled(
                chevron.to_string(),
                Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui()),
            ),
        ]
    }

    /// Tint each ruler column of the content area `content` in `color`.
    ///
    /// A ruler at column `c` sits after `c` display cells of an unscrolled
    /// line, the cell VS Code draws it on; horizontal scroll moves it left by
    /// [`EditorState::scroll_col`]. Only cells still on the editor's own
    /// `plain` backgrounds (the base and the cursor line) are tinted, so a
    /// selection, search match or swatch the ruler crosses keeps its colour.
    pub(super) fn paint_rulers(
        &self,
        buf: &mut Buffer,
        content: Rect,
        scroll_col: u32,
        plain: [Rgba; 2],
        color: Rgba,
    ) {
        let plain = plain.map(Rgba::to_ratatui);
        for &column in self.rulers {
            let Some(offset) = u32::from(column).checked_sub(scroll_col) else {
                continue;
            };
            let Ok(offset) = u16::try_from(offset) else {
                continue;
            };
            if offset >= content.width {
                continue;
            }
            let x = content.x.saturating_add(offset);
            for y in content.top()..content.bottom() {
                if let Some(cell) = buf.cell_mut((x, y))
                    && plain.contains(&cell.bg)
                {
                    cell.set_bg(color.to_ratatui());
                }
            }
        }
    }
}
