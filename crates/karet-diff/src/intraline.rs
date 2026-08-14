//! Intra-line highlighting: which runs of a changed line pair actually differ.
//!
//! Tokenizes both lines into alternating whitespace / non-whitespace runs, finds
//! the longest common subsequence of tokens, and marks the rest as changed. This
//! powers the brighter "what changed within this line" emphasis in a diff view.

/// A contiguous run of text from a diff line, tagged as changed or unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Segment {
    /// The run's text.
    pub text: String,
    /// Whether this run differs from the paired line.
    pub changed: bool,
}

/// The result of pairing an old line with a new line for inline highlighting.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HighlightedPair {
    /// Segments of the old line.
    pub old_segments: Vec<Segment>,
    /// Segments of the new line.
    pub new_segments: Vec<Segment>,
}

/// Tokenize both lines, run LCS on the tokens, and emit changed/unchanged segments.
#[must_use]
pub fn compute_highlights(old: &str, new: &str) -> HighlightedPair {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let pairs = lcs_tokens(&old_tokens, &new_tokens);

    let mut old_in_lcs = vec![false; old_tokens.len()];
    let mut new_in_lcs = vec![false; new_tokens.len()];
    for (i, j) in &pairs {
        old_in_lcs[*i] = true;
        new_in_lcs[*j] = true;
    }

    HighlightedPair {
        old_segments: to_segments(&old_tokens, &old_in_lcs),
        new_segments: to_segments(&new_tokens, &new_in_lcs),
    }
}

/// Split `s` into alternating whitespace / non-whitespace runs.
fn tokenize(s: &str) -> Vec<&str> {
    if s.is_empty() {
        return vec![];
    }

    let mut tokens: Vec<&str> = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut in_ws = bytes[0].is_ascii_whitespace();

    for i in 1..bytes.len() {
        let is_ws = bytes[i].is_ascii_whitespace();
        if is_ws != in_ws {
            tokens.push(&s[start..i]);
            start = i;
            in_ws = is_ws;
        }
    }
    tokens.push(&s[start..]);
    tokens
}

/// The largest DP table [`lcs_tokens`] will build, in cells.
///
/// The table is `(m + 1) * (n + 1)` `u32`s, so this caps it at ~4 MB. Ordinary
/// source and prose lines are orders of magnitude below it; minified bundles and
/// generated single-line files are what it exists to stop, since
/// [`crate::PreparedDiff`] runs this eagerly for every paired change row.
const MAX_LCS_CELLS: usize = 1_000_000;

/// Standard DP LCS returning matched `(old_idx, new_idx)` pairs in order.
///
/// Returns no pairs when the table would exceed [`MAX_LCS_CELLS`], which marks
/// every token changed — the line pair degrades to a whole-line replacement
/// instead of exhausting memory.
fn lcs_tokens(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();
    if m.saturating_add(1).saturating_mul(n.saturating_add(1)) > MAX_LCS_CELLS {
        return Vec::new();
    }
    let mut dp = vec![vec![0u32; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if old[i - 1] == new[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut pairs = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] >= dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    pairs
}

/// Concatenate adjacent same-`changed` tokens into [`Segment`]s.
fn to_segments(tokens: &[&str], in_lcs: &[bool]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let changed = !in_lcs[i];
        let mut text = tokens[i].to_string();
        i += 1;
        while i < tokens.len() && in_lcs[i] != changed {
            text.push_str(tokens[i]);
            i += 1;
        }
        segments.push(Segment { text, changed });
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, changed: bool) -> Segment {
        Segment {
            text: text.to_string(),
            changed,
        }
    }

    fn changed_text(segments: &[Segment]) -> String {
        segments
            .iter()
            .filter(|s| s.changed)
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn identical_lines_no_changes() {
        let p = compute_highlights("hello world", "hello world");
        assert!(p.old_segments.iter().all(|s| !s.changed));
        assert!(p.new_segments.iter().all(|s| !s.changed));
    }

    #[test]
    fn single_word_change() {
        let p = compute_highlights("hello world", "hello earth");
        assert_eq!(changed_text(&p.old_segments), "world");
        assert_eq!(changed_text(&p.new_segments), "earth");
    }

    #[test]
    fn empty_sides() {
        let p = compute_highlights("", "hello");
        assert!(p.old_segments.is_empty());
        assert_eq!(p.new_segments, vec![seg("hello", true)]);

        let p = compute_highlights("hello", "");
        assert_eq!(p.old_segments, vec![seg("hello", true)]);
        assert!(p.new_segments.is_empty());
    }

    #[test]
    fn whitespace_only_change() {
        let p = compute_highlights("fn  foo()", "fn foo()");
        assert_eq!(changed_text(&p.old_segments), "  ");
    }

    #[test]
    fn completely_different_lines_share_only_whitespace() {
        let p = compute_highlights("aaa\tbbb", "xxx\tyyy");
        let old_changed = changed_text(&p.old_segments);
        let new_changed = changed_text(&p.new_segments);
        assert!(old_changed.contains("aaa") && old_changed.contains("bbb"));
        assert!(new_changed.contains("xxx") && new_changed.contains("yyy"));
    }

    #[test]
    fn tokenize_preserves_content() {
        let s = "  hello world  ";
        assert_eq!(tokenize(s).concat(), s);
    }

    #[test]
    fn oversized_pairs_degrade_to_whole_line_replacement() {
        // Two minified-style lines: the DP table would be ~1.4 billion cells.
        let old: String = (0..60_000).map(|i| format!("a{i} ")).collect();
        let new: String = (0..60_000).map(|i| format!("b{i} ")).collect();

        let p = compute_highlights(&old, &new);

        // Everything is reported changed, and the text is still complete.
        assert!(p.old_segments.iter().all(|s| s.changed));
        assert!(p.new_segments.iter().all(|s| s.changed));
        let rebuilt: String = p.old_segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(rebuilt, old);
    }

    #[test]
    fn pairs_just_under_the_cap_still_diff_normally() {
        // 500 tokens a side is ~251k cells: well inside the cap, so the shared
        // prefix must still be recognized as unchanged.
        let common: String = (0..499).map(|i| format!("t{i} ")).collect();
        let p = compute_highlights(&format!("{common}old"), &format!("{common}new"));
        assert_eq!(changed_text(&p.old_segments), "old");
        assert_eq!(changed_text(&p.new_segments), "new");
    }
}
