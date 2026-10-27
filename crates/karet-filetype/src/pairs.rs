//! Per-file-type auto-closing pairs: the brackets and quotes an editor closes as
//! they are typed.
//!
//! Set per registry row, like the comment tokens. Every type closes `()`, `[]`
//! and `{}`; the quotes are where languages differ. A `'` is left out wherever
//! it is not always a quote: Rust lifetimes and labels (`'a`), OCaml type
//! variables, and the primes of Haskell-family names, Lisp quoting, and the
//! apostrophes of prose.

/// Brackets plus double quotes only: the default for a row that sets none
/// (plain text, an unknown file).
pub(crate) const DOUBLE: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"')];

/// Brackets plus double and single quotes.
pub(crate) const QUOTES: &[(char, char)] =
    &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];

/// Brackets plus double, single and backtick quotes (template literals, Go raw
/// strings, shell command substitution).
pub(crate) const BACKTICK: &[(char, char)] = &[
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('"', '"'),
    ('\'', '\''),
    ('`', '`'),
];

/// Brackets, double quotes and backticks: markdown's inline code, without the
/// apostrophe of its prose.
pub(crate) const MARKDOWN: &[(char, char)] =
    &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('`', '`')];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_type_for_language;
    use crate::file_type_for_path;

    fn pairs_of(path: &str) -> &'static [(char, char)] {
        file_type_for_path(std::path::Path::new(path)).auto_pairs()
    }

    fn pairs_for(id: &str) -> &'static [(char, char)] {
        file_type_for_language(id).map_or(DOUBLE, |file_type| file_type.auto_pairs())
    }

    #[test]
    fn every_type_closes_the_three_brackets() {
        for path in [
            "main.rs",
            "app.ts",
            "notes.md",
            "notes.txt",
            "x.no-such-extension",
        ] {
            for pair in [('(', ')'), ('[', ']'), ('{', '}')] {
                assert!(pairs_of(path).contains(&pair), "{path} closes {pair:?}");
            }
        }
        for file_type in crate::all_file_types() {
            for pair in [('(', ')'), ('[', ']'), ('{', '}')] {
                assert!(
                    file_type.auto_pairs().contains(&pair),
                    "{} closes {pair:?}",
                    file_type.name()
                );
            }
        }
    }

    #[test]
    fn single_quotes_close_only_where_they_are_always_quotes() {
        let closes = |id| pairs_for(id).contains(&('\'', '\''));
        assert!(closes("python") && closes("javascript") && closes("c"));
        assert!(!closes("rust"), "lifetimes");
        assert!(!closes("ocaml") && !closes("haskell") && !closes("clojure"));
        assert!(
            !closes("markdown") && !pairs_of("notes.txt").contains(&('\'', '\'')),
            "apostrophes"
        );
    }

    #[test]
    fn a_file_type_resolves_its_pairs_from_its_row() {
        assert!(!pairs_of("main.rs").contains(&('\'', '\'')));
        assert!(pairs_of("app.py").contains(&('\'', '\'')));
        assert!(pairs_of("app.tsx").contains(&('`', '`')));
        assert_eq!(pairs_of("notes.txt"), DOUBLE);
        assert_eq!(pairs_of("x.no-such-extension"), DOUBLE);
    }

    #[test]
    fn backticks_close_in_template_and_raw_string_languages() {
        let closes = |id| pairs_for(id).contains(&('`', '`'));
        assert!(closes("typescript") && closes("go") && closes("shellscript"));
        assert!(closes("markdown"));
        assert!(!closes("rust") && !closes("python"));
        assert!(
            pairs_of("a.ksh").contains(&('`', '`')),
            "a name-only shell row"
        );
    }
}
