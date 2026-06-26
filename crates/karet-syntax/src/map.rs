//! Mapping tree-sitter highlight capture names to karet's [`TokenId`] vocabulary.

use karet_core::{StandardToken, TokenId};

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
        match n.rfind('.') {
            Some(i) => n = &n[..i],
            None => return None,
        }
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
    fn unknown_capture_is_none() {
        assert_eq!(map_capture("nonsense.capture.name"), None);
        assert_eq!(map_capture("spell"), None);
    }
}
