//! Per-file-type comment syntax: the tokens a "toggle comment" command inserts.
//!
//! The table is keyed by a file type's protocol language id (or, for a type with
//! none, its display name), so two types that share a grammar can still differ —
//! `.json` has no comments while `.jsonc` takes `//`.

/// The line and block comment tokens of one file type.
///
/// Either half may be absent: CSS and HTML have only block comments, Python and
/// YAML only line comments, and data formats such as JSON have neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommentTokens {
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
}

impl CommentTokens {
    /// No comment syntax at all.
    pub const NONE: Self = Self::new(None, None);

    /// Tokens from an optional line prefix and an optional `(open, close)` pair.
    #[must_use]
    pub const fn new(
        line: Option<&'static str>,
        block: Option<(&'static str, &'static str)>,
    ) -> Self {
        Self { line, block }
    }

    /// The line-comment prefix (e.g. `"//"`, `"#"`), if the language has one.
    #[must_use]
    pub fn line(&self) -> Option<&'static str> {
        self.line
    }

    /// The block-comment delimiters (e.g. `("/*", "*/")`), if the language has them.
    #[must_use]
    pub fn block(&self) -> Option<(&'static str, &'static str)> {
        self.block
    }

    /// Whether the language has no comment syntax at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.line.is_none() && self.block.is_none()
    }
}

const fn line(token: &'static str) -> CommentTokens {
    CommentTokens::new(Some(token), None)
}

const fn both(token: &'static str, open: &'static str, close: &'static str) -> CommentTokens {
    CommentTokens::new(Some(token), Some((open, close)))
}

const fn block(open: &'static str, close: &'static str) -> CommentTokens {
    CommentTokens::new(None, Some((open, close)))
}

const C_STYLE: CommentTokens = both("//", "/*", "*/");
const HASH: CommentTokens = line("#");
const MARKUP: CommentTokens = block("<!--", "-->");

/// The comment tokens for a file type identified by `key` — its protocol language
/// id, or its display name when it has none. Unknown keys have no comment syntax.
pub(crate) fn tokens_for(key: &str) -> CommentTokens {
    match key {
        "rust" | "c" | "cpp" | "csharp" | "java" | "kotlin" | "go" | "swift" | "scala" | "dart"
        | "javascript" | "javascriptreact" | "typescript" | "typescriptreact" | "jsonc"
        | "json5" | "protobuf" | "scss" | "sass" | "less" | "pkl" | "php" => C_STYLE,
        "zig" | "asciidoc" => line("//"),
        "python" | "ruby" | "r" | "perl" | "elixir" | "yaml" | "toml" | "properties"
        | "lockfile" | "shellscript" | "zsh" | "fish" | "graphql" | "dockerfile" | "make"
        | "gitconfig" | "editorconfig" | "dotenv" | "Ksh" | "Git config" => HASH,
        "cmake" => both("#", "#[[", "]]"),
        "powershell" => both("#", "<#", "#>"),
        "ini" => line(";"),
        "lua" => both("--", "--[[", "]]"),
        "haskell" => both("--", "{-", "-}"),
        "sql" => both("--", "/*", "*/"),
        "ocaml" => block("(*", "*)"),
        "erlang" | "latex" => line("%"),
        "Mermaid" => line("%%"),
        "clojure" | "edn" | "elisp" => line(";"),
        "vim" => line("\""),
        "batch" => line("REM"),
        "restructuredtext" => line(".."),
        "css" => block("/*", "*/"),
        "html" | "xml" | "markdown" | "vue" | "svelte" | "astro" => MARKUP,
        "mdx" => block("{/*", "*/}"),
        "erb" => block("<%#", "%>"),
        _ => CommentTokens::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_family_has_both_halves() {
        let tokens = tokens_for("rust");
        assert_eq!(tokens.line(), Some("//"));
        assert_eq!(tokens.block(), Some(("/*", "*/")));
        assert!(!tokens.is_empty());
    }

    #[test]
    fn markup_is_block_only_and_data_has_none() {
        assert_eq!(tokens_for("html").line(), None);
        assert_eq!(tokens_for("html").block(), Some(("<!--", "-->")));
        assert!(tokens_for("json").is_empty());
        assert_eq!(tokens_for("no-such-language"), CommentTokens::NONE);
    }
}
