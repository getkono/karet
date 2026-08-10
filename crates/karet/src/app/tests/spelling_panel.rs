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
