//! Uncommitted-line gutter markers on the VCS worker: a document buffer diffed
//! against its `HEAD` blob, reduced to one gutter decoration per changed run.

use std::path::Path;

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::ThemeRole;
use karet_diff::DiffOptions;
use karet_diff::LineKind;

/// The glyph marking added and modified lines: a left bar, told apart by color.
pub(super) const CHANGED_GLYPH: char = '\u{258e}';
/// The glyph on the line just above a deletion: a bar along its bottom edge.
pub(super) const DELETED_BELOW_GLYPH: char = '\u{2581}';
/// The glyph on the first line when the deletion was the file's opening lines.
pub(super) const DELETED_ABOVE_GLYPH: char = '\u{2594}';

/// The gutter markers for `text` (the buffer at `path`) against its committed
/// `HEAD` content.
///
/// Empty whenever there is no committed side to compare against — a file
/// outside any repository, untracked, unborn, or not UTF-8 at `HEAD` — so an
/// untracked file carries no markers rather than a whole-file "added" bar.
pub(super) fn line_changes(path: &Path, text: &str) -> Vec<Decoration> {
    match super::file_at_rev(path, "HEAD") {
        Ok(Some(head)) => markers(&head, text),
        Ok(None) | Err(_) => Vec::new(),
    }
}

/// The gutter markers turning `head` into `current`.
///
/// Each contiguous changed run becomes one decoration: added lines when the run
/// only inserts, modified lines when it also removes, and a deletion marker on
/// the line above the gap when it only removes. Line endings are normalized
/// first, so a CRLF checkout of an LF blob (or a missing final newline) marks
/// nothing.
pub(super) fn markers(head: &str, current: &str) -> Vec<Decoration> {
    let diff = karet_diff::diff_text(
        &normalized(head),
        &normalized(current),
        &DiffOptions {
            context_lines: 0,
            ..DiffOptions::default()
        },
    );
    diff.hunks
        .iter()
        .filter_map(|hunk| {
            let removes = hunk.lines.iter().any(|line| line.kind == LineKind::Remove);
            let mut added = hunk
                .lines
                .iter()
                .filter(|line| line.kind == LineKind::Add)
                .filter_map(|line| line.new_lineno);
            match added.next() {
                Some(first) => {
                    let last = added.next_back().unwrap_or(first);
                    let role = if removes {
                        ThemeRole::GutterModified
                    } else {
                        ThemeRole::GutterAdded
                    };
                    Some(marker(first - 1, last - 1, CHANGED_GLYPH, role))
                },
                // A pure deletion: `new_start` counts the surviving lines above it.
                None if removes => Some(match hunk.new_start.checked_sub(1) {
                    Some(above) => {
                        marker(above, above, DELETED_BELOW_GLYPH, ThemeRole::GutterDeleted)
                    },
                    None => marker(0, 0, DELETED_ABOVE_GLYPH, ThemeRole::GutterDeleted),
                }),
                None => None,
            }
        })
        .collect()
}

/// `text` with every line `\n`-terminated, so terminator differences never
/// read as changed content.
///
/// Lines are split where the editor's rope splits them (a lone `\r`, VT, FF,
/// NEL, LS and PS included), so line `i` here is row `i` in the editor.
fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    for line in karet_text::lines(text) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// One gutter decoration spanning zero-based lines `first..=last`.
fn marker(first: u32, last: u32, glyph: char, role: ThemeRole) -> Decoration {
    Decoration {
        range: Range {
            start: LineCol::new(first, 0),
            end: LineCol::new(last, 0),
        },
        kind: DecorationKind::GutterMarker { glyph },
        role: Some(role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each decoration as `(first line, last line, glyph, role)`.
    fn spans(head: &str, current: &str) -> Vec<(u32, u32, char, Option<ThemeRole>)> {
        markers(head, current)
            .into_iter()
            .map(|decoration| {
                let glyph = match decoration.kind {
                    DecorationKind::GutterMarker { glyph } => glyph,
                    _ => '?',
                };
                (
                    decoration.range.start.line,
                    decoration.range.end.line,
                    glyph,
                    decoration.role,
                )
            })
            .collect()
    }

    #[test]
    fn unchanged_text_has_no_markers() {
        assert!(spans("a\nb\nc\n", "a\nb\nc\n").is_empty());
    }

    #[test]
    fn inserted_lines_are_marked_added() {
        assert_eq!(
            spans("a\nb\nc\n", "a\nx\ny\nb\nc\n"),
            vec![(1, 2, CHANGED_GLYPH, Some(ThemeRole::GutterAdded))]
        );
    }

    #[test]
    fn rewritten_lines_are_marked_modified() {
        assert_eq!(
            spans("a\nb\nc\n", "a\nB\nc\n"),
            vec![(1, 1, CHANGED_GLYPH, Some(ThemeRole::GutterModified))]
        );
    }

    #[test]
    fn a_rewrite_that_grows_marks_every_new_line_modified() {
        assert_eq!(
            spans("a\nb\nc\n", "a\nB1\nB2\nB3\nc\n"),
            vec![(1, 3, CHANGED_GLYPH, Some(ThemeRole::GutterModified))]
        );
    }

    #[test]
    fn a_deletion_marks_the_line_above_the_gap() {
        assert_eq!(
            spans("a\nb\nc\nd\n", "a\nd\n"),
            vec![(0, 0, DELETED_BELOW_GLYPH, Some(ThemeRole::GutterDeleted))]
        );
    }

    #[test]
    fn deleting_the_opening_lines_marks_the_first_line() {
        assert_eq!(
            spans("a\nb\nc\n", "c\n"),
            vec![(0, 0, DELETED_ABOVE_GLYPH, Some(ThemeRole::GutterDeleted))]
        );
    }

    #[test]
    fn deleting_everything_still_leaves_one_marker() {
        assert_eq!(
            spans("a\nb\n", ""),
            vec![(0, 0, DELETED_ABOVE_GLYPH, Some(ThemeRole::GutterDeleted))]
        );
    }

    #[test]
    fn separate_runs_get_separate_markers() {
        assert_eq!(
            spans("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\nf\n"),
            vec![
                (1, 1, CHANGED_GLYPH, Some(ThemeRole::GutterModified)),
                (5, 5, CHANGED_GLYPH, Some(ThemeRole::GutterAdded)),
            ]
        );
    }

    #[test]
    fn line_ending_differences_are_not_changes() {
        assert!(spans("a\r\nb\r\n", "a\nb").is_empty());
        assert!(spans("a\nb", "a\nb\n").is_empty());
        assert!(spans("a\rb\r", "a\nb\n").is_empty());
        assert!(spans("a\u{2028}b", "a\r\nb").is_empty());
    }

    #[test]
    fn markers_land_on_the_editor_row_past_non_lf_breaks() {
        // A lone `\r` and a U+2028 each start a row in the editor, so the
        // change sits on row 4 of both sides.
        let head = "a\rb\u{2028}c\nd\ne\n";
        let current = "a\rb\u{2028}c\nd\nE\n";
        assert_eq!(
            karet_text::TextBuffer::from_text(current)
                .line(4)
                .as_deref(),
            Some("E")
        );
        assert_eq!(
            spans(head, current),
            vec![(4, 4, CHANGED_GLYPH, Some(ThemeRole::GutterModified))]
        );
        // A change above the breaks stays where it is.
        assert_eq!(
            spans(head, "A\rb\u{2028}c\nd\ne\n"),
            vec![(0, 0, CHANGED_GLYPH, Some(ThemeRole::GutterModified))]
        );
    }

    #[test]
    fn a_new_file_against_an_empty_blob_is_all_added() {
        assert_eq!(
            spans("", "a\nb\n"),
            vec![(0, 1, CHANGED_GLYPH, Some(ThemeRole::GutterAdded))]
        );
    }

    #[test]
    fn a_file_outside_any_repository_has_no_markers() -> Result<(), std::io::Error> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("loose.txt");
        std::fs::write(&path, "a\n")?;
        assert!(line_changes(&path, "a\nb\n").is_empty());
        Ok(())
    }
}
