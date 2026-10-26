//! `editor.keymap`: the editing model of a code tab — modeless, or one of
//! the modal presets the app's keymap implements.

use serde::Deserialize;
use serde::Serialize;

/// `editor.keymap` — which editing model a code tab uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum KeymapPreset {
    /// Modeless editing: a printable key types at the caret.
    #[default]
    Default,
    /// Vim's modal editing: Normal, Insert and Visual modes, operators over
    /// motions and text objects, registers, macros and `.` repeat.
    Vim,
    /// Helix's selection-first modal editing: a motion selects, then an
    /// action applies to the selection.
    Helix,
}

impl KeymapPreset {
    /// Whether this preset adds a modal Normal/Insert layer.
    #[must_use]
    pub fn is_modal(self) -> bool {
        self != Self::Default
    }
}

#[cfg(test)]
mod tests {
    use super::super::Editor;
    use super::*;

    #[test]
    fn keymap_defaults_to_modeless_and_parses_each_preset() {
        let parse = |json: &str| serde_json::from_str::<Editor>(json).map(|editor| editor.keymap);
        assert_eq!(parse("{}").ok(), Some(KeymapPreset::Default));
        assert!(!KeymapPreset::Default.is_modal());
        assert_eq!(
            parse(r#"{ "keymap": "vim" }"#).ok(),
            Some(KeymapPreset::Vim)
        );
        assert_eq!(
            parse(r#"{ "keymap": "helix" }"#).ok(),
            Some(KeymapPreset::Helix)
        );
        assert!(KeymapPreset::Vim.is_modal() && KeymapPreset::Helix.is_modal());
        // A preset karet does not ship is an invalid `editor` section, not a
        // silent fall-back to the default.
        assert!(parse(r#"{ "keymap": "emacs" }"#).is_err());
    }
}
