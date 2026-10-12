use super::support::*;
use crate::app::*;

/// A workspace holding `files`, with an app rooted on it and the receiver its
/// file walks arrive on.
fn workspace_app(
    files: &[&str],
) -> (
    tempfile::TempDir,
    App,
    tokio::sync::mpsc::UnboundedReceiver<workspace::Scan>,
) {
    let dir = tempfile::Builder::new()
        .prefix("karet-quick-open")
        .tempdir()
        .expect("temp dir");
    for file in files {
        let _ = std::fs::write(dir.path().join(file), "x\n");
    }
    let mut app = App::new(dir.path().to_path_buf(), Vec::new(), Vec::new(), false);
    let rx = app.file_index.take_receiver().expect("receiver");
    (dir, app, rx)
}

/// Wait for the walk in flight and hand it to the app, as the event loop does.
fn land(app: &mut App, rx: &mut tokio::sync::mpsc::UnboundedReceiver<workspace::Scan>) {
    let scan = rx.blocking_recv().expect("a walk lands");
    app.on_file_index(scan);
}

fn rows(app: &App) -> Vec<String> {
    app.overlay
        .as_ref()
        .map(|overlay| overlay.rows().into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

#[test]
fn quick_open_opens_before_the_walk_and_fills_in_keeping_the_query() {
    let (_dir, mut app, mut rx) = workspace_app(&["alpha.rs", "beta.rs", "gamma.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    assert!(app.overlay.as_ref().is_some_and(Overlay::is_quick_open));
    assert!(rows(&app).is_empty(), "nothing is walked on the UI thread");
    assert!(app.quick_open_loading().is_some());
    assert!(
        app.pendings()
            .contains(&app.quick_open_loading().expect("loading")),
        "the indexing placeholder schedules its own reveal"
    );
    if let Some(overlay) = app.overlay.as_mut() {
        overlay.push_char('b');
    }
    land(&mut app, &mut rx);
    assert_eq!(
        rows(&app),
        ["beta.rs"],
        "the typed query survives the refill"
    );
    assert!(app.quick_open_loading().is_none());
}

#[test]
fn recently_opened_files_rank_first() {
    let (dir, mut app, mut rx) = workspace_app(&["a.rs", "b.rs", "c.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    land(&mut app, &mut rx);
    assert_eq!(rows(&app), ["a.rs", "b.rs", "c.rs"]);
    app.overlay = None;
    app.open_path(&dir.path().join("c.rs"));
    app.open_path(&dir.path().join("b.rs"));
    app.dispatch(Command::OpenQuickOpen);
    assert_eq!(rows(&app), ["b.rs", "c.rs", "a.rs"]);
}

#[test]
fn a_deleted_file_leaves_an_open_quick_open() {
    let (dir, mut app, mut rx) = workspace_app(&["a.rs", "b.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    land(&mut app, &mut rx);
    let gone = dir.path().join("a.rs");
    let _ = std::fs::remove_file(&gone);
    app.on_backend_event(None, SessionEvent::FsChanged { paths: vec![gone] });
    assert_eq!(rows(&app), ["b.rs"]);
}

#[test]
fn a_created_file_joins_an_open_quick_open() {
    let (dir, mut app, mut rx) = workspace_app(&["a.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    land(&mut app, &mut rx);
    let new = dir.path().join("b.rs");
    let _ = std::fs::write(&new, "x\n");
    app.on_backend_event(None, SessionEvent::FsChanged { paths: vec![new] });
    land(&mut app, &mut rx);
    assert_eq!(rows(&app), ["a.rs", "b.rs"]);
}

#[test]
fn open_recent_lists_other_existing_workspaces_and_reopens_on_one() {
    let (dir, mut app, _rx) = workspace_app(&[]);
    let other = tempfile::tempdir().expect("temp dir");
    app.recent.touch_workspace(other.path());
    app.recent
        .touch_workspace(Path::new("/karet-test/no/such/workspace"));
    app.recent.touch_workspace(dir.path());
    app.dispatch(Command::OpenRecentWorkspace);
    let listed = rows(&app);
    assert_eq!(
        listed.len(),
        1,
        "only the other workspace that exists: {listed:?}"
    );
    app.overlay_accept();
    assert!(app.should_quit);
    assert_eq!(app.take_relaunch().as_deref(), Some(other.path()));
}

#[test]
fn open_recent_with_nothing_to_offer_says_so() {
    let (dir, mut app, _rx) = workspace_app(&[]);
    app.recent.touch_workspace(dir.path());
    app.dispatch(Command::OpenRecentWorkspace);
    assert!(app.overlay.is_none());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("open recent: no other recent workspaces")
    );
}

#[test]
fn cancelling_the_unsaved_changes_prompt_cancels_the_reopen() {
    let (_dir, mut app, _rx) = workspace_app(&[]);
    let other = tempfile::tempdir().expect("temp dir");
    app.push_tab(text_tab("t.rs", "x"));
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DocumentId(1));
    }
    app.tabs[idx].dirty = true;
    app.reopen_on_workspace(other.path().to_path_buf());
    assert_eq!(app.pending_close, Some(CloseRequest::Quit));
    assert!(!app.should_quit);
    app.dispatch(Command::CloseConfirmCancel);
    assert!(app.take_relaunch().is_none());
}

#[test]
fn a_plain_quit_after_a_reopen_whose_save_failed_just_exits() {
    let (_dir, mut app, _rx) = workspace_app(&[]);
    app.backend = Some(Arc::new(RecordingBackend::new()));
    let other = tempfile::tempdir().expect("temp dir");
    app.push_tab(text_tab("t.rs", "x"));
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DocumentId(1));
    }
    app.tabs[idx].dirty = true;
    app.reopen_on_workspace(other.path().to_path_buf());
    app.dispatch(Command::CloseConfirmSave);
    assert_eq!(app.saving_close, Some(CloseRequest::Quit));
    let request = *app.pending_saves.keys().next().expect("a save in flight");
    app.on_backend_event(
        Some(request),
        SessionEvent::ExternalConflict { doc: DocumentId(1) },
    );
    assert!(
        app.saving_close.is_none(),
        "the failed save abandons the quit"
    );
    assert!(!app.should_quit);

    app.tabs[idx].dirty = false;
    app.dispatch(Command::Quit);
    assert!(app.should_quit);
    assert!(
        app.take_relaunch().is_none(),
        "the abandoned reopen does not ride on a later plain quit"
    );
}

/// The explorer row for `name`, selected.
fn select_tree_row(app: &mut App, dir: &Path, name: &str) {
    app.sidebar_panel = SidebarPanel::Explorer;
    app.explorer.ensure_built(dir);
    let row = app
        .explorer
        .rows()
        .iter()
        .position(|row| row.path == dir.join(name))
        .expect("the file is in the tree");
    app.explorer.select_index(row);
}

#[test]
fn only_an_explicit_tree_open_counts_as_recently_opened() {
    let (dir, mut app, _rx) = workspace_app(&["a.rs", "b.rs", "c.rs"]);
    let root = dir.path().to_path_buf();
    // Arrowing onto a file previews it without taking focus.
    select_tree_row(&mut app, &root, "a.rs");
    app.preview_selected_explorer_row();
    assert!(
        app.tabs
            .iter()
            .any(|tab| tab.path() == Some(&root.join("a.rs"))),
        "the arrowed-onto file is previewed"
    );
    assert!(
        app.recent.files(&root).is_empty(),
        "passing over a file is not opening it"
    );
    // A click previews with focus.
    select_tree_row(&mut app, &root, "b.rs");
    app.explorer_preview_with_focus();
    assert_eq!(app.recent.files(&root), [root.join("b.rs")]);
    // Enter on a file arrowing already previewed opens it.
    select_tree_row(&mut app, &root, "a.rs");
    app.preview_selected_explorer_row();
    app.sidebar_promote_or_open_permanent();
    assert_eq!(
        app.recent.files(&root),
        [root.join("a.rs"), root.join("b.rs")]
    );
    // Enter on a file not yet open.
    select_tree_row(&mut app, &root, "c.rs");
    app.sidebar_promote_or_open_permanent();
    assert_eq!(
        app.recent.files(&root),
        [root.join("c.rs"), root.join("a.rs"), root.join("b.rs")]
    );
}
