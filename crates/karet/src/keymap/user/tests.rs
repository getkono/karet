use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::*;
use crate::command::COMMAND_IDS;

/// The live chord for a key press, as the resolver sees it.
fn press(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    KeyChord::from_event(KeyEvent::new(code, mods))
}

fn ctrl(c: char) -> KeyChord {
    press(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Merge one in-memory user file over the defaults.
fn user(text: &str) -> (Keymap, KeymapReport) {
    files(&[(Source::User, text)])
}

/// Merge in-memory files (lowest precedence first) over the defaults.
fn files(texts: &[(Source, &str)]) -> (Keymap, KeymapReport) {
    let paths: Vec<PathBuf> = texts
        .iter()
        .map(|(source, _)| PathBuf::from(format!("/{}/keymap.jsonc", source.label())))
        .collect();
    let inputs: Vec<Input<'_>> = texts
        .iter()
        .zip(&paths)
        .map(|((source, text), path)| Input {
            source: *source,
            path,
            text: Ok(Some((*text).to_string())),
        })
        .collect();
    merge(&inputs)
}

fn messages(report: &KeymapReport) -> Vec<&str> {
    report
        .issues
        .iter()
        .map(|issue| issue.message.as_str())
        .collect()
}

fn editor() -> Context {
    Context::focus(FocusTarget::Editor)
}

fn explorer() -> Context {
    Context::focus(FocusTarget::Explorer)
}

#[test]
fn chords_parse_to_the_live_key_press() {
    let cases = [
        ("ctrl+p", ctrl('p')),
        ("Ctrl+P", ctrl('p')),
        ("control+p", ctrl('p')),
        (
            "ctrl+shift+p",
            press(
                KeyCode::Char('P'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ),
        ("alt+1", press(KeyCode::Char('1'), KeyModifiers::ALT)),
        // A bare letter carries Shift in its case, both ways of writing it.
        ("shift+g", press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
        ("G", press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
        ("g", press(KeyCode::Char('g'), KeyModifiers::NONE)),
        ("shift+tab", press(KeyCode::BackTab, KeyModifiers::NONE)),
        ("f12", press(KeyCode::F(12), KeyModifiers::NONE)),
        ("shift+F8", press(KeyCode::F(8), KeyModifiers::SHIFT)),
        (
            "ctrl+space",
            press(KeyCode::Char(' '), KeyModifiers::CONTROL),
        ),
        ("escape", press(KeyCode::Esc, KeyModifiers::NONE)),
        ("pgdn", press(KeyCode::PageDown, KeyModifiers::NONE)),
        ("ctrl++", press(KeyCode::Char('+'), KeyModifiers::CONTROL)),
        ("+", press(KeyCode::Char('+'), KeyModifiers::NONE)),
        ("?", press(KeyCode::Char('?'), KeyModifiers::SHIFT)),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_chord(text), Ok(expected), "{text}");
    }
}

#[test]
fn malformed_chords_are_rejected_with_the_offending_part() {
    for (text, needle) in [
        ("hyper+p", "unknown modifier `hyper`"),
        ("ctrl+", "unknown key"),
        ("ctrl+nope", "unknown key `nope`"),
        ("f25", "unknown key `f25`"),
        ("ctrl++x", "unknown modifier ``"),
        // Terminals send `!`, never Shift+1.
        ("shift+1", "bind the character Shift produces"),
    ] {
        let Err(message) = parse_chord(text) else {
            panic!("{text} parsed");
        };
        assert!(message.contains(needle), "{text}: {message}");
    }
    assert!(parse_sequence("   ").is_err());
    assert_eq!(
        parse_sequence("ctrl+k  ctrl+w"),
        Ok(vec![ctrl('k'), ctrl('w')])
    );
}

#[test]
fn no_files_leave_the_defaults_untouched() {
    let (keymap, report) = files(&[]);
    assert_eq!(keymap.bindings.len(), BINDINGS.len());
    assert!(report.issues.is_empty());
    assert_eq!(
        keymap.resolve(editor(), &[ctrl('p')]),
        Resolved::Command(Command::OpenQuickOpen)
    );
}

#[test]
fn a_binding_replaces_the_default_on_the_same_keys_and_leads_the_hint() {
    let (keymap, report) = user(
        r#"{
            // comments and trailing commas are JSONC
            "bindings": [{ "key": "ctrl+p", "command": "quit" },],
        }"#,
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        keymap.resolve(editor(), &[ctrl('p')]),
        Resolved::Command(Command::Quit)
    );
    // The user's chord is the one the palette shows, ahead of the default Ctrl+Q.
    assert_eq!(
        keymap
            .hint_for(Command::Quit, ChordStyle::Verbose)
            .as_deref(),
        Some("Ctrl+P")
    );
    // Quick Open lost Ctrl+P but keeps its other default chords (if any).
    assert_ne!(
        keymap
            .hint_for(Command::OpenQuickOpen, ChordStyle::Verbose)
            .as_deref(),
        Some("Ctrl+P")
    );
    assert_eq!(report.files[0].status, FileStatus::Loaded { entries: 1 });
}

#[test]
fn when_scopes_a_binding_to_its_layer() {
    let (keymap, report) = user(
        r#"{ "bindings": [{ "key": "ctrl+alt+j", "command": "toggleLineComment", "when": "editor" }] }"#,
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let chord = press(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(
        keymap.resolve(editor(), &[chord]),
        Resolved::Command(Command::ToggleLineComment)
    );
    assert_eq!(keymap.resolve(explorer(), &[chord]), Resolved::None);
}

#[test]
fn a_sequence_binding_waits_for_its_second_chord() {
    let (keymap, report) = user(
        r#"{ "bindings": [{ "key": "ctrl+k ctrl+j", "command": "toggleBlockComment", "when": "editor" }] }"#,
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(keymap.resolve(editor(), &[ctrl('k')]), Resolved::Pending);
    assert_eq!(
        keymap.resolve(editor(), &[ctrl('k'), ctrl('j')]),
        Resolved::Command(Command::ToggleBlockComment)
    );
    let completions = keymap.completions_for(editor(), &[ctrl('k')], ChordStyle::Caret);
    assert!(
        completions
            .iter()
            .any(|hint| hint.command == Command::ToggleBlockComment && hint.chord == "^J")
    );
}

#[test]
fn a_sequence_in_a_modal_layer_is_refused() {
    // A modal resolves one chord at a time, so this could never fire; it is
    // reported and dropped rather than loaded silently.
    let (keymap, report) = user(
        r#"{ "bindings": [{ "key": "ctrl+k ctrl+j", "command": "findNext", "when": "find" }] }"#,
    );
    assert_eq!(
        messages(&report),
        [
            "binding 1: `find` is a modal layer, which takes single chords only; a key \
             sequence there can never be typed; skipped"
        ]
    );
    assert_eq!(keymap.bindings.len(), BINDINGS.len());
    // A single chord in the same layer is fine.
    let (_, report) =
        user(r#"{ "bindings": [{ "key": "ctrl+alt+j", "command": "findNext", "when": "find" }] }"#);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

#[test]
fn a_sequence_in_the_terminal_layer_is_refused() {
    // The terminal hands each unbound chord to the shell at once, so a
    // sequence's first chord never waits for its second.
    let (keymap, report) = user(
        r#"{ "bindings": [{ "key": "ctrl+k ctrl+t", "command": "newTerminal", "when": "terminal" }] }"#,
    );
    assert_eq!(
        messages(&report),
        [
            "binding 1: `terminal` sends every key it does not bind to the shell, so it \
             takes single chords only; a key sequence there can never be typed; skipped"
        ]
    );
    assert_eq!(keymap.bindings.len(), BINDINGS.len());
    let (_, report) = user(
        r#"{ "bindings": [{ "key": "ctrl+alt+t", "command": "newTerminal", "when": "terminal" }] }"#,
    );
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

#[test]
fn unbinding_removes_only_what_it_names() {
    // Narrowed to a key: Ctrl+Q stops quitting, nothing else changes.
    let (keymap, report) = user(r#"{ "bindings": [{ "key": "ctrl+q", "command": "-quit" }] }"#);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(keymap.resolve(editor(), &[ctrl('q')]), Resolved::None);
    assert_eq!(keymap.bindings.len(), BINDINGS.len() - 1);

    // Without a key: every binding of the command goes, so it shows no hint.
    let (keymap, _) = user(r#"{ "bindings": [{ "command": "-nextTab" }] }"#);
    assert_eq!(keymap.hint_for(Command::NextTab, ChordStyle::Verbose), None);

    // Narrowed to a layer the command is not bound in: nothing to remove.
    let (keymap, report) = user(r#"{ "bindings": [{ "command": "-quit", "when": "editor" }] }"#);
    assert_eq!(
        keymap.resolve(editor(), &[ctrl('q')]),
        Resolved::Command(Command::Quit)
    );
    assert_eq!(messages(&report), ["binding 1: unbinds nothing"]);
}

#[test]
fn the_project_file_overrides_the_user_file_silently() {
    let (keymap, report) = files(&[
        (
            Source::User,
            r#"{ "bindings": [{ "key": "ctrl+alt+j", "command": "save" }] }"#,
        ),
        (
            Source::Project,
            r#"{ "bindings": [{ "key": "ctrl+alt+j", "command": "undo" }] }"#,
        ),
    ]);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    let chord = press(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(
        keymap.resolve(editor(), &[chord]),
        Resolved::Command(Command::Undo)
    );
    assert_eq!(
        keymap
            .hint_for(Command::Save, ChordStyle::Verbose)
            .as_deref(),
        Some("Ctrl+S")
    );
}

#[test]
fn one_file_binding_the_same_keys_twice_is_a_conflict() {
    let (keymap, report) = user(
        r#"{ "bindings": [
            { "key": "ctrl+alt+j", "command": "save" },
            { "key": "ctrl+alt+j", "command": "undo" }
        ] }"#,
    );
    let chord = press(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    // The later entry wins, and the clash is reported against it.
    assert_eq!(
        keymap.resolve(editor(), &[chord]),
        Resolved::Command(Command::Undo)
    );
    assert_eq!(
        messages(&report),
        ["binding 2 rebinds Ctrl+Alt+J in `global`, overriding binding 1"]
    );
}

#[test]
fn a_binding_that_prefixes_another_is_reported() {
    // Ctrl+K alone would swallow every Ctrl+K chord the defaults define.
    let (keymap, report) = user(r#"{ "bindings": [{ "key": "ctrl+k", "command": "save" }] }"#);
    assert_eq!(
        keymap.resolve(editor(), &[ctrl('k')]),
        Resolved::Command(Command::Save)
    );
    let shadow: Vec<&str> = messages(&report)
        .into_iter()
        .filter(|m| m.contains("can never be typed"))
        .collect();
    assert!(
        shadow
            .iter()
            .any(|m| m.starts_with("binding 1: Ctrl+K (save in `global`)")
                && m.contains("Ctrl+K Ctrl+W (closeAllTabs in `global`)")),
        "{shadow:?}"
    );
    // A pair that never meets in one stack is no conflict: a Find-bar Ctrl+K
    // cannot shadow the editor's Ctrl+K chords.
    let (_, report) =
        user(r#"{ "bindings": [{ "key": "ctrl+k", "command": "findNext", "when": "find" }] }"#);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
}

#[test]
fn a_bad_entry_is_skipped_alone() {
    let (keymap, report) = user(
        r#"{ "bindings": [
            { "key": "ctrl+alt+j", "command": "nope" },
            { "key": "ctrl+alt+j", "command": "save", "when": "nowhere" },
            { "command": "save" },
            { "key": "hyper+j", "command": "save" },
            { "key": "ctrl+alt+j", "command": "save", "args": 1 },
            "ctrl+alt+j",
            ["ctrl+alt+j", "save"],
            { "key": "ctrl+alt+k", "command": "save" }
        ] }"#,
    );
    let all = messages(&report);
    assert_eq!(all.len(), 7, "{all:?}");
    assert!(all[5].starts_with("binding 6: expected an object"));
    assert!(all[6].starts_with("binding 7: expected an object"));
    assert!(all[0].starts_with("binding 1: unknown command `nope`"));
    assert!(all[1].starts_with("binding 2: unknown `when` layer `nowhere` (known: global,"));
    assert!(all[2].starts_with("binding 3: a binding needs a `key`"));
    assert!(all[3].starts_with("binding 4: unknown modifier `hyper`"));
    assert!(all[4].contains("unknown field `args`"));
    assert!(all.iter().all(|m| m.ends_with("; skipped")));
    // The one good entry still applies.
    let chord = press(
        KeyCode::Char('k'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(
        keymap.resolve(editor(), &[chord]),
        Resolved::Command(Command::Save)
    );
    assert_eq!(report.files[0].status, FileStatus::Loaded { entries: 8 });
}

#[test]
fn a_malformed_file_is_ignored_whole() {
    for (text, needle) in [
        ("{ \"bindings\": [ ", "invalid JSONC"),
        ("[]", "expected an object with a `bindings` array"),
        (r#"{ "bindings": {} }"#, "invalid keymap file"),
        (r#"{ "binding": [] }"#, "unknown field `binding`"),
    ] {
        let (keymap, report) = user(text);
        assert_eq!(keymap.bindings.len(), BINDINGS.len(), "{text}");
        assert_eq!(report.files[0].status, FileStatus::Invalid, "{text}");
        assert_eq!(report.issues.len(), 1, "{text}");
        assert!(report.issues[0].message.contains(needle), "{text}");
    }
    // An empty or `$schema`-only file is valid and binds nothing.
    for text in [
        "",
        "// nothing yet",
        r#"{ "$schema": "./keymap.schema.json" }"#,
    ] {
        let (_, report) = user(text);
        assert!(report.issues.is_empty(), "{text}");
        assert_eq!(report.files[0].status, FileStatus::Loaded { entries: 0 });
    }
}

#[test]
fn files_are_read_from_disk_and_a_missing_one_is_skipped() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let present = dir.path().join("user.jsonc");
    let absent = dir.path().join("absent.jsonc");
    if std::fs::write(
        &present,
        r#"{ "bindings": [{ "key": "ctrl+alt+j", "command": "save" }] }"#,
    )
    .is_err()
    {
        return;
    }
    let (keymap, report) = load_files(&[
        (Source::User, present.clone()),
        (Source::Project, absent.clone()),
    ]);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(
        report.files,
        vec![
            FileReport {
                source: Source::User,
                path: present,
                status: FileStatus::Loaded { entries: 1 },
            },
            FileReport {
                source: Source::Project,
                path: absent,
                status: FileStatus::Missing,
            },
        ]
    );
    assert_eq!(
        keymap
            .hint_for(Command::Save, ChordStyle::Verbose)
            .as_deref(),
        Some("Ctrl+Alt+J")
    );
    // A directory where the file should be is unreadable, not missing.
    let (_, report) = load_files(&[(Source::User, dir.path().to_path_buf())]);
    assert_eq!(report.files[0].status, FileStatus::Invalid);
    assert!(report.issues[0].message.contains("could not read"));
}

#[test]
fn the_project_file_sits_beside_the_project_settings() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let root = dir.path().join("repo");
    if std::fs::create_dir_all(root.join(".git")).is_err() {
        return;
    }
    let found = paths(std::slice::from_ref(&root));
    assert!(found.contains(&(Source::Project, root.join(".karet").join(FILE_NAME))));
    // Outside a worktree there is only (at most) the user file, beside the user's
    // `setting.jsonc`.
    let settings_dir = karet_session::config::load::user_config_dir();
    for (source, path) in paths(&[]) {
        assert_eq!(source, Source::User);
        assert_eq!(
            settings_dir.as_ref().map(|dir| dir.join(FILE_NAME)),
            Some(path)
        );
    }
}

#[test]
fn notifications_are_capped_with_a_pointer_to_the_doctor() {
    let issue = |n: usize| Issue {
        path: PathBuf::from("/k.jsonc"),
        message: format!("problem {n}"),
    };
    let report = KeymapReport {
        files: Vec::new(),
        issues: (1..=5).map(issue).collect(),
    };
    let diags = report.diagnostics();
    assert_eq!(diags.len(), MAX_NOTIFIED + 1);
    assert_eq!(diags[0].message, "/k.jsonc: problem 1");
    assert!(
        diags[MAX_NOTIFIED]
            .message
            .starts_with("2 more keymap problems")
    );
    assert!(diags.iter().all(|d| d.severity == Severity::Warning));
    assert!(KeymapReport::default().diagnostics().is_empty());
}

/// The JSON Schema for a keymap file, generated from the same id tables the
/// parser reads, so an editor's autocomplete can never offer a name the parser
/// rejects.
fn json_schema() -> Value {
    let ids: Vec<&str> = COMMAND_IDS.iter().map(|(_, id)| *id).collect();
    let commands: Vec<String> = ids
        .iter()
        .map(|id| (*id).to_string())
        .chain(ids.iter().map(|id| format!("-{id}")))
        .collect();
    let layers: Vec<&str> = Layer::ALL.iter().map(|layer| layer.id()).collect();
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "karet keymap",
        "description": "User (`$XDG_CONFIG_HOME/karet/keymap.jsonc`) or project \
            (`$GIT_ROOT/.karet/keymap.jsonc`) key bindings, applied in order over the \
            built-in keymap. The project file applies after the user file.",
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "$schema": { "type": "string" },
            "bindings": {
                "description": "Bindings and unbindings, applied in order.",
                "type": "array",
                "items": { "$ref": "#/$defs/binding" }
            }
        },
        "$defs": {
            "binding": {
                "type": "object",
                "additionalProperties": false,
                "required": ["command"],
                "properties": {
                    "key": {
                        "description": "One chord (`ctrl+shift+p`) or a space-separated \
                            sequence (`ctrl+k ctrl+w`). Modifiers: ctrl, shift, alt. \
                            Required to bind; optional to unbind.",
                        "type": "string"
                    },
                    "command": {
                        "description": "The command id to bind, or `-` + an id to \
                            remove that command's bindings (narrowed by `key` and \
                            `when` when given).",
                        "type": "string",
                        "enum": commands
                    },
                    "when": {
                        "description": "The layer the binding lives in; a more specific \
                            layer shadows `global`.",
                        "type": "string",
                        "enum": layers,
                        "default": "global"
                    }
                }
            }
        }
    })
}

/// Guards the checked-in `docs/keymap.schema.json` against drift. Regenerate it
/// with `KARET_BLESS=1 cargo test -p karet keymap_schema`.
#[test]
fn keymap_schema_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/keymap.schema.json");
    let generated = json_schema();
    if std::env::var_os("KARET_BLESS").is_some() {
        let text = serde_json::to_string_pretty(&generated).unwrap_or_default();
        assert!(std::fs::write(path, text + "\n").is_ok(), "write {path}");
        return;
    }
    // Compared as parsed values: object order depends on serde_json features.
    let on_disk: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert!(
        on_disk == generated,
        "docs/keymap.schema.json is stale; run `KARET_BLESS=1 cargo test -p karet keymap_schema`"
    );
}
