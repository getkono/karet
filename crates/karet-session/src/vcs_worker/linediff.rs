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

/// One change-lane decoration spanning zero-based lines `first..=last`.
fn marker(first: u32, last: u32, glyph: char, role: ThemeRole) -> Decoration {
    Decoration::new(
        Range {
            start: LineCol::new(first, 0),
            end: LineCol::new(last, 0),
        },
        DecorationKind::ChangeMarker { glyph },
        Some(role),
    )
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
                    DecorationKind::ChangeMarker { glyph } => glyph,
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

    /// Seeded `(head, current)` pairs: short files over a tiny vocabulary (so
    /// lines repeat and the diff has ambiguity to resolve), the current side an
    /// edited copy of the head.
    fn adversarial_pairs() -> Vec<(Vec<&'static str>, Vec<&'static str>)> {
        const WORDS: &[&str] = &["a", "b", "c", "", "a"];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut roll = move |bound: usize| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap_or_default() % bound
        };
        (0..300)
            .map(|_| {
                let head: Vec<&str> = (0..roll(8)).map(|_| WORDS[roll(WORDS.len())]).collect();
                let mut current = head.clone();
                for _ in 0..roll(4) {
                    let at = roll(current.len() + 1);
                    match roll(3) {
                        0 => current.insert(at, WORDS[roll(WORDS.len())]),
                        1 if at < current.len() => {
                            current.remove(at);
                        },
                        _ if at < current.len() => current[at] = "x",
                        _ => current.push("y"),
                    }
                }
                (head, current)
            })
            .collect()
    }

    #[test]
    fn line_terminators_never_change_the_markers() {
        for (head, current) in adversarial_pairs() {
            let lf = |lines: &[&str]| {
                lines
                    .iter()
                    .map(|line| format!("{line}\n"))
                    .collect::<String>()
            };
            let crlf = |lines: &[&str]| {
                lines
                    .iter()
                    .map(|line| format!("{line}\r\n"))
                    .collect::<String>()
            };
            let expected = spans(&lf(&head), &lf(&current));
            for (old, new) in [
                (crlf(&head), lf(&current)),
                (lf(&head), crlf(&current)),
                (crlf(&head), crlf(&current)),
                (lf(&head).trim_end_matches('\n').to_owned(), lf(&current)),
                (lf(&head), lf(&current).trim_end_matches('\n').to_owned()),
            ] {
                // Trimming every final newline also drops trailing empty lines,
                // which is a real content change; only compare where it is not.
                if karet_text::lines(&old).count() != head.len()
                    || karet_text::lines(&new).count() != current.len()
                {
                    continue;
                }
                assert_eq!(spans(&old, &new), expected, "{old:?} -> {new:?}");
            }
        }
    }

    #[test]
    fn markers_are_ordered_disjoint_and_inside_the_current_text() {
        for (head, current) in adversarial_pairs() {
            let text = |lines: &[&str]| {
                lines
                    .iter()
                    .map(|line| format!("{line}\n"))
                    .collect::<String>()
            };
            let got = spans(&text(&head), &text(&current));
            assert_eq!(
                got.is_empty(),
                head == current,
                "{head:?} -> {current:?}: {got:?}"
            );
            let rows = u32::try_from(current.len().max(1)).unwrap_or(u32::MAX);
            let mut previous: Option<u32> = None;
            for &(first, last, _, _) in &got {
                assert!(
                    first <= last && last < rows,
                    "{head:?} -> {current:?}: {got:?}"
                );
                // A deletion marker can share the row a modification above it
                // ends on, but no two markers ever start out of order.
                assert!(
                    previous.is_none_or(|end| first >= end),
                    "{head:?} -> {current:?}: {got:?}"
                );
                previous = Some(last);
            }
        }
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
