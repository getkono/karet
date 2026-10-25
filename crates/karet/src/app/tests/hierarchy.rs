//! The call and type hierarchy tree: opened at once, filled by the roots,
//! grown one level per expansion, and walked with the arrows.

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::Location;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::SymbolKind;
use karet_session::LanguageServerId;

use super::support::*;
use crate::app::*;

/// The hierarchy requests a backend received, in order.
fn sent(backend: &RecordingBackend) -> Vec<(RequestId, SessionCommand)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| {
                    matches!(
                        command,
                        SessionCommand::PrepareHierarchy { .. }
                            | SessionCommand::ExpandHierarchy { .. }
                    )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn item(name: &str, path: &Path, line: u32) -> HierarchyItem {
    let at = LineCol::new(line, 3);
    HierarchyItem {
        name: name.to_owned(),
        kind: SymbolKind::Function,
        detail: Some(format!("fn {name}()")),
        location: Location {
            path: path.to_path_buf(),
            range: Range { start: at, end: at },
        },
        payload: format!("{{\"name\":\"{name}\"}}"),
    }
}

fn answer(app: &mut App, id: RequestId, items: Vec<HierarchyItem>) {
    let direction = HierarchyDirection::IncomingCalls;
    app.on_backend_event(Some(id), SessionEvent::Hierarchy { direction, items });
}

fn rows(app: &App) -> Vec<String> {
    app.overlay
        .as_ref()
        .map(|overlay| overlay.rows().into_iter().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// A workspace with `main.rs` open and `lib.rs` beside it on disk, so an
/// item there can be opened.
fn workspace(name: &str) -> (Arc<RecordingBackend>, App, PathBuf) {
    let root = test_dir(name);
    write_file(&root, "lib.rs", b"one\ntwo\nfn caller() {}\n");
    write_file(&root, "main.rs", b"fn main() {}\n");
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.root = root.clone();
    let active = app.active;
    if let TabKind::Code { path, .. } = &mut app.tabs[active].kind {
        *path = root.join("main.rs");
    }
    (backend, app, root)
}

/// Each command asks for its own direction at the caret, and the tree opens
/// at once -- empty, titled -- rather than when the server answers.
#[test]
fn each_command_opens_its_tree_at_once_asking_at_the_caret() {
    for (command, direction, title) in [
        (
            Command::ShowIncomingCalls,
            HierarchyDirection::IncomingCalls,
            "Incoming Calls",
        ),
        (
            Command::ShowOutgoingCalls,
            HierarchyDirection::OutgoingCalls,
            "Outgoing Calls",
        ),
        (
            Command::ShowSupertypes,
            HierarchyDirection::Supertypes,
            "Supertypes",
        ),
        (
            Command::ShowSubtypes,
            HierarchyDirection::Subtypes,
            "Subtypes",
        ),
    ] {
        let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
        app.dispatch(command);
        let requests = sent(&backend);
        assert!(
            matches!(
                requests.as_slice(),
                [(_, SessionCommand::PrepareHierarchy { doc, position, direction: asked })]
                    if *doc == DocumentId(9) && *position == LineCol::new(0, 4) && *asked == direction
            ),
            "{command:?}: {requests:?}"
        );
        let overlay = app.overlay.as_ref().map(Overlay::title).unwrap_or_default();
        assert!(overlay.starts_with(title), "{overlay}");
        assert!(rows(&app).is_empty(), "nothing is shown before the answer");
    }
}

/// The roots fill the tree and the first is expanded straight away -- its
/// own item handed back -- so the tree opens on an answer; its children
/// then appear under it, indented.
#[test]
fn the_roots_fill_the_tree_and_the_first_is_expanded() {
    let (backend, mut app, root) = workspace("hierarchy-roots");
    let lib = root.join("lib.rs");
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;

    let main = item("main", &root.join("main.rs"), 0);
    answer(
        &mut app,
        prepare,
        vec![main.clone(), item("other", &lib, 0)],
    );
    let requests = sent(&backend);
    let Some((
        expand,
        SessionCommand::ExpandHierarchy {
            doc, item: asked, ..
        },
    )) = requests.get(1).cloned()
    else {
        panic!("the first root is expanded: {requests:?}");
    };
    assert_eq!((doc, *asked), (DocumentId(9), main));
    assert_eq!(rows(&app), vec!["▾ main", "▸ other"]);

    answer(&mut app, expand, vec![item("caller", &lib, 2)]);
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller", "▸ other"]);
    let hints = app
        .overlay
        .as_ref()
        .map(Overlay::row_hints)
        .unwrap_or_default();
    assert_eq!(hints[1].as_deref(), Some("lib.rs:3"));
}

/// Right expands the selected node, asking only the first time; Left
/// collapses it, and on a collapsed node climbs to its parent; Enter opens
/// the selected item where it is named.
#[test]
fn the_arrows_walk_the_tree_and_enter_opens_the_item() {
    let (backend, mut app, root) = workspace("hierarchy-walk");
    let lib = root.join("lib.rs");
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        prepare,
        vec![item("main", &root.join("main.rs"), 0)],
    );
    let expand = sent(&backend)[1].0;
    answer(&mut app, expand, vec![item("caller", &lib, 2)]);

    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Right);
    assert_eq!(
        sent(&backend).len(),
        3,
        "the caller's callers are asked for"
    );
    let deeper = sent(&backend)[2].0;
    answer(&mut app, deeper, Vec::new());
    assert_eq!(rows(&app), vec!["▾ main", "  · caller"], "none: a leaf");

    key(&mut app, KeyCode::Left);
    assert_eq!(app.overlay.as_ref().map(Overlay::selected), Some(0));
    key(&mut app, KeyCode::Left);
    assert_eq!(rows(&app), vec!["▸ main"]);
    key(&mut app, KeyCode::Right);
    assert_eq!(
        sent(&backend).len(),
        3,
        "known children are not asked again"
    );
    assert_eq!(rows(&app), vec!["▾ main", "  · caller"]);

    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    assert_eq!(app.tabs[app.active].path(), Some(lib.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 3));
}

/// No roots closes the tree and says why, in terms of the server when
/// there is none to ask.
#[test]
fn no_roots_closes_the_tree_and_says_why() {
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.dispatch(Command::ShowSubtypes);
    let prepare = sent(&backend)[0].0;
    answer(&mut app, prepare, Vec::new());
    assert!(app.overlay.is_none());
    let titles: Vec<_> = app
        .notifications
        .active()
        .iter()
        .map(|note| note.title.clone())
        .collect();
    assert_eq!(titles, vec!["no language server for this file".to_owned()]);
}

/// No roots from a server that is starting, has failed, or is running says
/// which: an instant empty answer from a server that cannot answer yet is not
/// a claim about the symbol.
#[test]
fn no_roots_names_the_server_state() {
    for (runtime, reason) in [
        (
            LanguageServerRuntimeState::Starting,
            "language server is still starting",
        ),
        (
            LanguageServerRuntimeState::Retrying,
            "language server is still starting",
        ),
        (
            LanguageServerRuntimeState::CircuitOpen,
            "the language server for this file is not running",
        ),
        (
            LanguageServerRuntimeState::Running,
            "no type hierarchy at the caret",
        ),
    ] {
        let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
        let active = app.active;
        if let TabKind::Code { path, .. } = &mut app.tabs[active].kind {
            *path = PathBuf::from("/workspace/main.rs");
        }
        app.show_language_server_status(
            None,
            vec![language_server_status(
                LanguageServerId::RustAnalyzer,
                "rust",
                true,
            )],
        );
        app.update_language_server_runtime(
            LanguageServerId::RustAnalyzer,
            PathBuf::from("/workspace"),
            runtime,
            None,
        );
        app.dispatch(Command::ShowSupertypes);
        let prepare = sent(&backend)[0].0;
        answer(&mut app, prepare, Vec::new());
        assert!(app.overlay.is_none(), "{runtime:?}");
        let titles: Vec<_> = app
            .notifications
            .active()
            .iter()
            .map(|note| note.title.clone())
            .collect();
        // The status updates may toast on their own; the refusal is the last.
        assert_eq!(
            titles.last().map(String::as_str),
            Some(reason),
            "{runtime:?}"
        );
    }
}

/// Records like [`RecordingBackend`], but refuses every expansion, as a
/// backend that closed after the roots were asked for would.
struct RefusingExpansion(RecordingBackend);

impl Backend for RefusingExpansion {
    fn send(&self, id: RequestId, command: SessionCommand) -> Result<(), BackendError> {
        if matches!(command, SessionCommand::ExpandHierarchy { .. }) {
            return Err(BackendError::Closed);
        }
        self.0.send(id, command)
    }

    fn next_id(&self) -> RequestId {
        self.0.next_id()
    }

    fn take_events(&self) -> Option<karet_session::EventRx> {
        None
    }
}

/// An expansion that could not be sent leaves no node waiting on an answer
/// that will never come: the node shows as a leaf, and no loading note
/// lingers.
#[test]
fn an_expansion_that_cannot_be_sent_becomes_a_leaf() {
    let (_, mut app, root) = workspace("hierarchy-unsent");
    let backend = Arc::new(RefusingExpansion(RecordingBackend::new()));
    app.backend = Some(backend.clone());
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend.0)[0].0;
    answer(
        &mut app,
        prepare,
        vec![item("main", &root.join("main.rs"), 0)],
    );
    assert_eq!(rows(&app), vec!["· main"]);
    assert!(app.hierarchy_loading().is_none());
    assert_eq!(sent(&backend.0).len(), 1, "only the roots were sent");
}

/// A server without the hierarchy is reported once: the tree closes on the
/// notice, and the empty answer after it adds nothing.
#[test]
fn an_unsupported_hierarchy_is_reported_once() {
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.dispatch(Command::ShowOutgoingCalls);
    let prepare = sent(&backend)[0].0;
    app.on_backend_event(
        Some(prepare),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::CallHierarchy,
        },
    );
    answer(&mut app, prepare, Vec::new());
    assert!(app.overlay.is_none());
    let titles: Vec<_> = app
        .notifications
        .active()
        .iter()
        .map(|note| note.title.clone())
        .collect();
    let server = LanguageServerId::RustAnalyzer.display_name().to_string();
    assert_eq!(
        titles,
        vec![format!("{server} does not support the call hierarchy")]
    );
}

/// An answer for a tree the user has closed, or for a request the tree
/// does not wait on, changes nothing.
#[test]
fn a_stale_answer_changes_nothing() {
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        RequestId(prepare.0 + 100),
        vec![item("x", Path::new("/x.rs"), 0)],
    );
    assert!(rows(&app).is_empty());

    app.overlay = None;
    answer(&mut app, prepare, vec![item("x", Path::new("/x.rs"), 0)]);
    assert!(app.overlay.is_none());
    assert_eq!(sent(&backend).len(), 1, "nothing was expanded");
}

/// A tree waiting on its server schedules the wake that reveals the
/// loading note, and the note names no quick-open work.
#[test]
fn a_slow_answer_reveals_the_loading_note() {
    let (_backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.dispatch(Command::ShowIncomingCalls);
    let loading = app.hierarchy_loading();
    assert!(loading.is_some());
    assert!(
        app.pendings()
            .iter()
            .any(|pending| Some(*pending) == loading)
    );
    let note = app.overlay.as_ref().map(Overlay::loading_note);
    assert_eq!(note, Some("loading…"));
}
