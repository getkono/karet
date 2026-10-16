//! `editor.*` keys that change how the text is drawn rather than how it is
//! edited: the line-number gutter, indentation guides, visible whitespace and
//! bracket-pair colourisation. The keys are VS Code's, so a setting copied
//! from VS Code means here what it meant there.

use serde::Deserialize;
use serde::Serialize;

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
}
