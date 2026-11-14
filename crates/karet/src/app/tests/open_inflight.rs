//! Content requests in flight while the tabs they serve change under them: a
//! move retargets the tab, "open anyway" overrides the guard, and a second
//! tab on the same file asks for its own verdict.

use karet_filetype::FileKind;
use karet_session::DocumentCommand;
use karet_session::WorkspaceCommand;
use karet_session::api::PathClass;

use super::support::*;
use crate::app::*;

/// A remote app with a recording backend, rooted at a path not on this machine.
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

/// The classifications `backend` was asked for: request id, path, and whether
/// the size guard was waived.
fn classifications(backend: &RecordingBackend) -> Vec<(RequestId, PathBuf, bool)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Workspace(WorkspaceCommand::ClassifyPath {
                        path,
                        ignore_size,
                    }) => Some((*id, path.clone(), *ignore_size)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The paths `backend` was asked to read bytes of.
fn reads(backend: &RecordingBackend) -> Vec<PathBuf> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::Workspace(WorkspaceCommand::ReadFileBytes { path, .. }) => {
                        Some(path.clone())
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Answer the classification `id` of `path` with `kind`.
fn classify(app: &mut App, id: RequestId, path: &Path, kind: FileKind, len: u64) {
    app.on_backend_event(
        Some(id),
        SessionEvent::PathClassified {
            path: path.to_path_buf(),
            result: Ok(PathClass {
                kind,
                len,
                head: Vec::new(),
            }),
        },
    );
}

/// A file moved while its classification is in flight is asked about again
/// under its new name: the answer for the old one no longer matches the tab,
/// and without a new request the tab would wait for ever.
#[test]
fn a_move_during_a_classification_asks_again_under_the_new_path() {
    let (backend, mut app) = remote_app();
    let from = PathBuf::from("/not/on/this/machine/blob.bin");
    let to = PathBuf::from("/not/on/this/machine/moved.bin");
    app.open_path(&from);
    app.register_open_tabs();
    assert_eq!(classifications(&backend).len(), 1);

    app.retarget_open_paths(&from, &to);

    let asked = classifications(&backend);
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].1, to);
    let (id, _, _) = asked[1].clone();
    classify(&mut app, id, &to, FileKind::Binary, 4);
    assert_eq!(reads(&backend), vec![to]);
}

/// A file moved while its bytes are streaming is read again from the start
/// under its new name.
#[test]
fn a_move_during_a_read_reads_the_new_path() {
    let (backend, mut app) = remote_app();
    let from = PathBuf::from("/not/on/this/machine/blob.bin");
    let to = PathBuf::from("/not/on/this/machine/moved.bin");
    app.open_path(&from);
    app.register_open_tabs();
    let (id, _, _) = classifications(&backend)[0].clone();
    classify(&mut app, id, &from, FileKind::Binary, 4);
    assert_eq!(reads(&backend), vec![from.clone()]);

    app.retarget_open_paths(&from, &to);

    assert_eq!(reads(&backend), vec![from, to]);
}

/// A tab whose document is already being opened -- here one opened past the
/// guard -- is not a reservation: a guarded "too large" asked for by another
/// tab on the same file replaces that tab alone.
#[test]
fn a_too_large_verdict_leaves_a_tab_opened_anyway_alone() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/big.txt");
    app.push_tab(Tab::open_ignoring_size(&path));
    app.push_tab(Tab::open(&path));
    app.register_open_tabs();
    let asked = classifications(&backend);
    assert_eq!(asked.len(), 1, "only the guarded tab asks");

    let len = karet_filetype::SIZE_GUARD + 1;
    classify(&mut app, asked[0].0, &path, FileKind::TooLarge { len }, len);

    let kinds: Vec<_> = app
        .tabs
        .iter()
        .filter(|tab| tab.path() == Some(path.as_path()))
        .map(|tab| matches!(tab.kind, TabKind::Code { .. }))
        .collect();
    assert_eq!(kinds, vec![true, false], "the opened tab stays code");
}

/// "Open anyway" forgets the guarded classification in flight, but another tab
/// on the same file was waiting on it too: a fresh guarded one is sent for that
/// tab, and its answer settles that tab without undoing the override.
#[test]
fn opening_anyway_keeps_another_tabs_size_check_alive() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/big.txt");
    let len = karet_filetype::SIZE_GUARD + 1;
    app.push_tab(Tab::open(&path));
    app.register_open_tabs();
    let first = classifications(&backend);
    assert_eq!(first.len(), 1);
    app.push_tab(crate::tab::open::reserve(
        &path,
        FileKind::TooLarge { len },
        len,
    ));

    app.open_active_anyway();

    let asked = classifications(&backend);
    assert_eq!(asked.len(), 2, "a fresh check for the waiting tab");
    assert!(!asked[1].2, "still guarded");
    // The forgotten answer changes nothing; the fresh one settles the waiting
    // tab alone.
    classify(&mut app, first[0].0, &path, FileKind::TooLarge { len }, len);
    assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));
    classify(&mut app, asked[1].0, &path, FileKind::TooLarge { len }, len);
    assert!(matches!(app.tabs[0].kind, TabKind::Placeholder { .. }));
    assert!(
        matches!(app.tabs[app.active].kind, TabKind::Code { .. }),
        "the override stands"
    );
    let opened = backend.sent.lock().is_ok_and(|sent| {
        sent.iter().any(|(_, command)| {
            matches!(
                command,
                SessionCommand::Document(DocumentCommand::Open { .. })
            )
        })
    });
    assert!(opened, "the overriding tab opens its document");
}
