//! Per-file-type comment syntax: the tokens a "toggle comment" command inserts.
//!
//! Each registry row sets its own tokens, so two types that share a grammar can
//! still differ — `.json` has no comments while `.jsonc` takes `//` — and a
//! renamed type keeps them.

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

/// A line comment only.
pub(crate) const fn line(token: &'static str) -> CommentTokens {
    CommentTokens::new(Some(token), None)
}

/// A line comment and a block comment.
pub(crate) const fn both(
    token: &'static str,
    open: &'static str,
    close: &'static str,
) -> CommentTokens {
    CommentTokens::new(Some(token), Some((open, close)))
}

/// A block comment only.
pub(crate) const fn block(open: &'static str, close: &'static str) -> CommentTokens {
    CommentTokens::new(None, Some((open, close)))
}

/// `//` and `/* */`: the C family and everything that borrowed its syntax.
pub(crate) const C_STYLE: CommentTokens = both("//", "/*", "*/");
/// `#` line comments: shells, scripting languages, and most config formats.
pub(crate) const HASH: CommentTokens = line("#");
/// `<!-- -->`: HTML, XML and markdown.
pub(crate) const MARKUP: CommentTokens = block("<!--", "-->");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_type_for_language;
    use crate::file_type_for_path;

    fn tokens(id: &str) -> Option<CommentTokens> {
        file_type_for_language(id).map(|file_type| file_type.comment_tokens())
    }

    #[test]
    fn c_family_has_both_halves() {
        let rust = tokens("rust").unwrap_or_default();
        assert_eq!(rust.line(), Some("//"));
        assert_eq!(rust.block(), Some(("/*", "*/")));
        assert!(!rust.is_empty());
    }

    #[test]
    fn markup_is_block_only_and_data_has_none() {
        let html = tokens("html").unwrap_or_default();
        assert_eq!(html.line(), None);
        assert_eq!(html.block(), Some(("<!--", "-->")));
        assert_eq!(tokens("json"), Some(CommentTokens::NONE));
        assert!(tokens("json").is_some_and(|json| json.is_empty()));
        let unknown = file_type_for_path(std::path::Path::new("x.no-such-extension"));
        assert_eq!(unknown.comment_tokens(), CommentTokens::NONE);
    }

    /// Types that share a grammar keep their own tokens: `.json` has none while
    /// `.jsonc` takes C-style comments, and the name-only types (no protocol
    /// id) carry theirs on the row too.
    #[test]
    fn each_row_carries_its_own_tokens() {
        let at = |path: &str| file_type_for_path(std::path::Path::new(path)).comment_tokens();
        assert_eq!(at("a.json"), CommentTokens::NONE);
        assert_eq!(at("a.jsonc"), C_STYLE);
        assert_eq!(at("a.ksh"), HASH);
        assert_eq!(at(".gitignore"), HASH);
        assert_eq!(at("a.mmd").line(), Some("%%"));
        assert_eq!(at("a.mdx").block(), Some(("{/*", "*/}")));
    }
}
