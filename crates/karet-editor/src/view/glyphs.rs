//! Structure drawn over the text: indentation guides (with the caret's scope
//! emphasised), visible whitespace, and bracket-pair colourisation.
//!
//! None of it moves a character. A guide or a whitespace mark takes the place
//! of a blank cell that whitespace already occupies, and a bracket keeps its
//! glyph and only changes colour, so caret placement, hit-testing and wrapping
//! are untouched by every option here.

use super::*;
use crate::bracket::BracketDepths;
use crate::bracket::SCAN_LINES;

/// The glyph an indentation guide draws.
const GUIDE: char = '\u{2502}'; // │
/// The glyph a visible space draws.
const SPACE: char = '\u{b7}'; // ·
/// The glyph a visible tab draws in its first cell.
const TAB: char = '\u{2192}'; // →
/// How many lines a blank line looks past, either way, for the indentation
/// of the code around it.
const BLANK_SCAN: u32 = 1_000;

/// Which whitespace characters the editor draws visibly (see
/// [`Editor::render_whitespace`]), after VS Code's `editor.renderWhitespace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderWhitespace {
    /// None: whitespace stays blank.
    #[default]
    None,
    /// Every whitespace character except a single space between two
    /// non-whitespace characters.
    Boundary,
    /// Only whitespace inside a selection.
    Selection,
    /// Only whitespace after the last non-whitespace character of a line.
    Trailing,
    /// Every whitespace character.
    All,
}

impl<'a> Editor<'a> {
    /// Draw a vertical guide at each indentation level (off by default):
    /// every [`tab_width`](Self::tab_width) columns inside a line's leading
    /// whitespace. A blank line takes the deeper indentation of the code
    /// above and below it, so a guide runs unbroken through the blank lines
    /// of a block.
    #[must_use]
    pub fn indent_guides(mut self, enabled: bool) -> Self {
        self.indent_guides = enabled;
        self
    }

    /// Emphasise the indentation guide of the block holding the primary
    /// caret (on by default; it draws only with
    /// [`indent_guides`](Self::indent_guides)). On a line that opens a
    /// deeper block — a function header — that block is the one emphasised.
    /// A read-only editor has no caret and emphasises none.
    #[must_use]
    pub fn active_indent_guide(mut self, enabled: bool) -> Self {
        self.active_indent_guide = enabled;
        self
    }

    /// Choose which whitespace characters are drawn visibly
    /// ([`RenderWhitespace::None`] by default): a space as `·`, a tab as
    /// `→` in its first cell.
    #[must_use]
    pub fn render_whitespace(mut self, mode: RenderWhitespace) -> Self {
        self.render_whitespace = mode;
        self
    }

    /// Colour each code bracket by its nesting depth (off by default),
    /// cycling through [`Theme::bracket_pair_colors`]. A bracket inside a
    /// string or comment is text and keeps its colour, as does a closer
    /// with no opener.
    #[must_use]
    pub fn rainbow_brackets(mut self, enabled: bool) -> Self {
        self.rainbow_brackets = enabled;
        self
    }

    /// Paint the guides blank line `l` runs past its end on the row that
    /// starts at char column `start`, `x` being where its text starts on
    /// screen, into blank cells left of `limit`. A non-blank line has none.
    pub(super) fn paint_blank_guides(
        &self,
        buf: &mut Buffer,
        glyphs: &RowGlyphs,
        (l, start): (u32, u32),
        (x, y): (u16, u16),
        limit: u16,
    ) {
        if glyphs.blank_past(line_len(self.buffer, l)) {
            let origin = display_origin(self.buffer, l, start, self.tab_width);
            glyphs.paint_past_end(buf, (x, y), origin, limit);
        }
    }

    /// Whether any option here changes what a row paints.
    pub(super) fn draws_glyphs(&self) -> bool {
        self.indent_guides
            || self.rainbow_brackets
            || self.render_whitespace != RenderWhitespace::None
    }

    /// The per-frame state the rows' glyphs are drawn from: the caret's
    /// guide and the bracket scan.
    pub(super) fn glyph_frame(&self, theme: &Theme, cursor: Option<LineCol>) -> GlyphFrame<'a> {
        let active = cursor
            .filter(|_| self.indent_guides && self.active_indent_guide && !self.read_only)
            .and_then(|at| active_guide(self.buffer, at.line, self.tab_width));
        GlyphFrame {
            buffer: self.buffer,
            tab_width: self.tab_width,
            guides: self.indent_guides,
            whitespace: self.render_whitespace,
            active,
            brackets: self
                .rainbow_brackets
                .then(|| BracketDepths::new(self.buffer, self.highlights)),
            colors: GlyphColors {
                guide: theme.role(ThemeRole::IndentGuide),
                active: theme.role(ThemeRole::LineNumberActive),
                brackets: theme.bracket_pair_colors(),
            },
        }
    }
}

/// The colours the glyphs draw in.
#[derive(Clone, Copy, Debug)]
struct GlyphColors {
    /// An indentation guide and a visible whitespace mark.
    guide: Rgba,
    /// The indentation guide of the caret's block.
    active: Rgba,
    /// Bracket depths, cycled outermost first.
    brackets: [Rgba; 3],
}

/// The caret's indentation guide: its display column, over a run of lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveGuide {
    column: u32,
    first: u32,
    last: u32,
}

/// What every row of one frame draws its glyphs from.
pub(super) struct GlyphFrame<'a> {
    buffer: &'a TextBuffer,
    tab_width: u16,
    guides: bool,
    whitespace: RenderWhitespace,
    active: Option<ActiveGuide>,
    brackets: Option<BracketDepths<'a>>,
    colors: GlyphColors,
}

impl GlyphFrame<'_> {
    /// The glyphs a row of line `l` draws, `chars` being the prefix of the
    /// line the row walks (up to its right edge): bracket depths are read only
    /// that far, so a row of a minified line costs its window, not the line.
    /// Asked for in ascending line order, as the bracket scan requires.
    pub(super) fn row(&mut self, l: u32, chars: &[char]) -> RowGlyphs {
        let unit = u32::from(self.tab_width.max(1));
        let (leading_end, own) = leading(self.buffer, l, self.tab_width);
        let indent = if self.guides {
            effective_indent(self.buffer, l, self.tab_width)
        } else {
            0
        };
        let trailing_start = if self.whitespace == RenderWhitespace::Trailing {
            trailing_start(self.buffer, l)
        } else {
            0
        };
        let depths = self
            .brackets
            .as_mut()
            .map(|scan| scan.line(l, u32::try_from(chars.len()).unwrap_or(u32::MAX)))
            .unwrap_or_default();
        RowGlyphs {
            unit,
            indent,
            own,
            active: self
                .active
                .filter(|a| a.first <= l && l <= a.last)
                .map(|a| a.column),
            leading_end,
            trailing_start,
            whitespace: self.whitespace,
            depths,
            colors: self.colors,
        }
    }
}

/// The glyphs of one line.
pub(super) struct RowGlyphs {
    /// Display columns between guides.
    unit: u32,
    /// The display width the line's guides run up to (0 draws none).
    indent: u32,
    /// The display width of the line's own leading whitespace.
    own: u32,
    /// The display column of the caret's guide, when this line is in its block.
    active: Option<u32>,
    /// The char column of the line's first non-whitespace character.
    leading_end: u32,
    /// The char column past the line's last non-whitespace character.
    trailing_start: u32,
    whitespace: RenderWhitespace,
    /// Each char's bracket depth (empty when brackets are not coloured).
    depths: Vec<Option<usize>>,
    colors: GlyphColors,
}

impl RowGlyphs {
    /// The guide colour at display column `at`, when a guide stands there.
    fn guide_at(&self, at: u32) -> Option<Rgba> {
        (at < self.indent && at.is_multiple_of(self.unit)).then(|| {
            if self.active == Some(at) {
                self.colors.active
            } else {
                self.colors.guide
            }
        })
    }

    /// Whether the whitespace char at `col` of `chars` is drawn visibly.
    fn shows(&self, chars: &[char], col: u32, selected: bool) -> bool {
        match self.whitespace {
            RenderWhitespace::None => false,
            RenderWhitespace::All => true,
            RenderWhitespace::Selection => selected,
            RenderWhitespace::Trailing => col >= self.trailing_start,
            RenderWhitespace::Boundary => {
                let word = |at: Option<u32>| {
                    at.and_then(|at| chars.get(at as usize))
                        .is_some_and(|c| !c.is_whitespace())
                };
                let lone_space = chars.get(col as usize) == Some(&' ')
                    && word(col.checked_sub(1))
                    && word(col.checked_add(1));
                !lone_space
            },
        }
    }

    /// What the whitespace char at `col` (`width` cells from display column
    /// `display_col`) draws instead of blanks, and in what colour: a guide
    /// on each cell a guide crosses, else the whitespace mark. `None` leaves
    /// it blank.
    pub(super) fn whitespace(
        &self,
        chars: &[char],
        (col, display_col, width): (u32, u32, u32),
        selected: bool,
    ) -> Option<(String, Rgba)> {
        let ch = *chars.get(col as usize)?;
        if ch != ' ' && ch != '\t' {
            return None;
        }
        let shows = self.shows(chars, col, selected);
        if !shows && (col >= self.leading_end || self.indent == 0) {
            return None;
        }
        let mut color = shows.then_some(self.colors.guide);
        let mut text = String::new();
        for offset in 0..width.max(1) {
            let guide = (col < self.leading_end)
                .then(|| self.guide_at(display_col.saturating_add(offset)))
                .flatten();
            if let Some(guide) = guide {
                // The caret's guide wins a tab that also crosses another.
                if color != Some(self.colors.active) {
                    color = Some(guide);
                }
                text.push(GUIDE);
            } else if offset == 0 && shows {
                text.push(if ch == '\t' { TAB } else { SPACE });
            } else {
                text.push(' ');
            }
        }
        Some((text, color?))
    }

    /// The colour of the bracket at `col`, by its depth.
    pub(super) fn bracket(&self, col: u32) -> Option<Rgba> {
        let depth = self.depths.get(col as usize).copied().flatten()?;
        self.colors.brackets.get(depth % 3).copied()
    }

    /// Paint the guides a blank line runs past its own whitespace, into the
    /// cells of row `y` left of `limit` that are still blank. `x` is where
    /// display column `origin` (the row's first) is on screen.
    fn paint_past_end(&self, buf: &mut Buffer, (x, y): (u16, u16), origin: u32, limit: u16) {
        let mut at = self.own.next_multiple_of(self.unit);
        while at < self.indent {
            let screen = at
                .checked_sub(origin)
                .and_then(|offset| u16::try_from(offset).ok())
                .map(|offset| x.saturating_add(offset));
            if let (Some(screen), Some(color)) = (screen, self.guide_at(at))
                && screen < limit
                && let Some(cell) = buf.cell_mut((screen, y))
                && cell.symbol() == " "
            {
                cell.set_char(GUIDE).set_fg(color.to_ratatui());
            }
            at = at.saturating_add(self.unit);
        }
    }

    /// Whether this line is blank (whitespace only), so its guides run past
    /// its end.
    fn blank_past(&self, line_len: u32) -> bool {
        self.leading_end >= line_len && self.indent > self.own
    }
}

/// The char column of line `l`'s first non-whitespace character (its length
/// when it has none), and the display width of the whitespace before it.
fn leading(buffer: &TextBuffer, l: u32, tab_width: u16) -> (u32, u32) {
    let mut cap = 256_u32;
    loop {
        let chars = line_chars_prefix(buffer, l, cap);
        let mut width = 0_u32;
        let mut end = 0_u32;
        for &ch in &chars {
            if ch != ' ' && ch != '\t' {
                return (end, width);
            }
            width = width.saturating_add(character_width(ch, 1, width, tab_width));
            end += 1;
        }
        if chars.len() < cap as usize || cap == u32::MAX {
            return (end, width);
        }
        cap = cap.saturating_mul(2);
    }
}

/// The display column a row of whitespace-only line `l` that starts at char
/// column `start` begins at: the width of the chars scrolled off, and one
/// cell per column scrolled past the line's end.
fn display_origin(buffer: &TextBuffer, l: u32, start: u32, tab_width: u16) -> u32 {
    let chars = line_chars_prefix(buffer, l, start);
    let mut width = 0_u32;
    let mut cols = 0_u32;
    for &ch in chars.iter().take_while(|&&ch| ch == ' ' || ch == '\t') {
        width = width.saturating_add(character_width(ch, 1, width, tab_width));
        cols += 1;
    }
    width.saturating_add(start.saturating_sub(cols))
}

/// The display width of line `l`'s indentation, or `None` for a blank line
/// (or one past the end).
fn indent_of(buffer: &TextBuffer, l: u32, tab_width: u16) -> Option<u32> {
    if l as usize >= buffer.line_count() {
        return None;
    }
    let (end, width) = leading(buffer, l, tab_width);
    (end < line_len(buffer, l)).then_some(width)
}

/// The indentation line `l` draws guides up to: its own, or for a blank
/// line the deeper of the nearest non-blank lines above and below.
fn effective_indent(buffer: &TextBuffer, l: u32, tab_width: u16) -> u32 {
    if let Some(own) = indent_of(buffer, l, tab_width) {
        return own;
    }
    let above = (l.saturating_sub(BLANK_SCAN)..l)
        .rev()
        .find_map(|at| indent_of(buffer, at, tab_width));
    let below = nonblank_below(buffer, l, tab_width).map(|(_, indent)| indent);
    above.unwrap_or(0).max(below.unwrap_or(0))
}

/// The nearest non-blank line below `l`, and its indentation.
fn nonblank_below(buffer: &TextBuffer, l: u32, tab_width: u16) -> Option<(u32, u32)> {
    let last = u32::try_from(buffer.line_count()).unwrap_or(u32::MAX);
    (l.saturating_add(1)..last.min(l.saturating_add(BLANK_SCAN)))
        .find_map(|at| Some((at, indent_of(buffer, at, tab_width)?)))
}

/// The char column past the last non-whitespace character of line `l`.
///
/// Read from the line's end in doubling windows, so a long line costs its
/// trailing whitespace rather than its length.
fn trailing_start(buffer: &TextBuffer, l: u32) -> u32 {
    let (Some(span), len) = (buffer.line_to_byte_range(l as usize), line_len(buffer, l)) else {
        return 0;
    };
    let mut window = 64_u32;
    loop {
        let from = len.saturating_sub(window);
        let start = buffer
            .line_col_to_byte(LineCol::new(l, from))
            .unwrap_or(span.start);
        let tail = buffer
            .slice(karet_core::Span {
                start,
                end: span.end,
            })
            .unwrap_or_default();
        let blank = tail.chars().rev().take_while(|&ch| ch == ' ' || ch == '\t');
        let blank = u32::try_from(blank.count()).unwrap_or(u32::MAX);
        if from == 0 || blank < len - from {
            return len.saturating_sub(blank);
        }
        window = window.saturating_mul(2);
    }
}

/// The guide of the block holding a caret on line `cursor`: the deepest
/// guide left of the line's text, or, where the next non-blank line is
/// deeper (the caret is on a block's header), the guide of that block. The
/// block runs while lines stay right of the guide; blank lines continue it.
fn active_guide(buffer: &TextBuffer, cursor: u32, tab_width: u16) -> Option<ActiveGuide> {
    let unit = u32::from(tab_width.max(1));
    let own = effective_indent(buffer, cursor, tab_width);
    let below = nonblank_below(buffer, cursor, tab_width);
    let header = indent_of(buffer, cursor, tab_width).is_some()
        && below.is_some_and(|(_, indent)| indent > own);
    let (start, target) = match below {
        Some((line, indent)) if header => (line, indent),
        _ => (cursor, own),
    };
    let column = (target.checked_sub(1)? / unit) * unit;
    let inside = |at: u32| indent_of(buffer, at, tab_width).is_none_or(|indent| indent > column);
    let scan = u32::try_from(SCAN_LINES).unwrap_or(u32::MAX);
    let first = (start.saturating_sub(scan)..start)
        .rev()
        .take_while(|&at| inside(at))
        .last()
        .unwrap_or(start);
    let end = u32::try_from(buffer.line_count()).unwrap_or(u32::MAX);
    let last = (start.saturating_add(1)..end.min(start.saturating_add(scan)))
        .take_while(|&at| inside(at))
        .last()
        .unwrap_or(start);
    Some(ActiveGuide {
        column,
        first,
        last,
    })
}
