//! Where an open lands, and what a reserved tab shows until it is filled.
//!
//! A co-located workspace routes on a file's content, as it always did; a remote
//! one routes on the name and is corrected by the backend. A tab reserved for
//! bytes loads quietly, says why when the read fails, and stops reading when
//! nobody is waiting for it any more.

use karet_filetype::FileKind;
use karet_session::DocumentCommand;
use karet_session::WorkspaceCommand;
use karet_session::api::FileChunk;
use karet_session::api::PathClass;

use super::support::*;
use crate::app::*;

/// The commands `backend` recorded, paired with their request ids.
fn sent(backend: &RecordingBackend) -> Vec<(RequestId, SessionCommand)> {
    backend
        .sent
        .lock()
        .map(|sent| sent.clone())
        .unwrap_or_default()
}

fn requests(
    backend: &RecordingBackend,
    matches: impl Fn(&SessionCommand) -> bool,
) -> Vec<RequestId> {
    sent(backend)
        .into_iter()
        .filter(|(_, command)| matches(command))
        .map(|(id, _)| id)
        .collect()
}

fn is_classify(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Workspace(WorkspaceCommand::ClassifyPath { .. })
    )
}

fn is_read(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Workspace(WorkspaceCommand::ReadFileBytes { .. })
    )
}

#[cfg(feature = "docx")]
fn is_convert(command: &SessionCommand) -> bool {
    matches!(
        command,
        SessionCommand::Document(DocumentCommand::Convert { .. })
    )
}

/// A local app with a recording backend, rooted at `root`.
fn local_app(root: &Path) -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(root.to_path_buf(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    (backend, app)
}

/// A remote app with a recording backend, rooted at a path not on this machine.
fn remote_app() -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = local_app(Path::new("/not/on/this/machine"));
    app.workspace_remote = true;
    (backend, app)
}

/// The eight-byte PNG signature, which is all a classification reads.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// A co-located open reads the file it is about to route, as it did before the
/// seam: a name that says nothing (or lies) is settled by the content.
#[test]
fn a_local_open_routes_on_the_files_content_not_its_name() {
    let dir = test_dir("open-routing-content");
    write_file(&dir, "picture", PNG);
    write_file(&dir, "program", b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    write_file(&dir, "notes.txt", b"text\0with a NUL\n");
    let (_backend, app) = local_app(&dir);

    let picture = app.open_tab(&dir.join("picture"));
    let program = app.open_tab(&dir.join("program"));
    let nul = app.open_tab(&dir.join("notes.txt"));

    assert!(
        matches!(
            picture.kind,
            TabKind::Placeholder {
                kind: FileKind::Image,
                ..
            }
        ),
        "an extensionless PNG is an image"
    );
    assert!(
        matches!(program.kind, TabKind::Hex { .. }),
        "an ELF is bytes"
    );
    assert!(
        matches!(nul.kind, TabKind::Hex { .. }),
        "NUL-filled text is bytes"
    );
    assert!(!picture.size_unchecked && !program.size_unchecked && !nul.size_unchecked);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A remote open cannot see the content, so the same names route as names.
#[test]
fn a_remote_open_routes_on_the_name() {
    let (_backend, app) = remote_app();

    let tab = app.open_tab(Path::new("/not/on/this/machine/notes.txt"));

    assert!(matches!(tab.kind, TabKind::Code { doc: None, .. }));
    assert!(tab.size_unchecked);
}

/// A co-located file over the size guard becomes the too-large placeholder at
/// once, a DOCX included: its conversion reads the whole file too.
#[test]
fn a_local_file_over_the_guard_opens_as_too_large() {
    let dir = test_dir("open-routing-large");
    let big = dir.join("big.docx");
    std::fs::create_dir_all(&dir).ok();
    let file = std::fs::File::create(&big).expect("the file");
    file.set_len(karet_filetype::SIZE_GUARD + 1)
        .expect("its length");
    let (_backend, app) = local_app(&dir);

    let tab = app.open_tab(&big);

    assert!(matches!(
        tab.kind,
        TabKind::Placeholder {
            kind: FileKind::TooLarge { .. },
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A remote DOCX is converted only once its size is known to be under the
/// guard; a too-large verdict replaces the preview without converting.
#[cfg(feature = "docx")]
#[test]
fn a_remote_docx_settles_the_size_guard_before_converting() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/report.docx");

    app.open_path(&path);
    app.register_open_tabs();

    assert!(requests(&backend, is_convert).is_empty());
    let classify = requests(&backend, is_classify);
    assert_eq!(classify.len(), 1);
    let len = karet_filetype::SIZE_GUARD + 1;
    app.on_backend_event(
        Some(classify[0]),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::TooLarge { len },
                len,
                head: Vec::new(),
            }),
        },
    );

    assert!(requests(&backend, is_convert).is_empty());
    assert!(matches!(
        app.tabs[app.active].kind,
        TabKind::Placeholder {
            kind: FileKind::TooLarge { .. },
            ..
        }
    ));
}

/// Under the guard, the same DOCX is converted once the verdict arrives.
#[cfg(feature = "docx")]
#[test]
fn a_remote_docx_under_the_guard_converts_on_the_verdict() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/report.docx");
    app.open_path(&path);
    app.register_open_tabs();
    let classify = requests(&backend, is_classify);

    app.on_backend_event(
        Some(classify[0]),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Docx,
                len: 10,
                head: Vec::new(),
            }),
        },
    );

    assert_eq!(requests(&backend, is_convert).len(), 1);
}

/// "Open anyway" on a too-large image asks for a classification that waives
/// the guard, or the override would be refused again.
#[cfg(feature = "images")]
#[test]
fn opening_anyway_classifies_with_the_guard_waived() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/big.png");
    let len = karet_filetype::SIZE_GUARD + 1;
    app.push_tab(crate::tab::open::reserve(&path, FileKind::TooLarge { len }));

    app.open_active_anyway();

    let waived = sent(&backend).into_iter().any(|(_, command)| {
        matches!(
            command,
            SessionCommand::Workspace(WorkspaceCommand::ClassifyPath {
                ignore_size: true,
                ..
            })
        )
    });
    assert!(waived);
}

/// A media reservation loads quietly: it carries the loading anchor rather than
/// rendering as the placeholder an unreadable file settles at.
#[cfg(feature = "images")]
#[test]
fn a_media_reservation_is_loading_until_its_bytes_arrive() {
    let (_backend, app) = remote_app();

    let tab = app.open_tab(Path::new("/not/on/this/machine/a.png"));

    assert!(tab.fill_since.is_some());
    assert!(tab.fill_error.is_none());
}

/// A byte reservation draws nothing while young, a muted loading line past the
/// reveal delay, and the read's failure once one is known — never the
/// "unavailable · 0 B" placeholder an unreadable file settles at.
#[test]
fn a_byte_reservation_renders_loading_then_its_failure() {
    let (_backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/blob.bin");
    let mut tab = crate::tab::open::reserve(&path, FileKind::Binary);
    tab.fill_since = Some(crate::pending::Pending::revealed());
    app.push_tab(tab);

    let loading = screen(&mut app, 80, 12).join("\n");
    assert!(loading.contains("Loading blob.bin"), "{loading}");

    app.tabs[app.active].fill_since = None;
    app.tabs[app.active].fill_error = Some("permission denied".to_owned());
    let failed = screen(&mut app, 80, 12).join("\n");
    assert!(failed.contains("permission denied"), "{failed}");
    assert!(!failed.contains("Loading"), "{failed}");
}

/// Open `path` as a byte view in a remote app and return the id of the read
/// its verdict starts.
fn reading(backend: &RecordingBackend, app: &mut App, path: &Path) -> RequestId {
    app.push_tab(crate::tab::open::reserve(path, FileKind::Binary));
    app.register_open_tabs();
    let classify = requests(backend, is_classify);
    app.on_backend_event(
        Some(classify[0]),
        SessionEvent::PathClassified {
            path: path.to_path_buf(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 8,
                head: Vec::new(),
            }),
        },
    );
    requests(backend, is_read)[0]
}

/// A read that fails says why on the tab, rather than leaving an empty view.
#[test]
fn a_failed_read_shows_its_error_on_the_reserved_tab() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/blob");
    let read = reading(&backend, &mut app, &path);

    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Err("permission denied".to_owned()),
        },
    );

    let tab = &app.tabs[app.active];
    assert_eq!(tab.fill_error.as_deref(), Some("permission denied"));
    assert!(tab.fill_since.is_none());
}

/// A chunked read stops once no tab is waiting for it, and a second verdict
/// while it streams starts no second stream.
#[test]
fn a_read_stops_when_its_tab_closes_and_is_never_doubled() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/blob");
    let read = reading(&backend, &mut app, &path);

    app.request_bytes(&path, FileKind::Binary, 0);
    assert_eq!(requests(&backend, is_read).len(), 1, "one stream per path");

    app.tabs.clear();
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0; 4],
                total_len: 8,
            }),
        },
    );

    assert_eq!(requests(&backend, is_read).len(), 1, "no chunk for nobody");
}

/// Splitting a text tab whose size guard has not settled keeps the copy
/// waiting for the verdict too, rather than opening its document past it.
#[test]
fn a_split_copy_waits_for_the_size_guard_like_its_source() {
    let (backend, mut app) = remote_app();
    app.open_path(Path::new("/not/on/this/machine/main.rs"));

    let copy = app.duplicate_active_tab();

    assert!(copy.size_unchecked);
    let opened = sent(&backend).into_iter().any(|(_, command)| {
        matches!(
            command,
            SessionCommand::Document(DocumentCommand::Open { .. })
        )
    });
    assert!(!opened);
}
