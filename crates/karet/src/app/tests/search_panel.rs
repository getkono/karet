//! The workspace Search panel's grouped result rows: grouping, folding, adaptive
//! expansion, streaming staleness, and what a row opens.

use super::support::*;
use crate::app::*;

/// Build a panel hit with `count` matches on consecutive lines.
fn search_hit(path: &std::path::Path, count: usize) -> karet_session::SearchHit {
    karet_session::SearchHit {
        path: path.to_path_buf(),
        matches: (0..count)
            .map(|i| karet_session::SearchMatch {
                range: Range {
                    start: LineCol::new(i as u32, 4),
                    end: LineCol::new(i as u32, 10),
                },
                line_text: format!("let needle{i} = 1;"),
                preview_start: 4,
                preview_end: 10,
            })
            .collect(),
    }
}

#[test]
fn rows_group_every_file_over_its_matches() {
    let mut app = app();
    app.search.hits = vec![
        search_hit(std::path::Path::new("/w/a.rs"), 2),
        search_hit(std::path::Path::new("/w/b.rs"), 1),
    ];
    app.search.rebuild_rows();
    assert_eq!(
        app.search.rows,
        vec![
            SearchRow::File {
                hit: 0,
                count: 2,
                expanded: true
            },
            SearchRow::Match { hit: 0, index: 0 },
            SearchRow::Match { hit: 0, index: 1 },
            SearchRow::File {
                hit: 1,
                count: 1,
                expanded: true
            },
            SearchRow::Match { hit: 1, index: 0 },
        ]
    );
}

#[test]
fn collapsing_a_file_hides_only_its_own_matches() {
    let mut app = app();
    app.search.hits = vec![
        search_hit(std::path::Path::new("/w/a.rs"), 2),
        search_hit(std::path::Path::new("/w/b.rs"), 1),
    ];
    app.search.rebuild_rows();
    app.search.toggle_file(std::path::Path::new("/w/a.rs"));
    assert_eq!(
        app.search.rows,
        vec![
            SearchRow::File {
                hit: 0,
                count: 2,
                expanded: false
            },
            SearchRow::File {
                hit: 1,
                count: 1,
                expanded: true
            },
            SearchRow::Match { hit: 1, index: 0 },
        ]
    );
}

/// Collapse is stored by path, so a file arriving in a later streaming batch
/// does not inherit some other file's fold state by index.
#[test]
fn collapse_survives_a_later_streaming_batch() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 2)];
    app.search.rebuild_rows();
    app.search.toggle_file(std::path::Path::new("/w/a.rs"));
    app.search
        .hits
        .insert(0, search_hit(std::path::Path::new("/w/z.rs"), 1));
    app.search.rebuild_rows();
    assert_eq!(
        app.search.rows.iter().find(|row| row.hit() == 1),
        Some(&SearchRow::File {
            hit: 1,
            count: 2,
            expanded: false
        }),
        "a.rs stays folded even though its index moved"
    );
}

#[tokio::test]
async fn a_small_result_set_arrives_expanded_and_a_large_one_collapsed() {
    let dir = test_dir("search-adaptive");
    write_file(&dir, "a.rs", b"needle\nneedle\nneedle\n");

    let (local_backend, _snaps) = local(SessionConfig {
        roots: vec![dir.clone()],
        ..SessionConfig::default()
    });
    let backend: Arc<dyn Backend> = Arc::new(local_backend);
    let mut events = backend.take_events().expect("backend event stream");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend);
    app.search.query = "needle".to_string();
    app.run_global_search();
    pump_until(&mut app, &mut events, |app| app.search.searched).await;

    assert_eq!(app.search.matches_found, 3);
    assert!(
        app.search.collapsed.is_empty(),
        "three matches is small, so groups open"
    );
    assert_eq!(app.search.rows.len(), 4, "one heading over three matches");

    // The same panel, told it found far more, folds instead.
    app.search.searching = Some(RequestId(u64::MAX));
    app.search_finished(Some(RequestId(u64::MAX)), 1, 5_000, true, None);
    assert_eq!(app.search.rows.len(), 1, "only the heading survives");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancelling cannot recall a batch already in the event channel, so the panel
/// must drop answers that do not belong to the search it is waiting for.
#[test]
fn a_stale_batch_never_overwrites_a_newer_search() {
    let mut app = app();
    app.search.searching = Some(RequestId(7));
    app.search_progress(
        Some(RequestId(6)),
        vec![search_hit(std::path::Path::new("/w/stale.rs"), 1)],
        10,
        1,
    );
    assert!(app.search.hits.is_empty(), "the stale batch is dropped");

    app.search_progress(
        Some(RequestId(7)),
        vec![search_hit(std::path::Path::new("/w/live.rs"), 1)],
        10,
        1,
    );
    assert_eq!(app.search.hits.len(), 1);
    assert!(app.search.hits[0].path.ends_with("live.rs"));
}

#[test]
fn a_stale_completion_does_not_end_the_running_search() {
    let mut app = app();
    app.search.searching = Some(RequestId(7));
    app.search_finished(Some(RequestId(6)), 1, 1, false, None);
    assert_eq!(
        app.search.searching,
        Some(RequestId(7)),
        "the live search is still running"
    );
    assert!(!app.search.searched);
}

/// This command is labelled "Search: Leave Panel" and bound to plain Esc, but it
/// set the app-level quit flag — so Esc while browsing results quit the editor.
#[test]
fn leaving_the_search_panel_does_not_quit_the_editor() {
    let mut app = app();
    app.start_global_search();
    app.dispatch(Command::SearchQuit);
    assert!(!app.should_quit, "Esc in the results list must not quit");
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn opening_a_match_row_lands_on_that_match_not_the_first() {
    let dir = test_dir("search-open-match");
    write_file(&dir, "a.rs", b"let a = 1;\nlet needle = 2;\nlet c = 3;\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.hits = vec![karet_session::SearchHit {
        path: dir.join("a.rs"),
        matches: vec![
            karet_session::SearchMatch {
                range: Range {
                    start: LineCol::new(0, 4),
                    end: LineCol::new(0, 5),
                },
                line_text: "let a = 1;".into(),
                preview_start: 4,
                preview_end: 5,
            },
            karet_session::SearchMatch {
                range: Range {
                    start: LineCol::new(1, 4),
                    end: LineCol::new(1, 10),
                },
                line_text: "let needle = 2;".into(),
                preview_start: 4,
                preview_end: 10,
            },
        ],
    }];
    app.search.rebuild_rows();
    // Row 2 is the *second* match, not the heading and not the first match.
    app.search.selection.move_to(2);
    app.open_selected_result();

    assert_eq!(
        app.tabs[app.active].editor.cursor(),
        LineCol::new(1, 4),
        "lands on the match's own line and column"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn collapse_from_a_match_row_walks_up_to_its_heading() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 3)];
    app.search.rebuild_rows();
    app.search.selection.move_to(3); // the third match
    app.dispatch(Command::SearchCollapse);
    assert_eq!(app.search.selection.cursor(), 0, "cursor is on the heading");
    // A second press folds the group it just walked up to.
    app.dispatch(Command::SearchCollapse);
    assert_eq!(app.search.rows.len(), 1);
}

#[test]
fn globs_split_on_commas_and_whitespace() {
    assert_eq!(
        SearchPanel::globs("*.rs, src/**  ,\tdocs/*.md"),
        vec!["*.rs", "src/**", "docs/*.md"]
    );
    assert!(SearchPanel::globs("   ,, ").is_empty());
}

#[test]
fn the_glob_fields_reach_the_query() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.includes = "*.rs, src/**".into();
    app.search.excludes = "**/target/**".into();
    let query = app.build_search_query();
    assert_eq!(query.includes, vec!["*.rs", "src/**"]);
    assert_eq!(query.excludes, vec!["**/target/**"]);
}

/// Hiding the fields must also stop them filtering — a search left narrowed by
/// globs that are no longer on screen is a trap.
#[test]
fn hiding_the_filters_clears_them() {
    let mut app = app();
    app.dispatch(Command::SearchToggleFilters);
    assert!(app.search.filters_visible);
    assert_eq!(app.search.field, SearchPanelField::Includes);
    app.search.includes = "*.rs".into();

    app.dispatch(Command::SearchToggleFilters);
    assert!(!app.search.filters_visible);
    assert!(app.search.includes.is_empty());
    assert_eq!(app.search.field, SearchPanelField::Find);
    assert!(app.build_search_query().includes.is_empty());
}

/// Tab must never park the cursor on a field the user cannot see.
#[test]
fn field_cycling_skips_the_hidden_glob_fields() {
    let mut app = app();
    app.search_toggle_field(); // Find -> Replace
    assert_eq!(app.search.field, SearchPanelField::Replace);
    app.search_toggle_field(); // filters hidden, so back to Find
    assert_eq!(app.search.field, SearchPanelField::Find);

    app.search.filters_visible = true;
    app.search_toggle_field(); // Find -> Replace
    app.search_toggle_field(); // Replace -> Includes
    assert_eq!(app.search.field, SearchPanelField::Includes);
    app.search_toggle_field(); // Includes -> Excludes
    assert_eq!(app.search.field, SearchPanelField::Excludes);
    app.search_toggle_field(); // wraps to Find
    assert_eq!(app.search.field, SearchPanelField::Find);
}

#[test]
fn search_edit_targets_the_active_glob_field() {
    let mut app = app();
    app.search.field = SearchPanelField::Includes;
    app.search_edit(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.search.field = SearchPanelField::Excludes;
    app.search_edit(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(
        (app.search.includes.as_str(), app.search.excludes.as_str()),
        ("x", "y")
    );
}

#[tokio::test]
async fn an_include_glob_narrows_the_result_set() {
    let dir = test_dir("search-globs");
    write_file(&dir, "a.rs", b"needle\n");
    write_file(&dir, "b.txt", b"needle\n");

    let (local_backend, _snaps) = local(SessionConfig {
        roots: vec![dir.clone()],
        ..SessionConfig::default()
    });
    let backend: Arc<dyn Backend> = Arc::new(local_backend);
    let mut events = backend.take_events().expect("backend event stream");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend);
    app.search.query = "needle".to_string();
    app.run_global_search();
    pump_until(&mut app, &mut events, |app| app.search.searched).await;
    assert_eq!(app.search.hits.len(), 2, "both files match unfiltered");

    app.search.includes = "*.rs".into();
    app.run_global_search();
    pump_until(&mut app, &mut events, |app| app.search.searched).await;
    assert_eq!(app.search.hits.len(), 1);
    assert!(app.search.hits[0].path.ends_with("a.rs"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// An automatic default may choose the starting fold state, but it must not undo
/// a fold the user set while results were still streaming in.
#[test]
fn finishing_a_search_keeps_folds_the_user_set_while_it_streamed() {
    let mut app = app();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![
            search_hit(std::path::Path::new("/w/a.rs"), 2),
            search_hit(std::path::Path::new("/w/b.rs"), 1),
        ],
        2,
        3,
    );
    // The user folds one file mid-stream.
    app.search.selection.move_to(0);
    app.dispatch(Command::SearchCollapse);
    assert!(
        app.search
            .collapsed
            .contains(std::path::Path::new("/w/a.rs"))
    );

    // A small result set would otherwise expand everything on completion.
    app.search_finished(Some(RequestId(1)), 2, 3, false, None);
    assert!(
        app.search
            .collapsed
            .contains(std::path::Path::new("/w/a.rs")),
        "the user's fold survives the adaptive default"
    );
}

/// Any file save re-runs a live search through the watcher. Losing your place in
/// the results every time a file is written makes the panel unusable alongside
/// editing, and master's `set_len` clamped the cursor rather than resetting it.
#[test]
fn re_running_a_search_keeps_the_cursor() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 4)],
        1,
        4,
    );
    app.search.selection.move_to(3);

    // No backend in this app, so the command goes nowhere — the state handling
    // around it is what is under test.
    app.run_global_search();
    assert!(app.search.rows.is_empty(), "the re-run empties the list");

    // The cursor is restored once rows exist again.
    app.search.searching = Some(RequestId(2));
    app.search_progress(
        Some(RequestId(2)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 4)],
        1,
        4,
    );
    assert_eq!(
        app.search.selection.cursor(),
        3,
        "the reader keeps their place"
    );
}

#[test]
fn re_running_a_search_keeps_the_folds() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![
            search_hit(std::path::Path::new("/w/a.rs"), 2),
            search_hit(std::path::Path::new("/w/b.rs"), 2),
        ],
        2,
        4,
    );
    app.search.selection.move_to(0);
    app.dispatch(Command::SearchCollapse);
    let folded = app.search.collapsed.clone();
    assert!(!folded.is_empty());

    app.run_global_search();
    assert_eq!(
        app.search.collapsed, folded,
        "folds carry across the re-run"
    );
    assert!(
        app.search.folds_touched,
        "so the adaptive default still does not undo them"
    );
}

/// The cursor is clamped, not blindly restored, when the new result set is shorter.
#[test]
fn a_restored_cursor_clamps_into_a_shorter_result_set() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 8)],
        1,
        8,
    );
    app.search.selection.move_to(8);
    app.run_global_search();
    app.search.searching = Some(RequestId(2));
    app.search_progress(
        Some(RequestId(2)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 1)],
        1,
        1,
    );
    assert!(
        app.search.selection.cursor() < app.search.rows.len(),
        "cursor {} is inside {} rows",
        app.search.selection.cursor(),
        app.search.rows.len()
    );
}

/// Seed the layout a render would have produced, so a click can be hit-tested:
/// the results start at row 5 of a 20-row sidebar with no scroll offset.
fn seed_result_click_layout(app: &mut App) {
    app.sidebar_panel = SidebarPanel::Search;
    app.sidebar_visible = true;
    app.sidebar_rect = Rect::new(0, 0, 30, 20);
    app.search_ui.results_rect = Rect::new(0, 5, 30, 10);
    app.search_ui.offset = 0;
}

/// The chevron is a two-cell target in a narrow sidebar, which is easy to miss.
/// Double-clicking anywhere on the heading is the second way to fold a group.
#[test]
fn double_clicking_a_file_heading_folds_its_group() {
    let dir = test_dir("search-double-click-fold");
    write_file(&dir, "a.rs", b"let needle0 = 1;\nlet needle1 = 1;\n");
    let mut app = app();
    app.root = dir.clone();
    seed_result_click_layout(&mut app);
    app.search.hits = vec![search_hit(&dir.join("a.rs"), 2)];
    app.search.rebuild_rows();

    // Column 3 clears the chevron, so the first click is a plain open.
    app.handle_sidebar_click(3, 5, KeyModifiers::NONE);
    assert!(
        app.search.collapsed.is_empty(),
        "a single click opens, it does not fold"
    );

    // The second click lands in the same cell inside the streak window.
    app.handle_sidebar_click(3, 5, KeyModifiers::NONE);
    assert!(
        app.search.collapsed.contains(dir.join("a.rs").as_path()),
        "the double-click folds the group"
    );

    app.handle_sidebar_click(3, 5, KeyModifiers::NONE);
    app.handle_sidebar_click(3, 5, KeyModifiers::NONE);
    assert!(
        app.search.collapsed.is_empty(),
        "and folds it back open again"
    );
}

/// A match row is a leaf: it has no group of its own, so the second click must
/// not reach for its parent's fold.
#[test]
fn double_clicking_a_match_row_folds_nothing() {
    let dir = test_dir("search-double-click-match");
    write_file(&dir, "a.rs", b"let needle0 = 1;\nlet needle1 = 1;\n");
    let mut app = app();
    app.root = dir.clone();
    seed_result_click_layout(&mut app);
    app.search.hits = vec![search_hit(&dir.join("a.rs"), 2)];
    app.search.rebuild_rows();

    // Row 6 is the first match under the heading at row 5.
    app.handle_sidebar_click(6, 6, KeyModifiers::NONE);
    app.handle_sidebar_click(6, 6, KeyModifiers::NONE);
    assert!(
        app.search.collapsed.is_empty(),
        "a leaf has nothing to fold"
    );
    assert_eq!(app.search.selection.cursor(), 1);
}

/// Clicking a result moves the panel's focus onto the list, so the arrow keys
/// that follow navigate rows instead of editing whichever field was last active.
#[test]
fn clicking_a_result_moves_focus_out_of_the_fields() {
    let dir = test_dir("search-click-focus");
    write_file(&dir, "a.rs", b"let needle0 = 1;\n");
    let mut app = app();
    app.root = dir.clone();
    seed_result_click_layout(&mut app);
    app.search.hits = vec![search_hit(&dir.join("a.rs"), 1)];
    app.search.rebuild_rows();
    app.search.input = true;

    app.handle_sidebar_click(3, 5, KeyModifiers::NONE);
    assert!(!app.search.input, "the results hold the focus now");
}
