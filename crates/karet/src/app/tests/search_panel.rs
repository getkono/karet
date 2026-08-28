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
/// The click also opens the file, which must not undo that by handing the
/// keyboard to the editor.
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
    assert_eq!(app.focus, Focus::Sidebar, "and the panel still has them");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `Enter` on a hit is a step *through* the list, not out of it: the file opens
/// at the match, and the panel keeps the keyboard so the next arrow reaches the
/// next hit. Before this the open moved focus to the editor and ended the browse.
#[test]
fn opening_a_result_leaves_the_keyboard_with_the_panel() {
    let dir = test_dir("search-open-focus");
    write_file(
        &dir,
        "a.rs",
        b"let needle0 = 1;\nlet needle1 = 2;\nlet needle2 = 3;\n",
    );
    let mut app = app();
    app.root = dir.clone();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.hits = vec![search_hit(&dir.join("a.rs"), 3)];
    app.search.rebuild_rows();
    app.search.input = false;
    // Row 2 is the *second* match: a heading would open line 0 and pass anyway.
    app.search.selection.move_to(2);

    app.dispatch(Command::SearchOpen);
    assert_eq!(
        app.tabs[app.active].editor.cursor(),
        LineCol::new(1, 4),
        "the file opened at the match"
    );
    assert_eq!(app.focus, Focus::Sidebar, "but the panel kept the keyboard");

    // …so the browse carries on where it left off.
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(!app.search.input);
    assert_eq!(
        app.search.selection.cursor(),
        3,
        "the next arrow moves rows"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A match has no children, so `Right` has nothing to step into. Falling through
/// to a plain move made the key read as a stray `Down`.
#[test]
fn expand_on_a_match_row_is_a_no_op() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 3)];
    app.search.rebuild_rows();
    app.search.selection.move_to(1);
    app.dispatch(Command::SearchExpand);
    assert_eq!(app.search.selection.cursor(), 1, "a leaf absorbs the key");
}

#[test]
fn expand_steps_into_an_already_open_group() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 3)];
    app.search.rebuild_rows();
    app.search.selection.move_to(0);
    app.dispatch(Command::SearchExpand);
    assert_eq!(app.search.selection.cursor(), 1, "onto the first match");
}

/// Repeated `Left` walks up out of the tree: match → its heading → the heading
/// above it, so it steps through the result set a file at a time.
#[test]
fn collapse_on_a_shut_heading_walks_to_the_previous_file() {
    let mut app = app();
    app.search.hits = vec![
        search_hit(std::path::Path::new("/w/a.rs"), 2),
        search_hit(std::path::Path::new("/w/b.rs"), 2),
    ];
    app.search.collapsed = [std::path::PathBuf::from("/w/a.rs")].into_iter().collect();
    app.search.rebuild_rows();
    // Rows: 0 = a.rs (shut), 1 = b.rs, 2..3 = b's matches.
    app.search.selection.move_to(1);
    app.dispatch(Command::SearchCollapse);
    assert!(
        app.search
            .collapsed
            .contains(std::path::Path::new("/w/b.rs")),
        "the first press shuts the open group"
    );
    app.dispatch(Command::SearchCollapse);
    assert_eq!(
        app.search.selection.cursor(),
        0,
        "the second walks up to the file above"
    );
}

#[test]
fn collapse_on_the_first_shut_heading_stays_put() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 2)];
    app.search.collapsed = [std::path::PathBuf::from("/w/a.rs")].into_iter().collect();
    app.search.rebuild_rows();
    app.search.selection.move_to(0);
    app.dispatch(Command::SearchCollapse);
    assert_eq!(app.search.selection.cursor(), 0, "nothing above to walk to");
}

/// No arrow lifts the caret out of the field being typed in — not even off the
/// last visible one, where the ring this replaces used to drop into the results.
/// Driven through `handle_key` so the binding table and the `search_edit`
/// fall-through are both in the loop, which is where the crossing lived.
#[test]
fn an_arrow_never_leaves_the_field_it_starts_in() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 2)];
    app.search.rebuild_rows();
    app.search.replace_visible = true;
    app.search.filters_visible = true;
    app.search.query = "needle".into();

    for field in [
        SearchPanelField::Find,
        SearchPanelField::Replace,
        SearchPanelField::Includes,
        SearchPanelField::Excludes,
    ] {
        app.search_focus_field(field);
        for code in [KeyCode::Down, KeyCode::Up] {
            send_key(&mut app, code, KeyModifiers::NONE);
            assert!(app.search.input, "{field:?} keeps the focus on {code:?}");
            assert_eq!(app.search.field, field, "and stays this field");
            assert_eq!(app.search.selection.cursor(), 0, "the rows do not move");
        }
    }
    assert_eq!(app.search.query, "needle", "and no arrow typed anything");
}

/// The query is the only visible field when both sections are collapsed — the
/// case the old ring turned into a one-key hop into the results — and an arrow
/// must not reveal a hidden section the way `Tab` does either.
#[test]
fn an_arrow_in_the_lone_query_field_neither_enters_the_list_nor_reveals_a_section() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 1)];
    app.search.rebuild_rows();
    app.search.replace_visible = false;
    app.search.filters_visible = false;
    app.search_focus_field(SearchPanelField::Find);

    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(app.search.input, "the list is still one Enter/Esc away");
    assert!(
        !app.search.replace_visible,
        "Tab reveals, the arrows do not"
    );
    assert!(!app.search.filters_visible);
}

/// The mirror direction: browsing the results with the arrows never falls out of
/// the list into a text box at its first row, empty list included.
#[test]
fn an_arrow_never_leaves_the_result_list() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 2)];
    app.search.rebuild_rows();
    app.search.replace_visible = true;
    app.search.input = false;
    app.search.selection.move_to(0);

    send_key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert!(!app.search.input, "no step back into the fields");
    assert_eq!(app.search.selection.cursor(), 0);

    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        app.search.selection.cursor(),
        1,
        "a plain move within the list"
    );

    // …and with nothing under it either, where the old ring found its escape.
    app.search.hits.clear();
    app.search.rebuild_rows();
    app.search.input = false;
    send_key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert!(!app.search.input, "an empty list is left with Esc, not Up");
}

/// `Cmd`+arrow is the one arrow that still does something in a field: it moves
/// the caret. `KeyChord::from_event` normalizes it to `Ctrl+Home`/`Ctrl+End`,
/// which is unbound in the Search modal, so it reaches `search_edit` carrying its
/// original `SUPER`. Removing the plain bindings must not have taken this too.
#[test]
fn a_command_arrow_still_moves_the_caret_in_a_field() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.query = "needle".into();
    app.search_focus_field(SearchPanelField::Find);
    assert_eq!(app.search.query_edit.cursor(), 6);

    send_key(&mut app, KeyCode::Up, KeyModifiers::SUPER);
    assert_eq!(
        app.search.query_edit.cursor(),
        0,
        "Cmd+Up is caret-to-start"
    );
    assert!(
        app.search.input,
        "and still a caret motion, not a focus move"
    );

    send_key(&mut app, KeyCode::Down, KeyModifiers::SUPER);
    assert_eq!(
        app.search.query_edit.cursor(),
        6,
        "Cmd+Down is caret-to-end"
    );
    assert!(app.search.input);
}

/// `j`/`k` browse the list exactly as the arrows do — neither drops out of it at
/// the first row.
#[test]
fn k_at_the_first_row_stays_in_the_list() {
    let mut app = app();
    app.search.hits = vec![search_hit(std::path::Path::new("/w/a.rs"), 2)];
    app.search.rebuild_rows();
    app.search.input = false;
    app.search.selection.move_to(0);
    app.dispatch(Command::SearchSelectUp);
    assert!(!app.search.input, "list-only, like the arrows");
    assert_eq!(app.search.selection.cursor(), 0);
}

/// A settled search with nothing in it leaves the results holding a focus with
/// no row under it, so the query takes it back — that is what you go on to edit.
#[test]
fn a_search_that_ends_empty_hands_focus_back_to_the_query() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.input = false;
    app.search.searching = Some(RequestId(1));
    app.search_finished(Some(RequestId(1)), 12, 0, false, None);
    assert!(app.search.input);
    assert_eq!(app.search.field, SearchPanelField::Find);
}

/// …but only once it has settled. Any file save re-runs a live search through
/// the watcher, and the list is empty for the whole window between the re-run
/// and its first batch. Grabbing focus there turns a reader's next arrow press
/// into typing.
#[test]
fn a_re_run_that_empties_the_list_does_not_grab_focus() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 4)],
        1,
        4,
    );
    app.search.input = false;
    app.search.selection.move_to(3);

    app.run_global_search();
    assert!(app.search.rows.is_empty(), "the re-run empties the list");
    assert!(!app.search.input, "the reader keeps the list's focus");
}

/// A search that finds something leaves the focus where it was.
#[test]
fn a_search_with_results_leaves_the_focus_on_the_list() {
    let mut app = app();
    app.search.query = "needle".into();
    app.search.searching = Some(RequestId(1));
    app.search_progress(
        Some(RequestId(1)),
        vec![search_hit(std::path::Path::new("/w/a.rs"), 2)],
        1,
        2,
    );
    app.search.input = false;
    app.search_finished(Some(RequestId(1)), 1, 2, false, None);
    assert!(!app.search.input);
}

/// The focus can only sit on a field the panel paints. Every path that hides a
/// section already bounces the field; this pins it as an invariant so a future
/// one cannot strand the caret off screen.
#[test]
fn rebuilding_rows_pulls_focus_off_a_hidden_field() {
    let mut app = app();
    app.search.filters_visible = false;
    app.search.input = true;
    app.search.field = SearchPanelField::Excludes;
    app.search.rebuild_rows();
    assert_eq!(app.search.field, SearchPanelField::Find);
}
