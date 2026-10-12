use super::support::*;
use crate::app::*;
use crate::overlay::QuickOpenMode;

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

/// An app over one registered code tab (`DocumentId(17)`, `main.rs` in its
/// workspace) whose backend records what it is asked.
fn symbol_app() -> (tempfile::TempDir, App, Arc<RecordingBackend>) {
    let (dir, mut app, _rx) = workspace_app(&[]);
    let text = "fn run() {}\n\nfn stop() {}\n";
    let path = dir.path().join("main.rs");
    let _ = std::fs::write(&path, text);
    let backend = Arc::new(RecordingBackend::new());
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", text));
    if let TabKind::Code { doc, path: at, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(17));
        *at = path;
    }
    (dir, app, backend)
}

fn function(name: &str, line: u32) -> karet_core::Symbol {
    let at = LineCol::new(line, 3);
    karet_core::Symbol {
        name: name.to_string(),
        kind: karet_core::SymbolKind::Function,
        detail: None,
        range: Range {
            start: LineCol::new(line, 0),
            end: LineCol::new(line, 12),
        },
        selection_range: Range { start: at, end: at },
        container_name: None,
        children: Vec::new(),
    }
}

fn symbol_requests(backend: &RecordingBackend) -> usize {
    backend.sent.lock().map_or(0, |sent| {
        sent.iter()
            .filter(|(_, command)| {
                matches!(command, SessionCommand::DocumentSymbols { doc } if *doc == DocumentId(17))
            })
            .count()
    })
}

#[test]
fn go_to_symbol_opens_at_once_and_fills_in_when_the_symbols_land() {
    let (_dir, mut app, backend) = symbol_app();
    app.dispatch(Command::GoToSymbolInEditor);
    let overlay = app.overlay.as_ref().expect("the picker opens at once");
    assert_eq!(
        overlay.quick_open_mode(),
        Some(QuickOpenMode::DocumentSymbols)
    );
    assert_eq!(overlay.query(), "@");
    assert!(rows(&app).is_empty());
    assert_eq!(symbol_requests(&backend), 1, "the symbols are asked for");
    let loading = app.quick_open_loading().expect("waiting on the symbols");
    assert!(
        app.pendings().contains(&loading),
        "the note schedules its own reveal"
    );

    if let Some(overlay) = app.overlay.as_mut() {
        overlay.push_str("sto");
    }
    app.on_backend_event(
        None,
        SessionEvent::Symbols {
            doc: DocumentId(17),
            symbols: vec![function("run", 0), function("stop", 2)],
        },
    );
    assert!(app.quick_open_loading().is_none());
    assert_eq!(rows(&app), ["stop"], "the typed query survives");
    app.overlay_accept();
    assert!(app.overlay.is_none());
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 3));

    // A second open is served from the cache: nothing more is asked.
    app.dispatch(Command::GoToSymbolInEditor);
    assert_eq!(rows(&app), ["run", "stop"]);
    assert_eq!(symbol_requests(&backend), 1);
    assert!(app.quick_open_loading().is_none());
}

#[test]
fn typing_the_sigil_in_quick_open_switches_to_symbols_and_back() {
    let (_dir, mut app, backend) = symbol_app();
    app.dispatch(Command::OpenQuickOpen);
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    app.overlay_input(key('@'));
    let mode = |app: &App| app.overlay.as_ref().and_then(Overlay::quick_open_mode);
    assert_eq!(mode(&app), Some(QuickOpenMode::DocumentSymbols));
    assert_eq!(symbol_requests(&backend), 1);
    app.overlay_input(key('r'));
    assert_eq!(app.overlay.as_ref().map(Overlay::query), Some("@r"));
    app.overlay_input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    app.overlay_input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(
        mode(&app),
        Some(QuickOpenMode::Files),
        "deleting the sigil lists files"
    );
    // Pasting a sigilled query goes straight to symbols, keeping the rest.
    app.modal_paste(Modal::Overlay, "@ru");
    assert_eq!(mode(&app), Some(QuickOpenMode::DocumentSymbols));
    assert_eq!(app.overlay.as_ref().map(Overlay::query), Some("@ru"));
}

fn type_keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.overlay_input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn backspace(app: &mut App, times: usize) {
    for _ in 0..times {
        app.overlay_input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
}

fn mode(app: &App) -> Option<QuickOpenMode> {
    app.overlay.as_ref().and_then(Overlay::quick_open_mode)
}

#[test]
fn go_to_line_opens_on_the_caret_and_jumps_to_what_is_typed() {
    let (_dir, mut app, _backend) = symbol_app();
    app.dispatch(Command::GoToLine);
    assert_eq!(mode(&app), Some(QuickOpenMode::Line));
    assert_eq!(app.overlay.as_ref().map(Overlay::query), Some(":"));
    assert_eq!(
        rows(&app),
        ["Current line 1, column 1 · type a line number between 1 and 4"]
    );
    assert!(app.quick_open_loading().is_none(), "nothing to wait for");

    type_keys(&mut app, "3:4");
    assert_eq!(rows(&app), ["Go to line 3, column 4"]);
    backspace(&mut app, 2);
    assert_eq!(rows(&app), ["Go to line 3"], "each edit recomputes the row");
    app.overlay_accept();
    assert!(app.overlay.is_none());
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 0));
}

#[test]
fn typing_a_colon_in_quick_open_goes_to_a_line_and_back() {
    let (_dir, mut app, _backend) = symbol_app();
    app.dispatch(Command::OpenQuickOpen);
    type_keys(&mut app, ":2");
    assert_eq!(mode(&app), Some(QuickOpenMode::Line));
    assert_eq!(rows(&app), ["Go to line 2"]);
    backspace(&mut app, 1);
    assert_eq!(
        rows(&app),
        ["Current line 1, column 1 · type a line number between 1 and 4"]
    );
    backspace(&mut app, 1);
    assert_eq!(mode(&app), Some(QuickOpenMode::Files), "the sigil is gone");
}

#[test]
fn a_file_query_ending_in_a_line_opens_the_file_there() {
    let (dir, mut app, mut rx) = workspace_app(&["a.rs", "b.rs"]);
    let _ = std::fs::write(dir.path().join("b.rs"), "one\ntwo\nthree\n");
    app.dispatch(Command::OpenQuickOpen);
    land(&mut app, &mut rx);
    app.modal_paste(Modal::Overlay, "rs:2");
    assert_eq!(mode(&app), Some(QuickOpenMode::FileAtLine));
    assert_eq!(rows(&app), ["a.rs", "b.rs"]);
    // The list follows the filesystem without losing the line.
    let gone = dir.path().join("a.rs");
    let _ = std::fs::remove_file(&gone);
    app.on_backend_event(None, SessionEvent::FsChanged { paths: vec![gone] });
    assert_eq!(mode(&app), Some(QuickOpenMode::FileAtLine));
    assert_eq!(rows(&app), ["b.rs"]);
    type_keys(&mut app, ":3");
    assert_eq!(app.overlay.as_ref().map(Overlay::query), Some("rs:2:3"));
    app.overlay_accept();
    let tab = &app.tabs[app.active];
    assert_eq!(tab.path(), Some(dir.path().join("b.rs").as_path()));
    assert_eq!(tab.editor.cursor(), LineCol::new(1, 2));
    // Backspacing the line away is a plain file search again.
    app.dispatch(Command::OpenQuickOpen);
    app.modal_paste(Modal::Overlay, "b.rs:2");
    backspace(&mut app, 2);
    assert_eq!(mode(&app), Some(QuickOpenMode::Files));
    assert_eq!(rows(&app), ["b.rs"]);
}
