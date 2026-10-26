//! Virtual text: cells a renderer paints that are not in the buffer, either on
//! a buffer line ([`VirtualText`]) or on a row of their own ([`VirtualLine`]).

use crate::token::ThemeRole;

/// Which edge of the end-of-line slot a [`VirtualTextPlacement::EndOfLine`]
/// text keeps to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EolAlign {
    /// Immediately after the line's content (and any left-aligned text before
    /// it).
    #[default]
    Left,
    /// Flush against the viewport's right edge, when the row leaves room for
    /// it; otherwise immediately after the left-aligned text.
    Right,
}

/// Where a [`VirtualText`] sits on its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum VirtualTextPlacement {
    /// Between characters: immediately before the character at `col`, after
    /// that column's caret slot, so a caret at `col` sits against the text it
    /// edits rather than past the annotation. A `col` at or past the line's
    /// end trails its last character.
    ///
    /// It is part of the line's layout: it takes cells when the line wraps,
    /// shifts every caret and click after it, and a horizontal reveal scrolls
    /// as far as it is drawn. A click on it resolves to `col`.
    Inline {
        /// The buffer column (in `char`s) it renders before.
        col: u32,
    },
    /// The end-of-line slot: the cells a line's last row leaves free after
    /// its content and trailing inline text. It never wraps, never shifts the
    /// text, and no caret enters it (a click on it resolves to the end of the
    /// line). When the slot is too narrow for everything on the line, higher
    /// `priority` keeps its cells first, and a text cut short ends in `…`.
    ///
    /// The slot begins at the end-of-line caret cell: a text that should
    /// stand apart from the code carries its own leading gap.
    EndOfLine {
        /// Which edge of the slot it keeps to.
        align: EolAlign,
        /// Truncation priority: higher is cut last. Equal priorities are cut
        /// last-supplied first.
        priority: u8,
    },
}

/// Presentation-neutral virtual text: cells a renderer paints on a line that
/// are not in the buffer (inlay hints, inline blame, dependency versions,
/// inline diagnostics, ghost text).
///
/// One model for every producer, so a renderer has one path that wrapping,
/// caret placement, hit-testing and horizontal scrolling all account for.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VirtualText {
    /// The line it is on.
    pub line: u32,
    /// Where on the line.
    pub placement: VirtualTextPlacement,
    /// The text to paint, on one row (a renderer does not break it).
    pub text: String,
    /// The theme role supplying its color.
    pub role: ThemeRole,
}

impl VirtualText {
    /// Text anchored before the character at `col` on `line`
    /// ([`VirtualTextPlacement::Inline`]).
    #[must_use]
    pub fn inline(line: u32, col: u32, text: impl Into<String>, role: ThemeRole) -> Self {
        Self {
            line,
            placement: VirtualTextPlacement::Inline { col },
            text: text.into(),
            role,
        }
    }

    /// Left-aligned text in `line`'s end-of-line slot at `priority`
    /// ([`VirtualTextPlacement::EndOfLine`]).
    #[must_use]
    pub fn end_of_line(line: u32, text: impl Into<String>, role: ThemeRole, priority: u8) -> Self {
        Self {
            line,
            placement: VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority,
            },
            text: text.into(),
            role,
        }
    }
}

/// Which side of its buffer line a [`VirtualLine`] sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum VirtualLineSide {
    /// On its own row immediately above the line's first row (a code lens,
    /// say).
    Above,
    /// On its own row immediately below the line's last row (a multi-line
    /// inline diagnostic, an inline diff hunk, ghost text continuing a
    /// completion).
    Below,
}

/// A whole row of presentation-neutral text between buffer lines: cells a
/// renderer paints on a row of their own that no buffer line owns.
///
/// It belongs to the buffer line it is anchored to: it scrolls with it, is
/// hidden with it when a collapsed fold hides that line, and a click on it is
/// routed to it. Several on one side of one line stack in supply order, so a
/// multi-line block is one `VirtualLine` per row.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VirtualLine {
    /// The buffer line it is anchored to.
    pub line: u32,
    /// Which side of that line.
    pub side: VirtualLineSide,
    /// The text to paint, on one row (a renderer does not wrap it).
    pub text: String,
    /// The theme role supplying its color.
    pub role: ThemeRole,
}

impl VirtualLine {
    /// A row above `line` ([`VirtualLineSide::Above`]).
    #[must_use]
    pub fn above(line: u32, text: impl Into<String>, role: ThemeRole) -> Self {
        Self {
            line,
            side: VirtualLineSide::Above,
            text: text.into(),
            role,
        }
    }

    /// A row below `line` ([`VirtualLineSide::Below`]).
    #[must_use]
    pub fn below(line: u32, text: impl Into<String>, role: ThemeRole) -> Self {
        Self {
            line,
            side: VirtualLineSide::Below,
            text: text.into(),
            role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_of_line_text_defaults_to_the_left_edge_of_the_slot() {
        let text = VirtualText::end_of_line(4, "  Ada", ThemeRole::Muted, 3);
        assert_eq!(text.line, 4);
        assert_eq!(
            text.placement,
            VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority: 3,
            }
        );
        assert_eq!(EolAlign::default(), EolAlign::Left);
    }

    #[test]
    fn virtual_lines_are_built_on_the_side_their_constructor_names() {
        let above = VirtualLine::above(2, "3 references", ThemeRole::Muted);
        let below = VirtualLine::below(2, "  ^ expected i32", ThemeRole::DiagnosticError);
        assert_eq!((above.line, above.side), (2, VirtualLineSide::Above));
        assert_eq!((below.line, below.side), (2, VirtualLineSide::Below));
        assert_eq!(below.text, "  ^ expected i32");
        assert_eq!(below.role, ThemeRole::DiagnosticError);
    }
}
