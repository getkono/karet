//! `karet-syntax` — tree-sitter-powered syntactic analysis for karet editors.
//!
//! Produces *data*, not rendering: highlight spans tagged with semantic
//! [`TokenId`](karet_core::TokenId)s. Consumers apply a theme (`karet-theme`)
//! and render. This is the crate behind the standalone "highlight a code
//! snippet" use case. Highlighting runs a grammar's query through
//! `karet-treesitter`'s single parse host.
//!
//! [`Highlighter`] highlights one language. [`LayeredHighlighter`] highlights a
//! `LayeredTree` — a document plus its injected languages — so a markdown code fence
//! is coloured as the language it names and a Rust doc comment as the markdown it is.
//! [`mark_semantic_comments`] is an optional post-pass that retints codetag comment
//! blocks (`TODO: …` and friends) so they stand out from ordinary comments.
//! [`Folder`] computes fold regions from per-language fold queries, with
//! [`fold`] (any grammar) and [`fold_by_indent`] (no grammar) as fallbacks;
//! [`SemanticBlocker`] extracts sticky-scroll
//! blocks; [`OutlineExtractor`] builds symbol outlines from declaration queries;
//! [`Indenter`] computes a new line's indentation from indent queries;
//! [`TableFinder`] finds the lines GFM tables occupy.

mod blocks;
mod codetag;
mod folding;
mod highlight;
mod indent;
mod inline_macro;
mod map;
mod outline;
mod tables;

pub use blocks::SemanticBlocker;
pub use codetag::CodetagHit;
pub use codetag::SemanticCommentConfig;
pub use codetag::find_codetags;
pub use codetag::mark_semantic_comments;
pub use folding::Folder;
pub use folding::fold;
pub use folding::fold_by_indent;
pub use highlight::Highlighter;
pub use highlight::LayeredHighlighter;
pub use indent::Indenter;
pub use inline_macro::InlineMacroEngine;
pub use inline_macro::InlineMacroExpansion;
pub use inline_macro::InlineMacroTrigger;
// The parse-host items this crate's signatures take or return, plus what
// produces them (a language from a path or fence name, a tree from a parser),
// so a consumer can depend on `karet-syntax` alone: resolve a language, parse
// a buffer, then analyze it. The highlight/fold/semantic-block *data* shapes
// are `karet-core` vocabulary and are named from there
// (`karet_core::Highlights`, …), not re-exported here.
pub use karet_treesitter::LanguageId;
pub use karet_treesitter::LayeredParser;
pub use karet_treesitter::LayeredTree;
pub use karet_treesitter::ParserPool;
pub use karet_treesitter::SyntaxTree;
pub use karet_treesitter::TsError;
pub use karet_treesitter::language_id_from_injection_name;
pub use karet_treesitter::language_id_from_path;
pub use outline::OutlineExtractor;
pub use tables::TableFinder;

/// Errors produced by syntactic analysis.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyntaxError {
    /// No highlight configuration exists (is compiled in) for the language.
    #[error("unsupported language")]
    UnsupportedLanguage,
    /// A highlight/fold query failed, as the parse host reported it.
    #[error("query error: {0}")]
    Query(#[from] TsError),
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;
    use karet_core::Highlights;
    use karet_core::StandardToken;
    use karet_core::TokenId;

    use super::*;

    #[test]
    fn error_displays() {
        assert_eq!(
            SyntaxError::UnsupportedLanguage.to_string(),
            "unsupported language"
        );
    }

    #[test]
    fn query_error_keeps_the_parse_host_error_as_its_source() {
        let err = SyntaxError::from(TsError::InvalidQuery("bad capture".to_owned()));
        assert!(matches!(
            &err,
            SyntaxError::Query(TsError::InvalidQuery(message)) if message == "bad capture"
        ));
        assert_eq!(err.to_string(), "query error: invalid query: bad capture");
        assert!(std::error::Error::source(&err).is_some());
    }

    /// The token covering `needle`'s first byte in `text`, per `highlights`.
    fn token_of(highlights: &Highlights, text: &str, needle: &str) -> Option<TokenId> {
        let at = text.find(needle)?;
        highlights
            .all()
            .iter()
            .find(|s| s.span.start.0 <= at && at < s.span.end.0)
            .map(|s| s.token)
    }

    #[test]
    fn markdown_code_fence_is_highlighted_as_its_language() -> Result<(), Box<dyn std::error::Error>>
    {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        let src = "# Title\n\n```rust\nfn main() { let x = 42; }\n```\n";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        // The embedded rust wins over markdown's coarse `text.literal` fence span.
        assert_eq!(
            token_of(&highlights, src, "fn main"),
            Some(TokenId::KEYWORD)
        );
        assert_eq!(token_of(&highlights, src, "42"), Some(TokenId::CONSTANT));
        // Markdown's own structure keeps its markup colors.
        assert_eq!(
            token_of(&highlights, src, "Title"),
            Some(StandardToken::MarkupHeading.id())
        );
        // Spans remain sorted and non-overlapping after the cross-layer merge.
        assert!(
            highlights
                .all()
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0)
        );
        Ok(())
    }

    #[test]
    fn markdown_inline_emphasis_is_highlighted() -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        // Emphasis and links live in the *inline* grammar, reachable only by injection.
        let src = "Some *slanted* and **heavy** text, plus <http://example.com>.\n";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        assert_eq!(
            token_of(&highlights, src, "slanted"),
            Some(StandardToken::MarkupItalic.id())
        );
        assert_eq!(
            token_of(&highlights, src, "heavy"),
            Some(StandardToken::MarkupBold.id())
        );
        assert_eq!(
            token_of(&highlights, src, "http://example.com"),
            Some(StandardToken::MarkupLink.id())
        );
        Ok(())
    }

    #[test]
    fn latex_sections_math_and_commands_are_highlighted() -> Result<(), Box<dyn std::error::Error>>
    {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(latex) = language_id_from_injection_name("latex") else {
            return Ok(());
        };
        let src = "\\section{Methods}\nThe value is $x + 1$. % note\n";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(latex, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        assert_eq!(
            token_of(&highlights, src, "Methods"),
            Some(StandardToken::MarkupHeading.id())
        );
        assert_eq!(
            token_of(&highlights, src, "x + 1"),
            Some(StandardToken::MarkupRaw.id())
        );
        assert_eq!(token_of(&highlights, src, "note"), Some(TokenId::COMMENT));
        Ok(())
    }

    #[test]
    fn rust_doctest_in_a_doc_comment_is_highlighted_as_rust()
    -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(rust) = language_id_from_injection_name("rust") else {
            return Ok(());
        };
        let src = "\
/// Adds one to `x`.
///
/// ```rust
/// let y = 1 + 1;
/// ```
pub fn add_one() {}
";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        // The doctest body is real rust: `let` is a keyword, not comment text.
        assert_eq!(token_of(&highlights, src, "let y"), Some(TokenId::KEYWORD));
        // The doc comment's prose is a doc comment, distinct from a plain comment.
        assert_eq!(
            token_of(&highlights, src, "Adds one"),
            Some(StandardToken::CommentDoc.id())
        );
        // And the outer function is still highlighted normally.
        assert_eq!(token_of(&highlights, src, "pub"), Some(TokenId::KEYWORD));
        Ok(())
    }

    #[test]
    fn layered_highlight_tracks_a_live_edit() -> Result<(), Box<dyn std::error::Error>> {
        use karet_core::AppliedEdit;
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        let old = "text\n";
        let new = "text\n\n```rust\nfn f() {}\n```\n";

        let mut parser = LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut tree = parser.parse(md, old)?;
        assert!(
            token_of(&highlighter.highlight(&tree, old), old, "text") != Some(TokenId::KEYWORD)
        );

        parser.reparse(
            &mut tree,
            &[AppliedEdit {
                start_byte: old.len(),
                old_end_byte: old.len(),
                new_end_byte: new.len(),
                start_point: karet_core::BytePoint::new(1, 0),
                old_end_point: karet_core::BytePoint::new(1, 0),
                new_end_point: karet_core::BytePoint::new(5, 0),
                replaced: String::new(),
            }],
            new,
        )?;
        let after = highlighter.highlight(&tree, new);
        // Typing the fence lights up the embedded language immediately.
        assert_eq!(token_of(&after, new, "fn f"), Some(TokenId::KEYWORD));
        // ...and matches a cold parse of the same text.
        let cold = LayeredHighlighter::new().highlight(&parser.parse(md, new)?, new);
        assert_eq!(after.all(), cold.all());
        Ok(())
    }

    /// The indentation Enter at `<|>` in `marked`, parsed as `lang`, gets with
    /// `unit` as one level.
    fn enter(
        lang: &str,
        marked: &str,
        unit: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let lang = language_id_from_injection_name(lang).ok_or("grammar not compiled in")?;
        let at = BytePos(marked.find("<|>").ok_or("no caret")?);
        let text = marked.replacen("<|>", "", 1);
        let tree = LayeredParser::new().parse(lang, &text)?;
        Ok(Indenter::for_tree(&tree).newline_indent(&tree, &text, (at, at), unit))
    }

    #[test]
    fn indenter_adds_one_unit_to_the_enclosing_line() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            enter("rust", "fn f() {\n\tif x {<|>\n\t}\n}", "\t")?.as_deref(),
            Some("\t\t")
        );
        assert_eq!(enter("rust", "fn f() {<|>}", "  ")?.as_deref(), Some(""));
        // No enclosing node, and no indent query at all: the caller copies.
        assert_eq!(enter("rust", "fn f() {}<|>", "  ")?, None);
        assert_eq!(enter("toml", "[a]\nb = 1<|>", "  ")?, None);
        Ok(())
    }

    #[test]
    fn indenter_follows_the_files_indentation_over_the_setting()
    -> Result<(), Box<dyn std::error::Error>> {
        // The enclosing node's existing body decides, whatever the setting.
        assert_eq!(
            enter("go", "func f() {\n\tx()<|>", "    ")?.as_deref(),
            Some("\t")
        );
        assert_eq!(
            enter("python", "class A:\n  def f(self):\n    pass<|>", "    ")?.as_deref(),
            Some("    ")
        );
        assert_eq!(
            enter("yaml", "a:\n  b:\n    c: 1<|>", "    ")?.as_deref(),
            Some("    ")
        );
        assert_eq!(
            enter("yaml", "a:\n  steps:\n  - x<|>", "    ")?.as_deref(),
            Some("  ")
        );
        // With no body yet, the file's own step; the setting only in a flat file.
        assert_eq!(
            enter("python", "def g():\n  pass\n\ndef f():<|>", "    ")?.as_deref(),
            Some("  ")
        );
        assert_eq!(enter("python", "def f():<|>", "\t")?.as_deref(), Some("\t"));
        Ok(())
    }

    #[test]
    fn indenter_ignores_string_and_comment_lines_as_body() -> Result<(), Box<dyn std::error::Error>>
    {
        // A string's closing `"""` and a comment, both at 6 spaces in an 8-space
        // body: the code lines decide.
        let src = "class A:\n    def f(self):\n        x = \"\"\"\n  a\n      \"\"\"\n      # c\n        y<|>";
        assert_eq!(enter("python", src, "\t")?.as_deref(), Some("        "));
        let src = "fn f() {\n    /*\n  c\n  */\n    x();<|>\n}";
        assert_eq!(enter("rust", src, "\t")?.as_deref(), Some("    "));
        // A line that opens a string is code: a docstring sets the body.
        let src = "def f():\n  \"\"\"doc\n\"\"\"<|>";
        assert_eq!(enter("python", src, "\t")?.as_deref(), Some("  "));
        Ok(())
    }

    #[test]
    fn indenter_leaves_doc_comments_out_of_the_files_step() -> Result<(), Box<dyn std::error::Error>>
    {
        // Each ` * ` steps one space in from its `/**`; there are more of those
        // than four-space code steps, yet a new `{` with no body still opens
        // four spaces in.
        let src = "/**\n * a\n */\nfunction f() {\n    /**\n     * b\n     */\n    function g() {\n        /**\n         * c\n         */\n        if (x) {<|>\n    }\n}\n";
        assert_eq!(
            enter("javascript", src, "\t")?.as_deref(),
            Some("            ")
        );
        Ok(())
    }

    #[test]
    fn indenter_follows_the_injected_language_at_the_caret()
    -> Result<(), Box<dyn std::error::Error>> {
        // Markdown ships no indent query; the fence's Rust does.
        let src = "# T\n\n```rust\nfn f() {<|>\n}\n```\n";
        assert_eq!(enter("markdown", src, "    ")?.as_deref(), Some("    "));
        assert_eq!(enter("markdown", "# T\n\nprose<|>\n", "    ")?, None);
        Ok(())
    }

    #[test]
    fn highlights_rust_code_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
        use std::path::Path;

        // Every parse-host item here comes through this crate's own re-exports
        // (`super::*`), which is what lets a consumer depend on karet-syntax alone.
        let Some(lang) = language_id_from_path(Path::new("x.rs")) else {
            return Ok(()); // rust grammar not compiled into this build; nothing to test
        };
        let src = "fn main() { let x = 42; }";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src)?;
        let highlights = Highlighter::new(lang)?.highlight(&tree, src)?;
        let spans = highlights.all();

        assert!(!spans.is_empty());
        // `fn` is a keyword starting at byte 0.
        assert!(
            spans
                .iter()
                .any(|s| s.token == TokenId::KEYWORD && s.span.start.0 == 0)
        );
        // `42` is a numeric literal — Rust's grammar tags it `constant.builtin`,
        // which maps to CONSTANT via the dotted fallback.
        assert!(spans.iter().any(|s| s.token == TokenId::CONSTANT));
        // Output is sorted and non-overlapping.
        assert!(
            spans
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0)
        );
        Ok(())
    }
}
