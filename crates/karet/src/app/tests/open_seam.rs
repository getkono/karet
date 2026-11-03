//! Opening a file entirely through the backend.
//!
//! These cover the shell's half of the contract: a tab is reserved from the
//! path's name, the backend is asked what the file really is, and the answer
//! either confirms the guess or corrects it. Nothing here touches the filesystem,
//! which is the property that lets the shell render a workspace on another
//! machine.

use karet_filetype::FileKind;
use karet_session::DocumentCommand;
use karet_session::WorkspaceCommand;
use karet_session::api::FileChunk;
use karet_session::api::PathClass;

use super::support::*;
use crate::app::*;

/// Whether `command` registers a document.
fn is_open(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Document(DocumentCommand::Open { .. })
    )
}

/// Whether `command` asks for a classification.
fn is_classify(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Workspace(WorkspaceCommand::ClassifyPath { .. })
    )
}

/// Whether `command` reads a file's bytes.
fn is_read(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Workspace(WorkspaceCommand::ReadFileBytes { .. })
    )
}

/// Whether `command` walks the workspace for quick-open.
fn is_list(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Workspace(WorkspaceCommand::ListFiles { .. })
    )
}

/// The first visible line a viewport report declares, if `command` is one.
fn viewport_first_line(command: &SessionCommand) -> Option<u32> {
    match command {
        SessionCommand::Document(DocumentCommand::SetViewport { first_line, .. }) => {
            Some(*first_line)
        },
        _ => None,
    }
}

/// An app with a recording backend, rooted at a path that does not exist here.
///
/// Deliberately absent: if any of this reached the filesystem, these tests would
/// be testing the wrong machine.
fn remote_app() -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(
        PathBuf::from("/not/on/this/machine"),
        Vec::new(),
        Vec::new(),
        false,
    );
    app.backend = Some(backend.clone());
    app.workspace_remote = true;
    (backend, app)
}

/// The commands `backend` recorded, paired with their request ids.
fn sent(backend: &RecordingBackend) -> Vec<(RequestId, SessionCommand)> {
    backend
        .sent
        .lock()
        .map(|sent| sent.clone())
        .unwrap_or_default()
}

fn find_request(
    backend: &RecordingBackend,
    matches: impl Fn(&SessionCommand) -> bool,
) -> Option<RequestId> {
    sent(backend)
        .into_iter()
        .find(|(_, command)| matches(command))
        .map(|(id, _)| id)
}

/// The id of the first request `matches` accepts; a test that needs one fails
/// here, by name, rather than passing without it.
fn request(backend: &RecordingBackend, matches: impl Fn(&SessionCommand) -> bool) -> RequestId {
    find_request(backend, matches).expect("the request was sent")
}

/// Answer the classification `path`'s open asked for with `kind`.
fn classify_as(app: &mut App, backend: &RecordingBackend, path: &Path, kind: FileKind) {
    let classify = request(backend, is_classify);
    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.to_path_buf(),
            result: Ok(PathClass {
                kind,
                len: 5,
                head: b"text\n".to_vec(),
            }),
        },
    );
}

/// Open the text file at `path` in a remote app, answer the size check it
/// asks for first, and return the id of the document open that follows.
fn open_text(backend: &RecordingBackend, app: &mut App, path: &Path) -> RequestId {
    app.open_path(path);
    app.register_open_tabs();
    classify_as(app, backend, path, FileKind::Text);
    request(backend, is_open)
}

#[test]
fn opening_a_source_file_reserves_a_tab_and_registers_the_document() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/main.rs");

    app.open_path(&path);
    app.register_open_tabs();
    classify_as(&mut app, &backend, &path, FileKind::Text);

    assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::Document(DocumentCommand::Open { path: asked, .. }) if asked == &path
        ))
        .is_some(),
        "the document is registered with the backend, not read from disk"
    );
}

/// Media is reserved from its extension and then confirmed: only the machine
/// holding the file can read its magic bytes or its length.
#[test]
fn opening_media_asks_the_backend_what_it_really_is() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");

    app.open_path(&path);
    app.register_open_tabs();

    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::Workspace(WorkspaceCommand::ClassifyPath { path: asked, .. })
                if asked == &path
        ))
        .is_some(),
        "only the machine holding the file can classify its bytes"
    );
}

/// The session loads a document's whole file, so a remote text file is checked
/// against the size guard before its document is opened.
#[test]
fn a_remote_text_file_is_size_checked_before_its_document_opens() {
    let (backend, mut app) = remote_app();

    app.open_path(Path::new("/not/on/this/machine/notes.txt"));
    app.register_open_tabs();

    assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
    assert!(find_request(&backend, is_classify).is_some());
    assert!(find_request(&backend, is_open).is_none());
}

/// A multi-gigabyte log becomes a too-large placeholder, and its document is
/// never opened: the session would read all of it.
#[test]
fn a_remote_text_file_over_the_guard_becomes_a_too_large_placeholder() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/huge.log");
    app.open_path(&path);
    let view = app.tabs[app.active].view;
    app.register_open_tabs();

    let len = karet_filetype::SIZE_GUARD * 300;
    classify_as(&mut app, &backend, &path, FileKind::TooLarge { len });

    assert!(matches!(
        app.tabs[app.active].kind,
        TabKind::Placeholder {
            kind: FileKind::TooLarge { .. },
            ..
        }
    ));
    assert_eq!(app.tabs[app.active].view, view, "the view is kept");
    assert!(find_request(&backend, is_open).is_none());
}

/// A co-located file over the guard is caught by a stat, at once.
#[test]
fn a_local_text_file_over_the_guard_becomes_a_too_large_placeholder() {
    let dir = test_dir("open-too-large");
    let path = dir.join("huge.log");
    let file = std::fs::File::create(&path).expect("a scratch file");
    file.set_len(karet_filetype::SIZE_GUARD + 1)
        .expect("a sparse length");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());

    app.open_path(&path);

    assert!(matches!(
        app.tabs[app.active].kind,
        TabKind::Placeholder {
            kind: FileKind::TooLarge { .. },
            ..
        }
    ));
    assert!(find_request(&backend, is_open).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A text file whose bytes turn out not to be UTF-8 falls back to a hex view,
/// and its bytes are read outright: classifying again would say "text" (the
/// head can look like it) and leave the view empty.
#[test]
fn a_text_open_that_is_not_utf8_reads_its_bytes_into_the_hex_view() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/latin1.txt");
    let open = open_text(&backend, &mut app, &path);

    app.on_backend_event(Some(open), SessionEvent::NotUtf8 { path: path.clone() });
    let read = request(&backend, is_read);
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0xe9, b'\n'],
                total_len: 2,
            }),
        },
    );

    let TabKind::Hex { bytes, .. } = &app.tabs[app.active].kind else {
        panic!("the fallback is a hex view");
    };
    assert_eq!(bytes, &vec![0xe9, b'\n']);
}

/// Media open in two panes fills both, each keeping its own view.
#[test]
fn media_open_in_two_panes_fills_both_and_keeps_their_views() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/blob.dat");
    app.open_path(&path);
    app.split_focused(SplitDir::Right);
    app.register_open_tabs();
    let views: Vec<ViewId> = app.all_tabs().map(|tab| tab.view).collect();
    classify_as(&mut app, &backend, &path, FileKind::Binary);
    let read = request(&backend, is_read);

    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![7, 7],
                total_len: 2,
            }),
        },
    );

    let filled: Vec<ViewId> = app
        .all_tabs()
        .filter(|tab| matches!(&tab.kind, TabKind::Hex { bytes, .. } if !bytes.is_empty()))
        .map(|tab| tab.view)
        .collect();
    assert_eq!(filled.len(), 2, "both panes are filled");
    for view in views {
        assert!(filled.contains(&view), "view {view:?} is kept");
    }
}

/// Attaching a backend re-walks every open tab, and a user can reopen a file
/// already loading. Asking twice would double the traffic for no answer.
#[test]
fn a_second_registration_does_not_ask_again() {
    let (backend, mut app) = remote_app();
    app.open_path(Path::new("/not/on/this/machine/logo.png"));

    app.register_open_tabs();
    app.register_open_tabs();

    let asks = sent(&backend)
        .iter()
        .filter(|(_, command)| is_classify(command))
        .count();
    assert_eq!(asks, 1);
}

#[test]
fn a_classified_binary_file_has_its_bytes_requested_and_rendered() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");
    app.open_path(&path);
    app.register_open_tabs();
    let classify = request(&backend, is_classify);

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 4,
                head: vec![0, 1, 2, 3],
            }),
        },
    );
    let read = request(&backend, is_read);
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0, 1, 2, 3],
                total_len: 4,
            }),
        },
    );

    let TabKind::Hex { bytes, .. } = &app.tabs[app.active].kind else {
        panic!("the bytes render as a hex view");
    };
    assert_eq!(bytes, &vec![0, 1, 2, 3]);
}

/// A file arriving in pieces must assemble, not restart: a large image or PDF
/// crosses a connection in chunks and every one of them counts.
#[test]
fn a_chunked_file_assembles_across_reads() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");
    app.open_path(&path);
    app.register_open_tabs();
    let classify = request(&backend, is_classify);
    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 6,
                head: vec![1, 2, 3],
            }),
        },
    );

    // Two chunks: the first short of the end, the second completing the file.
    for (offset, bytes) in [(0_u64, vec![1_u8, 2, 3]), (3, vec![4, 5, 6])] {
        let read = sent(&backend)
            .into_iter()
            .rev()
            .find(|(_, command)| is_read(command))
            .map(|(id, _)| id)
            .expect("each chunk is asked for");
        app.on_backend_event(
            Some(read),
            SessionEvent::FileBytes {
                path: path.clone(),
                result: Ok(FileChunk {
                    offset,
                    bytes,
                    total_len: 6,
                }),
            },
        );
    }

    let TabKind::Hex { bytes, .. } = &app.tabs[app.active].kind else {
        panic!("the assembled bytes render as a hex view");
    };
    assert_eq!(bytes, &vec![1, 2, 3, 4, 5, 6]);
}

/// An extension can lie. The backend sees the bytes, so its verdict re-routes the
/// tab to the renderer the content actually warrants.
#[test]
fn a_lying_extension_is_corrected_by_the_backends_verdict() {
    let (backend, mut app) = remote_app();
    // Named like a PDF, but the bytes say otherwise.
    let path = PathBuf::from("/not/on/this/machine/report.pdf");
    app.open_path(&path);
    app.register_open_tabs();
    let classify = request(&backend, is_classify);

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 2,
                head: vec![0, 1],
            }),
        },
    );
    let read = request(&backend, is_read);
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path,
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0, 1],
                total_len: 2,
            }),
        },
    );

    assert!(matches!(app.tabs[app.active].kind, TabKind::Hex { .. }));
}

/// An unreadable file must leave the placeholder the open reserved, not wedge the
/// tab or crash the shell.
#[test]
fn an_unreadable_file_keeps_its_placeholder() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/gone.png");
    app.open_path(&path);
    app.register_open_tabs();
    let classify = request(&backend, is_classify);

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path,
            result: Err("No such file or directory".to_owned()),
        },
    );

    assert!(matches!(
        app.tabs[app.active].kind,
        TabKind::Hex { .. } | TabKind::Placeholder { .. }
    ));
}

/// The rows of the open overlay, if one is open.
fn overlay_rows(app: &App) -> Vec<String> {
    app.keyboard
        .overlay()
        .map(|overlay| overlay.rows().into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A remote session's file index, opened in quick-open, with its walk sent.
fn remote_quick_open() -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = remote_app();
    app.file_index.walk_through_backend();
    app.open_quick_open();
    app.dispatch_file_index_walk();
    (backend, app)
}

#[test]
fn quick_open_shows_its_picker_before_the_file_list_arrives() {
    let (backend, app) = remote_quick_open();

    assert!(app.keyboard.overlay().is_some(), "the picker opens at once");
    assert!(
        find_request(&backend, is_list).is_some(),
        "the workspace walk runs on the machine holding the files"
    );
}

/// A local session walks its own disk, off the UI thread; only a remote one
/// asks the backend.
#[test]
fn a_local_quick_open_asks_the_backend_for_no_walk() {
    let (backend, mut app) = remote_app();
    app.open_quick_open();
    app.dispatch_file_index_walk();

    assert!(find_request(&backend, is_list).is_none());
}

#[test]
fn the_file_list_fills_the_open_picker_with_workspace_relative_rows() {
    let (backend, mut app) = remote_quick_open();
    let listing = request(&backend, is_list);

    app.on_backend_event(
        Some(listing),
        SessionEvent::FilesListed {
            files: vec![
                PathBuf::from("/not/on/this/machine/src/main.rs"),
                PathBuf::from("/not/on/this/machine/README.md"),
            ],
            truncated: false,
        },
    );

    assert_eq!(overlay_rows(&app), ["README.md", "src/main.rs"]);
}

/// An answer to a superseded walk must not stand in for the one the index is
/// still owed — the same rule every other request in the shell follows.
#[test]
fn a_stale_file_list_is_ignored() {
    let (backend, mut app) = remote_quick_open();
    let listing = request(&backend, is_list);
    // The backend named a different workspace, superseding the first walk.
    app.on_backend_event(
        None,
        SessionEvent::WorkspaceRoots {
            roots: vec![PathBuf::from("/elsewhere")],
        },
    );

    app.on_backend_event(
        Some(listing),
        SessionEvent::FilesListed {
            files: vec![PathBuf::from("/not/on/this/machine/stale.rs")],
            truncated: false,
        },
    );

    assert!(overlay_rows(&app).is_empty(), "{:?}", overlay_rows(&app));
}

/// Highlights are scoped to what a client can see, and the backend only knows
/// that if the client says so. Without this the whole file's spans cross the
/// connection on every recompute.
#[test]
fn a_laid_out_editor_reports_its_visible_lines() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/main.rs");
    let open = open_text(&backend, &mut app, &path);
    app.on_backend_event(
        Some(open),
        SessionEvent::Opened {
            doc: DocumentId(1),
            version: 1,
        },
    );

    app.report_viewports();

    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::Document(DocumentCommand::SetViewport { doc, .. })
                if *doc == DocumentId(1)
        ))
        .is_some(),
        "the backend cannot scope highlights to a viewport it was never told"
    );
}

/// A viewport that has not moved is a message that says nothing, and this runs
/// after every frame.
#[test]
fn an_unmoved_viewport_is_not_reported_again() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/main.rs");
    let open = open_text(&backend, &mut app, &path);
    app.on_backend_event(
        Some(open),
        SessionEvent::Opened {
            doc: DocumentId(1),
            version: 1,
        },
    );

    app.report_viewports();
    app.report_viewports();
    app.report_viewports();

    let reports = sent(&backend)
        .iter()
        .filter(|(_, command)| viewport_first_line(command).is_some())
        .count();
    assert_eq!(reports, 1);
}

/// A scroll must reach the backend, or the newly revealed lines stay
/// unhighlighted for as long as the user stays there.
#[test]
fn scrolling_reports_the_new_viewport() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/main.rs");
    let open = open_text(&backend, &mut app, &path);
    app.on_backend_event(
        Some(open),
        SessionEvent::Opened {
            doc: DocumentId(1),
            version: 1,
        },
    );
    app.report_viewports();

    let active = app.active;
    app.tabs[active].editor.scroll_to_line(400);
    app.report_viewports();

    let reports: Vec<u32> = sent(&backend)
        .iter()
        .filter_map(|(_, command)| viewport_first_line(command))
        .collect();
    assert_eq!(reports, vec![0, 400]);
}

/// A co-located session hands over whole-document highlights for free, so a
/// local app declares no viewport and a long jump is never painted uncoloured.
#[test]
fn a_local_workspace_reports_no_viewport() -> Result<(), &'static str> {
    let (backend, mut app) = remote_app();
    app.workspace_remote = false;
    app.open_path(&PathBuf::from("/not/on/this/machine/main.rs"));
    app.register_open_tabs();
    let open = find_request(&backend, is_open).ok_or("the open was sent")?;
    app.on_backend_event(
        Some(open),
        SessionEvent::Opened {
            doc: DocumentId(1),
            version: 1,
        },
    );

    app.report_viewports();

    assert!(
        sent(&backend)
            .iter()
            .all(|(_, command)| viewport_first_line(command).is_none())
    );
    Ok(())
}

/// A closed view's range must be withdrawn, or it keeps widening the window of
/// a document another view still shows.
#[test]
fn a_closed_view_withdraws_its_viewport() -> Result<(), &'static str> {
    let (backend, mut app) = remote_app();
    let open = open_text(
        &backend,
        &mut app,
        &PathBuf::from("/not/on/this/machine/main.rs"),
    );
    app.on_backend_event(
        Some(open),
        SessionEvent::Opened {
            doc: DocumentId(1),
            version: 1,
        },
    );
    app.report_viewports();
    let view = app.tabs[app.active].view;

    app.tabs.clear();
    app.report_viewports();

    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::Document(DocumentCommand::ClearViewport { doc, view: cleared })
                if *doc == DocumentId(1) && *cleared == view
        ))
        .is_some()
    );
    assert!(app.reported_viewports.is_empty());
    Ok(())
}

/// The workspace's configuration describes the code — indentation, linters,
/// language servers — and is the backend's to resolve.
#[test]
fn a_backends_configuration_replaces_the_clients_behavioural_settings() {
    let (_backend, mut app) = remote_app();
    app.adopt_local_presentation();
    let mut report = app.loaded_config.clone();
    report.settings.editor.tab_size = 7;

    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(report),
        },
    );

    assert_eq!(app.settings.editor.tab_size, 7);
}

/// …but it has no standing to say which theme suits the terminal the user is
/// looking at, or whether that terminal has a patched font.
#[test]
fn a_backends_configuration_does_not_replace_the_clients_presentation() {
    let (_backend, mut app) = remote_app();
    app.settings.workbench.color_theme = "my-local-theme".to_owned();
    app.adopt_local_presentation();
    let mut report = app.loaded_config.clone();
    report.settings.workbench.color_theme = "a-theme-on-the-server".to_owned();

    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(report),
        },
    );

    assert_eq!(app.settings.workbench.color_theme, "my-local-theme");
}

/// A local session has one configuration and nothing to arbitrate, so the theme
/// it loads is simply the one it resolved.
#[test]
fn a_local_session_takes_its_whole_configuration_as_given() {
    let (_backend, mut app) = remote_app();
    let mut report = app.loaded_config.clone();
    report.settings.workbench.color_theme = "configured".to_owned();

    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(report),
        },
    );

    assert_eq!(app.settings.workbench.color_theme, "configured");
}
