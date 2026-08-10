//! The word-character classification every text surface shares.
//!
//! One three-class model — whitespace, word (alphanumeric + `_`), symbol —
//! keeps word motions identical across the editor, text fields, and pickers:
//! a word jump skips whitespace, then consumes one run of a single class.

/// The class of a character for word motions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordClass {
    /// Whitespace: skipped before a word motion's run.
    Whitespace,
    /// A word character: alphanumeric or `_`.
    Word,
    /// Anything else (punctuation, operators): its own run class.
    Symbol,
}

/// Classify `character` for word motions.
#[must_use]
pub fn word_class(character: char) -> WordClass {
    if character.is_whitespace() {
        WordClass::Whitespace
    } else if character.is_alphanumeric() || character == '_' {
        WordClass::Word
    } else {
        WordClass::Symbol
    }
}

/// Whether `character` is part of a word (alphanumeric or `_`).
#[must_use]
pub fn is_word_char(character: char) -> bool {
    word_class(character) == WordClass::Word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_classes_partition_characters() {
        assert_eq!(word_class(' '), WordClass::Whitespace);
        assert_eq!(word_class('\t'), WordClass::Whitespace);
        assert_eq!(word_class('a'), WordClass::Word);
        assert_eq!(word_class('9'), WordClass::Word);
        assert_eq!(word_class('_'), WordClass::Word);
        assert_eq!(word_class('\u{65e5}'), WordClass::Word);
        assert_eq!(word_class('.'), WordClass::Symbol);
        assert_eq!(word_class('-'), WordClass::Symbol);
    }
}
