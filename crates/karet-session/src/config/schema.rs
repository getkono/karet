//! The typed `Settings` tree: the schema the JSONC config is verified against.
//!
//! Every section carries `#[serde(default, deny_unknown_fields, rename_all =
//! "camelCase")]`: missing fields fall back to the section's [`Default`] (the sane
//! defaults), unknown keys are rejected (so typos surface as diagnostics), and the
//! on-disk keys read like VS Code / Zed (`tabSize`, `formatOnSave`, …). The whole
//! tree also derives [`schemars::JsonSchema`] so the external `settings.schema.json`
//! is generated from this one source of truth.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// The full, validated karet configuration. Load it with
/// [`crate::config::load`]; the sane baseline is [`Settings::default`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Text-editing behaviour (indentation, gutters, on-save fixups).
    pub editor: Editor,
    /// File handling (auto-save, encoding, line endings, exclusions).
    pub files: Files,
    /// UI shell appearance (theme, icons, startup panel).
    pub workbench: Workbench,
    /// Workspace search behaviour.
    pub search: Search,
    /// Spell-checking of comments and strings.
    pub spellcheck: Spellcheck,
    /// Source-control integration.
    pub git: Git,
}

/// `editor.*` — text-editing behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Editor {
    /// Number of columns a tab renders as / spaces inserted for one indent level.
    pub tab_size: u8,
    /// Insert spaces instead of a hard tab when indenting.
    pub insert_spaces: bool,
    /// How the line-number gutter is numbered.
    pub line_numbers: LineNumbers,
    /// Highlight the line the caret is on.
    pub cursor_line: bool,
    /// Keep at least this many lines visible above and below the caret.
    pub scroll_off: u16,
    /// Columns to draw vertical rulers at (empty = none).
    pub rulers: Vec<u16>,
    /// Soft-wrap long lines instead of scrolling horizontally.
    pub word_wrap: bool,
    /// Strip trailing whitespace from each line on save.
    pub trim_trailing_whitespace: bool,
    /// Ensure the file ends with a single trailing newline on save.
    pub insert_final_newline: bool,
    /// Run the configured formatter on save.
    pub format_on_save: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            line_numbers: LineNumbers::On,
            cursor_line: true,
            scroll_off: 3,
            rulers: Vec::new(),
            word_wrap: false,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            format_on_save: false,
        }
    }
}

/// How the line-number gutter is numbered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LineNumbers {
    /// Absolute line numbers.
    #[default]
    On,
    /// No line-number gutter.
    Off,
    /// Numbers relative to the caret line (the caret line shows its absolute number).
    Relative,
}

/// `files.*` — file handling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Files {
    /// When to write dirty buffers back to disk automatically.
    pub auto_save: AutoSave,
    /// Delay in milliseconds before `afterDelay` auto-save fires.
    pub auto_save_delay: u64,
    /// Default text encoding label (informational; karet edits UTF-8).
    pub encoding: String,
    /// Line-ending style used when saving.
    pub eol: Eol,
    /// Glob patterns hidden from the file explorer.
    pub exclude: Vec<String>,
    /// Glob patterns excluded from the filesystem watcher.
    pub watcher_exclude: Vec<String>,
    /// Keep crash-recovery backups (swap files) of unsaved buffers.
    pub backup: bool,
    /// How long a buffer stays dirty (milliseconds) before its swap is written.
    pub backup_interval: u64,
    /// Prompt to save unsaved changes when quitting (rather than discarding them).
    pub confirm_on_exit: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            auto_save: AutoSave::Off,
            auto_save_delay: 1000,
            encoding: "utf-8".to_string(),
            eol: Eol::Auto,
            exclude: Vec::new(),
            watcher_exclude: Vec::new(),
            backup: true,
            backup_interval: 30_000,
            confirm_on_exit: true,
        }
    }
}

/// When dirty buffers are written back to disk automatically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AutoSave {
    /// Never auto-save; the user saves explicitly.
    #[default]
    Off,
    /// Save after `autoSaveDelay` milliseconds of inactivity.
    AfterDelay,
    /// Save when the editor loses focus / the active document changes.
    OnFocusChange,
}

/// Line-ending style used when saving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Eol {
    /// Preserve the file's existing endings (LF for new files).
    #[default]
    Auto,
    /// Line feed (`\n`).
    Lf,
    /// Carriage return + line feed (`\r\n`).
    Crlf,
}

/// `workbench.*` — UI shell appearance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Workbench {
    /// Colour theme: the built-in name `"dark"`, or a path to a `.tmTheme` /
    /// VS Code `.json` theme file.
    pub color_theme: String,
    /// Icon glyph set for the file tree and activity bar.
    pub icon_style: IconStyleSetting,
    /// Which sidebar panel is shown at startup (`none` starts collapsed).
    pub startup_panel: StartupPanel,
}

impl Default for Workbench {
    fn default() -> Self {
        Self {
            color_theme: "dark".to_string(),
            icon_style: IconStyleSetting::NerdFont,
            startup_panel: StartupPanel::Explorer,
        }
    }
}

/// Icon glyph set (mirrors `karet_filetype::IconStyle`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// Which sidebar panel is shown at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// `search.*` — workspace search behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Search {
    /// Glob patterns excluded from workspace search.
    pub exclude: Vec<String>,
    /// Honour `.gitignore` / `.ignore` files when searching.
    pub use_ignore_files: bool,
    /// Case-insensitive unless the query contains an uppercase letter.
    pub smart_case: bool,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            use_ignore_files: true,
            smart_case: true,
        }
    }
}

/// `spellcheck.*` — spell-checking of comments and strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Spellcheck {
    /// Enable spell-checking.
    pub enabled: bool,
    /// Dictionary language (e.g. `"en_US"`).
    pub language: String,
    /// Extra words treated as correctly spelled.
    pub words: Vec<String>,
}

impl Default for Spellcheck {
    fn default() -> Self {
        Self {
            enabled: false,
            language: "en_US".to_string(),
            words: Vec::new(),
        }
    }
}

/// `git.*` — source-control integration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Git {
    /// Show gutter change decorations and file-tree status colouring.
    pub decorations: bool,
    /// Show inline blame for the current line.
    pub blame: bool,
}

impl Default for Git {
    fn default() -> Self {
        Self {
            decorations: true,
            blame: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.editor.tab_size, 4);
        assert!(s.editor.insert_spaces);
        assert_eq!(s.editor.line_numbers, LineNumbers::On);
        assert_eq!(s.files.auto_save, AutoSave::Off);
        assert_eq!(s.workbench.color_theme, "dark");
        assert!(s.search.smart_case);
        assert!(!s.spellcheck.enabled);
        assert!(s.git.decorations);
    }

    #[test]
    fn camel_case_enum_labels_round_trip() {
        // Enum variants serialize to the VS Code / Zed style camelCase strings.
        assert_eq!(
            serde_json::to_string(&AutoSave::AfterDelay).ok().as_deref(),
            Some("\"afterDelay\"")
        );
        assert_eq!(
            serde_json::from_str::<AutoSave>("\"onFocusChange\"").ok(),
            Some(AutoSave::OnFocusChange)
        );
    }
}
