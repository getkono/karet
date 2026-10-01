//! Splitting plain text into lines exactly where the [`TextBuffer`] rope
//! breaks them, so a consumer working on a `&str` (a committed blob, a disk
//! read) numbers lines the way the editor shows them.
//!
//! [`TextBuffer`]: crate::TextBuffer

/// The lines of `text`, each without its terminator, split on the same break
/// set as the [`TextBuffer`](crate::TextBuffer) rope: `\r\n` as one break,
/// plus `\n`, `\r`, VT (`U+000B`), FF (`U+000C`), NEL (`U+0085`), LS
/// (`U+2028`) and PS (`U+2029`).
///
/// Line `i` of the result is line `i` of a buffer holding `text`. Like
/// [`str::lines`], a final terminator does not start an extra empty line, so
/// `"a\nb"` and `"a\nb\n"` both yield `["a", "b"]`, and an empty `text` yields
/// nothing.
pub fn lines(text: &str) -> impl Iterator<Item = &str> + '_ {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let Some((at, ch)) = rest.char_indices().find(|&(_, ch)| is_break(ch)) else {
            return Some(std::mem::take(&mut rest));
        };
        let line = &rest[..at];
        let mut next = at + ch.len_utf8();
        if ch == '\r' && rest[next..].starts_with('\n') {
            next += 1;
        }
        rest = &rest[next..];
        Some(line)
    })
}

/// Whether `ch` ends a line in the rope (`\r\n` is handled by the caller).
fn is_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        lines(text).collect()
    }

    /// The rope's own lines for `text`, terminators stripped, with the empty
    /// line after a final break dropped as [`lines`] documents.
    fn rope_lines(text: &str) -> Vec<String> {
        let rope = ropey::Rope::from_str(text);
        let mut out: Vec<String> = rope
            .lines()
            .map(|line| {
                let line = line.to_string();
                let content = line
                    .strip_suffix("\r\n")
                    .or_else(|| line.strip_suffix(is_break))
                    .unwrap_or(&line);
                content.to_owned()
            })
            .collect();
        if out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        out
    }

    #[test]
    fn splits_on_every_rope_break() {
        assert_eq!(
            split("a\nb\rc\r\nd\u{0B}e\u{0C}f\u{85}g\u{2028}h\u{2029}i"),
            vec!["a", "b", "c", "d", "e", "f", "g", "h", "i"]
        );
    }

    #[test]
    fn a_final_terminator_adds_no_empty_line() {
        assert_eq!(split("a\nb"), vec!["a", "b"]);
        assert_eq!(split("a\nb\n"), vec!["a", "b"]);
        assert_eq!(split("a\r\nb\r\n"), vec!["a", "b"]);
        assert_eq!(split("a\u{2028}"), vec!["a"]);
        assert!(split("").is_empty());
    }

    #[test]
    fn consecutive_breaks_keep_their_empty_lines() {
        assert_eq!(split("\n\na"), vec!["", "", "a"]);
        // `\n\r` is two breaks, unlike `\r\n`.
        assert_eq!(split("a\n\rb"), vec!["a", "", "b"]);
        assert_eq!(split("a\r\r\nb"), vec!["a", "", "b"]);
    }

    #[test]
    fn matches_the_rope_line_for_line() {
        for text in [
            "",
            "\n",
            "\r",
            "\r\n",
            "a\rb\r\nc\n",
            "x\u{2028}y\u{2029}\u{85}z",
            "\u{0B}\u{0C}é\r\r\n\n",
            "no break at all",
        ] {
            assert_eq!(split(text), rope_lines(text), "for {text:?}");
        }
    }
}
