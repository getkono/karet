//! The Expand Selection and Shrink Selection rows of `docs/editing-behavior.md`,
//! run through the conformance harness over a Rust buffer (`t.rs`). Which step
//! each caret takes is covered in `karet_editor`; the node chains themselves,
//! for Rust, TypeScript and Python, in `karet_treesitter`. A tab backed by a
//! session document asks the language server first: that wiring is covered
//! at the end, against a recording backend.

use super::harness::*;
use super::support::completion_app;
use crate::app::*;

/// Shift+Alt+Right.
fn expand() -> Step {
    Step::Key(KeyCode::Right, KeyModifiers::SHIFT | KeyModifiers::ALT)
}

/// Shift+Alt+Left.
fn shrink() -> Step {
    Step::Key(KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::ALT)
}

#[test]
fn expand_selection_grows_along_the_syntax_tree() {
    check(vec![
        case(
            "a caret in a word selects the word",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand()],
            "fn f() { g(\"[hello|] world\", 2); }",
        ),
        case(
            "then the string's contents, the string, the arguments, the call",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), expand(), expand()],
            "fn f() { [g(\"hello world\", 2)|]; }",
        ),
        case(
            "the string's contents before its quotes",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand()],
            "fn f() { g(\"[hello world|]\", 2); }",
        ),
        case(
            "every caret grows on its own",
            "fn f() { g(a|b, c#d); }",
            vec![expand()],
            "fn f() { g([ab|], [cd#]); }",
        ),
        case(
            "carets that grow into one node merge",
            "fn f() { g(a|b, c#d); }",
            vec![expand(), expand()],
            "fn f() { g[(ab, cd)|]; }",
        ),
    ]);
}

#[test]
fn shrink_selection_restores_what_expand_grew_from() {
    check(vec![
        case(
            "back through every step to the caret",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), shrink(), shrink(), shrink()],
            "fn f() { g(\"hel|lo world\", 2); }",
        ),
        case(
            "one step back",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), shrink()],
            "fn f() { g(\"[hello world|]\", 2); }",
        ),
        case(
            "merged carets come back apart, with the primary",
            "fn f() { g(a|b, c#d); }",
            vec![expand(), expand(), shrink(), shrink()],
            "fn f() { g(a|b, c#d); }",
        ),
        case(
            "a backward selection keeps its direction",
            "fn f() { g([|ab], c); }",
            vec![expand(), shrink()],
            "fn f() { g([|ab], c); }",
        ),
        case(
            "with nothing expanded, shrink does nothing",
            "fn f() { g([ab|], c); }",
            vec![shrink()],
            "fn f() { g([ab|], c); }",
        ),
        case(
            "any other selection change ends the history",
            "fn f() { g(a|b, c); }",
            vec![expand(), key(KeyCode::Right), shrink()],
            "fn f() { g(ab|, c); }",
        ),
    ]);
}

// --- the language server's selection ranges --------------------------------

/// Two lines, each with a call whose arguments the carets sit between.
const TWO_CALLS: &str = "fn f() { g(ab, cd); }\nfn h() { k(ef, gh); }\n";

/// Carets just after each `, `-preceding comma: no word under either, so
/// the first press consults the ranges rather than taking a word.
fn carets() -> [LineCol; 2] {
    [LineCol::new(0, 14), LineCol::new(1, 14)]
}

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

/// A focused editor over [`TWO_CALLS`], backed by session document 9 and a
/// recording backend, with a caret at each of [`carets`].
fn served_app() -> (Arc<super::support::RecordingBackend>, App) {
    let (backend, mut app) = completion_app(TWO_CALLS, LineCol::new(0, 0));
    let idx = app.active;
    app.tabs[idx].editor.set_carets(&carets());
    (backend, app)
}

/// The same editor with no session document behind it, so Expand grows
/// along tree-sitter at once.
fn unserved_app() -> App {
    let (_, mut app) = served_app();
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = None;
    }
    app
}

fn press_expand(app: &mut App) {
    app.handle_key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
}

fn selections(app: &App) -> Vec<Range> {
    app.tabs[app.active]
        .editor
        .cursors()
        .selections
        .iter()
        .map(|selection| selection.range())
        .collect()
}

/// The selection-range requests a backend received, as `(id, positions)`.
fn asked(backend: &super::support::RecordingBackend) -> Vec<(RequestId, Vec<LineCol>)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Lsp(LspCommand::SelectionRanges { positions, .. }) => {
                        Some((*id, positions.clone()))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Answer the latest request with `chains`.
fn answer(app: &mut App, backend: &super::support::RecordingBackend, chains: Vec<Vec<Range>>) {
    let id = asked(backend).last().map(|(id, _)| *id);
    app.on_backend_event(
        id,
        SessionEvent::SelectionRanges {
            doc: DocumentId(9),
            version: 0,
            chains,
        },
    );
}

/// What tree-sitter alone grows the carets to after `presses` presses.
fn tree_sitter_after(presses: usize) -> Vec<Range> {
    let mut app = unserved_app();
    for _ in 0..presses {
        press_expand(&mut app);
    }
    selections(&app)
}

/// The server's chains for the two carets: the arguments without their
/// parentheses -- a range no tree-sitter node has -- then the whole call.
fn server_chains() -> Vec<Vec<Range>> {
    vec![
        vec![span(0, 11, 17), span(0, 9, 18)],
        vec![span(1, 11, 17), span(1, 9, 18)],
    ]
}

#[test]
fn expand_asks_the_server_at_every_caret_and_waits_for_its_answer() {
    let (backend, mut app) = served_app();
    let before = selections(&app);
    press_expand(&mut app);
    let requests = asked(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1, carets().to_vec());
    assert_eq!(selections(&app), before, "nothing grows before the answer");
}

#[test]
fn expand_follows_the_servers_chain_at_every_caret() {
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    answer(&mut app, &backend, server_chains());
    assert_eq!(selections(&app), vec![span(0, 11, 17), span(1, 11, 17)]);
    assert_ne!(
        selections(&app),
        tree_sitter_after(1),
        "the server's ranges, not tree-sitter's"
    );

    press_expand(&mut app);
    answer(&mut app, &backend, server_chains());
    assert_eq!(selections(&app), vec![span(0, 9, 18), span(1, 9, 18)]);
}

#[test]
fn presses_made_while_waiting_each_take_a_step_from_one_answer() {
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    press_expand(&mut app);
    assert_eq!(asked(&backend).len(), 1, "the same selections ask once");
    answer(&mut app, &backend, server_chains());
    assert_eq!(selections(&app), vec![span(0, 9, 18), span(1, 9, 18)]);
}

#[test]
fn an_empty_answer_falls_back_to_tree_sitter() {
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    answer(&mut app, &backend, vec![Vec::new(), Vec::new()]);
    assert_eq!(selections(&app), tree_sitter_after(1));
    assert_ne!(selections(&app), vec![span(0, 14, 14), span(1, 14, 14)]);
}

#[test]
fn a_caret_the_servers_ranges_do_not_enclose_grows_along_tree_sitter() {
    // The server answered for the first caret only; the second still grows.
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    answer(
        &mut app,
        &backend,
        vec![server_chains()[0].clone(), Vec::new()],
    );
    let grown = selections(&app);
    assert_eq!(grown[0], span(0, 11, 17));
    assert_eq!(grown[1], tree_sitter_after(1)[1]);
}

#[test]
fn an_answer_for_selections_that_have_moved_on_is_dropped() {
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    let idx = app.active;
    app.tabs[idx].editor.set_carets(&[LineCol::new(0, 0)]);
    answer(&mut app, &backend, server_chains());
    assert_eq!(selections(&app), vec![span(0, 0, 0)]);
}

#[test]
fn an_answer_for_text_that_has_changed_is_dropped() {
    // The carets are put back where they were, so only the edit tells the
    // answer is stale.
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    let idx = app.active;
    app.tabs[idx].editor.set_carets(&[LineCol::new(2, 0)]);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.tabs[idx].editor.set_carets(&carets());
    answer(&mut app, &backend, server_chains());
    assert_eq!(selections(&app), vec![span(0, 14, 14), span(1, 14, 14)]);
}

#[test]
fn an_answer_to_another_request_is_ignored() {
    let (_, mut app) = served_app();
    press_expand(&mut app);
    app.on_backend_event(
        Some(RequestId(999)),
        SessionEvent::SelectionRanges {
            doc: DocumentId(9),
            version: 0,
            chains: server_chains(),
        },
    );
    assert_eq!(selections(&app), vec![span(0, 14, 14), span(1, 14, 14)]);
}

#[test]
fn shrink_walks_back_what_the_servers_ranges_grew() {
    let (backend, mut app) = served_app();
    press_expand(&mut app);
    answer(&mut app, &backend, server_chains());
    press_expand(&mut app);
    answer(&mut app, &backend, server_chains());
    app.handle_key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
    assert_eq!(selections(&app), vec![span(0, 11, 17), span(1, 11, 17)]);
    app.handle_key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
    assert_eq!(selections(&app), vec![span(0, 14, 14), span(1, 14, 14)]);
}

/// A backend whose connection has closed: every send fails.
struct ClosedBackend;

impl karet_session::Backend for ClosedBackend {
    fn send(&self, _: RequestId, _: SessionCommand) -> Result<(), karet_session::BackendError> {
        Err(karet_session::BackendError::Closed)
    }

    fn next_id(&self) -> RequestId {
        RequestId(1)
    }

    fn take_events(&self) -> Option<karet_session::EventRx> {
        None
    }
}

#[test]
fn a_session_document_with_no_backend_grows_along_tree_sitter_at_once() {
    let (_, mut app) = served_app();
    app.backend = None;
    press_expand(&mut app);
    assert_eq!(selections(&app), tree_sitter_after(1));
    assert!(app.pending_expand.is_none(), "nothing is left waiting");
}

#[test]
fn a_request_the_backend_refuses_grows_along_tree_sitter_at_once() {
    let (_, mut app) = served_app();
    app.backend = Some(Arc::new(ClosedBackend));
    press_expand(&mut app);
    assert_eq!(selections(&app), tree_sitter_after(1));
    assert!(app.pending_expand.is_none(), "nothing is left waiting");
    press_expand(&mut app);
    assert_eq!(selections(&app), tree_sitter_after(2), "every press grows");
}
