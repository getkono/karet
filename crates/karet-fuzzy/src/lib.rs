//! `karet-fuzzy` — fuzzy matching and ranking for the karet toolkit.
//!
//! Standalone (depends on no other karet crate). Wraps `nucleo`'s matching with
//! fzf pattern syntax and smart case, shared by the widgets toolkit and completion
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
    /// `pattern` uses fzf syntax, parsed by nucleo: it splits on unescaped
    /// whitespace into atoms that must **all** match, and each atom is a fuzzy
    /// subsequence match unless an operator changes it — a leading `^` anchors a
    /// prefix, a trailing `$` anchors a suffix (both together match exactly), a
    /// leading `'` asks for a contiguous substring, and a leading `!` inverts
    /// the atom, excluding the items it matches (a bare `!foo` excludes items
    /// containing `foo`). A backslash escapes an operator or a space.
    ///
    /// Matching uses **smart case** (case-insensitive until an atom contains an
    /// uppercase letter) and Unicode normalization. Items that do not match are
    /// dropped. An empty pattern keeps every item, unscored, in input order — the
    /// "just opened, nothing typed yet" state of a picker or completion popup; a
    /// whitespace-only pattern has no atoms, so it also keeps every item with
    /// score 0.
    #[must_use]
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

    /// Rank `items` against `pattern` and return the **original indices** of the
    /// matches, best first (ties keep input order). The index-preserving twin of
    /// [`rank`](Self::rank), for consumers that key rows by position (pickers),
    /// with the same fzf pattern syntax. An empty pattern keeps every index in
    /// input order.
    #[must_use]
    pub fn rank_indices<T: AsRef<str>>(&mut self, pattern: &str, items: &[T]) -> Vec<usize> {
        if pattern.is_empty() {
            return (0..items.len()).collect();
        }
        let pattern = nucleo::pattern::Pattern::parse(
            pattern,
            nucleo::pattern::CaseMatching::Smart,
            nucleo::pattern::Normalization::Smart,
        );
        let mut haystack_buf = Vec::new();
        let mut scored: Vec<(usize, u32)> = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let haystack = nucleo::Utf32Str::new(item.as_ref(), &mut haystack_buf);
                let score = pattern.score(haystack, &mut self.inner)?;
                Some((index, score))
            })
            .collect();
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score)); // stable: ties keep order
        scored.into_iter().map(|(index, _)| index).collect()
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

    /// The position of each ranked item within `items`, recovered by identity so
    /// duplicate strings stay distinguishable.
    fn positions<T: AsRef<str>>(scored: &[Scored<'_, T>], items: &[T]) -> Vec<usize> {
        scored
            .iter()
            .filter_map(|s| items.iter().position(|item| std::ptr::eq(item, s.item)))
            .collect()
    }

    #[test]
    fn rank_indices_agrees_with_rank_order() {
        let items = [
            "plus_unsigned",
            "push",
            "println",
            "process",
            "id",
            "push",
            "FooBar",
            "foobar",
        ];
        let mut matcher = Matcher::new();
        for pattern in ["", "pus", "prl", "p", "push", "foo", "FooB", "zzz"] {
            let by_rank = positions(&matcher.rank(pattern, &items), &items);
            assert_eq!(
                matcher.rank_indices(pattern, &items),
                by_rank,
                "pattern {pattern:?}"
            );
        }
    }

    #[test]
    fn rank_indices_keeps_every_index_for_an_empty_pattern() {
        let items = ["zebra", "apple", "mango"];
        assert_eq!(Matcher::new().rank_indices("", &items), [0, 1, 2]);
        let none: [&str; 0] = [];
        assert!(Matcher::new().rank_indices("", &none).is_empty());
    }

    #[test]
    fn rank_indices_orders_best_first_and_drops_non_matches() {
        let items = ["id", "plus_unsigned", "push"];
        assert_eq!(Matcher::new().rank_indices("pus", &items), [2, 1]);
    }

    #[test]
    fn rank_indices_ties_keep_input_order() {
        let items = ["other", "alpha_one", "nope", "alpha_one"];
        assert_eq!(Matcher::new().rank_indices("alpha", &items), [1, 3]);
    }
}
