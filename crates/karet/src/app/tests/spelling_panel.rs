use karet_session::SpellingHit;

use super::support::*;
use crate::app::*;

/// One scan hit for `path` at 0-based `line`.
fn hit(path: &Path, line: u32, col: u32, word: &str, line_text: &str) -> SpellingHit {
    SpellingHit {
        path: path.to_path_buf(),
        range: Range {
            start: LineCol::new(line, col),
            end: LineCol::new(line, col + word.chars().count() as u32),
        },
        word: word.to_owned(),
        line_text: line_text.to_owned(),
    }
}

/// An app with `hits` already adopted from a scan tagged `RequestId(1)`.
fn scanned(root: &Path, hits: Vec<SpellingHit>) -> App {
    let mut app = App::new(root.to_path_buf(), Vec::new(), Vec::new(), false);
    app.settings.spellcheck.enabled = true;
    app.sidebar_panel = SidebarPanel::Spelling;
    app.spelling.scanning = Some(RequestId(1));
    let count = hits.len();
    app.spelling_scan_progress(Some(RequestId(1)), hits, count);
    app.spelling_scan_finished(Some(RequestId(1)), count, false);
    app
}

#[test]
fn scan_results_are_grouped_under_one_row_per_file() {
    let dir = test_dir("spelling-rows");
    let a = dir.join("a.md");
    let b = dir.join("b.md");
    let app = scanned(
        &dir,
        vec![
            hit(&a, 0, 4, "wrld", "the wrld ends"),
            hit(&a, 3, 0, "teh", "teh end"),
            hit(&b, 1, 2, "recieve", "  recieve it"),
        ],
    );

    assert_eq!(
        app.spelling.rows,
        vec![
            SpellingRow::File { hit: 0, count: 2 },
            SpellingRow::Word { hit: 0 },
            SpellingRow::Word { hit: 1 },
            SpellingRow::File { hit: 2, count: 1 },
            SpellingRow::Word { hit: 2 },
        ]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn activating_a_word_row_opens_its_file_at_the_misspelling() {
    let dir = test_dir("spelling-open");
    write_file(&dir, "a.md", b"first line\nthe wrld ends\n");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 1, 4, "wrld", "the wrld ends")]);

    // Row 0 is the file heading, row 1 the word.
    app.spelling.selection.move_to(1);
    app.open_selected_spelling();

    assert_eq!(app.tabs[app.active].path(), Some(path.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 4));
    assert_eq!(app.focus, Focus::Editor);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn activating_a_file_heading_jumps_to_that_files_first_misspelling() {
    let dir = test_dir("spelling-heading");
    write_file(&dir, "a.md", b"one\ntwo\nthe wrld ends\n");
    let path = dir.join("a.md");
    let mut app = scanned(
        &dir,
        vec![
            hit(&path, 2, 4, "wrld", "the wrld ends"),
            hit(&path, 2, 9, "endz", "the wrld endz"),
        ],
    );

    app.spelling.selection.move_to(0); // the heading
    app.open_selected_spelling();

    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 4));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_superseded_scans_late_batch_is_ignored() {
    let dir = test_dir("spelling-stale");
    let path = dir.join("a.md");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.spelling.scanning = Some(RequestId(2));

    // A batch from the previous, cancelled scan must not mix into the new list.
    app.spelling_scan_progress(
        Some(RequestId(1)),
        vec![hit(&path, 0, 0, "stale", "stale")],
        3,
    );
    assert!(app.spelling.hits.is_empty());
    assert!(
        app.spelling.scanning.is_some(),
        "the live scan keeps running"
    );

    app.spelling_scan_progress(
        Some(RequestId(2)),
        vec![hit(&path, 0, 0, "fresh", "fresh")],
        4,
    );
    assert_eq!(
        app.spelling
            .hits
            .iter()
            .map(|h| h.word.as_str())
            .collect::<Vec<_>>(),
        vec!["fresh"]
    );

    // The stale scan's finish must not clear the live one's loading state either.
    app.spelling_scan_finished(Some(RequestId(1)), 3, false);
    assert_eq!(app.spelling.scanning, Some(RequestId(2)));
    app.spelling_scan_finished(Some(RequestId(2)), 4, true);
    assert_eq!(app.spelling.scanning, None);
    assert!(app.spelling.truncated);
    assert!(app.spelling.scanned);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_result_row_opens_it() {
    let dir = test_dir("spelling-click");
    write_file(&dir, "a.md", b"one\nthe wrld ends\n");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 1, 4, "wrld", "the wrld ends")]);
    app.spelling_ui.results_rect = Rect {
        x: 0,
        y: 2,
        width: 30,
        height: 8,
    };
    app.spelling_ui.offset = 0;

    // y = 3 is the second row: the word under its file heading.
    app.spelling_click(4, 3);

    assert_eq!(app.tabs[app.active].path(), Some(path.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 4));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_click_below_the_last_row_opens_nothing() {
    let dir = test_dir("spelling-click-empty");
    write_file(&dir, "a.md", b"the wrld ends\n");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);
    app.spelling_ui.results_rect = Rect {
        x: 0,
        y: 2,
        width: 30,
        height: 8,
    };

    app.spelling_click(4, 7); // past the two rendered rows

    assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_documents_own_layer_replaces_what_the_scan_said_about_that_file() {
    let dir = test_dir("spelling-reconcile");
    let a = dir.join("a.md");
    let b = dir.join("b.md");
    let mut app = scanned(
        &dir,
        vec![
            hit(&a, 0, 4, "wrld", "the wrld ends"),
            hit(&a, 3, 0, "teh", "teh end"),
            hit(&b, 1, 2, "recieve", "  recieve it"),
        ],
    );

    // Opening `a.md` re-checks it, and the editor marks only one of the two.
    app.spelling_updated(&a, vec![hit(&a, 0, 4, "wrld", "the wrld ends")]);

    assert_eq!(
        app.spelling
            .hits
            .iter()
            .map(|h| (h.path.clone(), h.word.clone()))
            .collect::<Vec<_>>(),
        vec![
            (a.clone(), "wrld".to_owned()),
            (b.clone(), "recieve".to_owned()),
        ],
        "the other file's hits are untouched and stay grouped"
    );
    assert_eq!(
        app.spelling.rows,
        vec![
            SpellingRow::File { hit: 0, count: 1 },
            SpellingRow::Word { hit: 0 },
            SpellingRow::File { hit: 1, count: 1 },
            SpellingRow::Word { hit: 1 },
        ]
    );

    // Fixing the last one drops the file's heading with it.
    app.spelling_updated(&a, Vec::new());
    assert_eq!(
        app.spelling.rows,
        vec![
            SpellingRow::File { hit: 0, count: 1 },
            SpellingRow::Word { hit: 0 },
        ]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clean_file_the_scan_never_listed_is_not_appended() {
    let dir = test_dir("spelling-reconcile-clean");
    let a = dir.join("a.md");
    let b = dir.join("b.md");
    let mut app = scanned(&dir, vec![hit(&a, 0, 4, "wrld", "the wrld ends")]);

    app.spelling_updated(&b, Vec::new());
    assert_eq!(app.spelling.hits.len(), 1);

    // A file that goes on to acquire a misspelling does get listed.
    app.spelling_updated(&b, vec![hit(&b, 0, 0, "teh", "teh end")]);
    assert_eq!(
        app.spelling
            .hits
            .iter()
            .map(|h| h.word.as_str())
            .collect::<Vec<_>>(),
        vec!["wrld", "teh"]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panel_that_never_scanned_ignores_document_spelling_updates() {
    let dir = test_dir("spelling-reconcile-idle");
    let path = dir.join("a.md");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

    app.spelling_updated(&path, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    assert!(
        app.spelling.hits.is_empty(),
        "an unasked-for panel has nothing to correct"
    );
    assert!(!app.spelling.scanned);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `ConfigChanged` carrying `settings`, as the watcher's reload would deliver it.
fn config_changed(app: &mut App, settings: karet_session::config::Settings) {
    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(LoadedConfig::from_settings(settings)),
        },
    );
}

#[test]
fn a_spellcheck_settings_change_re_runs_the_scan() {
    let dir = test_dir("spelling-config-rescan");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    let mut settings = app.settings.clone();
    settings.spellcheck.words.push("wrld".to_owned());
    config_changed(&mut app, settings);

    // There is no backend in this fixture, so a scan attempt clears the list and
    // leaves `scanning` unset — which is how the re-scan shows up here.
    assert!(
        app.spelling.hits.is_empty(),
        "the stale results were dropped"
    );
    assert!(!app.spelling.scanned);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unrelated_config_change_leaves_the_results_alone() {
    let dir = test_dir("spelling-config-unrelated");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    let mut settings = app.settings.clone();
    settings.editor.tab_size = 8;
    config_changed(&mut app, settings);

    assert_eq!(app.spelling.hits.len(), 1, "a walk is not free — don't");
    assert_eq!(app.settings.editor.tab_size, 8);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panel_that_never_scanned_stays_idle_when_the_dictionary_changes() {
    let dir = test_dir("spelling-config-idle");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

    let mut settings = app.settings.clone();
    settings.spellcheck.words.push("wrld".to_owned());
    config_changed(&mut app, settings);

    assert!(app.spelling.scanning.is_none());
    assert!(
        !app.spelling.scanned,
        "opening the panel is what asks for the walk"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn adding_a_dictionary_word_drops_it_from_the_results() {
    let dir = test_dir("spelling-dictionary-word");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    app.dictionary_word_added("wrld", &dir.join(".karet/setting.jsonc"));

    assert!(app.settings.spellcheck.words.iter().any(|w| w == "wrld"));
    assert!(app.spelling.hits.is_empty(), "the panel re-scans");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_panel_follows_the_spellcheck_enabled_setting() {
    let dir = test_dir("spelling-enabled-setting");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    let mut settings = app.settings.clone();
    settings.spellcheck.enabled = false;
    config_changed(&mut app, settings);

    assert_eq!(
        app.sidebar_panel,
        SidebarPanel::Explorer,
        "an open panel is left for one that still exists"
    );
    assert!(app.spelling.hits.is_empty(), "and its results go with it");

    // Nothing selects it back while the setting is off.
    app.dispatch(Command::SelectPanel(SidebarPanel::Spelling));
    assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);

    let mut settings = app.settings.clone();
    settings.spellcheck.enabled = true;
    config_changed(&mut app, settings);
    app.dispatch(Command::SelectPanel(SidebarPanel::Spelling));

    assert_eq!(app.sidebar_panel, SidebarPanel::Spelling);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn re_showing_the_panel_does_not_rescan_results_it_already_has() {
    let dir = test_dir("spelling-reshow");
    let path = dir.join("a.md");
    let mut app = scanned(&dir, vec![hit(&path, 0, 4, "wrld", "the wrld ends")]);

    // There is no backend in this fixture, so a scan attempt would clear the list
    // and leave `scanning` unset — which is exactly what must not happen here.
    app.show_spelling();

    assert_eq!(app.spelling.hits.len(), 1);
    assert_eq!(app.sidebar_panel, SidebarPanel::Spelling);
    assert_eq!(app.focus, Focus::Sidebar);

    let _ = std::fs::remove_dir_all(&dir);
}
