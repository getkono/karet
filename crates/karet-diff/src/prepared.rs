//! A diff prepared for display: the line diff plus per-line syntax token runs and
//! precomputed intra-line change pairs.
//!
//! Preparation is the expensive half of showing a diff — the line diff itself, the
//! per-line LCS for intra-line emphasis, and (for the producer) syntax highlighting
//! of both sides. [`PreparedDiff`] captures all of it as plain data so it can be
//! computed once off any UI thread (and, feature `serde`, cross a serialized
//! backend seam) while painting stays a cheap per-frame assembly step (see the
//! `view` feature).

use karet_core::TokenSpan;

use crate::HighlightedPair;
use crate::compute_highlights;
use crate::model::DiffLine;
use crate::model::FileDiff;
use crate::model::LineKind;

/// A file diff prepared for display: the [`FileDiff`] plus per-line syntax token
/// runs for both sides and precomputed intra-line change pairs.
///
/// The fields are private so the intra-line pairs, which are indexed by the
/// diff's hunks and rows, can only ever be the ones [`new`](Self::new) derived
/// from that diff. Deserializing (feature `serde`) checks the same shape and
/// rejects a payload whose pair table does not match its hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PreparedDiff {
    /// The line diff.
    diff: FileDiff,
    /// Per old-side line (0-based), the syntax token runs. Empty for plaintext.
    old_tokens: Vec<Vec<TokenSpan>>,
    /// Per new-side line (0-based), the syntax token runs. Empty for plaintext.
    new_tokens: Vec<Vec<TokenSpan>>,
    /// Intra-line pairs indexed by hunk and diff-line row. Computing these uses an
    /// LCS, so they are prepared once here rather than on every frame; both the
    /// unified and the side-by-side painters read them from here.
    intraline: Vec<Vec<Option<HighlightedPair>>>,
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

    /// The line diff this was prepared from.
    #[must_use]
    pub fn diff(&self) -> &FileDiff {
        &self.diff
    }

    /// The intra-line change pair for row `row` of hunk `hunk` (both indices
    /// into [`diff`](Self::diff)'s hunks and their lines).
    ///
    /// `None` for a context row, for the unpaired tail of an uneven
    /// remove/add run, and for an index out of range. Both rows of a pair carry
    /// the same pair, computed old → new.
    #[must_use]
    pub fn intraline(&self, hunk: usize, row: usize) -> Option<&HighlightedPair> {
        self.intraline.get(hunk)?.get(row)?.as_ref()
    }

    /// Forget every intra-line pair, so a painter test can show it reads the
    /// prepared pairs rather than recomputing them.
    #[cfg(all(test, feature = "view"))]
    pub(crate) fn drop_intraline_pairs(&mut self) {
        for rows in &mut self.intraline {
            rows.iter_mut().for_each(|pair| *pair = None);
        }
    }

    /// Per old-side line (0-based), the syntax token runs. Empty for plaintext.
    #[must_use]
    pub fn old_tokens(&self) -> &[Vec<TokenSpan>] {
        &self.old_tokens
    }

    /// Per new-side line (0-based), the syntax token runs. Empty for plaintext.
    #[must_use]
    pub fn new_tokens(&self) -> &[Vec<TokenSpan>] {
        &self.new_tokens
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
                        // Saturate: a parsed context line may sit at exactly `u32::MAX`.
                        new_line = line.new_lineno.unwrap_or(new_line).saturating_add(1);
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

/// The wire form of a [`PreparedDiff`], deserialized as-is and then checked.
#[cfg(feature = "serde")]
#[derive(serde::Deserialize)]
struct PreparedDiffWire {
    diff: FileDiff,
    old_tokens: Vec<Vec<TokenSpan>>,
    new_tokens: Vec<Vec<TokenSpan>>,
    intraline: Vec<Vec<Option<HighlightedPair>>>,
}

/// Accepts the payload only when its intra-line table has one row list per
/// hunk and one entry per hunk line, so a pair can never be read against a row
/// it was not computed for. The pairs themselves are trusted rather than
/// recomputed: recomputing them is the LCS work preparation exists to move off
/// the receiving thread.
#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PreparedDiff {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PreparedDiffWire::deserialize(deserializer)?;
        let shaped = wire.intraline.len() == wire.diff.hunks.len()
            && wire
                .intraline
                .iter()
                .zip(&wire.diff.hunks)
                .all(|(rows, hunk)| rows.len() == hunk.lines.len());
        if !shaped {
            return Err(serde::de::Error::custom(
                "intra-line pairs do not match the diff's hunks",
            ));
        }
        Ok(Self {
            diff: wire.diff,
            old_tokens: wire.old_tokens,
            new_tokens: wire.new_tokens,
            intraline: wire.intraline,
        })
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
    use karet_core::TokenId;

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
    fn first_changed_line_does_not_overflow_past_a_context_line_at_u32_max()
    -> Result<(), crate::DiffError> {
        // The parser accepts a context line numbered exactly `u32::MAX` on the new
        // side; a removal after it must not overflow stepping past that line.
        let raw = "diff --git a/f b/f\n--- a/f\n+++ b/f\n\
                   @@ -4294967294,2 +4294967295 @@\n last\n-gone\n";
        let first = crate::parse(raw)?
            .files
            .into_iter()
            .next()
            .map(|file| PreparedDiff::new(file, Vec::new(), Vec::new()).first_changed_line());
        assert_eq!(first, Some(Some(u32::MAX)));
        Ok(())
    }

    #[test]
    fn intraline_pairs_only_paired_change_rows() {
        let p = prepared("a\nold b\nc\n", "a\nnew b\nc\n");
        let rows: Vec<bool> = p.intraline[0].iter().map(Option::is_some).collect();
        // Context rows carry no pair; the remove/add pair both do.
        assert_eq!(rows, [false, true, true, false]);
        // Both rows of a pair hold the same pair, computed old → new.
        assert_eq!(p.intraline[0][1], p.intraline[0][2]);
        assert_eq!(
            p.intraline[0][1],
            Some(compute_highlights("old b", "new b"))
        );
    }

    #[test]
    fn intraline_leaves_the_ragged_tail_of_a_run_unpaired() {
        // Three removals against one addition: only the first of each pairs up.
        let p = prepared("a\nr1\nr2\nr3\nz\n", "a\nn1\nz\n");
        let rows: Vec<bool> = p.intraline[0].iter().map(Option::is_some).collect();
        assert_eq!(rows, [false, true, false, false, true, false]);
        assert_eq!(p.intraline[0][1], Some(compute_highlights("r1", "n1")));

        // And the mirror image: one removal, three additions.
        let p = prepared("a\nr1\nz\n", "a\nn1\nn2\nn3\nz\n");
        let rows: Vec<bool> = p.intraline[0].iter().map(Option::is_some).collect();
        assert_eq!(rows, [false, true, true, false, false, false]);
    }

    #[test]
    fn intraline_pairs_each_removal_with_the_addition_at_the_same_offset() {
        let p = prepared("a\nr1\nr2\nz\n", "a\nn1\nn2\nz\n");
        let rows = &p.intraline[0];
        let kinds: Vec<LineKind> = p.diff.hunks[0].lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Context,
                LineKind::Remove,
                LineKind::Remove,
                LineKind::Add,
                LineKind::Add,
                LineKind::Context,
            ]
        );
        // The second removal pairs with the second addition, and both rows of
        // that pair carry it.
        assert_eq!(rows[1], Some(compute_highlights("r1", "n1")));
        assert_eq!(rows[2], Some(compute_highlights("r2", "n2")));
        assert_eq!(rows[3], Some(compute_highlights("r1", "n1")));
        assert_eq!(rows[4], Some(compute_highlights("r2", "n2")));
    }

    #[test]
    fn intraline_is_indexed_per_hunk() {
        let diff = diff_text(
            "a\nb\nc\nd\ne\nf\ng\nh\n",
            "A\nb\nc\nd\ne\nf\ng\nH\n",
            &DiffOptions {
                context_lines: 1,
                ..Default::default()
            },
        );
        let p = PreparedDiff::new(diff, Vec::new(), Vec::new());
        assert_eq!(p.intraline.len(), p.diff.hunks.len());
        for (rows, hunk) in p.intraline.iter().zip(&p.diff.hunks) {
            assert_eq!(rows.len(), hunk.lines.len());
        }
    }

    /// The accessor reads the pair of the hunk it is asked for, not the first
    /// one, and is total past either index. Falsified by indexing rows across
    /// hunks or panicking out of range.
    #[test]
    fn intraline_accessor_reads_the_named_hunk_and_is_total() {
        let diff = diff_text(
            "a\nb\nc\nd\ne\nf\ng\nh\n",
            "A\nb\nc\nd\ne\nf\ng\nH\n",
            &DiffOptions {
                context_lines: 1,
                ..Default::default()
            },
        );
        let p = PreparedDiff::new(diff, Vec::new(), Vec::new());
        assert_eq!(p.diff().hunks.len(), 2);
        // Each hunk opens with its remove/add pair (no context precedes line 1,
        // and the second hunk's context row follows line 7's removal).
        assert_eq!(p.intraline(0, 0), Some(&compute_highlights("a", "A")));
        let second = &p.diff().hunks[1].lines;
        let removed = second.iter().position(|l| l.kind == LineKind::Remove);
        assert_eq!(
            removed.and_then(|row| p.intraline(1, row)),
            Some(&compute_highlights("h", "H"))
        );
        assert_eq!(p.intraline(0, 2), None, "a context row carries no pair");
        assert_eq!(p.intraline(0, 99), None);
        assert_eq!(p.intraline(9, 0), None);
    }

    #[test]
    fn token_accessors_return_the_tables_as_given() {
        let tok = TokenSpan {
            start: 0,
            end: 1,
            token: TokenId(4),
        };
        let p = PreparedDiff::new(
            diff_text("a\n", "b\n", &DiffOptions::default()),
            vec![vec![tok]],
            Vec::new(),
        );
        assert_eq!(p.old_tokens(), [vec![tok]]);
        assert!(p.new_tokens().is_empty());
    }

    /// A prepared diff survives the serialized seam, and a payload whose pair
    /// table no longer matches its hunks is refused rather than painted against
    /// the wrong rows. Falsified by deriving `Deserialize` without the check.
    #[cfg(feature = "serde")]
    #[test]
    fn deserializing_checks_the_pair_table_against_the_hunks() -> Result<(), serde_json::Error> {
        let p = prepared("a\nold b\nc\n", "a\nnew b\nc\n");
        let json = serde_json::to_value(&p)?;
        assert_eq!(serde_json::from_value::<PreparedDiff>(json.clone())?, p);

        let mut short_row = json.clone();
        if let Some(rows) = short_row["intraline"][0].as_array_mut() {
            rows.pop();
        }
        assert!(serde_json::from_value::<PreparedDiff>(short_row).is_err());

        let mut extra_hunk = json;
        if let Some(hunks) = extra_hunk["intraline"].as_array_mut() {
            hunks.push(serde_json::Value::Array(Vec::new()));
        }
        assert!(serde_json::from_value::<PreparedDiff>(extra_hunk).is_err());
        Ok(())
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
