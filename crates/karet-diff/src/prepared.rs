//! A diff prepared for display: the line diff plus per-line syntax token runs and
//! precomputed intra-line change pairs.
//!
//! Preparation is the expensive half of showing a diff — the line diff itself, the
//! per-line LCS for intra-line emphasis, and (for the producer) syntax highlighting
//! of both sides. [`PreparedDiff`] captures all of it as plain data so it can be
//! computed once off any UI thread (and, feature `serde`, cross a serialized
//! backend seam) while painting stays a cheap per-frame assembly step (see the
//! `view` feature).

use karet_core::TokenId;

use crate::HighlightedPair;
use crate::compute_highlights;
use crate::model::DiffLine;
use crate::model::FileDiff;
use crate::model::LineKind;

/// A syntax token run within a single line: a byte range and its color class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TokenSpan {
    /// The run's starting byte offset within its line.
    pub start: usize,
    /// The run's ending byte offset (exclusive) within its line.
    pub end: usize,
    /// The run's color class.
    pub token: TokenId,
}

/// A file diff prepared for display: the [`FileDiff`] plus per-line syntax token
/// runs for both sides and precomputed intra-line change pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PreparedDiff {
    /// The line diff.
    pub diff: FileDiff,
    /// Per old-side line (0-based), the syntax token runs. Empty for plaintext.
    pub old_tokens: Vec<Vec<TokenSpan>>,
    /// Per new-side line (0-based), the syntax token runs. Empty for plaintext.
    pub new_tokens: Vec<Vec<TokenSpan>>,
    /// Intra-line pairs indexed by hunk and diff-line row. Computing these uses an
    /// LCS, so they are prepared once here rather than on every frame.
    pub intraline: Vec<Vec<Option<HighlightedPair>>>,
}

impl PreparedDiff {
    /// Wrap `diff` with its token tables, precomputing the intra-line pairs.
    /// Empty token tables mean plaintext (no syntax colors).
    #[must_use]
    pub fn new(
        diff: FileDiff,
        old_tokens: Vec<Vec<TokenSpan>>,
        new_tokens: Vec<Vec<TokenSpan>>,
    ) -> Self {
        let intraline = diff
            .hunks
            .iter()
            .map(|hunk| prepare_intraline(&hunk.lines))
            .collect();
        Self {
            diff,
            old_tokens,
            new_tokens,
            intraline,
        }
    }

    /// Whether the underlying file changed as binary (no textual hunks).
    #[must_use]
    pub fn is_binary(&self) -> bool {
        self.diff.is_binary
    }

    /// The 1-based line, in the file's *new* (current) text, of the first change
    /// in this diff: the first added line's position, or — for a pure removal —
    /// the new-side line the removal collapsed onto. `None` when the diff has no
    /// changed lines (e.g. a binary or unchanged file). Used to land the caret on
    /// the first change when opening the underlying file from a diff view.
    #[must_use]
    pub fn first_changed_line(&self) -> Option<u32> {
        for hunk in &self.diff.hunks {
            // Track the new-side line the walk sits at, so a removal (which has
            // no new-side number of its own) can report where it happened.
            let mut new_line = hunk.new_start;
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Add => return Some(line.new_lineno.unwrap_or(new_line).max(1)),
                    LineKind::Remove => return Some(new_line.max(1)),
                    LineKind::Context => {
                        new_line = line.new_lineno.map_or(new_line + 1, |n| n + 1);
                    },
                }
            }
        }
        None
    }

    /// The count of `(added, removed)` lines across this diff, for per-file
    /// `+N −M` summaries.
    #[must_use]
    pub fn line_stats(&self) -> (usize, usize) {
        line_stats(&self.diff)
    }

    /// The token runs for the side a diff line's content came from: the new side
    /// for additions, the old side otherwise. Empty for plaintext or out-of-range
    /// line numbers.
    #[must_use]
    pub fn tokens_for(
        &self,
        kind: LineKind,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
    ) -> &[TokenSpan] {
        let (lineno, table) = match kind {
            LineKind::Add => (new_lineno, &self.new_tokens),
            _ => (old_lineno, &self.old_tokens),
        };
        lineno
            .and_then(|n| (n as usize).checked_sub(1))
            .and_then(|i| table.get(i))
            .map_or(&[][..], Vec::as_slice)
    }
}

/// The count of `(added, removed)` lines across a [`FileDiff`], for per-file
/// `+N −M` summaries without a full preparation.
#[must_use]
pub fn line_stats(diff: &FileDiff) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            match line.kind {
                LineKind::Add => added += 1,
                LineKind::Remove => removed += 1,
                LineKind::Context => {},
            }
        }
    }
    (added, removed)
}

/// Pair each remove-run with its following add-run and compute the intra-line
/// emphasis for the paired rows.
fn prepare_intraline(lines: &[DiffLine]) -> Vec<Option<HighlightedPair>> {
    let mut prepared = vec![None; lines.len()];
    let mut index = 0;
    while index < lines.len() {
        if lines[index].kind == LineKind::Context {
            index += 1;
            continue;
        }
        let removed = index;
        while index < lines.len() && lines[index].kind == LineKind::Remove {
            index += 1;
        }
        let added = index;
        while index < lines.len() && lines[index].kind == LineKind::Add {
            index += 1;
        }
        let paired = (added - removed).min(index - added);
        for offset in 0..paired {
            let pair = compute_highlights(
                &lines[removed + offset].content,
                &lines[added + offset].content,
            );
            prepared[removed + offset] = Some(pair.clone());
            prepared[added + offset] = Some(pair);
        }
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffOptions;
    use crate::diff_text;

    fn prepared(old: &str, new: &str) -> PreparedDiff {
        PreparedDiff::new(
            diff_text(old, new, &DiffOptions::default()),
            Vec::new(),
            Vec::new(),
        )
    }

    #[test]
    fn line_stats_count_adds_and_removes() {
        let p = prepared("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(p.line_stats(), (2, 1));
    }

    #[test]
    fn first_changed_line_points_at_the_first_addition() {
        // Lines 1-2 are context; line 3 changes ("c" → "x").
        let p = prepared("a\nb\nc\nd\n", "a\nb\nx\nd\n");
        assert_eq!(p.first_changed_line(), Some(3));
    }

    #[test]
    fn first_changed_line_for_a_pure_removal_lands_where_it_collapsed() {
        // "b" (old line 2) is removed with nothing added: the new side collapses
        // onto line 2 ("c"), which is where the caret should land.
        let p = prepared("a\nb\nc\n", "a\nc\n");
        assert_eq!(p.first_changed_line(), Some(2));
    }

    #[test]
    fn first_changed_line_is_none_when_nothing_changed() {
        assert_eq!(prepared("same\n", "same\n").first_changed_line(), None);
    }

    #[test]
    fn first_changed_line_clamps_an_emptied_file_to_line_one() {
        // Deleting every line leaves the new side empty (new_start 0): the caret
        // target still clamps to a valid 1-based line.
        assert_eq!(prepared("a\nb\n", "").first_changed_line(), Some(1));
    }

    #[test]
    fn intraline_pairs_only_paired_change_rows() {
        let p = prepared("a\nold b\nc\n", "a\nnew b\nc\n");
        let rows: Vec<bool> = p.intraline[0].iter().map(Option::is_some).collect();
        // Context rows carry no pair; the remove/add pair both do.
        assert!(rows.iter().filter(|set| **set).count() == 2);
    }

    #[test]
    fn tokens_for_selects_the_matching_side() {
        let diff = diff_text("old\n", "new\n", &DiffOptions::default());
        let tok = |token: u16| TokenSpan {
            start: 0,
            end: 3,
            token: TokenId(token),
        };
        let p = PreparedDiff::new(diff, vec![vec![tok(1)]], vec![vec![tok(2)]]);
        assert_eq!(p.tokens_for(LineKind::Remove, Some(1), None), &[tok(1)]);
        assert_eq!(p.tokens_for(LineKind::Add, None, Some(1)), &[tok(2)]);
        assert!(p.tokens_for(LineKind::Add, None, Some(9)).is_empty());
    }
}
