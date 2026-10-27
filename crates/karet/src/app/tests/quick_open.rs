use karet_session::LSP_CHANGE_DEBOUNCE;

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
    fill_reserved_tabs(&mut app);
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

#[test]
fn a_file_at_line_query_before_the_walk_waits_on_it() {
    let (_dir, mut app, mut rx) = workspace_app(&["a.rs", "b.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    type_keys(&mut app, "rs:2");
    assert_eq!(mode(&app), Some(QuickOpenMode::FileAtLine));
    assert!(rows(&app).is_empty(), "nothing is walked on the UI thread");
    assert!(
        app.quick_open_loading().is_some(),
        "the file:line list waits on the walk like the file list"
    );
    land(&mut app, &mut rx);
    assert_eq!(mode(&app), Some(QuickOpenMode::FileAtLine));
    assert_eq!(rows(&app), ["a.rs", "b.rs"]);
    assert!(app.quick_open_loading().is_none());
}

#[test]
fn a_key_that_edits_nothing_keeps_the_highlighted_file_at_line() {
    let (dir, mut app, mut rx) = workspace_app(&["a.rs", "b.rs"]);
    app.dispatch(Command::OpenQuickOpen);
    land(&mut app, &mut rx);
    type_keys(&mut app, "rs:2");
    if let Some(overlay) = app.overlay.as_mut() {
        overlay.select_down();
    }
    let b = dir.path().join("b.rs");
    for code in [
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Tab,
    ] {
        app.overlay_input(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(
            app.overlay.as_ref().and_then(Overlay::selected_file),
            Some(b.as_path()),
            "{code:?} edits no text, so the list is not rebuilt"
        );
    }
}

/// The workspace-symbol queries the backend was sent, in order, with their ids.
fn workspace_queries(backend: &RecordingBackend) -> Vec<(RequestId, String)> {
    backend.sent.lock().map_or_else(
        |_| Vec::new(),
        |sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::WorkspaceSymbols { query } => Some((*id, query.clone())),
                    _ => None,
                })
                .collect()
        },
    )
}

/// Run the frame-end probe once typing has paused.
fn settle(app: &mut App) {
    app.probe_workspace_symbols(Instant::now() + LSP_CHANGE_DEBOUNCE + Duration::from_millis(1));
}

fn workspace_symbol(name: &str, path: &Path, line: u32) -> karet_core::WorkspaceSymbol {
    let at = LineCol::new(line, 3);
    karet_core::WorkspaceSymbol {
        name: name.to_string(),
        kind: karet_core::SymbolKind::Function,
        container_name: None,
        path: path.to_path_buf(),
        range: Some(Range { start: at, end: at }),
    }
}

#[test]
fn ctrl_t_searches_the_workspace_once_typing_pauses_and_opens_the_pick() {
    let (dir, mut app, backend) = symbol_app();
    let lib = dir.path().join("lib.rs");
    let _ = std::fs::write(&lib, "\n\n\nfn launch() {}\n");
    app.dispatch(Command::GoToSymbolInWorkspace);
    assert_eq!(mode(&app), Some(QuickOpenMode::WorkspaceSymbols));
    assert_eq!(app.overlay.as_ref().map(Overlay::query), Some("#"));
    settle(&mut app);
    assert!(
        workspace_queries(&backend).is_empty(),
        "an empty query asks nothing"
    );
    assert!(app.quick_open_loading().is_none());

    type_keys(&mut app, "lau");
    let now = Instant::now();
    let wake = app.workspace_symbol_next_wake(now).expect("a query is due");
    assert!(!wake.is_zero() && wake <= LSP_CHANGE_DEBOUNCE);
    app.probe_workspace_symbols(now);
    assert!(
        workspace_queries(&backend).is_empty(),
        "nothing is sent per keystroke"
    );
    let loading = app.quick_open_loading().expect("waiting on the server");
    assert!(app.pendings().contains(&loading));
    settle(&mut app);
    let sent = workspace_queries(&backend);
    assert_eq!(
        sent.iter().map(|(_, q)| q.as_str()).collect::<Vec<_>>(),
        ["lau"]
    );
    assert_eq!(app.workspace_symbol_next_wake(Instant::now()), None);
    settle(&mut app);
    assert_eq!(workspace_queries(&backend).len(), 1, "sent once");

    app.on_backend_event(
        Some(sent[0].0),
        SessionEvent::WorkspaceSymbols {
            symbols: vec![
                workspace_symbol("launch", &lib, 3),
                workspace_symbol("other", &lib, 0),
            ],
        },
    );
    assert!(app.quick_open_loading().is_none());
    assert_eq!(
        rows(&app),
        ["launch", "other"],
        "the server's answer is shown as it gave it"
    );
    app.overlay_accept();
    fill_reserved_tabs(&mut app);
    assert!(app.overlay.is_none());
    let tab = &app.tabs[app.active];
    assert_eq!(tab.path(), Some(lib.as_path()));
    assert_eq!(tab.editor.cursor(), LineCol::new(3, 3));
}

#[test]
fn an_answer_to_a_query_typed_over_is_dropped() {
    let (dir, mut app, backend) = symbol_app();
    let lib = dir.path().join("lib.rs");
    app.dispatch(Command::OpenQuickOpen);
    type_keys(&mut app, "#r");
    assert_eq!(mode(&app), Some(QuickOpenMode::WorkspaceSymbols));
    settle(&mut app);
    type_keys(&mut app, "u");
    settle(&mut app);
    let sent = workspace_queries(&backend);
    assert_eq!(
        sent.iter().map(|(_, q)| q.as_str()).collect::<Vec<_>>(),
        ["r", "ru"]
    );

    let stale = SessionEvent::WorkspaceSymbols {
        symbols: vec![workspace_symbol("rust", &lib, 0)],
    };
    app.on_backend_event(Some(sent[0].0), stale);
    assert!(rows(&app).is_empty(), "the answer to `r` is superseded");
    assert!(app.quick_open_loading().is_some(), "`ru` is still awaited");
    app.on_backend_event(
        Some(sent[1].0),
        SessionEvent::WorkspaceSymbols {
            symbols: vec![workspace_symbol("run", &lib, 0)],
        },
    );
    assert_eq!(rows(&app), ["run"]);

    // Deleting the sigil leaves the mode, and forgets the search.
    backspace(&mut app, 3);
    assert_eq!(mode(&app), Some(QuickOpenMode::Files));
    assert_eq!(app.workspace_symbol_next_wake(Instant::now()), None);
}

#[test]
fn an_answer_after_the_picker_closed_is_dropped() {
    let (dir, mut app, backend) = symbol_app();
    app.dispatch(Command::GoToSymbolInWorkspace);
    type_keys(&mut app, "run");
    settle(&mut app);
    let sent = workspace_queries(&backend);
    app.overlay = None;
    settle(&mut app);
    // A file picker opened since is not filled with symbols either.
    app.dispatch(Command::OpenQuickOpen);
    app.on_backend_event(
        sent.first().map(|(id, _)| *id),
        SessionEvent::WorkspaceSymbols {
            symbols: vec![workspace_symbol("run", &dir.path().join("lib.rs"), 0)],
        },
    );
    assert_eq!(mode(&app), Some(QuickOpenMode::Files));
    assert!(!rows(&app).contains(&"run".to_string()));
    // Reopening starts a fresh search.
    app.dispatch(Command::GoToSymbolInWorkspace);
    assert!(rows(&app).is_empty());
    assert!(app.quick_open_loading().is_none());
}

/// `mod m` over the whole file, holding `run` (line 0) and `stop` (line 2).
fn module_tree() -> Vec<karet_core::Symbol> {
    let mut module = function("m", 0);
    module.kind = karet_core::SymbolKind::Module;
    module.range.end = LineCol::new(3, 0);
    module.children = vec![function("run", 0), function("stop", 2)];
    vec![module]
}

fn land_symbols(app: &mut App, symbols: Vec<karet_core::Symbol>) {
    app.on_backend_event(
        None,
        SessionEvent::Symbols {
            doc: DocumentId(17),
            symbols,
        },
    );
}

/// Draw the shell and return the only pane's painted breadcrumb row.
fn breadcrumb_row(app: &mut App) -> String {
    let painted = screen(app, 120, 12);
    let row = app.pane_frames.first().map_or(0, |f| f.breadcrumb_rect.y);
    painted.get(usize::from(row)).cloned().unwrap_or_default()
}

#[test]
fn the_breadcrumb_asks_for_symbols_and_names_those_around_the_caret() {
    let (_dir, mut app, backend) = symbol_app();
    let _ = screen(&mut app, 120, 12);
    assert_eq!(
        symbol_requests(&backend),
        1,
        "asked for without the outline or the picker"
    );
    let _ = screen(&mut app, 120, 12);
    assert_eq!(symbol_requests(&backend), 1, "one request per version");

    land_symbols(&mut app, module_tree());
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 5)]);
    let row = breadcrumb_row(&mut app);
    let sep = karet_widgets::breadcrumbs::BREADCRUMB_SEP;
    assert!(
        row.contains(&format!("main.rs{sep}m{sep}stop")),
        "the chain follows the file name: {row}"
    );
    let targets: Vec<BreadcrumbTarget> = app
        .pane_frames
        .first()
        .map(|f| f.breadcrumb_hits.iter().map(|h| h.target.clone()).collect())
        .unwrap_or_default();
    assert_eq!(
        targets.iter().rev().take(2).collect::<Vec<_>>(),
        [&BreadcrumbTarget::Symbol(1), &BreadcrumbTarget::Symbol(0)]
    );

    // Outside every symbol the path paints alone.
    land_symbols(&mut app, vec![function("run", 0)]);
    let row = breadcrumb_row(&mut app);
    assert!(row.trim_end().ends_with("main.rs"), "{row}");
}

#[test]
fn clicking_a_symbol_segment_opens_its_siblings_and_accepting_one_jumps_there() {
    let (_dir, mut app, _backend) = symbol_app();
    land_symbols(&mut app, module_tree());
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 5)]);
    let _ = screen(&mut app, 120, 12);
    let (row, stop) = app
        .pane_frames
        .first()
        .and_then(|f| {
            let hit = f
                .breadcrumb_hits
                .iter()
                .find(|h| h.target == BreadcrumbTarget::Symbol(1))?;
            Some((f.breadcrumb_rect.y, hit.start))
        })
        .unwrap_or_default();
    assert!(stop > 0, "the `stop` segment was recorded");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: stop,
        row,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(rows(&app), ["run", "stop"]);
    assert_eq!(
        app.overlay.as_ref().map(Overlay::selected),
        Some(1),
        "the clicked symbol is highlighted"
    );
    if let Some(overlay) = app.overlay.as_mut() {
        overlay.select_up();
    }
    app.overlay_accept();
    assert!(app.overlay.is_none());
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 3));
}

#[test]
fn a_symbol_depth_the_chain_no_longer_reaches_opens_nothing() {
    let (_dir, mut app, _backend) = symbol_app();
    land_symbols(&mut app, module_tree());
    app.open_sibling_symbols(3);
    assert!(app.overlay.is_none());
    app.open_sibling_symbols(0);
    assert_eq!(rows(&app), ["m"], "the top level, at the caret's module");
}
