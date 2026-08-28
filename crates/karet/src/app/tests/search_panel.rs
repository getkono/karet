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
