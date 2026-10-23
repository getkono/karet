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
use crate::semantic::SemanticRule;
use crate::semantic::SemanticStyle;

#[derive(Deserialize)]
struct Root {
    #[serde(default)]
    colors: HashMap<String, String>,
    #[serde(default, rename = "tokenColors")]
    token_colors: Vec<TokenColorEntry>,
    /// Selector → a colour string, or a style object. Kept as raw values so
    /// one malformed entry is skipped rather than failing the whole theme.
    #[serde(default, rename = "semanticTokenColors")]
    semantic_token_colors: InFileOrder,
}

/// A JSON object's entries in the order the file wrote them.
///
/// The later of two equally specific `semanticTokenColors` rules wins, so the
/// order is part of the theme. `serde_json::Map` keeps it only when some crate
/// in the build turns on `serde_json/preserve_order`, and is sorted otherwise:
/// reading the entries through a visitor keeps file order whatever features
/// the build unified.
#[derive(Default)]
struct InFileOrder(Vec<(String, serde_json::Value)>);

impl<'de> Deserialize<'de> for InFileOrder {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = InFileOrder;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object of semantic token selectors")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(InFileOrder(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
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
    ("editorInlayHint.foreground", ThemeRole::InlayHint),
    ("editorCodeLens.foreground", ThemeRole::CodeLens),
    ("diffEditor.insertedTextBackground", ThemeRole::DiffAdded),
    ("diffEditor.removedTextBackground", ThemeRole::DiffRemoved),
    ("editorGutter.addedBackground", ThemeRole::GutterAdded),
    ("editorGutter.modifiedBackground", ThemeRole::GutterModified),
    ("editorGutter.deletedBackground", ThemeRole::GutterDeleted),
    ("statusBar.background", ThemeRole::StatusBarBackground),
    ("statusBar.foreground", ThemeRole::StatusBarForeground),
    ("list.hoverBackground", ThemeRole::HoverHighlight),
    ("list.activeSelectionBackground", ThemeRole::ActiveEditorRow),
    ("scrollbarSlider.background", ThemeRole::ScrollbarThumb),
    ("editorOverviewRuler.border", ThemeRole::ScrollbarTrack),
];

/// VS Code find-match keys, composited over `editor.background` at load time.
///
/// Themes conventionally make these translucent (Dark+ ships
/// `#ea5c0055` for the other matches) so the text shows through, and a terminal
/// cell cannot carry alpha: taken as-is, the other matches would paint opaque and
/// saturated, louder than the current match.
const BLENDED_ROLE_KEYS: &[(&str, ThemeRole)] = &[
    ("editor.findMatchBackground", ThemeRole::SearchMatchCurrent),
    (
        "editor.findMatchHighlightBackground",
        ThemeRole::SearchMatch,
    ),
];

/// VS Code's per-kind inlay-hint color keys, each inheriting
/// `editorInlayHint.foreground` when the theme leaves it unset.
const INLAY_KIND_KEYS: &[(&str, ThemeRole)] = &[
    ("editorInlayHint.typeForeground", ThemeRole::InlayHintType),
    (
        "editorInlayHint.parameterForeground",
        ThemeRole::InlayHintParameter,
    ),
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
    // After `ROLE_KEYS`, so the backdrop is the theme's own background.
    let backdrop = theme.role(ThemeRole::Background);
    for (key, role) in BLENDED_ROLE_KEYS {
        if let Some(color) = root.colors.get(*key).and_then(|h| Rgba::from_hex(h)) {
            theme.roles[*role as usize] = composite_over(color, backdrop);
        }
    }
    // VS Code falls back from a per-kind hint color to the general one, so a
    // theme that sets only `editorInlayHint.foreground` colors every kind.
    let general = root
        .colors
        .get("editorInlayHint.foreground")
        .and_then(|h| Rgba::from_hex(h));
    for (key, role) in INLAY_KIND_KEYS {
        let specific = root.colors.get(*key).and_then(|h| Rgba::from_hex(h));
        if let Some(color) = specific.or(general) {
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

    // The built-in colour rules belong to the built-in palette; only the
    // colourless ones carry over, ahead of the theme's own so it can override
    // them.
    theme.semantic = crate::default::portable_semantic_rules();
    theme.semantic.extend(
        root.semantic_token_colors
            .0
            .iter()
            .filter_map(|(selector, value)| SemanticRule::parse(selector, semantic_style(value)?)),
    );

    // Foreground/dark are derived from the (possibly overridden) editor colors.
    theme.fallback_fg = theme.role(ThemeRole::Foreground);
    theme.dark = is_dark_color(theme.role(ThemeRole::Background));
    Ok(theme)
}

/// Alpha-composite `color` over the opaque `backdrop`, giving an opaque color
/// (source-over, rounded to nearest).
fn composite_over(color: Rgba, backdrop: Rgba) -> Rgba {
    let a = u16::from(color.a);
    let mix = |fg: u8, bg: u8| {
        let blended = (u16::from(fg) * a + u16::from(bg) * (255 - a) + 127) / 255;
        // `blended` is a weighted mean of two `u8`s, so it always fits.
        u8::try_from(blended).unwrap_or(u8::MAX)
    };
    Rgba::rgb(
        mix(color.r, backdrop.r),
        mix(color.g, backdrop.g),
        mix(color.b, backdrop.b),
    )
}

/// A single `scope` string may be a comma-separated list of selectors.
fn split_scopes(s: &str) -> Vec<String> {
    s.split(',').map(|p| p.trim().to_string()).collect()
}

/// Parse a VS Code `fontStyle` string (e.g. `"bold italic"`) into an
/// [`Emphasis`](karet_core::Emphasis).
/// An empty string explicitly clears emphasis, which is why this returns a value
/// rather than an `Option`.
fn parse_font_style(s: &str) -> Emphasis {
    Emphasis {
        bold: s.split_whitespace().any(|w| w == "bold"),
        italic: s.split_whitespace().any(|w| w == "italic"),
        underline: s.split_whitespace().any(|w| w == "underline"),
        strikethrough: s.split_whitespace().any(|w| w == "strikethrough"),
    }
}

/// One `semanticTokenColors` value: a colour string, or an object with any of
/// `foreground`, `fontStyle`, `bold`, `italic`, `underline` and
/// `strikethrough`. A `fontStyle` sets all four flags (`""` clears them); an
/// explicit flag overrides it. `None` for a value that sets nothing usable.
fn semantic_style(value: &serde_json::Value) -> Option<SemanticStyle> {
    if let Some(color) = value.as_str() {
        return Rgba::from_hex(color).map(SemanticStyle::fg);
    }
    let object = value.as_object()?;
    let mut style = SemanticStyle {
        fg: object
            .get("foreground")
            .and_then(serde_json::Value::as_str)
            .and_then(Rgba::from_hex),
        ..SemanticStyle::default()
    };
    if let Some(font) = object.get("fontStyle").and_then(serde_json::Value::as_str) {
        let em = parse_font_style(font);
        style.bold = Some(em.bold);
        style.italic = Some(em.italic);
        style.underline = Some(em.underline);
        style.strikethrough = Some(em.strikethrough);
    }
    let flag = |key: &str| object.get(key).and_then(serde_json::Value::as_bool);
    style.bold = flag("bold").or(style.bold);
    style.italic = flag("italic").or(style.italic);
    style.underline = flag("underline").or(style.underline);
    style.strikethrough = flag("strikethrough").or(style.strikethrough);
    (!style.is_empty()).then_some(style)
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
    fn gutter_keys_overlay_the_gutter_roles() -> Result<(), ThemeError> {
        let json = r##"{
            "colors": {
                "editorGutter.addedBackground": "#112233",
                "editorGutter.modifiedBackground": "#445566",
                "editorGutter.deletedBackground": "#778899"
            }
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(t.role(ThemeRole::GutterAdded), Rgba::rgb(0x11, 0x22, 0x33));
        assert_eq!(
            t.role(ThemeRole::GutterModified),
            Rgba::rgb(0x44, 0x55, 0x66)
        );
        assert_eq!(
            t.role(ThemeRole::GutterDeleted),
            Rgba::rgb(0x77, 0x88, 0x99)
        );
        Ok(())
    }

    #[test]
    fn inlay_hint_kind_keys_overlay_their_roles() -> Result<(), ThemeError> {
        let json = r##"{
            "colors": {
                "editorInlayHint.foreground": "#101010",
                "editorInlayHint.typeForeground": "#202020",
                "editorInlayHint.parameterForeground": "#303030"
            }
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(t.role(ThemeRole::InlayHint), Rgba::rgb(0x10, 0x10, 0x10));
        assert_eq!(
            t.role(ThemeRole::InlayHintType),
            Rgba::rgb(0x20, 0x20, 0x20)
        );
        assert_eq!(
            t.role(ThemeRole::InlayHintParameter),
            Rgba::rgb(0x30, 0x30, 0x30)
        );
        Ok(())
    }

    #[test]
    fn the_code_lens_key_colors_the_code_lens_role() -> Result<(), ThemeError> {
        let t = Theme::load_vscode(r##"{"colors":{"editorCodeLens.foreground":"#405060"}}"##)?;
        assert_eq!(t.role(ThemeRole::CodeLens), Rgba::rgb(0x40, 0x50, 0x60));
        Ok(())
    }

    #[test]
    fn find_match_keys_overlay_the_current_and_other_match_roles() -> Result<(), ThemeError> {
        // Dark+'s values: an opaque current match, translucent other matches.
        let json = r##"{
            "colors": {
                "editor.background": "#1e1e1e",
                "editor.findMatchBackground": "#515c6a",
                "editor.findMatchHighlightBackground": "#ea5c0055"
            }
        }"##;
        let t = Theme::load_vscode(json)?;
        // An opaque key is taken as written.
        assert_eq!(
            t.role(ThemeRole::SearchMatchCurrent),
            Rgba::rgb(0x51, 0x5c, 0x6a)
        );
        // A translucent key is composited over the theme's own background
        // (1/3 orange over #1e1e1e), not painted as opaque orange.
        assert_eq!(t.role(ThemeRole::SearchMatch), Rgba::rgb(98, 51, 20));
        Ok(())
    }

    /// The backdrop is the loaded theme's background, whichever order the keys
    /// appear in, and a fully transparent key leaves exactly that background.
    #[test]
    fn find_match_keys_composite_over_the_theme_background() -> Result<(), ThemeError> {
        let json = r##"{
            "colors": {
                "editor.findMatchHighlightBackground": "#ff000000",
                "editor.findMatchBackground": "#ffffff80",
                "editor.background": "#000000"
            }
        }"##;
        let t = Theme::load_vscode(json)?;
        assert_eq!(t.role(ThemeRole::SearchMatch), Rgba::rgb(0, 0, 0));
        assert_eq!(
            t.role(ThemeRole::SearchMatchCurrent),
            Rgba::rgb(128, 128, 128)
        );
        Ok(())
    }

    #[test]
    fn composite_over_bounds() {
        let bg = Rgba::rgb(10, 20, 30);
        let opaque = Rgba::rgb(200, 100, 0);
        assert_eq!(composite_over(opaque, bg), opaque);
        let clear = Rgba { a: 0, ..opaque };
        assert_eq!(composite_over(clear, bg), bg);
        let white = Rgba::rgb(255, 255, 255);
        assert_eq!(composite_over(Rgba { a: 254, ..white }, white), white);
    }

    /// A theme that colors only the general hint key colors every kind, as
    /// VS Code does; one that sets none keeps the built-in per-kind colors.
    #[test]
    fn per_kind_inlay_hint_roles_inherit_the_general_key() -> Result<(), ThemeError> {
        let general =
            Theme::load_vscode(r##"{"colors":{"editorInlayHint.foreground":"#abcdef"}}"##)?;
        let hint = Rgba::rgb(0xab, 0xcd, 0xef);
        assert_eq!(general.role(ThemeRole::InlayHintType), hint);
        assert_eq!(general.role(ThemeRole::InlayHintParameter), hint);
        let none = Theme::load_vscode(r##"{"colors":{}}"##)?;
        let dark = Theme::dark();
        assert_eq!(
            none.role(ThemeRole::InlayHintType),
            dark.role(ThemeRole::InlayHintType)
        );
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

    /// Every arm of the order-sensitive chain, each specific prefix paired with
    /// the general prefix it must be tested ahead of. Falsified by reordering
    /// any pair (e.g. `constant` before `constant.numeric`), by dropping an
    /// alternative spelling (`storage`, `support.class`, `markup.deleted`), or
    /// by widening the fallthrough so an unknown scope maps to something.
    #[test]
    fn every_scope_arm_maps_and_specific_prefixes_win() {
        use StandardToken as S;
        let table: &[(&str, Option<StandardToken>)] = &[
            ("markup.heading.2.markdown", Some(S::MarkupHeading)),
            ("entity.name.section.markdown", Some(S::MarkupHeading)),
            ("markup.bold.markdown", Some(S::MarkupBold)),
            ("markup.strong", Some(S::MarkupBold)),
            ("markup.italic.markdown", Some(S::MarkupItalic)),
            ("markup.underline.link.markdown", Some(S::MarkupLink)),
            ("markup.link", Some(S::MarkupLink)),
            ("markup.inline.raw.string", Some(S::MarkupRaw)),
            ("markup.raw.block", Some(S::MarkupRaw)),
            ("markup.fenced_code.block", Some(S::MarkupRaw)),
            ("markup.quote.markdown", Some(S::MarkupQuote)),
            ("markup.list.numbered", Some(S::MarkupListMarker)),
            ("markup.strikethrough", Some(S::MarkupStrikethrough)),
            ("markup.deleted.diff", Some(S::MarkupStrikethrough)),
            // `markup.underline` alone is not a link.
            ("markup.underline", None),
            ("comment.line.documentation.rust", Some(S::CommentDoc)),
            ("comment.block", Some(S::Comment)),
            // `documentation` outside a comment scope is not a doc comment.
            ("string.documentation", Some(S::String)),
            ("string.quoted.double", Some(S::String)),
            ("constant.numeric.integer", Some(S::Number)),
            ("constant.language.boolean", Some(S::Boolean)),
            ("constant.character.escape", Some(S::Constant)),
            ("keyword.operator.arithmetic", Some(S::Operator)),
            ("keyword.control.flow", Some(S::KeywordControl)),
            ("keyword.other", Some(S::Keyword)),
            ("storage.type.fn", Some(S::Keyword)),
            ("entity.name.function.rust", Some(S::Function)),
            ("support.function.builtin", Some(S::Function)),
            ("entity.name.type.struct", Some(S::Type)),
            ("entity.name.class", Some(S::Type)),
            ("support.type.primitive", Some(S::Type)),
            ("support.class.builtin", Some(S::Type)),
            ("variable.parameter.function", Some(S::Parameter)),
            ("variable.other.readwrite", Some(S::Variable)),
            ("entity.name.tag.html", Some(S::Tag)),
            ("entity.other.attribute-name.html", Some(S::Attribute)),
            ("entity.name.namespace", None),
            ("punctuation.definition", None),
            ("", None),
        ];
        for (scope, want) in table {
            assert_eq!(scope_to_token(scope), *want, "scope {scope:?}");
        }
    }

    /// One `scope` string listing several selectors colors each of them, with
    /// the whitespace around the commas ignored. Falsified by passing the
    /// string through unsplit (`"string, comment"` maps to `String` only).
    #[test]
    fn a_comma_separated_scope_string_colors_every_selector() -> Result<(), ThemeError> {
        let json = r##"{
            "tokenColors": [
                { "scope": "string, comment ,keyword.control", "settings": { "foreground": "#123456" } }
            ]
        }"##;
        let t = Theme::load_vscode(json)?;
        let c = Rgba::rgb(0x12, 0x34, 0x56);
        assert_eq!(t.color(StandardToken::String.id()), c);
        assert_eq!(t.color(StandardToken::Comment.id()), c);
        assert_eq!(t.color(StandardToken::KeywordControl.id()), c);
        // A token no selector named keeps its built-in color.
        assert_eq!(
            t.color(StandardToken::Function.id()),
            Theme::dark().color(StandardToken::Function.id())
        );
        Ok(())
    }

    /// The fallback foreground follows the theme's `editor.foreground`, so a
    /// token id outside the palette paints in the theme's text color rather
    /// than the built-in one. Falsified by deleting the `fallback_fg`
    /// assignment in `load`.
    #[test]
    fn the_fallback_foreground_is_the_theme_foreground() -> Result<(), ThemeError> {
        use karet_core::TokenId;

        let t = Theme::load_vscode(r##"{"colors":{"editor.foreground":"#0a0b0c"}}"##)?;
        let fg = Rgba::rgb(0x0a, 0x0b, 0x0c);
        assert_ne!(Theme::dark().fallback_fg, fg);
        assert_eq!(t.fallback_fg, fg);
        // The highest non-semantic id indexes past the standard palette.
        let unmapped = TokenId(TokenId::SEMANTIC_BASE - 1);
        assert_eq!(t.color(unmapped), fg);
        Ok(())
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
            Emphasis::ITALIC
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
                ..Emphasis::NONE
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

    /// A colour string, a style object, and a `fontStyle` whose explicit flag
    /// overrides it, all resolve; a selector outside the vocabulary or a value
    /// setting nothing is skipped without failing the theme.
    #[test]
    fn semantic_token_colors_overlay_semantic_tokens() -> Result<(), ThemeError> {
        use karet_core::SemanticTokenModifiers as M;
        use karet_core::SemanticTokenType as T;
        use karet_core::TokenId;

        let json = r##"{
            "tokenColors": [
                { "scope": "entity.name.function", "settings": { "foreground": "#0000ff" } }
            ],
            "semanticTokenColors": {
                "variable.readonly": "#ff0000",
                "parameter": { "foreground": "#00ff00", "fontStyle": "italic bold", "bold": false },
                "*.static": { "fontStyle": "underline" },
                "widget": "#123456",
                "function": { "fontStyle": 42 }
            }
        }"##;
        let t = Theme::load_vscode(json)?;
        let readonly = TokenId::semantic(T::Variable, M::READONLY);
        assert_eq!(t.color(readonly), Rgba::rgb(255, 0, 0));
        let parameter = TokenId::semantic(T::Parameter, M::NONE);
        assert_eq!(t.color(parameter), Rgba::rgb(0, 255, 0));
        assert_eq!(t.emphasis(parameter), Emphasis::ITALIC);
        // No rule colours a function: it takes the theme's own function colour.
        let function = TokenId::semantic(T::Function, M::STATIC);
        assert_eq!(t.color(function), Rgba::rgb(0, 0, 255));
        assert_eq!(t.emphasis(function), Emphasis::UNDERLINE);
        Ok(())
    }

    /// Of two equally specific rules the one later in the file wins, whatever
    /// order the selectors sort in. Falsified by reading the entries through
    /// a sorted map (`*.readonly` would sort first and lose to `*.static`),
    /// as `serde_json::Map` is without `preserve_order`.
    #[test]
    fn a_tie_goes_to_the_rule_later_in_the_file() -> Result<(), ThemeError> {
        use karet_core::SemanticTokenModifiers as M;
        use karet_core::SemanticTokenType as T;
        use karet_core::TokenId;

        let json = r##"{"semanticTokenColors": {"*.static": "#ff0000", "*.readonly": "#00ff00"}}"##;
        let t = Theme::load_vscode(json)?;
        let both = TokenId::semantic(T::Variable, M::READONLY | M::STATIC);
        assert_eq!(t.color(both), Rgba::rgb(0, 255, 0));
        Ok(())
    }

    /// The built-in palette's semantic colours do not leak into a loaded
    /// theme, while its colourless rules carry over and can be overridden.
    #[test]
    fn a_loaded_theme_keeps_only_the_colourless_built_in_rules() -> Result<(), ThemeError> {
        use karet_core::SemanticTokenModifiers as M;
        use karet_core::SemanticTokenType as T;
        use karet_core::TokenId;

        let plain = Theme::load_vscode("{}")?;
        let mac = TokenId::semantic(T::Macro, M::NONE);
        assert_eq!(plain.color(mac), plain.color(StandardToken::Function.id()));
        assert_ne!(Theme::dark().color(mac), plain.color(mac));
        let deprecated = TokenId::semantic(T::Function, M::DEPRECATED);
        assert!(plain.emphasis(deprecated).strikethrough);

        let cleared = Theme::load_vscode(
            r#"{"semanticTokenColors":{"*.deprecated":{"strikethrough":false}}}"#,
        )?;
        assert!(!cleared.emphasis(deprecated).strikethrough);
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
