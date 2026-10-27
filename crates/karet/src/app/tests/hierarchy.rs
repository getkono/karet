//! The call and type hierarchy panel: opened at once, filled by the roots,
//! grown one level per expansion, walked with the arrows, and left standing
//! when an item is opened.

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::Location;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::SymbolKind;
use karet_session::LanguageServerId;
use ratatui::style::Modifier;

use super::support::*;
use crate::app::hierarchy::HierarchyTree;
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
                        SessionCommand::Lsp(
                            LspCommand::PrepareHierarchy { .. }
                                | LspCommand::ExpandHierarchy { .. }
                        )
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
    app.hierarchy
        .tree
        .as_ref()
        .map(|tree| tree.rows().into_iter().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn selected(app: &App) -> Option<usize> {
    app.hierarchy.tree.as_ref().map(HierarchyTree::selected)
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

/// Each command asks for its own direction at the caret, and the panel opens
/// at once -- empty, titled, focused -- rather than when the server answers.
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
                [(_, SessionCommand::Lsp(LspCommand::PrepareHierarchy { doc, position, direction: asked }))]
                    if *doc == DocumentId(9) && *position == LineCol::new(0, 4) && *asked == direction
            ),
            "{command:?}: {requests:?}"
        );
        let shown = app.hierarchy.tree.as_ref().map(HierarchyTree::title);
        assert_eq!(shown, Some(title));
        assert!(app.overlay.is_none(), "the tree is a panel, not an overlay");
        assert_eq!(app.sidebar_panel, SidebarPanel::Hierarchy);
        assert!(app.sidebar_visible);
        assert_eq!(app.focus, Focus::Sidebar);
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
        SessionCommand::Lsp(LspCommand::ExpandHierarchy {
            doc, item: asked, ..
        }),
    )) = requests.get(1).cloned()
    else {
        panic!("the first root is expanded: {requests:?}");
    };
    assert_eq!((doc, *asked), (DocumentId(9), main));
    assert_eq!(rows(&app), vec!["▾ main", "▸ other"]);

    answer(&mut app, expand, vec![item("caller", &lib, 2)]);
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller", "▸ other"]);
    let hints = app
        .hierarchy
        .tree
        .as_ref()
        .map(HierarchyTree::row_hints)
        .unwrap_or_default();
    assert_eq!(hints[1].as_deref(), Some("lib.rs:3"));
}

/// Right expands the selected node, asking only the first time; Left
/// collapses it, and on a collapsed node climbs to its parent; Enter opens
/// the selected item where it is named and leaves the panel, its selection
/// and its expansion as they were, so Ctrl+8 lands back on the same row.
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
    assert_eq!(selected(&app), Some(0));
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
    assert_eq!(app.tabs[app.active].path(), Some(lib.as_path()));
    assert_eq!(app.focus, Focus::Editor, "the opened item takes the keys");
    // The opened tab waits for its text; the caret lands once it arrives.
    fill_reserved_tabs(&mut app);
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 3));

    // The panel is still there, as it was left.
    assert_eq!(app.sidebar_panel, SidebarPanel::Hierarchy);
    assert!(app.sidebar_visible);
    assert_eq!(rows(&app), vec!["▾ main", "  · caller"]);
    assert_eq!(selected(&app), Some(1));
    app.handle_key(KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL));
    assert_eq!(app.focus, Focus::Sidebar);
    assert_eq!(app.sidebar_panel, SidebarPanel::Hierarchy);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("main.rs").as_path())
    );
    assert_eq!(rows(&app), vec!["▾ main", "  · caller"]);
    assert_eq!(sent(&backend).len(), 3, "opening asks the server nothing");
}

/// Space toggles the selected node, `l` expands like Right, and `h`
/// collapses like Left.
#[test]
fn space_toggles_and_the_letter_keys_walk_like_the_arrows() {
    let (backend, mut app, root) = workspace("hierarchy-letters");
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

    key(&mut app, KeyCode::Char(' '));
    assert_eq!(rows(&app), vec!["▸ main"]);
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(rows(&app), vec!["▸ main"]);
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);
    assert_eq!(
        sent(&backend).len(),
        2,
        "known children are not asked again"
    );
}

/// A click on a row's marker folds or unfolds it; a click elsewhere on the
/// row opens the item, and the tree stays.
#[test]
fn clicking_a_marker_toggles_and_clicking_a_row_opens() {
    let (backend, mut app, root) = workspace("hierarchy-click");
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
    let painted = screen(&mut app, 100, 12);
    let rect = app.hierarchy_ui.results_rect;
    assert!(rect.height > 0, "{painted:?}");

    // Row 0 is `▾ main`: its marker sits one cell in from the panel edge.
    app.hierarchy_click(rect.x + 1, rect.y);
    assert_eq!(rows(&app), vec!["▸ main"]);
    app.hierarchy_click(rect.x + 2, rect.y);
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);

    // Row 1's marker is two cells further in; past it, the click opens.
    app.hierarchy_click(rect.x + 8, rect.y + 1);
    assert_eq!(app.tabs[app.active].path(), Some(lib.as_path()));
    assert_eq!(app.sidebar_panel, SidebarPanel::Hierarchy);
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);
    assert_eq!(selected(&app), Some(1));
    // Below the last row, a click does nothing.
    app.hierarchy_click(rect.x + 8, rect.y + 5);
    assert_eq!(selected(&app), Some(1));
}

/// The panel paints which way the tree grows, each row with its location,
/// and the switcher shows its button while a tree is held.
#[test]
fn the_panel_paints_its_rows_with_their_locations() {
    let (backend, mut app, root) = workspace("hierarchy-paint");
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
    let painted = screen(&mut app, 120, 12).join("\n");
    assert!(painted.contains("CALL HIERARCHY"), "{painted}");
    assert!(painted.contains("Incoming Calls"), "{painted}");
    assert!(!painted.contains("loading…"), "{painted}");
    assert!(painted.contains("▾ main  main.rs:1"), "{painted}");
    assert!(painted.contains("  ▸ caller  lib.rs:3"), "{painted}");
    assert!(
        app.last_frame
            .panel_hits
            .iter()
            .any(|&(_, _, panel)| panel == SidebarPanel::Hierarchy)
    );
}

/// The selected row is painted with the selection background, so the row
/// Enter would open is the one that stands out.
#[test]
fn the_selected_row_is_highlighted() {
    let (backend, mut app, root) = workspace("hierarchy-highlight");
    app.dispatch(Command::ShowSupertypes);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        prepare,
        vec![
            item("Base", &root.join("main.rs"), 0),
            item("Other", &root.join("lib.rs"), 0),
        ],
    );
    key(&mut app, KeyCode::Down);
    let buffer = frame(&mut app, 120, 12);
    let rect = app.hierarchy_ui.results_rect;
    let selection = app.theme.role(ThemeRole::Selection).to_ratatui();
    let first = &buffer[(rect.x + 3, rect.y)];
    let second = &buffer[(rect.x + 3, rect.y + 1)];
    assert_eq!(second.symbol(), "O");
    assert_eq!(second.bg, selection);
    assert!(second.modifier.contains(Modifier::BOLD));
    assert_ne!(first.bg, selection);
}

/// No roots drops the tree, hands the sidebar and the keys back to what the
/// panel replaced, and says why, in terms of the server when there is none
/// to ask.
#[test]
fn no_roots_closes_the_tree_and_says_why() {
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.sidebar_panel = SidebarPanel::Problems;
    app.sidebar_visible = false;
    app.dispatch(Command::ShowSubtypes);
    let prepare = sent(&backend)[0].0;
    answer(&mut app, prepare, Vec::new());
    assert!(app.hierarchy.tree.is_none());
    assert_eq!(app.sidebar_panel, SidebarPanel::Problems);
    assert!(!app.sidebar_visible);
    assert_eq!(app.focus, Focus::Editor);
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
        assert!(app.hierarchy.tree.is_none(), "{runtime:?}");
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
        if matches!(
            command,
            SessionCommand::Lsp(LspCommand::ExpandHierarchy { .. })
        ) {
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
    assert!(app.hierarchy.tree.is_none());
    assert_ne!(app.sidebar_panel, SidebarPanel::Hierarchy);
    answer(&mut app, prepare, Vec::new());
    assert!(app.hierarchy.tree.is_none());
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

/// An answer for a tree the user has replaced, or for a request the tree
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

    // A second hierarchy replaces the first; the first one's roots are late.
    app.dispatch(Command::ShowSupertypes);
    answer(&mut app, prepare, vec![item("x", Path::new("/x.rs"), 0)]);
    assert!(rows(&app).is_empty());
    let shown = app.hierarchy.tree.as_ref().map(HierarchyTree::title);
    assert_eq!(shown, Some("Supertypes"));
    assert_eq!(sent(&backend).len(), 2, "nothing was expanded");
}

/// A late child answer for a node of a replaced tree is dropped too, and
/// so is an answer that has no request id at all.
#[test]
fn a_late_child_answer_for_a_replaced_tree_changes_nothing() {
    let (backend, mut app, root) = workspace("hierarchy-late-child");
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        prepare,
        vec![item("main", &root.join("main.rs"), 0)],
    );
    let expand = sent(&backend)[1].0;
    app.dispatch(Command::ShowOutgoingCalls);
    let second = sent(&backend)[2].0;
    answer(
        &mut app,
        second,
        vec![item("callee", &root.join("lib.rs"), 2)],
    );
    let before = rows(&app);
    answer(
        &mut app,
        expand,
        vec![item("caller", &root.join("lib.rs"), 2)],
    );
    app.on_backend_event(
        None,
        SessionEvent::Hierarchy {
            direction: HierarchyDirection::OutgoingCalls,
            items: vec![item("stray", &root.join("lib.rs"), 0)],
        },
    );
    assert_eq!(rows(&app), before);
}

/// Replacing a tree on show keeps what the first one replaced: when the
/// second comes back empty, the sidebar returns there, not to an empty
/// Hierarchy panel.
#[test]
fn an_empty_replacement_returns_to_what_the_first_tree_replaced() {
    let (backend, mut app, root) = workspace("hierarchy-replace-empty");
    app.sidebar_panel = SidebarPanel::Todos;
    app.sidebar_visible = true;
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        prepare,
        vec![item("main", &root.join("main.rs"), 0)],
    );
    app.dispatch(Command::ShowSubtypes);
    let second = sent(&backend)[2].0;
    answer(&mut app, second, Vec::new());
    assert!(app.hierarchy.tree.is_none());
    assert_eq!(app.sidebar_panel, SidebarPanel::Todos);
    assert_eq!(app.focus, Focus::Editor);
}

/// A sidebar the user hid while the roots were on their way stays hidden
/// when they come back empty; only its panel goes back to what the
/// Hierarchy panel replaced.
#[test]
fn an_empty_answer_leaves_a_sidebar_hidden_meanwhile_hidden() {
    let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.sidebar_panel = SidebarPanel::Problems;
    app.sidebar_visible = true;
    app.dispatch(Command::ShowSubtypes);
    let prepare = sent(&backend)[0].0;
    app.sidebar_visible = false;
    app.focus = Focus::Editor;
    answer(&mut app, prepare, Vec::new());
    assert!(app.hierarchy.tree.is_none());
    assert_eq!(app.sidebar_panel, SidebarPanel::Problems);
    assert!(!app.sidebar_visible, "the hidden sidebar is not reopened");
    assert_eq!(app.focus, Focus::Editor);
}

/// A tree with one level answered: `main` (in `main.rs`) expanded onto
/// `caller` (in `lib.rs`), with `caller` selected.
fn answered_tree(name: &str) -> (Arc<RecordingBackend>, App, PathBuf) {
    let (backend, mut app, root) = workspace(name);
    app.dispatch(Command::ShowIncomingCalls);
    let prepare = sent(&backend)[0].0;
    answer(
        &mut app,
        prepare,
        vec![item("main", &root.join("main.rs"), 0)],
    );
    let expand = sent(&backend)[1].0;
    answer(
        &mut app,
        expand,
        vec![item("caller", &root.join("lib.rs"), 2)],
    );
    key(&mut app, KeyCode::Down);
    (backend, app, root)
}

/// Stand in for the tab the tree was asked from having closed: the only
/// code tab left is open on `path` as `doc`, or holds no document at all.
fn only_tab_open_on(app: &mut App, path: PathBuf, doc: Option<DocumentId>) {
    let active = app.active;
    if let TabKind::Code {
        path: shown,
        doc: held,
        ..
    } = &mut app.tabs[active].kind
    {
        *shown = path;
        *held = doc;
    }
}

/// Once the document the tree was asked from has closed, an expansion is
/// grown through a document still open on a file the tree names, not sent
/// for the closed one -- the session would answer that only with an error.
#[test]
fn an_expansion_after_the_origin_closes_goes_through_an_open_document() {
    let (backend, mut app, root) = answered_tree("hierarchy-retarget");
    only_tab_open_on(&mut app, root.join("lib.rs"), Some(DocumentId(12)));
    key(&mut app, KeyCode::Right);
    let requests = sent(&backend);
    assert!(
        matches!(
            requests.get(2),
            Some((_, SessionCommand::Lsp(LspCommand::ExpandHierarchy { doc, .. }))) if *doc == DocumentId(12)
        ),
        "{requests:?}"
    );
    assert_eq!(rows(&app), vec!["▾ main", "  ▾ caller"]);
}

/// With no document open on any file the tree names, an expansion asks
/// nothing, leaves no node loading, and says how to grow the tree again.
#[test]
fn an_expansion_with_no_open_document_says_so_and_waits_on_nothing() {
    let (backend, mut app, root) = answered_tree("hierarchy-no-doc");
    only_tab_open_on(&mut app, root.join("main.rs"), None);
    key(&mut app, KeyCode::Right);
    assert_eq!(
        sent(&backend).len(),
        2,
        "nothing is sent for a closed document"
    );
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);
    assert!(app.hierarchy_loading().is_none());
    let titles: Vec<_> = app
        .notifications
        .active()
        .iter()
        .map(|note| note.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec!["call hierarchy: open lib.rs again to expand it further".to_owned()]
    );
}

/// An expansion the session answers with an error (its document closed in
/// the meantime) puts the node back as never asked for, so no loading note
/// lingers and the next Right asks again; a root request answered that way
/// drops the tree.
#[test]
fn an_error_answer_settles_the_request_it_answers() {
    let (backend, mut app, _root) = answered_tree("hierarchy-error");
    key(&mut app, KeyCode::Right);
    let expand = sent(&backend)[2].0;
    let unknown = || SessionEvent::Notification {
        severity: Severity::Error,
        kind: NotificationKind::System,
        message: "unknown document 9".to_owned(),
    };
    app.on_backend_event(Some(expand), unknown());
    assert_eq!(rows(&app), vec!["▾ main", "  ▸ caller"]);
    assert!(app.hierarchy_loading().is_none());
    key(&mut app, KeyCode::Right);
    assert_eq!(sent(&backend).len(), 4, "the next expansion asks again");

    app.dispatch(Command::ShowSupertypes);
    let prepare = sent(&backend)[4].0;
    app.on_backend_event(Some(prepare), unknown());
    assert!(app.hierarchy.tree.is_none());
    assert_ne!(app.sidebar_panel, SidebarPanel::Hierarchy);
}

/// With no tree, the panel cannot be selected: Ctrl+8 says how to get one
/// and leaves the sidebar as it was.
#[test]
fn selecting_the_panel_without_a_tree_refuses() {
    let (_backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 4));
    app.handle_key(KeyEvent::new(KeyCode::Char('8'), KeyModifiers::CONTROL));
    assert_ne!(app.sidebar_panel, SidebarPanel::Hierarchy);
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(app.notifications.active().len(), 1);
}

/// A tree waiting on its server schedules the wake that reveals the
/// loading note while its panel is on screen, and paints the note once the
/// reveal delay has passed; with the panel hidden, no wake is scheduled.
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
    let painted = screen(&mut app, 120, 12).join("\n");
    assert!(!painted.contains("loading…"), "too soon: {painted}");

    if let Some(tree) = app.hierarchy.tree.as_mut() {
        tree.reveal_loading();
    }
    let painted = screen(&mut app, 120, 12).join("\n");
    assert!(painted.contains("Incoming Calls · loading…"), "{painted}");
    assert!(painted.contains("  loading…"), "{painted}");

    app.sidebar_visible = false;
    let loading = app.hierarchy_loading();
    assert!(
        !app.pendings()
            .iter()
            .any(|pending| Some(*pending) == loading)
    );
}
