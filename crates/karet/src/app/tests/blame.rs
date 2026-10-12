use super::support::*;
use crate::app::*;

#[test]
fn blame_without_a_code_tab_reports_status() {
    let mut app = app();
    app.settings.git.blame = false;
    app.dispatch(Command::ToggleInlineBlame);
    assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    assert!(app.settings.git.blame);
    assert_eq!(last_message(&app).as_deref(), Some("inline blame: on"));
}

#[test]
fn blame_command_toggles_inline_attribution() {
    let mut app = app();
    app.settings.git.blame = true;
    app.dispatch(Command::ToggleInlineBlame);
    assert!(!app.settings.git.blame);
    app.dispatch(Command::ToggleInlineBlame);
    assert!(app.settings.git.blame);
}

#[test]
fn live_blame_coalesces_motion_and_resolves_the_latest_cursor_line() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", "zero\none\ntwo\n"));
    app.focus = Focus::Editor;
    app.settings.git.blame = true;
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(9));
    }

    app.request_live_blame();
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 0)]);
    app.request_live_blame();
    let first = blame_commands(&backend);
    assert_eq!(first.len(), 1, "cursor motion must not queue obsolete work");
    assert_eq!(first[0].3, 0);

    app.on_backend_event(
        Some(first[0].0),
        SessionEvent::BlameResult {
            doc: DocumentId(9),
            version: 0,
            line: 0,
            attribution: Some(BlameAttribution::Uncommitted),
        },
    );
    let requests = blame_commands(&backend);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].3, 2, "the latest cursor line is requested next");

    app.on_backend_event(
        Some(requests[1].0),
        SessionEvent::BlameResult {
            doc: DocumentId(9),
            version: 0,
            line: 2,
            attribution: Some(BlameAttribution::Uncommitted),
        },
    );
    assert_eq!(app.live_blame.as_ref().map(|blame| blame.line), Some(2));
}

#[test]
fn mouse_cursor_motion_requests_inline_blame_without_typing() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", "zero\none\ntwo\n"));
    app.focus = Focus::Editor;
    app.settings.git.blame = true;
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(9));
    }
    let area = Rect::new(0, 0, 40, 4);
    app.editor_rect = area;
    app.pane_frames = vec![content_frame(&app, area)];

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.tabs[app.active].editor.cursor().line, 1);
    let requests = blame_commands(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].3, 1);
}

#[test]
fn committed_blame_is_compact_muted_text_and_opens_commit_detail() {
    let mut app = app();
    app.live_blame = Some(LiveBlame {
        doc: DocumentId(7),
        version: 3,
        line: 4,
        attribution: Some(BlameAttribution::Commit(karet_core::BlameCommit {
            hash: "1234567890abcdef".to_string(),
            author: "Ada".to_string(),
            author_time: i64::MAX,
        })),
    });
    let text = app.live_blame.as_ref().and_then(LiveBlame::virtual_text);
    assert_eq!(
        text,
        Some(karet_core::VirtualText::end_of_line(
            4,
            "  Ada just now",
            ThemeRole::Muted,
            LiveBlame::BLAME_EOL_PRIORITY,
        ))
    );

    app.dispatch(Command::OpenBlameDetail);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::CommitLoading { rev, .. } if rev == "1234567890abcdef"
    ));
}

#[test]
fn uncommitted_blame_is_informational_only() {
    let mut app = app();
    app.live_blame = Some(LiveBlame {
        doc: DocumentId(7),
        version: 3,
        line: 4,
        attribution: Some(BlameAttribution::Uncommitted),
    });
    assert_eq!(
        app.live_blame.as_ref().and_then(LiveBlame::text).as_deref(),
        Some("  Uncommitted changes")
    );
    let tabs = app.tabs.len();
    app.dispatch(Command::OpenBlameDetail);
    assert_eq!(app.tabs.len(), tabs);
}

#[test]
fn blame_requires_a_double_click_and_consumes_the_first_press() {
    let mut app = app();
    app.live_blame = Some(LiveBlame {
        doc: DocumentId(7),
        version: 3,
        line: 0,
        attribution: Some(BlameAttribution::Commit(karet_core::BlameCommit {
            hash: "abc123".to_string(),
            author: "Ada".to_string(),
            author_time: 0,
        })),
    });
    app.blame_rect = Some(Rect::new(20, 5, 12, 1));
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 22,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };
    let tabs = app.tabs.len();
    assert!(app.handle_blame_mouse(click));
    assert_eq!(app.tabs.len(), tabs, "a single click must not activate");
    assert!(!app.handle_blame_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        ..click
    }));
    assert_eq!(app.tabs.len(), tabs, "dragging must not activate");

    assert!(app.handle_blame_mouse(click));
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::CommitLoading { rev, .. } if rev == "abc123"
    ));
}

#[test]
fn the_blame_hit_region_is_the_painted_label_after_an_inlay_hint() {
    let mut app = app();
    app.push_tab(text_tab("main.rs", "let a\n"));
    app.focus = Focus::Editor;
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(7));
    }
    app.docs.inlay_hints.insert(
        DocumentId(7),
        vec![karet_core::InlayHint::new(
            LineCol::new(0, 5),
            ": i32",
            karet_core::InlayHintKind::Type,
        )],
    );
    app.live_blame = Some(LiveBlame {
        doc: DocumentId(7),
        version: 0,
        line: 0,
        attribution: Some(BlameAttribution::Commit(karet_core::BlameCommit {
            hash: "abc123".to_string(),
            author: "Ada".to_string(),
            author_time: 0,
        })),
    });
    let painted = frame(&mut app, 100, 12);
    let Some(rect) = app.blame_rect else {
        unreachable!("the label is on screen");
    };
    let label: String = (rect.x..rect.right())
        .map(|x| painted[(x, rect.y)].symbol().to_owned())
        .collect();
    assert!(label.starts_with("  Ada"), "the region covers {label:?}");
    // The hint ends right before it, outside the region, which used to start
    // at the end-of-line caret cell -- on the hint's `:`.
    assert_eq!(painted[(rect.x.saturating_sub(1), rect.y)].symbol(), "2");
}

#[test]
fn a_single_click_on_the_blame_label_focuses_its_pane() {
    let mut app = app();
    app.focus = Focus::Sidebar;
    app.blame_rect = Some(Rect::new(20, 5, 12, 1));
    app.pane_frames = vec![content_frame(&app, Rect::new(0, 0, 40, 10))];
    let tabs = app.tabs.len();
    assert!(app.handle_blame_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 22,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(app.focus, Focus::Editor, "the click is not swallowed");
    assert_eq!(app.tabs.len(), tabs, "and still does not activate");
}
