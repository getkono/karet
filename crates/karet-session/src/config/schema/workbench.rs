//! `workbench.*` keys: the UI shell's appearance and what it shows at startup.

use serde::Deserialize;
use serde::Serialize;

/// `workbench.*` — UI shell appearance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Workbench {
    /// Colour theme: the built-in name `"dark"`, or a path to a VS Code
    /// `.json` theme file.
    pub color_theme: String,
    /// Icon glyph set for the file tree and activity bar.
    pub icon_style: IconStyleSetting,
    /// Which sidebar panel is shown at startup (`none` starts collapsed).
    pub startup_panel: StartupPanel,
    /// Reopen a workspace's tabs, splits, and cursors as the last clean exit
    /// left them, when karet starts on it without a file to open.
    pub restore_layout: bool,
}

impl Default for Workbench {
    fn default() -> Self {
        Self {
            color_theme: "dark".to_string(),
            icon_style: IconStyleSetting::NerdFont,
            startup_panel: StartupPanel::Explorer,
            restore_layout: true,
        }
    }
}

/// Icon glyph set — the camelCase settings-file spelling of
/// [`karet_filetype::IconStyle`].
///
/// This wire enum exists for the config format alone (camelCase JSONC values +
/// the generated JSON Schema, which `karet-filetype`'s dependency-free charter
/// cannot provide); the [`From`] conversion below is the single translation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum IconStyleSetting {
    /// Rich Nerd Font glyphs (needs a patched font).
    #[default]
    NerdFont,
    /// 1-cell Unicode geometric glyphs.
    Unicode,
    /// Plain ASCII (maximally portable).
    Ascii,
}

impl From<IconStyleSetting> for karet_filetype::IconStyle {
    fn from(setting: IconStyleSetting) -> Self {
        match setting {
            IconStyleSetting::NerdFont => Self::NerdFont,
            IconStyleSetting::Unicode => Self::Unicode,
            IconStyleSetting::Ascii => Self::Ascii,
        }
    }
}

/// Which sidebar panel is shown at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum StartupPanel {
    /// The file explorer.
    #[default]
    Explorer,
    /// The search panel.
    Search,
    /// The source-control panel.
    SourceControl,
    /// Start with the sidebar collapsed.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_restore_defaults_on_and_can_be_switched_off() {
        assert!(Workbench::default().restore_layout);
        let off = serde_json::from_str::<Workbench>(r#"{ "restoreLayout": false }"#);
        assert!(off.is_ok_and(|workbench| !workbench.restore_layout));
    }
}
