//! The full-screen commit-graph view: how history pages are routed to the view that
//! asked for them, how the viewport pans independently of the selection, and how far
//! ahead history is kept loaded.

use std::path::PathBuf;
use std::sync::Arc;

use super::support::*;
use crate::app::*;

/// A commit for graph tests, parented to `parents`.
fn graph_commit(hash: &str, parents: Vec<String>) -> Commit {
    Commit {
        hash: hash.to_string(),
        short_hash: hash.chars().take(7).collect(),
        summary: format!("summary {hash}"),
        author: "Tester".to_string(),
        time: 0,
        parents,
    }
}

/// Several graph views can be open at once, so a history page belongs to the exact view
/// that asked for it. Broadcasting a page would let a whole-repo log overwrite an open
/// file history (and vice versa).
#[test]
fn a_history_page_fills_only_the_view_that_requested_it() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let first = app.tabs[app.active].view;
    app.apply_graph_log(first, 0, vec![graph_commit("aaaa", Vec::new())], false);

    app.dispatch(Command::ShowCommitGraph);
    let second = app.tabs[app.active].view;
    assert_ne!(first, second, "opening the view twice yields two tabs");
    app.apply_graph_log(
        second,
        0,
        vec![
            graph_commit("cccc", vec!["dddd".to_string()]),
            graph_commit("dddd", Vec::new()),
        ],
        false,
    );

    let commits_of = |app: &App, view| {
        app.all_tabs()
            .find(|tab| tab.view == view)
            .and_then(|tab| match &tab.kind {
                TabKind::CommitGraph { commits, .. } => {
                    Some(commits.iter().map(|c| c.hash.clone()).collect::<Vec<_>>())
                },
                _ => None,
            })
            .unwrap_or_default()
    };
    assert_eq!(commits_of(&app, first), vec!["aaaa".to_string()]);
    assert_eq!(
        commits_of(&app, second),
        vec!["cccc".to_string(), "dddd".to_string()]
    );
}

/// The lane layout is cached alongside the commits, so it has to be rebuilt whenever
/// they change — a stale rail would paint the wrong graph.
#[test]
fn the_cached_lane_layout_tracks_the_loaded_commits() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    app.apply_graph_log(
        view,
        0,
        vec![
            graph_commit("aaaa", vec!["bbbb".to_string()]),
            graph_commit("bbbb", Vec::new()),
        ],
        true,
    );
    let rails_len = |app: &App| match &app.tabs[app.active].kind {
        TabKind::CommitGraph { rails, commits, .. } => (rails.len(), commits.len()),
        _ => (0, 0),
    };
    assert_eq!(rails_len(&app), (2, 2));

    // A second page appends, and the rails grow with it.
    app.apply_graph_log(view, 2, vec![graph_commit("cccc", Vec::new())], false);
    assert_eq!(rails_len(&app), (3, 3));
}

/// The viewport and the selection are independent: panning the graph must not drag the
/// cursor along, or a wide history could not be read without losing your place.
#[test]
fn scrolling_the_graph_leaves_the_selection_alone() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    let commits: Vec<Commit> = (0..40)
        .map(|i| graph_commit(&format!("{i:04}"), Vec::new()))
        .collect();
    app.apply_graph_log(view, 0, commits, false);
    if let TabKind::CommitGraph { list_rect, .. } = &mut app.tabs[app.active].kind {
        *list_rect = Rect::new(0, 0, 80, 10);
    }

    app.scroll_lines(5);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::CommitGraph {
            list_offset: 5,
            selected: 0,
            ..
        }
    ));

    // Moving the selection, by contrast, still drags the viewport to keep it visible.
    app.graph_select_to(30);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::CommitGraph { selected: 30, list_offset, .. } if *list_offset == 21
    ));
}

/// History is fetched far ahead of the viewport, which is what lets the graph render
/// "as far as the eye can see": the next page is asked for while the end of the loaded
/// run is still several screens away, so the trailing "more" row stays out of reach.
#[test]
fn history_is_prefetched_well_before_the_viewport_reaches_the_end() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    let commits: Vec<Commit> = (0..100)
        .map(|i| graph_commit(&format!("{i:04}"), Vec::new()))
        .collect();
    app.apply_graph_log(view, 0, commits, true);
    if let TabKind::CommitGraph { list_rect, .. } = &mut app.tabs[app.active].kind {
        *list_rect = Rect::new(0, 0, 80, 10);
    }

    let pages = |backend: &RecordingBackend| {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| {
                        matches!(command, SessionCommand::VcsLog { skip: 100, .. })
                    })
                    .count()
            })
            .unwrap_or_default()
    };
    // Still 70 rows from the end, and already topping up.
    app.graph_scroll_to(70);
    assert_eq!(pages(&backend), 1, "the next page is requested early");

    // While that page is in flight a second request is not piled on.
    app.graph_scroll_to(80);
    assert_eq!(pages(&backend), 1, "one page in flight at a time");
}

/// Prefetching stops at the end of history rather than asking forever.
#[test]
fn a_fully_loaded_history_never_asks_for_another_page() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    app.apply_graph_log(view, 0, vec![graph_commit("aaaa", Vec::new())], false);
    let before = backend.sent.lock().map(|s| s.len()).unwrap_or_default();
    app.graph_scroll_to(0);
    app.graph_prefetch();
    let after = backend.sent.lock().map(|s| s.len()).unwrap_or_default();
    assert_eq!(before, after, "no more history, no more requests");
}

/// A commit made outside the editor lands in an open whole-repo graph view, but not in a
/// file-history view — the event doesn't say whether it touched that file.
#[test]
fn live_commits_prepend_only_into_whole_repository_graphs() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let repo_view = app.tabs[app.active].view;
    app.apply_graph_log(repo_view, 0, vec![graph_commit("aaaa", Vec::new())], false);

    app.push_tab(Tab::commit_graph(
        Some(PathBuf::from("src/main.rs")),
        "history",
    ));
    let file_view = app.tabs[app.active].view;
    app.apply_graph_log(file_view, 0, vec![graph_commit("aaaa", Vec::new())], false);

    app.apply_vcs_commits_prepended(vec![graph_commit("bbbb", vec!["aaaa".to_string()])]);

    let hashes = |app: &App, view| {
        app.all_tabs()
            .find(|tab| tab.view == view)
            .and_then(|tab| match &tab.kind {
                TabKind::CommitGraph { commits, .. } => {
                    Some(commits.iter().map(|c| c.hash.clone()).collect::<Vec<_>>())
                },
                _ => None,
            })
            .unwrap_or_default()
    };
    assert_eq!(
        hashes(&app, repo_view),
        vec!["bbbb".to_string(), "aaaa".to_string()],
        "the new tip leads the repository log"
    );
    assert_eq!(
        hashes(&app, file_view),
        vec!["aaaa".to_string()],
        "a file history is left alone"
    );
}

/// Clicking a commit in the graph opens it as its own tab — the graph keeps the whole
/// pane, so the detail has to land somewhere else.
#[test]
fn clicking_a_graph_row_opens_that_commit_as_a_tab() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    app.apply_graph_log(
        view,
        0,
        vec![
            graph_commit("aaaa", vec!["bbbb".to_string()]),
            graph_commit("bbbb", Vec::new()),
        ],
        false,
    );
    if let TabKind::CommitGraph { list_rect, .. } = &mut app.tabs[app.active].kind {
        *list_rect = Rect::new(0, 3, 80, 10);
    }

    // The second row is the second commit.
    assert!(app.graph_click((10, 4)), "the click is consumed");
    assert!(
        matches!(&app.tabs[app.active].kind, TabKind::CommitLoading { rev, .. } if rev == "bbbb"),
        "the clicked commit opens in its own tab"
    );
}

/// A click outside the painted rows isn't the graph's to take.
#[test]
fn a_click_above_the_graph_rows_is_not_consumed() {
    let mut app = app();
    app.dispatch(Command::ShowCommitGraph);
    let view = app.tabs[app.active].view;
    app.apply_graph_log(view, 0, vec![graph_commit("aaaa", Vec::new())], false);
    if let TabKind::CommitGraph { list_rect, .. } = &mut app.tabs[app.active].kind {
        *list_rect = Rect::new(0, 3, 80, 10);
    }
    // Row 1 is the header, well above the rows rect.
    assert!(!app.graph_click((10, 1)));
}
