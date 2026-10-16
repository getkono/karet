//! Case-preserving replacement: rewrite a replacement to follow the letter case
//! of the text it replaces, as VS Code's *Preserve Case* replace option does.

/// Rewrite `replacement` to follow the letter case of `matched`, the text it
/// replaces.
///
/// - `matched` with no lowercase letter (`FOO`) upper-cases the replacement.
/// - `matched` with no uppercase letter (`foo`) lower-cases it.
/// - Mixed case (`Foo`, `fooBar`) carries the case of `matched`'s first
///   character onto the replacement's first character and leaves the rest as
///   typed.
/// - `matched` split by `-` or `_` into as many segments as the replacement
///   (`foo-Bar` → `baz-qux`) is matched segment by segment, so `FOO_bar`
///   replaced with `baz_qux` gives `BAZ_qux`.
/// - `matched` with no cased letter at all (`123`, `+`) leaves the replacement
///   untouched: it says nothing about case.
///
/// ```
/// use karet_search::preserve_case;
///
/// assert_eq!(preserve_case("Apple", "banana"), "Banana");
/// assert_eq!(preserve_case("APPLE", "banana"), "BANANA");
/// assert_eq!(preserve_case("apple-Pie", "banana-tart"), "banana-Tart");
/// ```
#[must_use]
pub fn preserve_case(matched: &str, replacement: &str) -> String {
    for sep in ['-', '_'] {
        if !matched.contains(sep) {
            continue;
        }
        let from: Vec<&str> = matched.split(sep).collect();
        let to: Vec<&str> = replacement.split(sep).collect();
        if from.len() == to.len() {
            let parts: Vec<String> = from
                .iter()
                .zip(&to)
                .map(|(m, r)| preserve_case(m, r))
                .collect();
            return parts.join(&sep.to_string());
        }
    }
    word_case(matched, replacement)
}

/// [`preserve_case`] for one segment, with no separator handling.
fn word_case(matched: &str, replacement: &str) -> String {
    let has_lower = matched.chars().any(char::is_lowercase);
    let has_upper = matched.chars().any(char::is_uppercase);
    match (has_lower, has_upper) {
        (false, false) => replacement.to_string(),
        (false, true) => replacement.to_uppercase(),
        (true, false) => replacement.to_lowercase(),
        (true, true) => {
            let upper = matched.chars().next().is_some_and(char::is_uppercase);
            let mut chars = replacement.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut out: String = if upper {
                first.to_uppercase().collect()
            } else {
                first.to_lowercase().collect()
            };
            out.push_str(chars.as_str());
            out
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_case_follows_the_match() {
        assert_eq!(preserve_case("FOO", "bar"), "BAR");
        assert_eq!(preserve_case("foo", "BaR"), "bar");
        assert_eq!(preserve_case("Foo", "bar"), "Bar");
        // A single capital is all-uppercase, not "capitalised".
        assert_eq!(preserve_case("F", "bar"), "BAR");
    }

    #[test]
    fn mixed_case_carries_only_the_first_letter() {
        assert_eq!(preserve_case("fooBar", "BazQux"), "bazQux");
        assert_eq!(preserve_case("FooBar", "bazQux"), "BazQux");
    }

    #[test]
    fn separated_segments_are_matched_one_by_one() {
        assert_eq!(preserve_case("foo-Bar", "baz-qux"), "baz-Qux");
        assert_eq!(preserve_case("FOO_bar", "baz_qux"), "BAZ_qux");
        assert_eq!(preserve_case("Foo-BAR_baz", "a-b_c"), "A-B_c");
    }

    #[test]
    fn a_segment_count_mismatch_falls_back_to_the_whole_word() {
        // Two segments against one: the whole match is all-uppercase.
        assert_eq!(preserve_case("FOO-BAR", "baz"), "BAZ");
        assert_eq!(preserve_case("foo-bar", "Baz-Qux-Quux"), "baz-qux-quux");
    }

    #[test]
    fn a_match_without_cased_letters_keeps_the_replacement() {
        assert_eq!(preserve_case("123", "aBc"), "aBc");
        assert_eq!(preserve_case("", "aBc"), "aBc");
    }

    #[test]
    fn an_empty_replacement_stays_empty() {
        assert_eq!(preserve_case("Foo", ""), "");
        assert_eq!(preserve_case("FOO", ""), "");
    }

    #[test]
    fn case_mapping_is_unicode_aware() {
        assert_eq!(preserve_case("ÉTÉ", "straße"), "STRASSE");
        assert_eq!(preserve_case("Été", "ölçü"), "Ölçü");
    }
}
