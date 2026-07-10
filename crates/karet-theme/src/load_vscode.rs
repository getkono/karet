//! VS Code JSON theme loading: overlay a theme file onto the built-in dark theme.

use std::collections::HashMap;

use karet_core::StandardToken;
use karet_core::ThemeRole;
use serde::Deserialize;

use crate::Emphasis;
use crate::Rgba;
use crate::Theme;
use crate::ThemeError;
use crate::is_dark_color;

#[derive(Deserialize)]
struct Root {
    #[serde(default)]
    colors: HashMap<String, String>,
    #[serde(default, rename = "tokenColors")]
    token_colors: Vec<TokenColorEntry>,
}

#[derive(Deserialize)]
struct TokenColorEntry {
    #[serde(default)]
    scope: Option<StringOrVec>,
    #[serde(default)]
    settings: Settings,
}

#[derive(Deserialize, Default)]
struct Settings {
    #[serde(default)]
    foreground: Option<String>,
    /// A space-separated list, e.g. `"bold italic"` / `"underline"` / `""`.
    #[serde(default, rename = "fontStyle")]
    font_style: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

/// VS Code editor color keys → karet [`ThemeRole`]s.
const ROLE_KEYS: &[(&str, ThemeRole)] = &[
    ("editor.background", ThemeRole::Background),
    ("editor.foreground", ThemeRole::Foreground),
    ("editor.selectionBackground", ThemeRole::Selection),
    ("editorCursor.foreground", ThemeRole::Cursor),
    ("editor.lineHighlightBackground", ThemeRole::CursorLine),
    ("editorLineNumber.foreground", ThemeRole::LineNumber),
    (
        "editorLineNumber.activeForeground",
        ThemeRole::LineNumberActive,
    ),
    ("editorBracketMatch.background", ThemeRole::MatchingBracket),
    ("diffEditor.insertedTextBackground", ThemeRole::DiffAdded),
    ("diffEditor.removedTextBackground", ThemeRole::DiffRemoved),
    ("statusBar.background", ThemeRole::StatusBarBackground),
    ("statusBar.foreground", ThemeRole::StatusBarForeground),
    ("list.hoverBackground", ThemeRole::HoverHighlight),
    ("list.activeSelectionBackground", ThemeRole::ActiveEditorRow),
];

/// Load a VS Code JSON theme, falling back to the built-in dark theme for any key
/// the file doesn't specify.
pub(crate) fn load(json: &str) -> Result<Theme, ThemeError> {
    let root: Root = serde_json::from_str(json).map_err(|_| ThemeError::Parse)?;
    let mut theme = Theme::dark();

    for (key, role) in ROLE_KEYS {
        if let Some(color) = root.colors.get(*key).and_then(|h| Rgba::from_hex(h)) {
            theme.roles[*role as usize] = color;
        }
    }

    for entry in &root.token_colors {
        let fg = entry
            .settings
            .foreground
            .as_deref()
            .and_then(Rgba::from_hex);
        let em = entry.settings.font_style.as_deref().map(parse_font_style);
        // An entry may set only a color, only a font style, or both.
        if fg.is_none() && em.is_none() {
            continue;
        }
        let scopes = match &entry.scope {
            Some(StringOrVec::One(s)) => split_scopes(s),
            Some(StringOrVec::Many(v)) => v.iter().flat_map(|s| split_scopes(s)).collect(),
            None => Vec::new(),
        };
        for scope in scopes {
            if let Some(tok) = scope_to_token(scope.trim()) {
                let i = tok.id().0 as usize;
                if let Some(fg) = fg {
                    theme.tokens[i] = fg;
                }
                if let Some(em) = em {
                    theme.emphasis[i] = em;
                }
            }
        }
    }

    // Foreground/dark are derived from the (possibly overridden) editor colors.
    theme.fallback_fg = theme.role(ThemeRole::Foreground);
    theme.dark = is_dark_color(theme.role(ThemeRole::Background));
    Ok(theme)
}

/// A single `scope` string may be a comma-separated list of selectors.
fn split_scopes(s: &str) -> Vec<String> {
    s.split(',').map(|p| p.trim().to_string()).collect()
}

/// Parse a VS Code `fontStyle` string (e.g. `"bold italic"`) into an [`Emphasis`].
/// An empty string explicitly clears emphasis, which is why this returns a value
/// rather than an `Option`.
fn parse_font_style(s: &str) -> Emphasis {
    Emphasis {
        bold: s.split_whitespace().any(|w| w == "bold"),
        italic: s.split_whitespace().any(|w| w == "italic"),
        strikethrough: s.split_whitespace().any(|w| w == "strikethrough"),
    }
}

/// Map a TextMate scope selector to a [`StandardToken`], most-specific first.
fn scope_to_token(scope: &str) -> Option<StandardToken> {
    let s = scope;
    let t = if s.starts_with("markup.heading") || s.starts_with("entity.name.section") {
        StandardToken::MarkupHeading
    } else if s.starts_with("markup.bold") || s.starts_with("markup.strong") {
        StandardToken::MarkupBold
    } else if s.starts_with("markup.italic") {
        StandardToken::MarkupItalic
    } else if s.starts_with("markup.underline.link") || s.starts_with("markup.link") {
        StandardToken::MarkupLink
    } else if s.starts_with("markup.inline.raw")
        || s.starts_with("markup.raw")
        || s.starts_with("markup.fenced_code")
    {
        StandardToken::MarkupRaw
    } else if s.starts_with("markup.quote") {
        StandardToken::MarkupQuote
    } else if s.starts_with("markup.list") {
        StandardToken::MarkupListMarker
    // TextMate grammars spell struck text either way; `markup.deleted` is also a diff
    // scope, but no karet token claims that, so routing it here loses nothing.
    } else if s.starts_with("markup.strikethrough") || s.starts_with("markup.deleted") {
        StandardToken::MarkupStrikethrough
    // Doc comments must be tested before the general `comment` prefix.
    } else if s.starts_with("comment") && s.contains("documentation") {
        StandardToken::CommentDoc
    } else if s.starts_with("comment") {
        StandardToken::Comment
    } else if s.starts_with("string") {
        StandardToken::String
    } else if s.starts_with("constant.numeric") {
        StandardToken::Number
    } else if s.starts_with("constant.language") {
        StandardToken::Boolean
    } else if s.starts_with("constant") {
        StandardToken::Constant
    } else if s.starts_with("keyword.operator") {
        StandardToken::Operator
    } else if s.starts_with("keyword.control") {
        StandardToken::KeywordControl
    } else if s.starts_with("keyword") || s.starts_with("storage") {
        StandardToken::Keyword
    } else if s.starts_with("entity.name.function") || s.starts_with("support.function") {
        StandardToken::Function
    } else if s.starts_with("entity.name.type")
        || s.starts_with("entity.name.class")
        || s.starts_with("support.type")
        || s.starts_with("support.class")
    {
        StandardToken::Type
    } else if s.starts_with("variable.parameter") {
        StandardToken::Parameter
    } else if s.starts_with("variable") {
        StandardToken::Variable
    } else if s.starts_with("entity.name.tag") {
        StandardToken::Tag
    } else if s.starts_with("entity.other.attribute-name") {
        StandardToken::Attribute
    } else {
        return None;
    };
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_colors_and_token_scopes() -> Result<(), ThemeError> {
        let json = r##"{
            "colors": {
                "editor.background": "#000000",
                "editor.foreground": "#ffffff"
            },
            "tokenColors": [
                { "scope": "keyword.control", "settings": { "foreground": "#ff0000" } },
                { "scope": ["string", "comment"], "settings": { "foreground": "#00ff00" } }
            ]
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(t.role(ThemeRole::Background), Rgba::rgb(0, 0, 0));
        assert_eq!(t.role(ThemeRole::Foreground), Rgba::rgb(255, 255, 255));
        assert_eq!(
            t.color(StandardToken::KeywordControl.id()),
            Rgba::rgb(255, 0, 0)
        );
        assert_eq!(t.color(StandardToken::String.id()), Rgba::rgb(0, 255, 0));
        assert_eq!(t.color(StandardToken::Comment.id()), Rgba::rgb(0, 255, 0));
        // Background is dark → theme reports dark.
        assert!(t.is_dark());
        Ok(())
    }

    #[test]
    fn markup_and_doc_comment_scopes_map() {
        assert_eq!(
            scope_to_token("markup.heading.1.markdown"),
            Some(StandardToken::MarkupHeading)
        );
        assert_eq!(
            scope_to_token("markup.inline.raw"),
            Some(StandardToken::MarkupRaw)
        );
        assert_eq!(
            scope_to_token("markup.underline.link"),
            Some(StandardToken::MarkupLink)
        );
        // A doc comment must not be swallowed by the general `comment` prefix.
        assert_eq!(
            scope_to_token("comment.block.documentation"),
            Some(StandardToken::CommentDoc)
        );
        assert_eq!(scope_to_token("comment.line"), Some(StandardToken::Comment));
    }

    #[test]
    fn font_style_overlays_emphasis() -> Result<(), ThemeError> {
        let json = r##"{
            "tokenColors": [
                { "scope": "markup.italic", "settings": { "fontStyle": "italic" } },
                { "scope": "markup.heading", "settings": { "foreground": "#ff0000", "fontStyle": "bold italic" } },
                { "scope": "comment", "settings": { "fontStyle": "" } }
            ]
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(
            t.emphasis(StandardToken::MarkupItalic.id()),
            Emphasis {
                bold: false,
                italic: true,
                strikethrough: false
            }
        );
        // An entry may carry both a color and a font style.
        assert_eq!(
            t.color(StandardToken::MarkupHeading.id()),
            Rgba::rgb(255, 0, 0)
        );
        assert_eq!(
            t.emphasis(StandardToken::MarkupHeading.id()),
            Emphasis {
                bold: true,
                italic: true,
                strikethrough: false
            }
        );
        // An explicit empty fontStyle clears the built-in emphasis.
        assert_eq!(t.emphasis(StandardToken::Comment.id()), Emphasis::default());
        Ok(())
    }

    #[test]
    fn font_style_only_entry_preserves_default_color() -> Result<(), ThemeError> {
        // The entry sets no foreground, so the built-in dark color must survive.
        let json = r##"{
            "tokenColors": [
                { "scope": "markup.quote", "settings": { "fontStyle": "bold" } }
            ]
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(
            t.color(StandardToken::MarkupQuote.id()),
            Theme::dark().color(StandardToken::MarkupQuote.id())
        );
        assert!(t.emphasis(StandardToken::MarkupQuote.id()).bold);
        Ok(())
    }

    #[test]
    fn malformed_json_errors() {
        assert!(matches!(
            Theme::load_vscode("{ not json"),
            Err(ThemeError::Parse)
        ));
    }

    #[test]
    fn light_background_is_not_dark() -> Result<(), ThemeError> {
        let json = r##"{ "colors": { "editor.background": "#ffffff" } }"##;
        assert!(!Theme::load_vscode(json)?.is_dark());
        Ok(())
    }
}
