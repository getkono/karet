//! The typed `Settings` tree: the schema the JSONC config is verified against.
//!
//! Every section carries `#[serde(default, deny_unknown_fields, rename_all =
//! "camelCase")]`: missing fields fall back to the section's [`Default`] (the sane
//! defaults), unknown keys are rejected (so typos surface as diagnostics), and the
//! on-disk keys read like VS Code / Zed (`tabSize`, `formatOnSave`, …). The whole
//! tree also derives [`schemars::JsonSchema`] so the external `settings.schema.json`
//! is generated from this one source of truth.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use schemars::Schema;
use schemars::SchemaGenerator;
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
    /// Language-server integration (completions and future language features).
    pub lsp: Lsp,
}

/// `editor.*` — text-editing behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
#[schemars(transform = deny_additional_properties)]
pub struct Editor {
    /// Number of columns a tab renders as / spaces inserted for one indent level.
    pub tab_size: u8,
    /// Insert spaces instead of a hard tab when indenting.
    pub insert_spaces: bool,
    /// How the line-number gutter is numbered.
    pub line_numbers: LineNumbers,
    /// Highlight the line the caret is on.
    pub cursor_line: bool,
    /// Draw the caret with terminal graphics: `null` auto-enables when supported,
    /// `true` requests it and reports incompatibility, `false` disables it.
    pub graphical_cursor: Option<bool>,
    /// Keep at least this many lines visible above and below the caret.
    pub scroll_off: u16,
    /// Columns to draw vertical rulers at (empty = none).
    pub rulers: Vec<u16>,
    /// Override file-type wrapping: `null` uses the file default, `true` wraps,
    /// and `false` uses horizontal overflow.
    pub word_wrap: Option<bool>,
    /// Keep the active semantic block hierarchy pinned above scrolled text.
    pub sticky_scroll: bool,
    /// Strip trailing whitespace from each line on save.
    pub trim_trailing_whitespace: bool,
    /// Ensure the file ends with a single trailing newline on save.
    pub insert_final_newline: bool,
    /// Run the configured formatter on save.
    pub format_on_save: bool,
    /// Distinct highlighting of codetag comment blocks (`TODO:`, `FIXME:`, …).
    pub semantic_comments: SemanticComments,
    /// LSP-powered code completion (the popup).
    pub completion: Completion,
    /// Per-language patches keyed by selectors such as `[rust]`.
    ///
    /// This map is flattened in `setting.jsonc`, so its entries sit beside the
    /// global editor keys rather than under a separate `languageOverrides` key.
    #[serde(flatten)]
    pub language_overrides: BTreeMap<LanguageSelector, EditorOverride>,
}

fn deny_additional_properties(schema: &mut Schema) {
    schema.insert("additionalProperties".to_string(), false.into());
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            line_numbers: LineNumbers::On,
            cursor_line: true,
            graphical_cursor: None,
            scroll_off: 3,
            rulers: Vec::new(),
            word_wrap: None,
            sticky_scroll: true,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            format_on_save: false,
            semantic_comments: SemanticComments::default(),
            completion: Completion::default(),
            language_overrides: BTreeMap::new(),
        }
    }
}

impl Editor {
    /// Resolve this editor configuration for `language`.
    ///
    /// Language names are matched case-insensitively after trimming outer
    /// whitespace. The selected language patch is applied over the already-merged
    /// global editor values, so language specificity wins over config-layer
    /// specificity.
    #[must_use]
    pub fn for_language(&self, language: Option<&str>) -> ResolvedEditor<'_> {
        let override_ = language
            .and_then(LanguageSelector::from_language)
            .and_then(|selector| self.language_overrides.get(&selector));
        ResolvedEditor {
            base: self,
            override_,
        }
    }
}

/// A normalized `[language]` key used by [`Editor::language_overrides`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageSelector(String);

impl LanguageSelector {
    /// Build a selector from a display language or language id.
    #[must_use]
    pub fn from_language(language: &str) -> Option<Self> {
        let language = language.trim();
        (!language.is_empty() && !language.contains(['[', ']']))
            .then(|| Self(language.to_ascii_lowercase()))
    }

    /// Return the normalized language name without surrounding brackets.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0)
    }
}

impl Serialize for LanguageSelector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for LanguageSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let key = String::deserialize(deserializer)?;
        let Some(language) = key.strip_prefix('[').and_then(|k| k.strip_suffix(']')) else {
            return Err(D::Error::custom(format!(
                "unknown editor setting `{key}`; expected a `[language]` selector"
            )));
        };
        Self::from_language(language)
            .ok_or_else(|| D::Error::custom(format!("invalid language selector `{key}`")))
    }
}

impl JsonSchema for LanguageSelector {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("LanguageSelector")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"^\[[^\[\]]+\]$"
        })
    }
}

/// A partial per-language patch for [`Editor`].
///
/// Every field is optional: omitted fields inherit the merged global editor value.
/// Arrays replace the global value when present, and nested objects merge field by
/// field through their own partial patch types.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct EditorOverride {
    /// Override columns per indent level.
    pub tab_size: Option<u8>,
    /// Override spaces-versus-tabs indentation.
    pub insert_spaces: Option<bool>,
    /// Override line-number gutter mode.
    pub line_numbers: Option<LineNumbers>,
    /// Override current-line highlighting.
    pub cursor_line: Option<bool>,
    /// Override graphical-cursor behavior; explicit `null` restores auto mode.
    #[serde(default, skip_serializing_if = "NullableOverride::is_unset")]
    #[schemars(with = "Option<bool>")]
    pub graphical_cursor: NullableOverride<bool>,
    /// Override the caret scroll margin.
    pub scroll_off: Option<u16>,
    /// Replace the global ruler columns.
    pub rulers: Option<Vec<u16>>,
    /// Override wrapping; explicit `null` restores the file-type default.
    #[serde(default, skip_serializing_if = "NullableOverride::is_unset")]
    #[schemars(with = "Option<bool>")]
    pub word_wrap: NullableOverride<bool>,
    /// Override semantic sticky-scroll rendering.
    pub sticky_scroll: Option<bool>,
    /// Override trailing-whitespace trimming.
    pub trim_trailing_whitespace: Option<bool>,
    /// Override final-newline insertion.
    pub insert_final_newline: Option<bool>,
    /// Override format-on-save.
    pub format_on_save: Option<bool>,
    /// Partially override semantic-comment behavior.
    pub semantic_comments: Option<SemanticCommentsOverride>,
    /// Partially override completion behavior.
    pub completion: Option<CompletionOverride>,
}

/// A partial per-language patch for [`Completion`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct CompletionOverride {
    /// Override whether completion is enabled.
    pub enabled: Option<bool>,
    /// Override automatic completion triggering.
    pub auto_trigger: Option<bool>,
}

/// A partial per-language patch for [`SemanticComments`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticCommentsOverride {
    /// Override whether semantic comments are highlighted.
    pub enabled: Option<bool>,
    /// Replace the global semantic-comment tag list.
    pub tags: Option<Vec<String>>,
}

/// An optional override that distinguishes an omitted field from explicit `null`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NullableOverride<T> {
    /// The language patch inherits the global value.
    #[default]
    Unset,
    /// The language patch explicitly supplies a nullable value.
    Set(Option<T>),
}

impl<T> NullableOverride<T> {
    fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }
}

impl<T: Serialize> Serialize for NullableOverride<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Unset | Self::Set(None) => serializer.serialize_none(),
            Self::Set(Some(value)) => serializer.serialize_some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for NullableOverride<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self::Set)
    }
}

/// A zero-copy view of the final editor settings for one language.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedEditor<'a> {
    base: &'a Editor,
    override_: Option<&'a EditorOverride>,
}

impl<'a> ResolvedEditor<'a> {
    /// Final columns per indent level.
    #[must_use]
    pub fn tab_size(self) -> u8 {
        self.override_
            .and_then(|o| o.tab_size)
            .unwrap_or(self.base.tab_size)
    }

    /// Final spaces-versus-tabs indentation setting.
    #[must_use]
    pub fn insert_spaces(self) -> bool {
        self.override_
            .and_then(|o| o.insert_spaces)
            .unwrap_or(self.base.insert_spaces)
    }

    /// Final line-number gutter mode.
    #[must_use]
    pub fn line_numbers(self) -> LineNumbers {
        self.override_
            .and_then(|o| o.line_numbers)
            .unwrap_or(self.base.line_numbers)
    }

    /// Final current-line highlighting setting.
    #[must_use]
    pub fn cursor_line(self) -> bool {
        self.override_
            .and_then(|o| o.cursor_line)
            .unwrap_or(self.base.cursor_line)
    }

    /// Final graphical-cursor setting.
    #[must_use]
    pub fn graphical_cursor(self) -> Option<bool> {
        match self.override_.map(|o| &o.graphical_cursor) {
            Some(NullableOverride::Set(value)) => *value,
            _ => self.base.graphical_cursor,
        }
    }

    /// Final caret scroll margin.
    #[must_use]
    pub fn scroll_off(self) -> u16 {
        self.override_
            .and_then(|o| o.scroll_off)
            .unwrap_or(self.base.scroll_off)
    }

    /// Final ruler columns.
    #[must_use]
    pub fn rulers(self) -> &'a [u16] {
        self.override_
            .and_then(|o| o.rulers.as_deref())
            .unwrap_or(&self.base.rulers)
    }

    /// Final wrapping override.
    #[must_use]
    pub fn word_wrap(self) -> Option<bool> {
        match self.override_.map(|o| &o.word_wrap) {
            Some(NullableOverride::Set(value)) => *value,
            _ => self.base.word_wrap,
        }
    }

    /// Final semantic sticky-scroll setting.
    #[must_use]
    pub fn sticky_scroll(self) -> bool {
        self.override_
            .and_then(|o| o.sticky_scroll)
            .unwrap_or(self.base.sticky_scroll)
    }

    /// Final trailing-whitespace trimming setting.
    #[must_use]
    pub fn trim_trailing_whitespace(self) -> bool {
        self.override_
            .and_then(|o| o.trim_trailing_whitespace)
            .unwrap_or(self.base.trim_trailing_whitespace)
    }

    /// Final final-newline insertion setting.
    #[must_use]
    pub fn insert_final_newline(self) -> bool {
        self.override_
            .and_then(|o| o.insert_final_newline)
            .unwrap_or(self.base.insert_final_newline)
    }

    /// Final format-on-save setting.
    #[must_use]
    pub fn format_on_save(self) -> bool {
        self.override_
            .and_then(|o| o.format_on_save)
            .unwrap_or(self.base.format_on_save)
    }

    /// Final semantic-comment settings.
    #[must_use]
    pub fn semantic_comments(self) -> ResolvedSemanticComments<'a> {
        ResolvedSemanticComments {
            base: &self.base.semantic_comments,
            override_: self.override_.and_then(|o| o.semantic_comments.as_ref()),
        }
    }

    /// Final completion settings.
    #[must_use]
    pub fn completion(self) -> ResolvedCompletion<'a> {
        ResolvedCompletion {
            base: &self.base.completion,
            override_: self.override_.and_then(|o| o.completion.as_ref()),
        }
    }
}

/// A zero-copy view of resolved completion settings.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedCompletion<'a> {
    base: &'a Completion,
    override_: Option<&'a CompletionOverride>,
}

impl ResolvedCompletion<'_> {
    /// Whether completion is enabled.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.override_
            .and_then(|o| o.enabled)
            .unwrap_or(self.base.enabled)
    }

    /// Whether completion triggers automatically.
    #[must_use]
    pub fn auto_trigger(self) -> bool {
        self.override_
            .and_then(|o| o.auto_trigger)
            .unwrap_or(self.base.auto_trigger)
    }
}

/// A zero-copy view of resolved semantic-comment settings.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedSemanticComments<'a> {
    base: &'a SemanticComments,
    override_: Option<&'a SemanticCommentsOverride>,
}

impl<'a> ResolvedSemanticComments<'a> {
    /// Whether semantic-comment highlighting is enabled.
    #[must_use]
    pub fn enabled(self) -> bool {
        self.override_
            .and_then(|o| o.enabled)
            .unwrap_or(self.base.enabled)
    }

    /// The semantic-comment tags to recognize.
    #[must_use]
    pub fn tags(self) -> &'a [String] {
        self.override_
            .and_then(|o| o.tags.as_deref())
            .unwrap_or(&self.base.tags)
    }
}

/// `editor.completion.*` — LSP-powered code completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Completion {
    /// Offer completions at all (the popup, manual and automatic).
    pub enabled: bool,
    /// Open the popup automatically while typing identifier or trigger
    /// characters, when the caret's line has no syntax error. Manual
    /// completion (Ctrl+Space) works regardless and bypasses the error gate.
    pub auto_trigger: bool,
}

impl Default for Completion {
    /// On by default (issue #57): completions and auto-trigger both enabled.
    fn default() -> Self {
        Self {
            enabled: true,
            auto_trigger: true,
        }
    }
}

/// `editor.semanticComments.*` — distinct highlighting of codetag comment blocks.
///
/// A comment whose content opens with a configured tag — plus the immediately
/// following non-empty comment lines — is highlighted with an attention-drawing
/// style instead of the ordinary comment color.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SemanticComments {
    /// Highlight codetag comment blocks distinctly.
    pub enabled: bool,
    /// The codetags that open a block. Matching is case-sensitive and whole-word:
    /// the tag must start the comment's content and be followed by `:`, `(`,
    /// whitespace, or the end of the line. Setting this replaces the defaults.
    pub tags: Vec<String>,
}

impl Default for SemanticComments {
    /// Enabled, with the conventional codetags (`TODO`, `FIXME`, `HACK`, `XXX`,
    /// `BUG` — the single source of truth is `karet-syntax`'s default).
    fn default() -> Self {
        Self {
            enabled: true,
            tags: karet_syntax::SemanticCommentConfig::default().tags,
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

/// `lsp.*` — language-server integration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Lsp {
    /// Run language servers for open documents (powers completions).
    pub enabled: bool,
    /// Per-language server launch configurations, keyed by the lowercase
    /// language name (e.g. `"rust"`, `"typescript"`, `"python"`). Entries are
    /// merged *over* the built-in defaults (rust → `rust-analyzer`,
    /// typescript/javascript → `typescript-language-server --stdio`,
    /// python → `pyright-langserver --stdio`), so setting a language here
    /// overrides its default and unlisted languages keep theirs.
    pub servers: BTreeMap<String, LspServer>,
}

impl Default for Lsp {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: BTreeMap::new(),
        }
    }
}

/// How to launch one language server (see [`Lsp::servers`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LspServer {
    /// The server executable, looked up on `PATH` (or an absolute path).
    pub command: String,
    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

/// `git.*` — source-control integration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Git {
    /// Show gutter change decorations and file-tree status colouring.
    pub decorations: bool,
    /// Show inline blame for the current line.
    pub blame: bool,
    /// AI-generated commit messages from the staged diff.
    pub ai_commit: AiCommit,
}

impl Default for Git {
    fn default() -> Self {
        Self {
            decorations: true,
            blame: false,
            ai_commit: AiCommit::default(),
        }
    }
}

/// `git.aiCommit.*` — generate a commit message from the staged diff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct AiCommit {
    /// Allow generating commit messages from the staged diff (needs the `claude`
    /// CLI on `PATH`). When off, the generate action reports that it is disabled.
    pub enabled: bool,
    /// The model to run: `"auto"` picks a cheap model for small diffs and a stronger
    /// one for large or many-file diffs; any other value pins that model name
    /// (e.g. `"haiku"`, `"sonnet"`, or a full model id).
    pub model: String,
    /// Thinking effort for the model. `null` leaves the model's default; ignored when
    /// `model` is `"auto"` (which chooses its own effort).
    pub effort: Option<AiCommitEffort>,
    /// Extra natural-language instructions appended to the prompt (e.g. "mention the
    /// user-visible effect", "reference the ticket in the branch name").
    pub instructions: Vec<String>,
    /// Path to the `claude` binary. `null` searches `PATH`.
    pub binary: Option<String>,
}

impl Default for AiCommit {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "auto".to_string(),
            effort: None,
            instructions: Vec::new(),
            binary: None,
        }
    }
}

/// How much thinking the commit-message model spends (`git.aiCommit.effort`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AiCommitEffort {
    /// Fastest, cheapest.
    #[default]
    Low,
    /// A balance of speed and quality.
    Medium,
    /// Slowest, most thorough.
    High,
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
        assert_eq!(s.editor.graphical_cursor, None);
        assert_eq!(s.editor.word_wrap, None);
        assert!(s.editor.sticky_scroll);
        assert_eq!(s.files.auto_save, AutoSave::Off);
        assert_eq!(s.workbench.color_theme, "dark");
        assert!(s.search.smart_case);
        assert!(!s.spellcheck.enabled);
        assert!(s.git.decorations);
        assert!(s.editor.semantic_comments.enabled);
        assert!(s.lsp.enabled, "LSP is on by default (issue #57)");
        assert!(s.editor.completion.enabled, "completion defaults on (#57)");
        assert!(s.editor.completion.auto_trigger, "auto-trigger defaults on");
        assert!(s.lsp.servers.is_empty(), "no user overrides by default");
    }

    #[test]
    fn word_wrap_accepts_auto_and_boolean_overrides() {
        let automatic: Editor = serde_json::from_str(r#"{ "wordWrap": null }"#).unwrap_or_default();
        let wrapped: Editor = serde_json::from_str(r#"{ "wordWrap": true }"#).unwrap_or_default();
        let overflow: Editor = serde_json::from_str(r#"{ "wordWrap": false }"#).unwrap_or_default();
        assert_eq!(automatic.word_wrap, None);
        assert_eq!(wrapped.word_wrap, Some(true));
        assert_eq!(overflow.word_wrap, Some(false));
    }

    #[test]
    fn language_selector_normalizes_and_displays() {
        let selector = LanguageSelector::from_language(" Rust ");
        assert_eq!(
            selector.as_ref().map(LanguageSelector::language),
            Some("rust")
        );
        assert_eq!(selector.map(|s| s.to_string()).as_deref(), Some("[rust]"));
        assert!(LanguageSelector::from_language("").is_none());
        assert!(LanguageSelector::from_language("[rust]").is_none());
    }

    #[test]
    fn language_override_resolves_every_editor_setting() {
        let parsed: Editor = serde_json::from_str(
            r#"{
                "tabSize": 8,
                "wordWrap": true,
                "stickyScroll": true,
                "graphicalCursor": true,
                "semanticComments": { "enabled": true, "tags": ["TODO"] },
                "completion": { "enabled": true, "autoTrigger": true },
                "[Rust]": {
                    "tabSize": 2,
                    "insertSpaces": false,
                    "lineNumbers": "relative",
                    "cursorLine": false,
                    "graphicalCursor": null,
                    "scrollOff": 9,
                    "rulers": [80, 100],
                    "wordWrap": null,
                    "stickyScroll": false,
                    "trimTrailingWhitespace": false,
                    "insertFinalNewline": false,
                    "formatOnSave": true,
                    "semanticComments": { "enabled": false, "tags": ["NOTE"] },
                    "completion": { "enabled": false, "autoTrigger": false }
                }
            }"#,
        )
        .unwrap_or_default();

        let rust = parsed.for_language(Some("rust"));
        assert_eq!(rust.tab_size(), 2);
        assert!(!rust.insert_spaces());
        assert_eq!(rust.line_numbers(), LineNumbers::Relative);
        assert!(!rust.cursor_line());
        assert_eq!(rust.graphical_cursor(), None);
        assert_eq!(rust.scroll_off(), 9);
        assert_eq!(rust.rulers(), [80, 100]);
        assert_eq!(rust.word_wrap(), None);
        assert!(!rust.sticky_scroll());
        assert!(!rust.trim_trailing_whitespace());
        assert!(!rust.insert_final_newline());
        assert!(rust.format_on_save());
        assert!(!rust.semantic_comments().enabled());
        assert_eq!(rust.semantic_comments().tags(), ["NOTE"]);
        assert!(!rust.completion().enabled());
        assert!(!rust.completion().auto_trigger());

        let python = parsed.for_language(Some("python"));
        assert_eq!(python.tab_size(), 8);
        assert_eq!(python.graphical_cursor(), Some(true));
        assert_eq!(python.word_wrap(), Some(true));
        assert!(python.sticky_scroll());
        assert!(python.semantic_comments().enabled());
        assert_eq!(python.semantic_comments().tags(), ["TODO"]);
        assert!(python.completion().enabled());
        assert!(python.completion().auto_trigger());
    }

    #[test]
    fn language_override_rejects_malformed_selectors_and_unknown_keys() {
        assert!(serde_json::from_str::<Editor>(r#"{ "rust": { "tabSize": 2 } }"#).is_err());
        assert!(serde_json::from_str::<Editor>(r#"{ "[]": { "tabSize": 2 } }"#).is_err());
        assert!(serde_json::from_str::<Editor>(r#"{ "[rust][toml]": { "tabSize": 2 } }"#).is_err());
        assert!(serde_json::from_str::<Editor>(r#"{ "[rust]": { "wat": 1 } }"#).is_err());
    }

    #[test]
    fn lsp_server_overrides_deserialize_camel_case() {
        let parsed: Lsp = serde_json::from_str(
            r#"{ "enabled": false,
                 "servers": { "rust": { "command": "ra-custom", "args": ["--log"] } } }"#,
        )
        .unwrap_or_default();
        assert!(!parsed.enabled);
        let rust = parsed.servers.get("rust");
        assert_eq!(rust.map(|s| s.command.as_str()), Some("ra-custom"));
        assert_eq!(rust.map(|s| s.args.clone()), Some(vec!["--log".to_owned()]));
        // `args` is optional.
        let parsed: LspServer =
            serde_json::from_str(r#"{ "command": "pylsp" }"#).unwrap_or(LspServer {
                command: String::new(),
                args: vec!["sentinel".into()],
            });
        assert_eq!(parsed.command, "pylsp");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn semantic_comments_default_on_with_the_conventional_tags() {
        let s = SemanticComments::default();
        assert!(s.enabled, "the feature is on by default");
        assert_eq!(s.tags, ["TODO", "FIXME", "HACK", "XXX", "BUG"]);
        // And the on-disk key deserializes camelCase under `editor`.
        let parsed: Editor = serde_json::from_str(
            r#"{ "semanticComments": { "enabled": false, "tags": ["TODO"] } }"#,
        )
        .unwrap_or_default();
        assert!(!parsed.semantic_comments.enabled);
        assert_eq!(parsed.semantic_comments.tags, ["TODO"]);
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
