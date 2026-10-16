//! Code lenses: actionable annotations a producer draws on a row of their own
//! above the line they describe ("3 references", "Run test"), each of which
//! may run a command when clicked.

use super::CommandId;
use super::VirtualLine;
use crate::coord::Range;
use crate::token::ThemeRole;

/// What separates two lenses that share a row.
pub const CODE_LENS_SEPARATOR: &str = " | ";

/// One code lens: a title shown above `range`, and the command a click runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodeLens {
    /// The span the lens describes; it is drawn above the line `range`
    /// starts on.
    pub range: Range,
    /// The text shown, or `None` while the producer has deferred it: a lens
    /// without a title is not drawn until it has been resolved.
    pub title: Option<String>,
    /// The command a click runs, or `None` for a lens that only informs.
    pub command: Option<CommandId>,
    /// The producer's own encoding of the lens, opaque to everyone else.
    ///
    /// Handed back to the producer to resolve the lens or to run its
    /// command; `None` when the producer needs nothing back.
    pub payload: Option<String>,
}

impl CodeLens {
    /// Whether the lens has a title to show.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.title.is_some()
    }

    /// The line the lens is drawn above.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.range.start.line
    }
}

/// Where one lens sits in a [`CodeLensRow`]'s text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeLensSpan {
    /// The first character of its title, counted in characters from the
    /// start of the row's text.
    pub start: usize,
    /// One past its title's last character.
    pub end: usize,
    /// Its index in the slice the row was built from.
    pub lens: usize,
}

/// The lenses drawn above one line: one [`VirtualLine`] holding their titles,
/// and where each title sits in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeLensRow {
    /// The row to paint.
    pub line: VirtualLine,
    /// The titles in it, in order.
    pub spans: Vec<CodeLensSpan>,
}

impl CodeLensRow {
    /// The lens whose title holds character `column` of the row's text; `None`
    /// on the indent or a separator.
    #[must_use]
    pub fn lens_at(&self, column: usize) -> Option<usize> {
        self.spans
            .iter()
            .find(|span| (span.start..span.end).contains(&column))
            .map(|span| span.lens)
    }

    /// Build one row per line that has a resolved lens, lines ascending, each
    /// row's titles in the order `lenses` lists them, joined by
    /// [`CODE_LENS_SEPARATOR`] and indented by `indent(line)` spaces so they
    /// line up with the code they describe.
    ///
    /// A lens with no title is left out: it has nothing to show yet.
    #[must_use]
    pub fn build(lenses: &[CodeLens], indent: impl Fn(u32) -> usize) -> Vec<Self> {
        let mut shown: Vec<(usize, &CodeLens, &str)> = lenses
            .iter()
            .enumerate()
            .filter_map(|(index, lens)| Some((index, lens, lens.title.as_deref()?)))
            .collect();
        // Stable, so lenses on one line keep the producer's order.
        shown.sort_by_key(|(_, lens, _)| lens.line());
        let mut rows: Vec<Self> = Vec::new();
        for (index, lens, title) in shown {
            let line = lens.line();
            match rows.last_mut() {
                Some(row) if row.line.line == line => row.line.text.push_str(CODE_LENS_SEPARATOR),
                _ => rows.push(Self {
                    line: VirtualLine::above(line, " ".repeat(indent(line)), ThemeRole::CodeLens),
                    spans: Vec::new(),
                }),
            }
            let Some(row) = rows.last_mut() else {
                continue;
            };
            let start = row.line.text.chars().count();
            row.line.text.push_str(title);
            let end = start + title.chars().count();
            row.spans.push(CodeLensSpan {
                start,
                end,
                lens: index,
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LineCol;
    use crate::VirtualLineSide;

    fn lens(line: u32, title: Option<&str>) -> CodeLens {
        CodeLens {
            range: Range {
                start: LineCol::new(line, 4),
                end: LineCol::new(line, 9),
            },
            title: title.map(str::to_owned),
            command: None,
            payload: None,
        }
    }

    #[test]
    fn a_lens_is_resolved_once_it_has_a_title_and_sits_on_its_start_line() {
        assert!(!lens(3, None).is_resolved());
        assert!(lens(3, Some("Run")).is_resolved());
        assert_eq!(lens(3, None).line(), 3);
    }

    /// Lenses sharing a line share its row in the producer's order, rows come
    /// out by line whatever order the producer listed them in, and a lens
    /// still waiting for its title takes no room.
    #[test]
    fn lenses_group_into_one_indented_row_per_line() {
        let lenses = [
            lens(7, Some("2 references")),
            lens(2, Some("Run")),
            lens(2, None),
            lens(2, Some("Debug")),
        ];
        let rows = CodeLensRow::build(&lenses, |line| if line == 2 { 4 } else { 0 });
        assert_eq!(rows.len(), 2);
        let first = &rows[0];
        assert_eq!(first.line.line, 2);
        assert_eq!(first.line.side, VirtualLineSide::Above);
        assert_eq!(first.line.role, ThemeRole::CodeLens);
        assert_eq!(first.line.text, "    Run | Debug");
        assert_eq!(
            first.spans,
            [
                CodeLensSpan {
                    start: 4,
                    end: 7,
                    lens: 1
                },
                CodeLensSpan {
                    start: 10,
                    end: 15,
                    lens: 3
                },
            ]
        );
        assert_eq!(rows[1].line.text, "2 references");
        assert_eq!(rows[1].spans[0].lens, 0);
    }

    /// A click lands on the title under it, and nowhere on the indent or a
    /// separator, which belong to no lens.
    #[test]
    fn a_column_finds_the_title_it_falls_in() {
        let lenses = [lens(0, Some("Run")), lens(0, Some("Debug"))];
        let rows = CodeLensRow::build(&lenses, |_| 2);
        let row = &rows[0];
        assert_eq!(row.lens_at(0), None, "the indent");
        assert_eq!(row.lens_at(2), Some(0));
        assert_eq!(row.lens_at(4), Some(0));
        assert_eq!(row.lens_at(5), None, "the separator");
        assert_eq!(row.lens_at(8), Some(1));
        assert_eq!(row.lens_at(13), None, "past the end");
    }

    /// Spans count characters, not bytes, so a title with non-ASCII text
    /// does not push the next title's span off what is painted.
    #[test]
    fn spans_count_characters_not_bytes() {
        let lenses = [lens(0, Some("▶ Run")), lens(0, Some("Debug"))];
        let rows = CodeLensRow::build(&lenses, |_| 0);
        assert_eq!(rows[0].spans[1].start, 8);
        assert_eq!(rows[0].lens_at(8), Some(1));
    }

    #[test]
    fn no_resolved_lens_builds_no_row() {
        assert!(CodeLensRow::build(&[lens(1, None)], |_| 0).is_empty());
    }
}
