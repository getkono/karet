//! `editor.*` keys that change how the text is drawn rather than how it is
//! edited: the line-number gutter, indentation guides, visible whitespace,
//! bracket-pair colourisation and soft wrapping. The keys are VS Code's, so a
//! setting copied from VS Code means here what it meant there.

#[cfg(feature = "schema")]
use std::borrow::Cow;
use std::fmt;

#[cfg(feature = "schema")]
use schemars::Schema;
#[cfg(feature = "schema")]
use schemars::SchemaGenerator;
use serde::Deserialize;
use serde::Serialize;

/// How long lines are laid out (`editor.wordWrap`), in VS Code's spellings;
/// `true` and `false` are read as `on` and `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WordWrap {
    /// Long lines run on past the viewport and scroll horizontally.
    Off,
    /// Long lines wrap at the viewport.
    On,
    /// Long lines wrap at `editor.wordWrapColumn`. A soft-wrapped view has no
    /// horizontal scroll, so a viewport narrower than the column wraps at the
    /// viewport rather than clipping the text: in a terminal this reads the
    /// same as `bounded`.
    WordWrapColumn,
    /// Long lines wrap at the narrower of the viewport and
    /// `editor.wordWrapColumn`.
    Bounded,
}

impl WordWrap {
    /// The spellings a string value may take, in schema order.
    const NAMES: &'static [&'static str] = &["off", "on", "wordWrapColumn", "bounded"];

    /// Whether long lines soft-wrap (every mode but [`Off`](Self::Off)).
    #[must_use]
    pub const fn wraps(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether wrapping is capped at `editor.wordWrapColumn`.
    #[must_use]
    pub const fn uses_column(self) -> bool {
        matches!(self, Self::WordWrapColumn | Self::Bounded)
    }
}

impl<'de> Deserialize<'de> for WordWrap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads a boolean or one of [`WordWrap::NAMES`].
        struct Mode;

        impl serde::de::Visitor<'_> for Mode {
            type Value = WordWrap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#"a boolean or one of "off", "on", "wordWrapColumn", "bounded""#)
            }

            fn visit_bool<E: serde::de::Error>(self, on: bool) -> Result<WordWrap, E> {
                Ok(if on { WordWrap::On } else { WordWrap::Off })
            }

            fn visit_str<E: serde::de::Error>(self, mode: &str) -> Result<WordWrap, E> {
                match mode {
                    "off" => Ok(WordWrap::Off),
                    "on" => Ok(WordWrap::On),
                    "wordWrapColumn" => Ok(WordWrap::WordWrapColumn),
                    "bounded" => Ok(WordWrap::Bounded),
                    other => Err(E::unknown_variant(other, WordWrap::NAMES)),
                }
            }
        }

        deserializer.deserialize_any(Mode)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for WordWrap {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("WordWrap")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "description": "How long lines are laid out: `off` scrolls horizontally, `on` wraps at the viewport, `wordWrapColumn` and `bounded` wrap at `wordWrapColumn` (or the viewport, when narrower). `true` and `false` mean `on` and `off`.",
            "anyOf": [
                { "type": "boolean" },
                { "type": "string", "enum": WordWrap::NAMES }
            ]
        })
    }
}

/// How the continuation rows of a soft-wrapped line are indented
/// (`editor.wrappingIndent`). Capped at half the wrap width, so a deeply
/// indented line in a narrow pane keeps room for its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum WrappingIndent {
    /// Continuation rows start at the left edge of the text.
    None,
    /// Continuation rows line up with the line's own indentation, as in
    /// VS Code.
    #[default]
    Same,
    /// Continuation rows sit one indent level (`tabSize`) deeper than the
    /// line.
    Indent,
}

/// How the line-number gutter is numbered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum LineNumbers {
    /// Absolute line numbers.
    #[default]
    On,
    /// No numbers; a 2-cell gutter keeps the git, diagnostic and fold markers.
    Off,
    /// Numbers relative to the caret line (the caret line shows its absolute number).
    Relative,
}

/// `editor.guides.*` — vertical guides at each indentation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Guides {
    /// Draw a guide at each indentation level, every `tabSize` columns of a
    /// line's leading whitespace.
    pub indentation: bool,
    /// Draw the guide of the block holding the caret in a brighter colour.
    pub highlight_active_indentation: bool,
}

impl Default for Guides {
    /// Both on, as in VS Code.
    fn default() -> Self {
        Self {
            indentation: true,
            highlight_active_indentation: true,
        }
    }
}

/// Which whitespace characters are drawn visibly (`editor.renderWhitespace`):
/// a space as `·`, a tab as `→`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum RenderWhitespace {
    /// None: whitespace stays blank.
    #[default]
    None,
    /// Every whitespace character except a single space between words.
    Boundary,
    /// Only whitespace inside a selection.
    Selection,
    /// Only whitespace at the end of a line.
    Trailing,
    /// Every whitespace character.
    All,
}

/// `editor.bracketPairColorization.*` — colouring brackets by nesting depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BracketPairColorization {
    /// Colour each `()`, `[]` and `{}` pair by its nesting depth, cycling
    /// through three of the theme's colours. Off by default: the colours
    /// compete with syntax highlighting for attention.
    pub enabled: bool,
}

/// `editor.semanticHighlighting.*` — the language server's semantic tokens
/// laid over the tree-sitter highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticHighlighting {
    /// Ask the language's semantic-token server (its primary, unless
    /// `lsp.languages.<id>.semanticTokens` names another) for semantic tokens
    /// and recolour what it classifies -- macros, mutable bindings,
    /// unresolved names -- through the theme's `semanticTokenColors`.
    /// Tree-sitter stays the base layer: comments and strings keep its
    /// colours, and a server that offers no semantic tokens changes nothing.
    pub enabled: bool,
}

impl Default for SemanticHighlighting {
    /// On, as VS Code's built-in themes have it.
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_keys_parse_from_vs_code_spellings_and_default_sanely() {
        // A parse failure falls back to the defaults, which every assertion
        // below contradicts.
        let editor: crate::config::schema::Editor = serde_json::from_str(
            r#"{
                "guides": { "indentation": false, "highlightActiveIndentation": false },
                "renderWhitespace": "trailing",
                "bracketPairColorization": { "enabled": true }
            }"#,
        )
        .unwrap_or_default();
        assert_eq!(
            editor.guides,
            Guides {
                indentation: false,
                highlight_active_indentation: false,
            }
        );
        assert_eq!(editor.render_whitespace, RenderWhitespace::Trailing);
        assert!(editor.bracket_pair_colorization.enabled);

        let defaults = crate::config::schema::Editor::default();
        assert_eq!(defaults.guides, Guides::default());
        assert!(defaults.guides.indentation && defaults.guides.highlight_active_indentation);
        assert_eq!(defaults.render_whitespace, RenderWhitespace::None);
        assert!(!defaults.bracket_pair_colorization.enabled);

        for mode in ["none", "boundary", "selection", "trailing", "all"] {
            let parsed: Result<RenderWhitespace, _> = serde_json::from_str(&format!("\"{mode}\""));
            assert!(parsed.is_ok(), "{mode}");
        }
        let unknown: Result<Guides, _> = serde_json::from_str(r#"{ "bracketPairs": true }"#);
        assert!(unknown.is_err(), "unknown keys are rejected");
    }

    #[test]
    fn word_wrap_accepts_auto_and_boolean_overrides() {
        use crate::config::schema::Editor;
        let automatic: Editor = serde_json::from_str(r#"{ "wordWrap": null }"#).unwrap_or_default();
        let wrapped: Editor = serde_json::from_str(r#"{ "wordWrap": true }"#).unwrap_or_default();
        let overflow: Editor = serde_json::from_str(r#"{ "wordWrap": false }"#).unwrap_or_default();
        assert_eq!(automatic.word_wrap, None);
        assert_eq!(wrapped.word_wrap, Some(WordWrap::On));
        assert_eq!(overflow.word_wrap, Some(WordWrap::Off));
    }

    #[test]
    fn word_wrap_reads_every_vs_code_mode_and_refuses_others() {
        for (text, mode) in [
            ("off", WordWrap::Off),
            ("on", WordWrap::On),
            ("wordWrapColumn", WordWrap::WordWrapColumn),
            ("bounded", WordWrap::Bounded),
        ] {
            let parsed: Result<WordWrap, _> = serde_json::from_str(&format!("\"{text}\""));
            assert_eq!(parsed.ok(), Some(mode), "{text}");
            // And a mode written back reads as itself.
            let written = serde_json::to_string(&mode).unwrap_or_default();
            assert_eq!(written, format!("\"{text}\""));
        }
        for refused in [r#""wrap""#, r#""On""#, "1", "[]"] {
            let parsed: Result<WordWrap, _> = serde_json::from_str(refused);
            assert!(parsed.is_err(), "{refused} parsed");
        }
        assert!(WordWrap::Bounded.wraps() && WordWrap::WordWrapColumn.wraps());
        assert!(WordWrap::On.wraps() && !WordWrap::Off.wraps());
        assert!(WordWrap::Bounded.uses_column() && WordWrap::WordWrapColumn.uses_column());
        assert!(!WordWrap::On.uses_column() && !WordWrap::Off.uses_column());
    }

    #[test]
    fn the_wrap_column_applies_only_to_the_column_modes() {
        let settings: crate::config::schema::Editor = serde_json::from_str(
            r#"{
                "wordWrap": "bounded",
                "wordWrapColumn": 72,
                "wrappingIndent": "indent",
                "[markdown]": { "wordWrap": "on", "wrappingIndent": "none" },
                "[python]": { "wordWrapColumn": 100 },
                "[rust]": { "wordWrap": null }
            }"#,
        )
        .unwrap_or_default();
        let global = settings.for_language(None);
        assert_eq!(global.word_wrap_mode(), Some(WordWrap::Bounded));
        assert_eq!(global.word_wrap(), Some(true));
        assert_eq!(global.wrap_column(), Some(72));
        assert_eq!(global.wrapping_indent(), WrappingIndent::Indent);
        let markdown = settings.for_language(Some("markdown"));
        assert_eq!(markdown.wrap_column(), None, "`on` wraps at the viewport");
        assert_eq!(markdown.wrapping_indent(), WrappingIndent::None);
        let python = settings.for_language(Some("python"));
        assert_eq!(python.wrap_column(), Some(100));
        let rust = settings.for_language(Some("rust"));
        assert_eq!(rust.word_wrap(), None, "`null` restores the file type's");
        assert_eq!(rust.wrap_column(), None);
    }

    #[test]
    fn wrapping_defaults_follow_vs_code() {
        let defaults = crate::config::schema::Editor::default();
        assert_eq!(defaults.word_wrap_column, 80);
        assert_eq!(defaults.wrapping_indent, WrappingIndent::Same);
        for (text, indent) in [
            ("none", WrappingIndent::None),
            ("same", WrappingIndent::Same),
            ("indent", WrappingIndent::Indent),
        ] {
            let parsed: Result<WrappingIndent, _> = serde_json::from_str(&format!("\"{text}\""));
            assert_eq!(parsed.ok(), Some(indent), "{text}");
        }
    }

    /// On unless switched off, and a running session reads the switch live
    /// (see `Session::apply_config_report`).
    #[test]
    fn semantic_highlighting_defaults_on_and_parses_vs_code_spelling() {
        use crate::config::schema::Editor;
        assert!(Editor::default().semantic_highlighting.enabled);
        let off: Editor =
            serde_json::from_str(r#"{ "semanticHighlighting": { "enabled": false } }"#)
                .unwrap_or_default();
        assert!(!off.semantic_highlighting.enabled);
        let unknown: Result<SemanticHighlighting, _> =
            serde_json::from_str(r#"{ "enabled": "configuredByTheme" }"#);
        assert!(unknown.is_err(), "only a boolean is accepted");
    }
}
