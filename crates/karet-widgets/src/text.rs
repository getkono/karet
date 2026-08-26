//! Cell-accurate text fitting for fixed-width UI slots.
//!
//! The one blessed truncation family: every widget and panel that must fit a
//! string into a column budget uses these, so ellipsis behavior is identical
//! everywhere (a single `…` cell marks the cut; a zero budget yields nothing).
//! [`wrap`] is the counterpart for slots that grow downwards instead of
//! cutting: toast bodies, dialog bodies.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// The display width of `text` in terminal cells.
#[must_use]
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Fit `text` into `max` cells, cutting at the end: `abcdef` → `abc…`.
/// Unchanged when it already fits; empty when `max` is `0`.
#[must_use]
pub fn fit_end(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('\u{2026}');
    out
}

/// Fit `text` into `max` cells, cutting at the start: `abcdef` → `…def`.
/// Keeps the right-most, most-specific part (paths, breadcrumbs). Unchanged
/// when it already fits; empty when `max` is `0`.
#[must_use]
pub fn fit_start(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let budget = max - 1;
    let mut kept = Vec::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        kept.push(ch);
        used += w;
    }
    kept.reverse();
    let mut out = String::from('\u{2026}');
    out.extend(kept);
    out
}

/// Soft-wrap `text` into lines of at most `max` cells, breaking at whitespace
/// and falling back to a mid-word break for a word too long to fit on a line of
/// its own.
///
/// Existing line breaks are honoured (an empty source line stays an empty
/// line). A zero budget yields nothing at all; any other input yields at least
/// one line, so a caller sizing a box always has a row to paint.
#[must_use]
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for source in text.lines() {
        if source.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in source.split_whitespace() {
            let separator = usize::from(!current.is_empty());
            if width(&current)
                .saturating_add(separator)
                .saturating_add(width(word))
                <= max
            {
                if separator == 1 {
                    current.push(' ');
                }
                current.push_str(word);
                continue;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            for character in word.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                if !current.is_empty() && width(&current).saturating_add(character_width) > max {
                    lines.push(std::mem::take(&mut current));
                }
                current.push(character);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_text_is_returned_unchanged() {
        assert_eq!(fit_end("abc", 3), "abc");
        assert_eq!(fit_start("abc", 3), "abc");
    }

    #[test]
    fn fit_end_cuts_at_the_end_with_one_ellipsis_cell() {
        assert_eq!(fit_end("abcdef", 4), "abc\u{2026}");
        assert_eq!(width(&fit_end("abcdef", 4)), 4);
    }

    #[test]
    fn fit_start_keeps_the_right_most_part() {
        assert_eq!(fit_start("abcdef", 4), "\u{2026}def");
        assert_eq!(width(&fit_start("abcdef", 4)), 4);
    }

    #[test]
    fn zero_and_one_cell_budgets_degrade_gracefully() {
        assert_eq!(fit_end("abc", 0), "");
        assert_eq!(fit_start("abc", 0), "");
        assert_eq!(fit_end("abc", 1), "\u{2026}");
        assert_eq!(fit_start("abc", 1), "\u{2026}");
    }

    #[test]
    fn wrapping_breaks_at_whitespace_and_keeps_hard_breaks() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(
            wrap("a\n\nb", 4),
            vec!["a".to_owned(), String::new(), "b".to_owned()],
            "an empty source line stays an empty line"
        );
    }

    #[test]
    fn wrapping_breaks_mid_word_when_a_word_cannot_fit() {
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        // A wide character is never split across two lines.
        assert!(wrap("日本語", 3).iter().all(|line| width(line) <= 3));
    }

    #[test]
    fn wrapping_degenerate_input_never_panics() {
        assert!(
            wrap("anything", 0).is_empty(),
            "a zero budget paints nothing"
        );
        assert_eq!(wrap("", 10), vec![String::new()], "always one row to paint");
        assert_eq!(wrap("   ", 10), vec![String::new()]);
    }

    #[test]
    fn wide_characters_never_overflow_the_budget() {
        // 日 is two cells wide: a 4-cell budget fits "…日" (3 cells) but the
        // next wide character would overflow, so it stays within budget.
        let fitted = fit_start("日本語", 4);
        assert!(width(&fitted) <= 4);
        assert!(fitted.starts_with('\u{2026}'));
        let fitted = fit_end("日本語", 3);
        assert!(width(&fitted) <= 3);
        assert!(fitted.ends_with('\u{2026}'));
    }
}
