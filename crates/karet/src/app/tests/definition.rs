use karet_core::Location;
use karet_core::Range;

use super::harness::At;
use super::harness::Rig;
use super::harness::Step;
use super::support::*;
use crate::app::navigation::NAV_LINE_THRESHOLD;
use crate::app::*;

/// The definition requests a backend received, as `(id, position)`.
fn definition_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Definition { position, .. } => Some((*id, *position)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn at(line: u32, col: u32) -> Range {
    Range {
        start: LineCol::new(line, col),
        end: LineCol::new(line, col + 3),
    }
}

/// A workspace with `main.rs` open and `other.rs` beside it, both on disk so the
/// jump stack (which drops origins whose file has since vanished) sees real files.
fn workspace(name: &str) -> Option<(Arc<RecordingBackend>, App, PathBuf)> {
    let root = test_dir(name);
    write_file(&root, "other.rs", b"pub fn target() {}\nsecond line\n");
    write_file(&root, "main.rs", b"target();\n");
    let (backend, mut app) = completion_app("target();\n", LineCol::new(0, 1));
    app.root = root.clone();
    let active = app.active;
    if let TabKind::Code { path, .. } = &mut app.tabs[active].kind {
        *path = root.join("main.rs");
    }
    Some((backend, app, root.join("other.rs")))
}

#[test]
fn the_command_requests_the_definition_at_the_caret() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::GoToDefinition);

    let requests = definition_requests(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1, LineCol::new(0, 9));
}

#[test]
fn the_command_without_a_code_file_explains_itself_and_sends_nothing() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.dispatch(Command::GoToDefinition);

    assert!(definition_requests(&backend).is_empty());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("go to definition: open a code file first")
    );
}

#[test]
fn a_single_location_opens_the_file_and_lands_on_the_range_start() {
    let Some((backend, mut app, other)) = workspace("definition-single") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(0, 7),
            }],
        },
    );

    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    // The caret lands on the definition's start, as a bare caret — the end of the
    // range is deliberately not selected.
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 7));
    assert!(app.tabs[app.active].editor.selection_range().is_none());
}

#[test]
fn an_empty_answer_reports_why_and_leaves_the_caret_alone() {
    let (backend, mut app) = completion_app("target();\n", LineCol::new(0, 2));
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    app.on_backend_event(Some(id), SessionEvent::Definitions { locations: vec![] });

    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 2));
    // No server is attached in a test app, so the refusal says so rather than
    // claiming the symbol has no definition.
    assert_eq!(
        last_message(&app).as_deref(),
        Some("no language server for this file")
    );
}

#[test]
fn a_superseded_answer_is_ignored_and_the_live_request_still_lands() {
    let Some((backend, mut app, other)) = workspace("definition-superseded") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    app.dispatch(Command::GoToDefinition);
    let requests = definition_requests(&backend);
    assert_eq!(requests.len(), 2);

    // The first request answers late. `Event::Definitions` carries no document or
    // version, so the id is the only thing distinguishing it — and dropping the
    // pending record here would strand the live request forever.
    app.on_backend_event(
        Some(requests[0].0),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );
    assert_ne!(app.tabs[app.active].path(), Some(other.as_path()));

    app.on_backend_event(
        Some(requests[1].0),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
}

#[test]
fn an_answer_for_a_view_the_user_left_is_dropped() {
    let Some((backend, mut app, other)) = workspace("definition-stale-view") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    // Move to a different tab while the request is in flight.
    app.push_tab(text_tab("elsewhere.rs", "unrelated\n"));
    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(0, 7),
            }],
        },
    );

    assert_eq!(
        app.tabs[app.active].path().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new("elsewhere.rs")),
        "a jump must not yank the user out of the tab they moved to"
    );
}

#[test]
fn an_unsolicited_definition_event_is_ignored() {
    let (_backend, mut app) = completion_app("target();\n", LineCol::new(0, 1));
    let before = app.active;

    app.on_backend_event(
        Some(RequestId(77)),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: PathBuf::from("/nowhere.rs"),
                range: at(0, 0),
            }],
        },
    );

    assert_eq!(app.active, before);
}

#[test]
fn several_locations_open_a_picker_with_workspace_relative_rows() {
    let Some((backend, mut app, other)) = workspace("definition-picker") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![
                Location {
                    path: other.clone(),
                    range: at(0, 7),
                },
                // A duplicate of the first, which servers do sometimes send.
                Location {
                    path: other.clone(),
                    range: at(0, 7),
                },
                Location {
                    path: other.clone(),
                    range: at(1, 0),
                },
            ],
        },
    );

    let Some(overlay) = app.overlay.as_ref() else {
        panic!("several locations should offer a choice");
    };
    // Paths are shown relative to the workspace and lines 1-based, as everywhere
    // else in the UI, each with its trimmed source line; the duplicate is
    // dropped and server order is preserved.
    assert_eq!(overlay.title(), "Go to Definition");
    assert_eq!(
        overlay.rows(),
        vec!["other.rs:1  pub fn target() {}", "other.rs:2  second line"]
    );

    app.overlay_accept();
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 7));
}

/// Jump to `other.rs` and return the app positioned there.
fn jumped(name: &str) -> Option<(App, PathBuf, PathBuf)> {
    let (backend, mut app, other) = workspace(name)?;
    let origin = app.tabs[app.active].path()?.to_path_buf();
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;
    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );
    Some((app, origin, other))
}

#[test]
fn going_back_returns_to_the_position_the_jump_started_from() {
    let Some((mut app, origin, other)) = jumped("definition-back") else {
        return;
    };
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));

    app.dispatch(Command::JumpBack);

    assert_eq!(app.tabs[app.active].path(), Some(origin.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 1));
    // The stack is spent, so a second Go Back says so rather than bouncing.
    app.dispatch(Command::JumpBack);
    assert_eq!(last_message(&app).as_deref(), Some("nothing to go back to"));
}

#[test]
fn going_back_skips_an_origin_whose_file_is_gone() {
    let Some((mut app, origin, _other)) = jumped("definition-back-deleted") else {
        return;
    };
    // `workspace` writes main.rs only in memory, so remove the on-disk origin the
    // stack recorded and check we do not reopen it as an empty phantom buffer.
    let _ = std::fs::remove_file(&origin);

    app.dispatch(Command::JumpBack);
    assert_eq!(last_message(&app).as_deref(), Some("nothing to go back to"));
}

#[test]
fn the_jump_stack_stays_bounded() {
    let Some((backend, mut app, other)) = workspace("definition-back-bounded") else {
        return;
    };
    for line in 0..(64 + 5) {
        app.dispatch(Command::GoToDefinition);
        let requests = definition_requests(&backend);
        let Some(&(id, _)) = requests.last() else {
            return;
        };
        app.on_backend_event(
            Some(id),
            SessionEvent::Definitions {
                locations: vec![Location {
                    path: other.clone(),
                    range: at(line % 2, 0),
                }],
            },
        );
    }
    for history in app.nav_history.values() {
        let (back, forward) = history.depth();
        assert!(
            back <= 64 && forward == 0,
            "history grew to {back}/{forward}"
        );
    }
}

#[test]
fn going_forward_returns_to_where_go_back_left() {
    let Some((mut app, origin, other)) = jumped("definition-forward") else {
        return;
    };
    app.dispatch(Command::JumpBack);
    assert_eq!(app.tabs[app.active].path(), Some(origin.as_path()));

    app.dispatch(Command::JumpForward);
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 0));
    app.dispatch(Command::JumpForward);
    assert_eq!(
        last_message(&app).as_deref(),
        Some("nothing to go forward to")
    );
    // And back again still works from the far end.
    app.dispatch(Command::JumpBack);
    assert_eq!(app.tabs[app.active].path(), Some(origin.as_path()));
}

/// A code tab over `lines` numbered lines, saved on disk, caret at `caret`.
fn long_file(name: &str, lines: u32, caret: LineCol) -> (App, PathBuf) {
    let text: String = (0..lines).map(|i| format!("line {i}\n")).collect();
    let root = test_dir(name);
    write_file(&root, "long.rs", text.as_bytes());
    let (_backend, mut app) = completion_app(&text, caret);
    app.root = root.clone();
    let active = app.active;
    if let TabKind::Code { path, .. } = &mut app.tabs[active].kind {
        *path = root.join("long.rs");
    }
    app.observe_navigation();
    (app, root.join("long.rs"))
}

#[test]
fn a_large_caret_move_is_a_step_back_and_a_small_one_is_not() {
    let (mut app, _path) = long_file("nav-large-move", 60, LineCol::new(0, 0));
    app.dispatch(Command::CaretDown);
    app.observe_navigation();
    app.dispatch(Command::CaretDocEnd);
    app.observe_navigation();

    app.dispatch(Command::JumpBack);
    // Back to where the long move started, not the line before the small one.
    assert_eq!(app.tabs[app.active].editor.cursor().line, 1);
    app.dispatch(Command::JumpBack);
    assert_eq!(last_message(&app).as_deref(), Some("nothing to go back to"));
}

#[test]
fn a_history_entry_moves_with_an_edit_above_it() {
    let (mut app, _path) = long_file("nav-edit-shift", 60, LineCol::new(30, 0));
    app.dispatch(Command::CaretDocStart);
    app.observe_navigation();
    // Two new lines above the entry at line 30.
    app.dispatch(Command::InsertNewline);
    app.dispatch(Command::InsertNewline);
    app.observe_navigation();

    app.dispatch(Command::JumpBack);
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(32, 0));
}

#[test]
fn history_is_per_pane() {
    let (mut app, _path) = long_file("nav-per-pane", 60, LineCol::new(0, 0));
    app.dispatch(Command::CaretDocEnd);
    app.observe_navigation();
    app.dispatch(Command::SplitRight);
    app.observe_navigation();

    // The new pane has not been anywhere yet.
    app.dispatch(Command::JumpBack);
    assert_eq!(last_message(&app).as_deref(), Some("nothing to go back to"));
}

#[test]
fn a_paste_over_a_selection_records_nothing() {
    let (mut app, _path) = long_file("nav-paste-selection", 60, LineCol::new(5, 0));
    // A selection whose head, the caret, sits at its start.
    app.dispatch(Command::SelectUp);
    app.observe_navigation();
    app.handle_paste("pasted\n".repeat(20));
    app.observe_navigation();

    app.dispatch(Command::JumpBack);
    assert_eq!(last_message(&app).as_deref(), Some("nothing to go back to"));
}

#[test]
fn a_large_move_within_a_preview_tab_is_a_step_back() {
    let (mut app, _path) = long_file("nav-preview-move", 60, LineCol::new(0, 0));
    let active = app.active;
    app.tabs[active].is_preview = true;
    app.observe_navigation();
    app.dispatch(Command::CaretDocEnd);
    app.observe_navigation();

    app.dispatch(Command::JumpBack);
    assert_eq!(app.tabs[app.active].editor.cursor().line, 0);
}

#[test]
fn closing_a_split_pane_drops_its_history() {
    let (mut app, _path) = long_file("nav-closed-pane", 60, LineCol::new(0, 0));
    app.dispatch(Command::SplitRight);
    let split = app.focus_pane();
    app.dispatch(Command::CaretDocEnd);
    app.observe_navigation();
    assert!(app.nav_history.contains_key(&split));

    app.close_tab_at(app.active);
    app.observe_navigation();

    assert_eq!(app.layout.pane_count(), 1);
    assert!(!app.nav_history.contains_key(&split));
}

#[test]
fn an_edit_in_one_split_leaves_a_far_click_in_the_other_a_step_back() {
    let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let root = test_dir("nav-split-edit-click");
    write_file(&root, "t.rs", body.as_bytes());
    let mut rig = Rig::sized(&format!("|{body}"), 80, 40);
    let active = rig.app.active;
    if let TabKind::Code { path, .. } = &mut rig.app.tabs[active].kind {
        *path = root.join("t.rs");
    }
    rig.app.observe_navigation();
    rig.step(Step::Cmd(Command::SplitRight));
    rig.app.observe_navigation();
    // Back to the first split, without moving its caret, and edit there.
    rig.step(Step::Click(At::Pane(0, 0, 0), KeyModifiers::NONE));
    rig.app.observe_navigation();
    rig.step(Step::Cmd(Command::InsertNewline));
    rig.app.observe_navigation();
    // The session's echo brings the second split's copy of the text up to date.
    let buffer = match &rig.app.tabs[rig.app.active].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => karet_text::TextBuffer::from_text(""),
    };
    let snap = DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::new(karet_syntax::Highlights::default()),
        semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
        folds: Arc::new(FoldRegions::default()),
        decorations: Arc::new(Vec::new()),
        syntax_error_lines: Arc::new(Vec::new()),
        language: Some("Rust"),
        dirty: true,
        cursor: None,
    };
    rig.app.on_snapshot(DocumentId(9), &snap);

    // One click both focuses the second split and moves its caret far.
    rig.step(Step::Click(
        At::Pane(1, 2 * NAV_LINE_THRESHOLD, 0),
        KeyModifiers::NONE,
    ));
    rig.app.observe_navigation();
    assert_eq!(rig.focused_index(), Some(1));
    assert_eq!(
        rig.tab().editor.cursor().line,
        2 * NAV_LINE_THRESHOLD,
        "the click placed the caret"
    );

    rig.step(Step::Cmd(Command::JumpBack));
    assert_eq!(rig.focused_index(), Some(1));
    assert!(
        rig.tab().editor.cursor().line < NAV_LINE_THRESHOLD,
        "Go Back returns to where the second split was before the click"
    );
}

#[test]
fn alt_left_in_the_search_field_is_still_word_motion() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Search;
    app.search.query = "foo bar".into();
    app.search_focus_field(SearchPanelField::Find);
    app.search
        .query_edit
        .set_cursor(&app.search.query, 7, false);

    send_key(&mut app, KeyCode::Left, KeyModifiers::ALT);
    send_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);

    assert_eq!(app.search.query, "foo xbar");
    assert!(app.search.input);
}

/// A left click at `(col, row)` carrying `mods`.
fn click(col: u16, row: u16, mods: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: mods,
    }
}

#[test]
fn ctrl_click_requests_the_definition_at_the_click_not_the_caret() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 0));
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 40, 5))];

    // Screen column 11 is buffer column 8 (a 3-cell gutter), inside "target".
    app.handle_editor_click(click(11, 0, KeyModifiers::CONTROL));

    let requests = definition_requests(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1, LineCol::new(0, 8));
    // The caret moved to the click, so Go Back returns to the clicked symbol.
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 8));
}

#[test]
fn a_plain_click_sends_no_definition_request() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 0));
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 40, 5))];

    app.handle_editor_click(click(11, 0, KeyModifiers::NONE));

    assert!(definition_requests(&backend).is_empty());
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 8));
}

#[test]
fn alt_and_shift_clicks_keep_their_own_gestures() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 0));
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 40, 5))];

    // Alt+click adds a caret rather than jumping...
    app.handle_editor_click(click(11, 0, KeyModifiers::ALT));
    assert!(app.tabs[app.active].editor.has_multiple_cursors());
    // ...and Ctrl+Alt+click stays out of the way of multi-cursor editing.
    app.handle_editor_click(click(13, 0, KeyModifiers::ALT | KeyModifiers::CONTROL));
    // Shift+click extends the selection.
    app.tabs[app.active].editor.collapse_to_primary();
    app.handle_editor_click(click(6, 0, KeyModifiers::SHIFT));
    assert!(app.tabs[app.active].editor.selection_range().is_some());

    assert!(definition_requests(&backend).is_empty());
}

/// Mark a running language server covering `root`, so the Ctrl-hover gate (which
/// promises only what a click can deliver) is satisfied.
fn attach_running_server(app: &mut App, root: &Path) {
    use karet_session::LanguageServerId;
    use karet_session::LanguageServerInstanceStatus;
    use karet_session::LanguageServerRuntimeState;
    use karet_session::LanguageServerSource;
    use karet_session::LanguageServerStatus;

    app.show_language_server_status(
        None,
        vec![LanguageServerStatus {
            ever_installed: false,
            declined: false,
            server: LanguageServerId::new("rust-analyzer"),
            languages: vec!["rust".to_owned()],
            enabled: true,
            managed: true,
            manual_install_reason: None,
            installed: Some("1.0".to_owned()),
            cleanup_pending: false,
            instances: vec![LanguageServerInstanceStatus {
                root: root.to_path_buf(),
                source: LanguageServerSource::Path,
                command: Some("rust-analyzer".to_owned()),
                args: Vec::new(),
                runtime: LanguageServerRuntimeState::Running,
                open_documents: 1,
                error: None,
            }],
        }],
    );
}

/// A hover at `(col, row)` with `mods` held.
fn hover(col: u16, row: u16, mods: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Moved,
        column: col,
        row,
        modifiers: mods,
    }
}

/// An app with a running server, one pane frame, and the cursor over "target".
fn hover_app(name: &str) -> Option<(App, Rect)> {
    let (_backend, mut app, _other) = workspace(name)?;
    let root = app.root.clone();
    attach_running_server(&mut app, &root);
    let rect = Rect::new(0, 0, 40, 5);
    app.pane_frames = vec![content_frame(&app, rect)];
    Some((app, rect))
}

#[test]
fn ctrl_hover_underlines_the_word_under_the_pointer() {
    let Some((mut app, _rect)) = hover_app("definition-hover") else {
        return;
    };
    // Screen column 4 is buffer column 1, inside "target" on `target();`.
    app.update_definition_hover(&hover(4, 0, KeyModifiers::CONTROL));

    let Some(decoration) = app.definition_underline_decoration() else {
        panic!("a Ctrl-hovered identifier should be underlined");
    };
    assert_eq!(decoration.kind, karet_core::DecorationKind::Underline);
    assert_eq!(decoration.range.start, LineCol::new(0, 0));
    assert_eq!(decoration.range.end, LineCol::new(0, 6));
    // No role: the underline takes the token's own colour.
    assert!(decoration.role.is_none());
}

#[test]
fn a_hover_without_ctrl_or_over_the_gutter_underlines_nothing() {
    let Some((mut app, _rect)) = hover_app("definition-hover-negative") else {
        return;
    };

    app.update_definition_hover(&hover(4, 0, KeyModifiers::NONE));
    assert!(
        app.definition_underline_decoration().is_none(),
        "no modifier"
    );

    // The gutter: `pos_at` clamps it onto column 0, which would underline the first
    // word of the line; the render round-trip guard rejects it.
    app.update_definition_hover(&hover(0, 0, KeyModifiers::CONTROL));
    assert!(app.definition_underline_decoration().is_none(), "gutter");

    // Past the end of the line, likewise.
    app.update_definition_hover(&hover(30, 0, KeyModifiers::CONTROL));
    assert!(app.definition_underline_decoration().is_none(), "past eol");
}

#[test]
fn ctrl_hover_underlines_nothing_without_a_running_server() {
    // The underline promises only what a click can deliver, and a click needs a
    // server that is already running.
    let Some((_backend, mut app, _other)) = workspace("definition-hover-no-server") else {
        return;
    };
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 40, 5))];

    app.update_definition_hover(&hover(4, 0, KeyModifiers::CONTROL));
    assert!(app.definition_underline_decoration().is_none());
}

#[test]
fn a_key_press_clears_the_hover_underline() {
    let Some((mut app, _rect)) = hover_app("definition-hover-cleared") else {
        return;
    };
    app.update_definition_hover(&hover(4, 0, KeyModifiers::CONTROL));
    assert!(app.definition_underline_decoration().is_some());

    // Releasing Ctrl without moving the pointer fires no event at all, so typing
    // is one of the ways a stale underline is dropped.
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.definition_underline_decoration().is_none());
}

/// The navigation requests a backend received, as `(id, kind, position)`.
fn location_requests(backend: &RecordingBackend) -> Vec<(RequestId, LocationKind, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::FindLocations { position, kind, .. } => {
                        Some((*id, *kind, *position))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn each_navigation_command_asks_for_its_own_kind_at_the_caret() {
    for (command, kind) in [
        (Command::GoToDeclaration, LocationKind::Declaration),
        (Command::GoToTypeDefinition, LocationKind::TypeDefinition),
        (Command::GoToImplementation, LocationKind::Implementation),
        (Command::FindReferences, LocationKind::References),
    ] {
        let (backend, mut app) = completion_app("target();\n", LineCol::new(0, 2));
        app.dispatch(command);
        let requests = location_requests(&backend);
        assert_eq!(requests.len(), 1, "{command:?}");
        assert_eq!((requests[0].1, requests[0].2), (kind, LineCol::new(0, 2)));
        // Go to Definition keeps its own request.
        assert!(definition_requests(&backend).is_empty());
    }
}

#[test]
fn a_single_reference_still_opens_the_picker() {
    let Some((backend, mut app, other)) = workspace("references-single") else {
        return;
    };
    app.dispatch(Command::FindReferences);
    let id = location_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Locations {
            kind: LocationKind::References,
            locations: vec![Location {
                path: other.clone(),
                range: at(0, 7),
            }],
        },
    );

    // Where the symbol is used is the answer, so one use is listed rather than
    // jumped to; the caret stays put until the user picks it.
    let Some(overlay) = app.overlay.as_ref() else {
        panic!("references should be listed");
    };
    assert_eq!(overlay.title(), "References");
    assert_eq!(overlay.rows(), vec!["other.rs:1  pub fn target() {}"]);
    assert_ne!(app.tabs[app.active].path(), Some(other.as_path()));

    app.overlay_accept();
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 7));
}

#[test]
fn a_single_implementation_is_jumped_to() {
    let Some((backend, mut app, other)) = workspace("implementation-single") else {
        return;
    };
    app.dispatch(Command::GoToImplementation);
    let id = location_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Locations {
            kind: LocationKind::Implementation,
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );

    assert!(app.overlay.is_none());
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 0));
}

#[test]
fn a_reference_preview_reads_the_open_buffer_over_the_file_on_disk() {
    let Some((backend, mut app, other)) = workspace("references-preview") else {
        return;
    };
    let root = app.root.clone();
    // The open `main.rs` holds `target();`; what is on disk is out of date.
    write_file(&root, "main.rs", b"stale on disk\n");
    app.dispatch(Command::FindReferences);
    let id = location_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Locations {
            kind: LocationKind::References,
            locations: vec![
                Location {
                    path: other.clone(),
                    range: at(0, 7),
                },
                Location {
                    path: root.join("main.rs"),
                    range: at(0, 0),
                },
                // Nothing to read: the row is still offered, without a preview.
                Location {
                    path: root.join("gone.rs"),
                    range: at(4, 0),
                },
            ],
        },
    );

    let rows = app.overlay.as_ref().map(Overlay::rows).unwrap_or_default();
    assert_eq!(
        rows,
        vec![
            "other.rs:1  pub fn target() {}",
            "main.rs:1  target();",
            "gone.rs:5"
        ]
    );
}

#[test]
fn an_empty_references_answer_names_what_was_not_found() {
    let Some((backend, mut app, _other)) = workspace("references-empty") else {
        return;
    };
    let root = app.root.clone();
    attach_running_server(&mut app, &root);
    app.dispatch(Command::FindReferences);
    let id = location_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Locations {
            kind: LocationKind::References,
            locations: Vec::new(),
        },
    );

    assert!(app.overlay.is_none());
    assert_eq!(last_message(&app).as_deref(), Some("no references found"));
}

#[test]
fn an_answer_of_another_kind_does_not_settle_the_pending_request() {
    let Some((backend, mut app, other)) = workspace("navigation-kind-mismatch") else {
        return;
    };
    app.dispatch(Command::GoToTypeDefinition);
    let id = location_requests(&backend)[0].0;
    let answer = |kind| SessionEvent::Locations {
        kind,
        locations: vec![Location {
            path: other.clone(),
            range: at(1, 0),
        }],
    };

    app.on_backend_event(Some(id), answer(LocationKind::Declaration));
    assert_ne!(app.tabs[app.active].path(), Some(other.as_path()));

    app.on_backend_event(Some(id), answer(LocationKind::TypeDefinition));
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
}
