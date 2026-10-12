//! Per-file-type auto-closing pairs: the brackets and quotes an editor closes as
//! they are typed.
//!
//! Keyed like the comment table, by a file type's protocol language id (or, for a
//! type with none, its display name). Every type closes `()`, `[]` and `{}`; the
//! quotes are where languages differ. A `'` is left out wherever it is not
//! always a quote: Rust lifetimes and labels (`'a`), OCaml type variables, and
//! the primes of Haskell-family names, Lisp quoting, and the apostrophes of prose.

/// Brackets plus double quotes only.
const DOUBLE: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"')];

/// Brackets plus double and single quotes.
const QUOTES: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];

/// Brackets plus double, single and backtick quotes (template literals, Go raw
/// strings, shell command substitution).
const BACKTICK: &[(char, char)] = &[
    ('(', ')'),
    ('[', ']'),
    ('{', '}'),
    ('"', '"'),
    ('\'', '\''),
    ('`', '`'),
];

/// Brackets, double quotes and backticks: markdown's inline code, without the
/// apostrophe of its prose.
const MARKDOWN: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('`', '`')];

/// The auto-closing pairs for a file type identified by `key` — its protocol
/// language id, or its display name when it has none. A key not listed (plain
/// text, an unknown file) gets brackets and double quotes.
pub(crate) fn pairs_for(key: &str) -> &'static [(char, char)] {
    match key {
        "javascript" | "javascriptreact" | "typescript" | "typescriptreact" | "go"
        | "shellscript" | "zsh" | "fish" | "Ksh" | "vue" | "svelte" | "astro" | "kotlin"
        | "scala" | "sql" | "dart" => BACKTICK,
        "c" | "cpp" | "csharp" | "java" | "swift" | "python" | "ruby" | "perl" | "php" | "lua"
        | "r" | "elixir" | "erlang" | "powershell" | "css" | "scss" | "sass" | "less" | "html"
        | "xml" | "yaml" | "toml" | "json5" | "zig" | "cmake" | "dockerfile" | "make"
        | "graphql" | "protobuf" | "pkl" | "erb" | "properties" | "ini" | "gitconfig"
        | "editorconfig" | "dotenv" => QUOTES,
        "markdown" | "mdx" => MARKDOWN,
        _ => DOUBLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_closes_the_three_brackets() {
        for key in [
            "rust",
            "typescript",
            "markdown",
            "Plain Text",
            "no-such-language",
        ] {
            for pair in [('(', ')'), ('[', ']'), ('{', '}')] {
                assert!(pairs_for(key).contains(&pair), "{key} closes {pair:?}");
            }
        }
    }

    #[test]
    fn single_quotes_close_only_where_they_are_always_quotes() {
        let closes = |key| pairs_for(key).contains(&('\'', '\''));
        assert!(closes("python") && closes("javascript") && closes("c"));
        assert!(!closes("rust"), "lifetimes");
        assert!(!closes("ocaml") && !closes("haskell") && !closes("clojure"));
        assert!(!closes("markdown") && !closes("Plain Text"), "apostrophes");
    }

    #[test]
    fn a_file_type_resolves_its_pairs_from_its_language_id() {
        let pairs = |path: &str| crate::file_type_for_path(std::path::Path::new(path)).auto_pairs();
        assert!(!pairs("main.rs").contains(&('\'', '\'')));
        assert!(pairs("app.py").contains(&('\'', '\'')));
        assert!(pairs("app.tsx").contains(&('`', '`')));
        assert_eq!(pairs("notes.txt"), pairs_for("no-such-language"));
    }

    #[test]
    fn backticks_close_in_template_and_raw_string_languages() {
        let closes = |key| pairs_for(key).contains(&('`', '`'));
        assert!(closes("typescript") && closes("go") && closes("shellscript"));
        assert!(closes("markdown"));
        assert!(!closes("rust") && !closes("python"));
    }
}
