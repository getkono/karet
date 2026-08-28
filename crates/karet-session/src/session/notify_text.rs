//! Making untrusted text safe to put in a notification.
//!
//! Two of the session's notification paths quote text karet did not write: a
//! language server's dying words, and a failing installer's stderr. Both are as
//! long and as hostile as whatever produced them, and both end up in a toast
//! that a terminal paints. They share this one sanitiser so the rule cannot
//! drift between them.

/// Reduce arbitrary output to one short, printable line.
///
/// Control characters and the invisible or reordering formatting characters
/// become spaces, runs of whitespace collapse, and the result is capped. An
/// escape sequence would otherwise move the cursor or repaint the terminal from
/// inside a notification, and a bidi override would reverse the text around it.
pub(super) fn one_line(text: &str) -> String {
    const LIMIT: usize = 160;
    let flattened = text
        .chars()
        .map(|character| {
            if character.is_control() || is_invisible_or_reordering(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let collapsed = flattened.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(LIMIT) {
        Some((cut, _)) => format!("{}\u{2026}", &collapsed[..cut]),
        None => collapsed,
    }
}

/// Whether `character` is invisible or can reorder the text around it.
///
/// [`char::is_control`] covers only the `Cc` category, which leaves the
/// formatting characters hostile output would actually reach for: a
/// right-to-left override reverses what follows it, and a zero-width space
/// hides a word boundary. Neither is a control character, and both reach the
/// terminal through a notification.
///
/// Hand-rolled rather than pulled from a Unicode crate: it is a short, stable
/// list, and a notification sanitiser is not worth a dependency.
fn is_invisible_or_reordering(character: char) -> bool {
    matches!(character,
        '\u{00ad}'                  // soft hyphen
        | '\u{200b}'..='\u{200f}'   // zero-width spaces, LRM/RLM
        | '\u{202a}'..='\u{202e}'   // bidi embeddings and overrides
        | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
        | '\u{2066}'..='\u{2069}'   // bidi isolates
        | '\u{feff}'                // zero-width no-break space
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_flattened_and_capped_before_it_is_shown() {
        assert_eq!(one_line("  hello\n  world  "), "hello world");
        assert_eq!(one_line("clear\u{1b}[2Jscreen"), "clear [2Jscreen");
        let long = "x".repeat(400);
        let shown = one_line(&long);
        assert_eq!(shown.chars().count(), 161);
        assert!(shown.ends_with('\u{2026}'));
    }

    #[test]
    fn a_control_character_cannot_reach_the_terminal() {
        assert!(!one_line("a\u{7}b\rc").contains(|c: char| c.is_control()));
    }

    /// The characters that reorder or hide text are not control characters, so
    /// `char::is_control` alone would pass every one of these through.
    #[test]
    fn text_cannot_be_reordered_or_hidden_on_its_way_to_the_terminal() {
        assert_eq!(one_line("safe\u{202e}txet"), "safe txet");
        assert_eq!(one_line("zero\u{200b}width"), "zero width");
        assert_eq!(one_line("iso\u{2066}late\u{2069}d"), "iso late d");
        assert_eq!(one_line("bom\u{feff}mark"), "bom mark");
        for hidden in ['\u{00ad}', '\u{200f}', '\u{202a}', '\u{2060}'] {
            assert!(
                !one_line(&format!("a{hidden}b")).contains(hidden),
                "{hidden:?} reached the notification"
            );
        }
    }
}
