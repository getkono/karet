//! The typed `Settings` tree: the schema the JSONC config is verified against.
//!
//! Every section carries `#[serde(default, deny_unknown_fields, rename_all =
//! "camelCase")]`: missing fields fall back to the section's [`Default`] (the sane
//! defaults), unknown keys are rejected (so typos surface as diagnostics), and the
//! on-disk keys read like VS Code / Zed (`tabSize`, `formatOnSave`, …). The whole
//! tree also derives [`schemars::JsonSchema`] so the external `settings.schema.json`
//! is generated from this one source of truth.

use std::collections::BTreeMap;

#[cfg(feature = "schema")]
use schemars::Schema;
use serde::Deserialize;
use serde::Serialize;

/// The full, validated karet configuration. Load it with
/// [`crate::config::load`]; the sane baseline is [`Settings::default`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// The Seam view.
    pub seam: Seam,
    /// Spell-checking of prose and selected source-code tokens.
    pub spellcheck: Spellcheck,
    /// External LaTeX build and preview tooling.
    pub latex: Latex,
    /// Source-control integration.
    pub git: Git,
    /// Language-server integration (completions and future language features).
    pub lsp: Lsp,
    /// Markdown-specific editing behaviour.
    pub markdown: Markdown,
    /// WakaTime time-tracking (opt-in).
    pub wakatime: Wakatime,
    /// Dependency-manifest hints (Cargo.toml freshness and advisories).
    pub deps: Deps,
    /// TOML-specific behaviour.
    pub toml: Toml,
    /// Debugger integration (adapters and launch configurations).
    pub debug: Debugger,
    /// Jupyter-notebook behaviour (kernel execution).
    pub notebook: NotebookSettings,
    /// Named shell commands the Run Task picker offers, keyed by label.
    #[serde(default)]
    pub tasks: BTreeMap<String, Task>,
}

/// `wakatime.*` — WakaTime time tracking. **Off by default**: enabling it
/// sends file names, project names, and editing activity to the configured
/// WakaTime-compatible service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Wakatime {
    /// Send heartbeats. The API key and endpoint come from the standard
    /// `$WAKATIME_HOME/.wakatime.cfg`, never from this file.
    pub enabled: bool,
    /// Show today's coding total in the status bar.
    pub status_bar: bool,
}

impl Default for Wakatime {
    /// Disabled; the status segment shows once enabled.
    fn default() -> Self {
        Self {
            enabled: false,
            status_bar: true,
        }
    }
}

/// `deps.*` — dependency-manifest hints. Checking consults the crates.io
/// sparse index and the OSV advisory database over the network (cached
/// in-process); air-gapped setups can turn it off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Deps {
    /// Annotate open `Cargo.toml` dependencies with their freshness and
    /// known advisories.
    pub enabled: bool,
}

impl Default for Deps {
    /// On by default.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `notebook.*` — Jupyter-notebook behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct NotebookSettings {
    /// Kernel execution.
    pub kernel: NotebookKernel,
}

/// `notebook.kernel.*` — the kernel lifecycle behind Run All / Run Cell.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct NotebookKernel {
    /// Start the notebook's kernel as soon as its preview opens, instead of
    /// on the first run command. **Off by default** — a kernel is an
    /// arbitrary-code process.
    pub auto_start: bool,
}

/// `debug.*` — debugger integration. karet speaks the Debug Adapter Protocol;
/// an adapter entry says how to launch one, and configurations are offered by
/// the Debug: Start command (there is no launch.json compatibility layer).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Debugger {
    /// Debug adapters by name. Names referenced from configurations; the
    /// built-in fallbacks `codelldb`, `lldb-dap`, `gdb`, and `debugpy` work
    /// without an entry when the executable is on `PATH`.
    pub adapters: BTreeMap<String, DebugAdapter>,
    /// The launch configurations the Debug: Start command offers, first entry
    /// first.
    pub configurations: Vec<DebugConfiguration>,
}

/// One `debug.adapters` entry: how to launch a debug adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct DebugAdapter {
    /// The adapter executable, looked up on `PATH` (or an absolute path).
    pub command: String,
    /// Command-line arguments; under the `tcp` transport, `${port}` is
    /// replaced with the chosen port.
    pub args: Vec<String>,
    /// How the adapter exposes its protocol endpoint.
    pub transport: DebugTransport,
}

/// How a debug adapter speaks DAP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum DebugTransport {
    /// Over the adapter's stdin/stdout (the common case).
    #[default]
    Stdio,
    /// The adapter listens on a TCP port passed via a `${port}` argument
    /// (codelldb style).
    Tcp,
}

/// One `debug.configurations` entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct DebugConfiguration {
    /// The display name the Debug: Start picker shows.
    pub name: String,
    /// The adapter to use: a `debug.adapters` key or a built-in fallback name.
    pub adapter: String,
    /// Attach to a running debuggee instead of launching one.
    pub attach: bool,
    /// Adapter-specific launch/attach arguments (`program`, `args`, `cwd`,
    /// `pid`, …), passed through verbatim — each adapter documents its own.
    pub arguments: serde_json::Value,
}

/// `toml.*` — TOML-specific behaviour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Toml {
    /// Format TOML documents with the built-in taplo formatter (honoring the
    /// workspace's `.taplo.toml`) when no language server offers formatting.
    pub format: bool,
}

impl Default for Toml {
    /// On by default.
    fn default() -> Self {
        Self { format: true }
    }
}

/// `markdown.*` — Markdown-specific editing behaviour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Markdown {
    /// Continue list items on Enter: repeat the marker, advance ordered
    /// numbering (renumbering the run), carry task checkboxes, and end the
    /// list when Enter is pressed on an empty item.
    pub list_continuation: bool,
    /// Table-of-contents generation.
    pub toc: MarkdownToc,
    /// Markdown linting (the markdownlint rule core).
    pub lint: MarkdownLint,
    /// Mermaid diagram rendering in the preview.
    pub mermaid: MarkdownMermaid,
}

impl Default for Markdown {
    /// List continuation on by default.
    fn default() -> Self {
        Self {
            list_continuation: true,
            toc: MarkdownToc::default(),
            lint: MarkdownLint::default(),
            mermaid: MarkdownMermaid::default(),
        }
    }
}

/// `markdown.mermaid.*` — mermaid diagram rendering in the preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct MarkdownMermaid {
    /// Render matching code fences as Unicode diagrams in the preview
    /// (unsupported diagram types fall back to showing the fence source).
    pub enabled: bool,
    /// The fence info strings treated as mermaid.
    pub fence_languages: Vec<String>,
}

impl Default for MarkdownMermaid {
    /// On by default, for ```mermaid fences.
    fn default() -> Self {
        Self {
            enabled: true,
            fence_languages: vec!["mermaid".to_owned()],
        }
    }
}

/// `markdown.lint.*` — markdown linting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct MarkdownLint {
    /// Lint Markdown documents as they change, surfacing issues as
    /// diagnostics. Rule selection follows the workspace's
    /// `.markdownlint.json` when present.
    pub enabled: bool,
}

impl Default for MarkdownLint {
    /// On by default.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `markdown.toc.*` — table-of-contents generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct MarkdownToc {
    /// The smallest heading level included (GitHub convention keeps the `#`
    /// document title out of its own table).
    pub min_level: u8,
    /// The largest heading level included.
    pub max_level: u8,
}

impl Default for MarkdownToc {
    /// Levels 2–6.
    fn default() -> Self {
        Self {
            min_level: 2,
            max_level: 6,
        }
    }
}

/// `editor.*` — text-editing behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(transform = deny_additional_properties))]
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
    /// Keep at least this many lines visible above and below the caret when
    /// the keyboard moves it (capped to what the viewport can spare; a click
    /// never scrolls).
    pub scroll_off: u16,
    /// Columns to draw vertical rulers after (empty = none).
    pub rulers: Vec<u16>,
    /// `editor.guides.*` — vertical guides at each indentation level.
    pub guides: Guides,
    /// Which whitespace characters are drawn visibly (`·` for a space, `→`
    /// for a tab).
    pub render_whitespace: RenderWhitespace,
    /// `editor.bracketPairColorization.*` — brackets coloured by nesting depth.
    pub bracket_pair_colorization: BracketPairColorization,
    /// Override file-type wrapping: `null` uses the file default, `true` or
    /// `"on"` wraps at the viewport, `false` or `"off"` uses horizontal
    /// overflow, and `"wordWrapColumn"` or `"bounded"` wrap at
    /// `wordWrapColumn` (or the viewport, when it is narrower).
    pub word_wrap: Option<WordWrap>,
    /// The column `"wordWrapColumn"` and `"bounded"` wrapping break at; `0`
    /// wraps at the viewport.
    pub word_wrap_column: u16,
    /// How the continuation rows of a soft-wrapped line are indented.
    pub wrapping_indent: WrappingIndent,
    /// Keep the active semantic block hierarchy pinned above scrolled text.
    pub sticky_scroll: bool,
    /// Paste the primary selection at a middle click in the text: `null` turns
    /// it on for Linux, where the primary selection is a platform convention,
    /// and off elsewhere.
    pub middle_click_paste: Option<bool>,
    /// The editing model of a code tab: `"default"` types at the caret, as in
    /// VS Code; `"vim"` and `"helix"` add a modal Normal/Insert layer with
    /// that editor's motions, operators, text objects, registers and macros.
    pub keymap: KeymapPreset,
    /// When an opening bracket inserts its closing bracket. Typing over a
    /// closer it inserted steps past it, and Backspace between an empty pair
    /// deletes both halves.
    pub auto_closing_brackets: AutoClosing,
    /// When an opening quote inserts its closing quote (the language decides
    /// which quotes pair: Rust's `'` does not).
    pub auto_closing_quotes: AutoClosing,
    /// Strip trailing whitespace from each line on save.
    pub trim_trailing_whitespace: bool,
    /// Ensure the file ends with a single trailing newline on save.
    pub insert_final_newline: bool,
    /// Run the configured formatter on save.
    pub format_on_save: bool,
    /// Milliseconds a save waits on a language-server formatter before it is
    /// written unformatted (`1..=30000`).
    #[serde(deserialize_with = "format_on_save_timeout")]
    #[cfg_attr(feature = "schema", schemars(range(min = 1, max = FORMAT_ON_SAVE_TIMEOUT_MAX_MS)))]
    pub format_on_save_timeout: u64,
    /// Apply the language server's on-type formatting
    /// (`textDocument/onTypeFormatting`) after typing one of its trigger
    /// characters. Off by default, as in VS Code.
    pub format_on_type: bool,
    /// Distinct highlighting of codetag comment blocks (`TODO:`, `FIXME:`, …).
    pub semantic_comments: SemanticComments,
    /// Re-render TypeScript diagnostics as markdown (quoted types lifted into
    /// highlighted code blocks) wherever rich rendering is available.
    pub pretty_errors: bool,
    /// LSP-powered code completion (the popup).
    pub completion: Completion,
    /// `editor.inlineCompletion.*` — ghost text a language server (such as
    /// Copilot) proposes at the caret.
    pub inline_completion: InlineCompletion,
    /// The hover popup (documentation + diagnostics at the caret).
    pub hover: HoverPopup,
    /// `editor.inlayHints.*` — inferred types and parameter names drawn inline.
    pub inlay_hints: InlayHints,
    /// Show the language server's code lenses on rows above the code they describe.
    pub code_lens: bool,
    /// `editor.semanticHighlighting.*` — semantic tokens over the syntax colours.
    pub semantic_highlighting: SemanticHighlighting,
    /// Inline swatches on color literals (hex, `rgb()`, `hsl()`) and on the
    /// colours a language server reports.
    pub color_highlight: ColorHighlight,
    /// `editor.inlineDiagnostics.*` — which diagnostics show their message at
    /// the end of their line.
    pub inline_diagnostics: InlineDiagnostics,
    /// Per-language patches keyed by selectors such as `[rust]`.
    ///
    /// This map is flattened in `setting.jsonc`, so its entries sit beside the
    /// global editor keys rather than under a separate `languageOverrides` key.
    #[serde(flatten)]
    pub language_overrides: BTreeMap<LanguageSelector, EditorOverride>,
}

#[cfg(feature = "schema")]
fn deny_additional_properties(schema: &mut Schema) {
    schema.insert("additionalProperties".to_string(), false.into());
}

/// The default `editor.formatOnSaveTimeout`, in milliseconds.
///
/// Sized to the slowest formatting a working server plausibly does — a cold
/// process, a large file, a loaded machine — because the cost of being wrong is
/// asymmetric. Too generous and a rare save waits, visibly, with the tab
/// spinner running; too tight and an ordinary save silently stops formatting.
/// Three seconds was tight enough to lose the second way.
pub const FORMAT_ON_SAVE_TIMEOUT_DEFAULT_MS: u64 = 10_000;

/// The largest `editor.formatOnSaveTimeout` accepted, in milliseconds.
///
/// This is `karet-jsonrpc`'s request timeout, the only other bound on the
/// wait: past it the request has expired and no answer is left to come, so a
/// longer deadline would mean nothing. Keep the two equal.
pub const FORMAT_ON_SAVE_TIMEOUT_MAX_MS: u64 = 30_000;

/// Accept a `formatOnSaveTimeout` only within `1..=`[`FORMAT_ON_SAVE_TIMEOUT_MAX_MS`].
///
/// Zero is refused rather than read as "never wait": that would quietly stop
/// language-server formatting, which `formatOnSave: false` already says plainly.
fn format_on_save_timeout<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u64, D::Error> {
    let millis = u64::deserialize(de)?;
    if (1..=FORMAT_ON_SAVE_TIMEOUT_MAX_MS).contains(&millis) {
        return Ok(millis);
    }
    Err(serde::de::Error::custom(format!(
        "`formatOnSaveTimeout` must be between 1 and {FORMAT_ON_SAVE_TIMEOUT_MAX_MS} \
         milliseconds (the language-server request timeout), got {millis}"
    )))
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
            guides: Guides::default(),
            render_whitespace: RenderWhitespace::None,
            bracket_pair_colorization: BracketPairColorization::default(),
            word_wrap: None,
            word_wrap_column: 80,
            wrapping_indent: WrappingIndent::Same,
            sticky_scroll: true,
            middle_click_paste: None,
            keymap: KeymapPreset::Default,
            auto_closing_brackets: AutoClosing::LanguageDefined,
            auto_closing_quotes: AutoClosing::LanguageDefined,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            format_on_save: true,
            format_on_save_timeout: FORMAT_ON_SAVE_TIMEOUT_DEFAULT_MS,
            format_on_type: false,
            semantic_comments: SemanticComments::default(),
            pretty_errors: true,
            completion: Completion::default(),
            inline_completion: InlineCompletion::default(),
            hover: HoverPopup::default(),
            inlay_hints: InlayHints::default(),
            code_lens: true,
            semantic_highlighting: SemanticHighlighting::default(),
            color_highlight: ColorHighlight::default(),
            inline_diagnostics: InlineDiagnostics::default(),
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
        ResolvedEditor::new(self, override_)
    }

    /// Whether a middle click in the text pastes the primary selection:
    /// [`middle_click_paste`](Self::middle_click_paste) when set, otherwise on
    /// for Linux only.
    #[must_use]
    pub fn middle_click_paste(&self) -> bool {
        self.middle_click_paste.unwrap_or(cfg!(target_os = "linux"))
    }
}

pub use aicommit::AUTO_MODEL;
pub use aicommit::AiCommit;
pub use aicommit::AiCommitAgent;
pub use aicommit::AiCommitEffort;
pub use aicommit::DEFAULT_TIMEOUT_MS as AI_COMMIT_DEFAULT_TIMEOUT_MS;
pub use display::BracketPairColorization;
pub use display::Guides;
pub use display::LineNumbers;
pub use display::RenderWhitespace;
pub use display::SemanticHighlighting;
pub use display::WordWrap;
pub use display::WrappingIndent;
pub use inlay_hints::InlayHints;
pub use inlay_hints::InlayHintsEnabled;
pub use inline_diagnostics::DiagnosticSeverity;
pub use inline_diagnostics::InlineDiagnostics;
pub use inline_diagnostics::InlineDiagnosticsMode;
pub use keymap::KeymapPreset;
pub use lsp::Lsp;
pub use lsp::LspLanguage;
pub use lsp::LspServer;
pub use lsp::ManagedDownloads;
pub use overrides::EditorOverride;
pub use overrides::InlineDiagnosticsOverride;
pub use overrides::LanguageSelector;
pub use overrides::NullableOverride;
pub use overrides::ResolvedCompletion;
pub use overrides::ResolvedEditor;
pub use overrides::ResolvedInlineDiagnostics;
pub use overrides::ResolvedSemanticComments;
pub use tasks::BuiltinMatcher;
pub use tasks::MatcherSeverity;
pub use tasks::PatternMatcher;
pub use tasks::ProblemMatcher;
pub use tasks::Task;
pub use workbench::IconStyleSetting;
pub use workbench::StartupPanel;
pub use workbench::Workbench;

mod aicommit;
mod display;
mod inlay_hints;
mod inline_diagnostics;
mod keymap;
mod lsp;
mod overrides;
mod tasks;
mod workbench;

/// `editor.completion.*` — LSP-powered code completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// `editor.inlineCompletion.*` — ghost text proposed at the caret by a
/// language server offering LSP 3.18 inline completions (such as
/// `copilot-language-server`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct InlineCompletion {
    /// Ask for ghost text once typing pauses, and on Alt+] / Alt+[. Nothing
    /// is shown unless a server attached to the document offers it, so this
    /// costs nothing where none does; switch it off per language with
    /// `"[markdown]": { "inlineCompletion": { "enabled": false } }`.
    pub enabled: bool,
}

impl Default for InlineCompletion {
    /// On: a server that offers inline completions was attached on purpose.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `editor.colorHighlight.*` — inline swatches on color literals and on the
/// colours a language server reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ColorHighlight {
    /// Tint color literals (hex 3/4/6/8, `rgb()`/`rgba()`, `hsl()`/`hsla()`),
    /// and the colours the file's language server reports, with their own
    /// color, on the visible lines of the editor.
    pub enabled: bool,
}

impl Default for ColorHighlight {
    /// On by default.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `editor.hover.*` — the hover popup showing documentation from the language
/// server plus the diagnostics under the caret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct HoverPopup {
    /// Offer the hover popup at all (Ctrl+K Ctrl+I).
    pub enabled: bool,
}

impl Default for HoverPopup {
    /// On by default.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `editor.semanticComments.*` — distinct highlighting of codetag comment blocks.
///
/// A comment whose content opens with a configured tag — plus the immediately
/// following non-empty comment lines — is highlighted with an attention-drawing
/// style instead of the ordinary comment color.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// When typing an opening bracket or quote inserts its closing partner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum AutoClosing {
    /// Whenever the opener is typed, wherever the caret is.
    Always,
    /// Only where the language's pairs say so: before whitespace, the end of
    /// the line or closing punctuation, and, for quotes, outside strings and
    /// comments and not straight after a word.
    #[default]
    LanguageDefined,
    /// Never.
    Never,
}

/// `files.*` — file handling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// The most undo steps kept per buffer; the oldest are forgotten first.
    /// `0` keeps every step (no cap).
    pub undo_limit: u64,
    /// Keep undo history across restarts: it is written next to the
    /// crash-recovery swaps when a file is saved or closed, and restored when
    /// the file is reopened unchanged.
    pub persistent_undo: bool,
}

impl Files {
    /// [`Self::undo_limit`] as a buffer cap: `None` (no cap) for `0`.
    #[must_use]
    pub fn undo_cap(&self) -> Option<usize> {
        match self.undo_limit {
            0 => None,
            limit => Some(usize::try_from(limit).unwrap_or(usize::MAX)),
        }
    }
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
            undo_limit: 1000,
            persistent_undo: false,
        }
    }
}

/// When dirty buffers are written back to disk automatically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// `search.*` — workspace search behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// How the Seam view's lens filter treats rows that do not match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum SeamLensFilter {
    /// Dim them, keeping the tree's shape stable as filters change.
    #[default]
    Demote,
    /// Remove them.
    Hide,
}

/// How the Seam view renders its containment tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum SeamSpine {
    /// Cascading columns when the terminal is wide enough, an indented tree below that.
    #[default]
    Auto,
    /// Always cascade, however narrow the terminal.
    Columns,
    /// Always use the indented tree.
    Tree,
}

/// `seam.*` — the Seam view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Seam {
    /// How the containment tree is rendered.
    pub spine: SeamSpine,
    /// Whether a lens filter dims non-matching rows or removes them.
    ///
    /// Demoting keeps the tree's shape stable as filters change, so the reader never
    /// loses their place to a row that vanished.
    pub lens_filter: SeamLensFilter,
    /// Whether nodes excluded by the active configuration are hidden rather than dimmed.
    ///
    /// Off by default: a package is a family of trees, and hiding the branches this
    /// build excludes makes the one on screen look like the whole of it.
    pub hide_inactive: bool,
    /// Lenses to enable when the view opens.
    pub default_lenses: Vec<String>,
    /// The configuration to read the package under when the view opens.
    pub default_configuration: Option<String>,
    /// Index the workspace package as soon as the session starts.
    ///
    /// Off by default so startup is never delayed by a parse of every file; the index
    /// begins when the view is first opened.
    pub index_on_startup: bool,
    /// Stop indexing after this many files, marking the index truncated.
    pub max_indexed_files: usize,
}

impl Default for Seam {
    fn default() -> Self {
        Self {
            spine: SeamSpine::default(),
            lens_filter: SeamLensFilter::default(),
            hide_inactive: false,
            default_lenses: Vec::new(),
            default_configuration: None,
            index_on_startup: false,
            // Far above any real package: a runaway guard, not a policy.
            max_indexed_files: 20_000,
        }
    }
}

/// `spellcheck.*` — spell-checking of prose and selected source-code tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Spellcheck {
    /// Enable spell-checking.
    pub enabled: bool,
    /// Dictionary language (e.g. `"en_US"`).
    pub language: String,
    /// Extra words treated as correctly spelled.
    pub words: Vec<String>,
    /// Check prose-oriented editable documents such as Markdown.
    pub documents: bool,
    /// Check comments in source code.
    pub comments: bool,
    /// Check string literals in source code.
    pub strings: bool,
    /// Check identifier-like syntax tokens when the file parses cleanly.
    pub identifiers: bool,
    /// Quiet period after the latest token update before checking.
    pub debounce_ms: u64,
}

impl Default for Spellcheck {
    fn default() -> Self {
        Self {
            enabled: false,
            language: "en_US".to_string(),
            words: Vec::new(),
            documents: true,
            comments: true,
            strings: false,
            identifiers: false,
            debounce_ms: 500,
        }
    }
}

/// `latex.*` — external LaTeX compilation and preview behavior.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Latex {
    /// Compile the root document after each successful manual or automatic save.
    pub build_on_save: bool,
    /// External executable. It is launched directly, never through a shell.
    pub command: String,
    /// Arguments supporting `{file}`, `{fileDir}`, and `{outputDir}` placeholders.
    pub args: Vec<String>,
    /// Build output directory, absolute or relative to the workspace root. Empty
    /// uses karet's platform cache and keeps generated files out of the repository.
    pub output_directory: String,
    /// Maximum compiler runtime before the process is terminated.
    pub timeout_ms: u64,
}

impl Default for Latex {
    fn default() -> Self {
        Self {
            build_on_save: false,
            command: "latexmk".to_owned(),
            args: vec![
                "-pdf".to_owned(),
                "-interaction=nonstopmode".to_owned(),
                "-synctex=1".to_owned(),
                "-file-line-error".to_owned(),
                "-outdir={outputDir}".to_owned(),
                "{file}".to_owned(),
            ],
            output_directory: String::new(),
            timeout_ms: 120_000,
        }
    }
}

/// `git.*` — source-control integration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Git {
    /// URL template for `#123` issue references in commit messages; `$1`
    /// substitutes the number (e.g.
    /// `"https://issues.example.com/browse/PROJ-$1"`). Left unset, GitHub
    /// origins derive `https://github.com/<owner>/<repo>/issues/$1`.
    pub issue_url: Option<String>,
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
            blame: true,
            issue_url: None,
            ai_commit: AiCommit::default(),
        }
    }
}

#[cfg(test)]
mod tests;
