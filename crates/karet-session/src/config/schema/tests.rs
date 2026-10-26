//! Tests for the settings sections in [`super`]. Split out of `schema.rs` to
//! keep that file under the workspace code-line ceiling.

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
    assert_eq!(s.spellcheck.language, "en_US");
    assert!(s.spellcheck.documents);
    assert!(s.spellcheck.comments);
    assert!(!s.spellcheck.strings);
    assert!(!s.spellcheck.identifiers);
    assert_eq!(s.spellcheck.debounce_ms, 500);
    assert!(!s.latex.build_on_save);
    assert_eq!(s.latex.command, "latexmk");
    assert!(s.latex.args.iter().any(|argument| argument == "{file}"));
    assert!(s.latex.output_directory.is_empty());
    assert_eq!(s.latex.timeout_ms, 120_000);
    assert!(s.git.decorations);
    assert!(s.editor.semantic_comments.enabled);
    assert!(s.lsp.enabled, "LSP is on by default (issue #57)");
    assert!(s.editor.completion.enabled, "completion defaults on (#57)");
    assert!(s.editor.completion.auto_trigger, "auto-trigger defaults on");
    assert!(
        s.editor.inline_completion.enabled,
        "ghost text is asked for wherever a server offers it"
    );
    assert!(s.lsp.servers.is_empty(), "no user overrides by default");
}

/// Ghost text switches off per language without touching the rest: an
/// override that names only `inlineCompletion` leaves the popup alone.
#[test]
fn inline_completion_switches_off_per_language() {
    let parsed: Editor =
        serde_json::from_str(r#"{ "[markdown]": { "inlineCompletion": { "enabled": false } } }"#)
            .unwrap_or_default();
    let markdown = parsed.for_language(Some("markdown"));
    assert!(!markdown.inline_completion());
    assert!(markdown.completion().enabled());
    assert!(parsed.for_language(Some("rust")).inline_completion());
    let off: Editor =
        serde_json::from_str(r#"{ "inlineCompletion": { "enabled": false } }"#).unwrap_or_default();
    assert!(!off.for_language(Some("rust")).inline_completion());
}

#[test]
fn undo_settings_default_to_a_cap_and_no_persistence() {
    let files = Files::default();
    assert_eq!(files.undo_limit, 1000);
    assert_eq!(files.undo_cap(), Some(1000));
    assert!(!files.persistent_undo, "persistent undo is opt-in");
}

#[test]
fn a_zero_undo_limit_lifts_the_cap() {
    let parse = |json: &str| serde_json::from_str::<Files>(json).unwrap_or_default();
    let unbounded = parse(r#"{ "undoLimit": 0, "persistentUndo": true }"#);
    assert_eq!(unbounded.undo_cap(), None);
    assert!(unbounded.persistent_undo);
    assert_eq!(parse(r#"{ "undoLimit": 5 }"#).undo_cap(), Some(5));
}

/// The bound is the JSON-RPC request timeout at the top and one millisecond
/// at the bottom, both inclusive; anything outside fails the section.
#[test]
fn format_on_save_timeout_is_held_inside_the_request_timeout() {
    let parse = |value: &str| {
        serde_json::from_str::<Editor>(&format!(r#"{{ "formatOnSaveTimeout": {value} }}"#))
            .map(|editor| editor.format_on_save_timeout)
    };
    assert_eq!(Editor::default().format_on_save_timeout, 10_000);
    assert_eq!(parse("1").ok(), Some(1));
    assert_eq!(parse("30000").ok(), Some(30_000));
    assert!(parse("30001").is_err_and(|e| e.to_string().contains("between 1 and 30000")));
    assert!(parse("0").is_err());
    assert!(parse("-5").is_err());
    assert!(parse("\"10s\"").is_err());
}

#[test]
fn middle_click_paste_defaults_to_the_platform_and_obeys_an_explicit_value() {
    let parse = |json: &str| {
        serde_json::from_str::<Editor>(json)
            .unwrap_or_default()
            .middle_click_paste()
    };
    assert_eq!(parse("{}"), cfg!(target_os = "linux"));
    assert_eq!(
        parse(r#"{ "middleClickPaste": null }"#),
        cfg!(target_os = "linux")
    );
    assert!(parse(r#"{ "middleClickPaste": true }"#));
    assert!(!parse(r#"{ "middleClickPaste": false }"#));
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
                "autoClosingQuotes": "always",
                "inlineDiagnostics": { "mode": "all", "minSeverity": "warning" },
                "[Rust]": {
                    "inlineDiagnostics": { "minSeverity": "error" },
                    "autoClosingBrackets": "never",
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
    assert_eq!(rust.auto_closing_brackets(), AutoClosing::Never);
    assert_eq!(rust.auto_closing_quotes(), AutoClosing::Always);
    assert_eq!(rust.inline_diagnostics().mode(), InlineDiagnosticsMode::All);
    assert_eq!(
        rust.inline_diagnostics().min_severity(),
        DiagnosticSeverity::Error
    );

    let python = parsed.for_language(Some("python"));
    assert_eq!(python.auto_closing_brackets(), AutoClosing::LanguageDefined);
    assert_eq!(python.tab_size(), 8);
    assert_eq!(
        python.inline_diagnostics().min_severity(),
        DiagnosticSeverity::Warning
    );
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
fn semantic_comments_default_on_with_the_conventional_tags() {
    let s = SemanticComments::default();
    assert!(s.enabled, "the feature is on by default");
    assert_eq!(s.tags, ["TODO", "FIXME", "HACK", "XXX", "BUG"]);
    // And the on-disk key deserializes camelCase under `editor`.
    let parsed: Editor =
        serde_json::from_str(r#"{ "semanticComments": { "enabled": false, "tags": ["TODO"] } }"#)
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
