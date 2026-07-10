//! Mapping tree-sitter highlight capture names to karet's [`TokenId`] vocabulary.

use karet_core::StandardToken;
use karet_core::TokenId;

/// Resolve a tree-sitter capture name (e.g. `keyword.control`, `string.escape`) to
/// a [`TokenId`], or `None` to leave it unhighlighted.
///
/// Tries a small alias table for common captures with no direct standard class,
/// then the progressive dotted-name fallback: the full name, then with each
/// trailing `.segment` dropped (so `function.macro` resolves to `function`).
pub(crate) fn map_capture(name: &str) -> Option<TokenId> {
    if let Some(tok) = alias(name) {
        return Some(tok.id());
    }
    let mut n = name;
    loop {
        if let Some(tok) = StandardToken::from_capture_name(n) {
            return Some(tok.id());
        }
        let i = n.rfind('.')?;
        n = &n[..i];
    }
}

/// Aliases for common grammar captures that don't match a [`StandardToken`] name
/// (and wouldn't via the dotted fallback either). Without these, e.g. Rust's bare
/// `@escape` and `@constructor` would render unhighlighted.
fn alias(name: &str) -> Option<StandardToken> {
    Some(match name {
        "escape" => StandardToken::StringEscape,
        "constructor" => StandardToken::Type,
        "field" | "variable.member" => StandardToken::Property,
        "comment.doc" => StandardToken::CommentDoc,
        // tree-sitter-md (block + inline grammars) predates the `markup.*` convention
        // and spells its captures `text.*`. Route them onto the markup vocabulary so
        // markdown gets native colors instead of borrowing Keyword/String/Constant.
        "text.title" => StandardToken::MarkupHeading,
        "text.literal" => StandardToken::MarkupRaw,
        "text.emphasis" => StandardToken::MarkupItalic,
        "text.strong" => StandardToken::MarkupBold,
        "text.quote" => StandardToken::MarkupQuote,
        "text.uri" | "text.reference" => StandardToken::MarkupLink,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_names_map() {
        assert_eq!(map_capture("keyword"), Some(TokenId::KEYWORD));
        assert_eq!(map_capture("string"), Some(TokenId::STRING));
        assert_eq!(
            map_capture("variable.parameter"),
            Some(StandardToken::Parameter.id())
        );
    }

    #[test]
    fn dotted_fallback_strips_segments() {
        assert_eq!(
            map_capture("keyword.control.import"),
            Some(StandardToken::KeywordControl.id())
        );
        assert_eq!(map_capture("function.macro"), Some(TokenId::FUNCTION));
        assert_eq!(
            map_capture("punctuation.bracket"),
            Some(StandardToken::Punctuation.id())
        );
        assert_eq!(map_capture("constant.builtin"), Some(TokenId::CONSTANT));
    }

    #[test]
    fn aliases_cover_bare_captures() {
        assert_eq!(
            map_capture("escape"),
            Some(StandardToken::StringEscape.id())
        );
        assert_eq!(map_capture("constructor"), Some(TokenId::TYPE));
        assert_eq!(map_capture("field"), Some(StandardToken::Property.id()));
    }

    #[test]
    fn markdown_captures_map_to_markup_tokens() {
        assert_eq!(
            map_capture("text.title"),
            Some(StandardToken::MarkupHeading.id())
        );
        assert_eq!(
            map_capture("text.literal"),
            Some(StandardToken::MarkupRaw.id())
        );
        assert_eq!(
            map_capture("text.emphasis"),
            Some(StandardToken::MarkupItalic.id())
        );
        assert_eq!(
            map_capture("text.strong"),
            Some(StandardToken::MarkupBold.id())
        );
        assert_eq!(
            map_capture("text.uri"),
            Some(StandardToken::MarkupLink.id())
        );
        assert_eq!(
            map_capture("text.reference"),
            Some(StandardToken::MarkupLink.id())
        );
    }

    #[test]
    fn markup_convention_names_map_directly_and_by_fallback() {
        assert_eq!(
            map_capture("markup.heading"),
            Some(StandardToken::MarkupHeading.id())
        );
        // Grammars that qualify the scope still land on the base class.
        assert_eq!(
            map_capture("markup.heading.1.markdown"),
            Some(StandardToken::MarkupHeading.id())
        );
        assert_eq!(
            map_capture("markup.link.url"),
            Some(StandardToken::MarkupLink.id())
        );
    }

    #[test]
    fn doc_comments_outrank_plain_comments() {
        // tree-sitter-rust emits `@comment.documentation` for `///` and `//!`.
        assert_eq!(
            map_capture("comment.documentation"),
            Some(StandardToken::CommentDoc.id())
        );
        assert_eq!(
            map_capture("comment.doc"),
            Some(StandardToken::CommentDoc.id())
        );
        // An ordinary comment still resolves to the plain class...
        assert_eq!(map_capture("comment"), Some(TokenId::COMMENT));
        // ...as does an unrecognized comment sub-scope, via the dotted fallback.
        assert_eq!(map_capture("comment.line"), Some(TokenId::COMMENT));
    }

    #[test]
    fn unknown_capture_is_none() {
        assert_eq!(map_capture("nonsense.capture.name"), None);
        assert_eq!(map_capture("spell"), None);
        // tree-sitter-md tags fence content `@none` — deliberately unhighlighted, so
        // the injected language's own captures show through.
        assert_eq!(map_capture("none"), None);
    }
}
