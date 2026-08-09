//! `karet-fuzzy` — fuzzy matching and ranking for the karet toolkit.
//!
//! Standalone (depends on no other karet crate). Wraps `nucleo`'s subsequence
//! matching with smart case, shared by the widgets toolkit and completion
//! ranking so neither has to depend on the other.

/// A ranked item: a reference to the source item, its score, and the indices of
/// the matched characters (for highlighting).
#[derive(Clone, Debug)]
pub struct Scored<'a, T> {
    /// The matched item.
    pub item: &'a T,
    /// Higher is a better match.
    pub score: u32,
    /// Character indices within the item's string that matched.
    pub matched: Vec<u32>,
}

/// A fuzzy matcher over arbitrary string-like items.
///
/// Holds nucleo's reusable match state, so keep one around and feed it every
/// query rather than constructing one per keystroke.
pub struct Matcher {
    inner: nucleo::Matcher,
}

impl Default for Matcher {
    fn default() -> Self {
        Self {
            inner: nucleo::Matcher::new(nucleo::Config::DEFAULT),
        }
    }
}

impl Matcher {
    /// Create a matcher.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rank `items` against `pattern`, best match first (ties keep input order,
    /// so a pre-sorted candidate list stays meaningful).
    ///
    /// Matching is nucleo's fuzzy subsequence match with **smart case**
    /// (case-insensitive until the pattern contains an uppercase letter) and
    /// Unicode normalization. Items that do not match are dropped. An empty
    /// pattern keeps every item, unscored, in input order — the "just opened,
    /// nothing typed yet" state of a picker or completion popup.
    pub fn rank<'a, T: AsRef<str>>(&mut self, pattern: &str, items: &'a [T]) -> Vec<Scored<'a, T>> {
        if pattern.is_empty() {
            return items
                .iter()
                .map(|item| Scored {
                    item,
                    score: 0,
                    matched: Vec::new(),
                })
                .collect();
        }
        let pattern = nucleo::pattern::Pattern::parse(
            pattern,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
        );
        let mut haystack_buf = Vec::new();
        let mut scored: Vec<Scored<'a, T>> = items
            .iter()
            .filter_map(|item| {
                let haystack = nucleo::Utf32Str::new(item.as_ref(), &mut haystack_buf);
                let mut matched = Vec::new();
                let score = pattern.indices(haystack, &mut self.inner, &mut matched)?;
                matched.sort_unstable();
                matched.dedup();
                Some(Scored {
                    item,
                    score,
                    matched,
                })
            })
            .collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.score)); // stable: ties keep order
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels<'a, T: AsRef<str>>(scored: &[Scored<'a, T>]) -> Vec<&'a str> {
        scored.iter().map(|s| s.item.as_ref()).collect()
    }

    #[test]
    fn empty_pattern_keeps_everything_in_order() {
        let items = ["zebra", "apple", "mango"];
        let ranked = Matcher::new().rank("", &items);
        assert_eq!(labels(&ranked), ["zebra", "apple", "mango"]);
        assert!(ranked.iter().all(|s| s.score == 0 && s.matched.is_empty()));
    }

    #[test]
    fn subsequences_match_and_non_matches_drop() {
        let items = ["println", "process", "id"];
        let ranked = Matcher::new().rank("prl", &items);
        assert_eq!(labels(&ranked), ["println"]);
    }

    #[test]
    fn better_matches_rank_first() {
        // An exact-prefix run beats a scattered subsequence.
        let items = ["plus_unsigned", "push"];
        let ranked = Matcher::new().rank("pus", &items);
        assert_eq!(labels(&ranked).first(), Some(&"push"));
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn ties_keep_input_order() {
        // Identical strings score identically; the sort must be stable.
        let items = ["alpha_one", "alpha_one"];
        let ranked = Matcher::new().rank("alpha", &items);
        assert_eq!(ranked.len(), 2);
        assert!(std::ptr::eq(ranked[0].item, &items[0]));
        assert!(std::ptr::eq(ranked[1].item, &items[1]));
    }

    #[test]
    fn matched_indices_point_at_the_matched_chars() {
        let items = ["abcdef"];
        let ranked = Matcher::new().rank("ace", &items);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].matched, vec![0, 2, 4]);
    }

    #[test]
    fn smart_case_is_insensitive_until_uppercase_appears() {
        let items = ["FooBar", "foobar"];
        assert_eq!(labels(&Matcher::new().rank("foo", &items)).len(), 2);
        let upper = Matcher::new().rank("FooB", &items);
        assert_eq!(labels(&upper), ["FooBar"]);
    }
}
