//! Opening the Seam view, and the guards that keep one index behind one view.
//!
//! The pure navigation model is covered in `app::seam::tests`; these cover what the app
//! does around it — which root is asked for, which tab an answer lands on, and which
//! answers are refused.

use std::sync::Arc;

use karet_session::api::SeamNodeView;
use karet_session::api::SeamSummary;

use super::support::*;
use crate::app::*;

/// The index requests a backend received, in order.
fn index_requests(backend: &RecordingBackend) -> Vec<(RequestId, PathBuf)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::IndexSeams {
                        root: Some(root), ..
                    } => Some((*id, root.clone())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The re-index requests a backend received.
fn reindex_paths(backend: &RecordingBackend) -> Vec<PathBuf> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::ReindexSeams { path, .. } => Some(path.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One indexed node, located in `file`.
fn node(id: &str, file: &str) -> SeamNodeView {
    SeamNodeView {
        id: id.to_owned(),
        name: id.rsplit("::").next().unwrap_or(id).to_owned(),
        kind: "function".to_owned(),
        detail: None,
        file: PathBuf::from(file),
        range: karet_core::Range::default(),
        selection: karet_core::Range::default(),
        parent: None,
        children: Vec::new(),
        facets: Vec::new(),
        rollups: [0; 5],
        visibility: None,
        membership: "active".to_owned(),
        provisional: false,
    }
}

fn summary(package: &str, packages: usize) -> SeamSummary {
    SeamSummary {
        package: package.to_owned(),
        packages,
        nodes: 1,
        files: 1,
        configuration: "unconfigured".to_owned(),
        available_configurations: vec!["unconfigured".to_owned()],
        variation_complete: false,
        truncated_after: None,
        unresolved_modules: Vec::new(),
    }
}

/// An app with a recording backend and the Seam view open on `root`.
fn seam_app(root: &str) -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_seam_view_at(PathBuf::from(root));
    (backend, app)
}

fn seam_tabs(app: &App) -> usize {
    app.all_tabs()
        .filter(|tab| matches!(tab.kind, TabKind::Seam(_)))
        .count()
}

#[test]
fn opening_reserves_the_tab_before_the_index_exists() {
    // The surface has to switch at once; a repository index takes seconds behind it.
    let (backend, mut app) = seam_app("/repo/crates/core");

    assert!(matches!(app.tabs[app.active].kind, TabKind::Seam(_)));
    assert!(app.seam_view().is_some_and(|state| state.is_loading()));
    assert_eq!(
        index_requests(&backend)
            .into_iter()
            .map(|(_, root)| root)
            .collect::<Vec<_>>(),
        [PathBuf::from("/repo/crates/core")]
    );
}

#[test]
fn opening_at_another_root_re_points_the_one_seam_tab() {
    // One index sits behind the view, so a second tab on another root would answer this
    // one's questions.
    let (backend, mut app) = seam_app("/repo/crates/core");
    let view = app.tabs[app.active].view;

    app.open_seam_view_at(PathBuf::from("/repo/crates/text"));

    assert_eq!(seam_tabs(&app), 1, "a second Seam tab was opened");
    // Re-pointed rather than replaced: moving where a view reads is not closing it.
    assert_eq!(app.tabs[app.active].view, view);
    // The title follows the new root, and is known without waiting for an answer.
    assert_eq!(app.tabs[app.active].title, "⌗ text");
    assert_eq!(index_requests(&backend).len(), 2);
}

#[test]
fn a_tab_that_failed_is_re_pointed_rather_than_left_behind() {
    let (_backend, mut app) = seam_app("/repo/nothing");
    let id = app.seam_index_req;
    app.on_seam_index_failed(id, "nothing to index".to_owned());
    assert!(app.seam_view().is_some_and(|state| state.error.is_some()));

    app.open_seam_view_at(PathBuf::from("/repo/crates/core"));

    assert_eq!(seam_tabs(&app), 1, "the failed tab was left behind");
    assert!(app.seam_view().is_some_and(|state| state.error.is_none()));
}

#[test]
fn an_answer_reaches_the_seam_view_even_when_another_tab_is_active() {
    // Indexing a repository takes seconds; a reader who moved on must still get the tree.
    let (_backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.push_tab(text_tab("main.rs", "fn main() {}\n"));
    assert!(!matches!(app.tabs[app.active].kind, TabKind::Seam(_)));

    app.on_seam_indexed(
        id,
        summary("repo", 2),
        vec![node("core::thing", "/repo/a.rs")],
    );

    assert!(app.seam_view().is_some_and(|state| state.nodes.len() == 1));
}

#[test]
fn a_stale_index_answer_never_lands_on_the_view() {
    // Open at one root, immediately at another: the first index is still running, and its
    // answer belongs to a view that no longer exists.
    let (_backend, mut app) = seam_app("/repo/crates/core");
    let stale = app.seam_index_req;
    app.open_seam_view_at(PathBuf::from("/repo/crates/text"));

    app.on_seam_indexed(
        stale,
        summary("core", 1),
        vec![node("core::thing", "/a.rs")],
    );

    assert!(
        app.seam_view().is_some_and(|state| state.is_loading()),
        "a superseded answer was adopted"
    );
}

#[test]
fn a_stale_failure_never_lands_on_the_view() {
    let (_backend, mut app) = seam_app("/repo/crates/core");
    let stale = app.seam_index_req;
    app.open_seam_view_at(PathBuf::from("/repo/crates/text"));

    app.on_seam_index_failed(stale, "no package".to_owned());

    assert!(app.seam_view().is_some_and(|state| state.error.is_none()));
}

#[test]
fn an_answer_is_adopted_only_once() {
    let (_backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.on_seam_indexed(id, summary("repo", 1), vec![node("core::thing", "/a.rs")]);
    assert!(app.seam_view().is_some_and(|state| state.error.is_none()));

    // The request is settled, so a duplicate delivery is as stale as any other.
    app.on_seam_index_failed(id, "should not apply".to_owned());
    assert!(app.seam_view().is_some_and(|state| state.error.is_none()));
}

#[test]
fn a_save_inside_the_index_re_indexes_it() {
    let (backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.on_seam_indexed(
        id,
        summary("repo", 1),
        vec![node("core::thing", "/repo/a.rs")],
    );

    app.reindex_seams(Path::new("/repo/a.rs"), "fn thing() {}".to_owned());

    assert_eq!(reindex_paths(&backend), [PathBuf::from("/repo/a.rs")]);
}

#[test]
fn a_save_the_index_never_read_does_not_re_index() {
    // With a repository root every file sits under the root, so containment would answer
    // yes here and re-index the whole repository on every save.
    let (backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.on_seam_indexed(
        id,
        summary("repo", 1),
        vec![node("core::thing", "/repo/a.rs")],
    );

    app.reindex_seams(
        Path::new("/repo/deep/elsewhere.rs"),
        "fn other() {}".to_owned(),
    );

    assert!(reindex_paths(&backend).is_empty());
}

#[test]
fn nothing_is_re_indexed_before_the_first_index_arrives() {
    // A re-index is never coalesced away, so one queued behind a first index would run in
    // full against a tree about to be replaced.
    let (backend, mut app) = seam_app("/repo");

    app.reindex_seams(Path::new("/repo/a.rs"), "fn thing() {}".to_owned());

    assert!(reindex_paths(&backend).is_empty());
}

#[test]
fn nothing_is_re_indexed_after_the_index_failed() {
    let (backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.on_seam_index_failed(id, "nothing to index".to_owned());

    app.reindex_seams(Path::new("/repo/a.rs"), "fn thing() {}".to_owned());

    assert!(reindex_paths(&backend).is_empty());
}

/// A source preview for `id`, three lines of context either side of one body line.
fn preview(file: &str) -> karet_session::api::SeamPreview {
    karet_session::api::SeamPreview {
        file: PathBuf::from(file),
        lines: (0..7).map(|n| format!("line {n}")).collect(),
        numbers: (10..17).collect(),
        body_start: 3,
        head_end: 4,
        body_end: 4,
        dropped: 0,
        context: 3,
        tokens: Vec::new(),
    }
}

/// Land an index and answer the detail request it fires, leaving a settled view.
fn settled(app: &mut App) {
    let id = app.seam_index_req;
    app.on_seam_indexed(
        id,
        summary("repo", 1),
        vec![node("core::thing", "/repo/a.rs")],
    );
    let detail = app.seam_node_req;
    app.on_seam_node_detail(
        detail,
        "core::thing".to_owned(),
        Vec::new(),
        Ok(preview("/repo/a.rs")),
    );
}

#[test]
fn a_fresh_index_asks_for_the_landing_nodes_detail() {
    // Without this the detail pane stays empty until a key arrives, which reads as a
    // view that failed to load rather than one waiting on nothing.
    let (_backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    app.on_seam_indexed(
        id,
        summary("repo", 1),
        vec![node("core::thing", "/repo/a.rs")],
    );
    assert!(app.seam_node_req.is_some());
}

#[test]
fn a_detail_request_is_pending_until_it_is_answered() {
    let (_backend, mut app) = seam_app("/repo");
    settled(&mut app);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    assert!(state.detail_since.is_none());
    assert!(matches!(state.preview, Some(Ok(_))));
}

#[test]
fn a_preview_for_a_node_the_reader_left_is_ignored() {
    let (_backend, mut app) = seam_app("/repo");
    settled(&mut app);
    let detail = app.seam_node_req;
    // The guard already protected the edges; the source must not be the exception.
    app.on_seam_node_detail(
        detail,
        "core::somewhere_else".to_owned(),
        Vec::new(),
        Ok(preview("/repo/b.rs")),
    );
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    let Some(Ok(held)) = &state.preview else {
        panic!("expected the earlier preview to stand");
    };
    assert_eq!(held.file, PathBuf::from("/repo/a.rs"));
}

#[test]
fn moving_the_selection_forgets_the_previous_nodes_source() {
    let (_backend, mut app) = seam_app("/repo");
    settled(&mut app);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    state.move_row(1);
    // Edges and source describe the same node, so they go together — a pane holding
    // half of each would be a lie about the other half.
    assert!(state.preview.is_none());
    assert!(state.edges.is_empty());
}

#[test]
fn an_indexing_view_schedules_its_own_reveal() {
    // Otherwise the placeholder appears only if a key happens to arrive, which at
    // repository scale means seconds of a blank pane.
    let (_backend, mut app) = seam_app("/repo");
    assert!(!app.pendings().is_empty(), "no repaint was scheduled");

    let id = app.seam_index_req;
    app.on_seam_indexed(
        id,
        summary("repo", 1),
        vec![node("core::thing", "/repo/a.rs")],
    );
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    assert!(
        state.loading_since.is_none(),
        "the reveal outlived the answer"
    );
    // The landing node's detail is now what is outstanding, and it schedules its own.
    assert!(state.detail_since.is_some());
    assert!(!app.pendings().is_empty());
}

// --- the mouse --------------------------------------------------------------

/// A seam view with a small tree and a hand-seeded hit map, ready for the pointer.
///
/// Seeded rather than rendered: what the renderer records is pinned by the render tests,
/// and driving a terminal here would only couple these to a layout they do not assert.
fn moused_app() -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = seam_app("/repo");
    let id = app.seam_index_req;
    let mut root = node("pkg", "/repo/a.rs");
    root.children = vec!["pkg::one".to_owned(), "pkg::two".to_owned()];
    let mut one = node("pkg::one", "/repo/a.rs");
    one.parent = Some("pkg".to_owned());
    one.children = vec!["pkg::one::leaf".to_owned()];
    let mut two = node("pkg::two", "/repo/a.rs");
    two.parent = Some("pkg".to_owned());
    let mut leaf = node("pkg::one::leaf", "/repo/a.rs");
    leaf.parent = Some("pkg::one".to_owned());
    app.on_seam_indexed(id, summary("pkg", 1), vec![root, one, two, leaf]);

    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    state.hits.reset(Rect::new(0, 0, 100, 24));
    state.hits.header = Rect::new(0, 0, 100, 1);
    state.hits.spine = Rect::new(0, 1, 100, 12);
    state.hits.facets = Rect::new(0, 13, 100, 9);
    state.hits.query = Rect::new(0, 23, 100, 1);
    state.hits.config = Rect::new(30, 0, 12, 1);
    state.hits.lenses = vec![(Rect::new(50, 0, 8, 1), 0), (Rect::new(58, 0, 8, 1), 4)];
    state.hits.rows = vec![
        (Rect::new(0, 1, 20, 1), "pkg".to_owned()),
        (Rect::new(21, 1, 20, 1), "pkg::one".to_owned()),
        (Rect::new(21, 2, 20, 1), "pkg::two".to_owned()),
    ];
    (backend, app)
}

fn press(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn wheel(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn node_requests(backend: &RecordingBackend) -> Vec<String> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::SeamNode { path } => Some(path.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn clicking_a_row_selects_it_and_asks_for_its_detail() {
    let (backend, mut app) = moused_app();
    press(&mut app, 22, 2);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    assert_eq!(state.selected_id(), Some("pkg::two"));
    assert!(node_requests(&backend).contains(&"pkg::two".to_owned()));
}

#[test]
fn clicking_a_row_twice_steps_into_it() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 22, 1);
    press(&mut app, 22, 1);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    // The same thing Enter does, and it must not stack a crumb either.
    assert_eq!(state.narrow.len(), 1);
    assert_eq!(state.root_set(), ["pkg::one"]);
}

#[test]
fn clicking_a_crumb_widens_back_to_it() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 22, 1);
    press(&mut app, 22, 1);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    state.hits.crumbs = vec![(Rect::new(0, 0, 6, 1), 0), (Rect::new(8, 0, 5, 1), 1)];
    // The package name is the crumb before every other: it widens all the way out.
    press(&mut app, 1, 0);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    assert!(state.narrow.is_empty());
}

#[test]
fn clicking_a_lens_toggles_it_and_clicking_again_clears_it() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 51, 0);
    assert!(
        app.active_seam()
            .is_some_and(|state| state.lenses.contains("api"))
    );
    press(&mut app, 51, 0);
    assert!(
        app.active_seam()
            .is_some_and(|state| state.lenses.is_empty())
    );
}

#[test]
fn clicking_the_configuration_marker_cycles_it() {
    let (backend, mut app) = moused_app();
    if let Some(state) = app.active_seam() {
        state.summary.available_configurations = vec!["default".to_owned(), "tests".to_owned()];
        state.summary.configuration = "default".to_owned();
    }
    press(&mut app, 31, 0);
    let cycled = backend.sent.lock().is_ok_and(|sent| {
        sent.iter()
            .any(|(_, c)| matches!(c, SessionCommand::SetSeamConfiguration { .. }))
    });
    assert!(
        cycled,
        "the marker names it, so clicking it should change it"
    );
}

#[test]
fn clicking_the_widen_affordance_steps_out_one_narrow() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 22, 1);
    press(&mut app, 22, 1);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    state.hits.widen = Rect::new(60, 23, 12, 1);
    press(&mut app, 62, 23);
    assert!(
        app.active_seam()
            .is_some_and(|state| state.narrow.is_empty())
    );
}

#[test]
fn clicking_the_query_line_focuses_it() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 4, 23);
    assert!(
        app.active_seam()
            .is_some_and(|state| state.focus == crate::app::seam::SeamFocus::Query)
    );
}

#[test]
fn a_wheel_notch_moves_the_selection_by_one_row() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 22, 1);
    wheel(&mut app, MouseEventKind::ScrollDown, 22, 1);
    let Some(state) = app.active_seam() else {
        panic!("no seam view");
    };
    // One row per notch: the offset is pinned to the selection by the render, so a free
    // scroll would snap straight back.
    assert_eq!(state.selected_id(), Some("pkg::two"));
}

#[test]
fn a_horizontal_wheel_moves_between_columns() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 2, 1);
    let before = app.active_seam().map_or(0, |state| state.focused_column);
    wheel(&mut app, MouseEventKind::ScrollRight, 2, 1);
    let after = app.active_seam().map_or(0, |state| state.focused_column);
    assert_eq!(after, before + 1);
}

#[test]
fn a_press_on_the_seam_view_never_arms_a_text_drag() {
    let (_backend, mut app) = moused_app();
    press(&mut app, 22, 2);
    // Falling through to the editor would leave the shell waiting for a selection drag
    // that is never coming, and swallow the next release as if it were one.
    assert!(app.editor_drag.is_none());
}

#[test]
fn a_press_outside_the_view_is_not_this_views_business() {
    let (_backend, mut app) = moused_app();
    let handled = app.seam_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 200,
        row: 200,
        modifiers: KeyModifiers::NONE,
    });
    assert!(!handled);
}

#[test]
fn a_right_press_falls_through_so_the_pane_menu_still_opens() {
    let (_backend, mut app) = moused_app();
    let handled = app.seam_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 22,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });
    assert!(!handled);
}
