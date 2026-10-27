use super::harness::*;
use super::support::*;
use crate::app::util::tab_at;
use crate::app::*;

#[test]
fn a_selection_slices_the_buffer() {
    use karet_text::TextBuffer;
    let buffer = TextBuffer::from_text("foo bar\nbaz");
    let range = Range {
        start: LineCol::new(0, 4),
        end: LineCol::new(1, 3),
    };
    assert_eq!(buffer.slice_range(range).as_deref(), Some("bar\nbaz"));
}

#[test]
fn active_indentation_uses_the_backend_resolved_document_settings() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", ""));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(9));
    }
    app.docs.settings.insert(
        DocumentId(9),
        DocumentSettings {
            insert_spaces: false,
            indent_size: 6,
            tab_width: 4,
            ..DocumentSettings::default()
        },
    );

    assert_eq!(app.active_indentation(), "\t  ");
}

#[test]
fn markdown_table_formatting_is_one_undoable_document_edit() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("table.md", "|a|long|\n|-|:-:|\n|x|y|\n"));
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { doc, buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        *doc = Some(DocumentId(9));
        editor.set_selection(buffer, LineCol::new(0, 1), LineCol::new(2, 1));
    }

    app.dispatch(Command::FormatMarkdownTables);

    let TabKind::Code { text, .. } = &app.tabs[active].kind else {
        return;
    };
    assert_eq!(text, "| a   | long |\n| --- | :--: |\n| x   |  y   |\n");
    assert_eq!(
        app.tabs[active].editor.selection_range(),
        Some(Range {
            start: LineCol::new(0, 1),
            end: LineCol::new(2, 1),
        })
    );
    let apply_count = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| {
                    matches!(
                        command,
                        SessionCommand::Document(DocumentCommand::ApplyChange { .. })
                    )
                })
                .count()
        })
        .unwrap_or_default();
    assert_eq!(apply_count, 1);
}

#[test]
fn copy_reports_status() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "hello world"));
    app.keyboard.focus_editor();
    app.dispatch(Command::SelectRight);
    app.dispatch(Command::Copy);
    assert_eq!(last_message(&app).as_deref(), Some("copied selection"));
}

#[test]
fn right_clicking_an_error_toast_copies_it_without_dismissing() {
    let mut app = app();
    app.notify(Report::Failure, NotificationKind::Lsp, "server failed");
    let id = app.notifications.active()[0].id;
    app.last_frame.toast_hits = vec![ToastHit {
        rect: Rect::new(5, 3, 20, 4),
        id,
    }];

    assert!(app.handle_toast_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    }));
    assert_eq!(last_message(&app).as_deref(), Some("copied error"));
    // The copy confirms with a card of its own, so the error is no longer newest
    // — but it is still there, which is what "without dismissing" means.
    assert!(
        app.notifications.active().iter().any(|note| note.id == id),
        "the error survives the copy"
    );
}

#[test]
fn copied_notification_text_includes_the_body() {
    let notification = Notification {
        id: NotificationId(7),
        severity: Severity::Error,
        kind: NotificationKind::Lsp,
        title: "server failed".to_owned(),
        body: Some("permission denied".to_owned()),
        tag: None,
        timeout: None,
        dismissable: true,
    };

    assert_eq!(
        mouse::chrome::notification_clipboard_text(&notification),
        "server failed\npermission denied"
    );
}

/// Keyboard motion, selection and multi-caret rows (`docs/editing-behavior.md`).
#[test]
fn keyboard_caret_and_selection_conformance() {
    use KeyCode::*;
    let shift = KeyModifiers::SHIFT;
    let ctrl = KeyModifiers::CONTROL;
    check(vec![
        case(
            "End moves the caret without selecting",
            "|hello",
            vec![key(End)],
            "hello|",
        ),
        case(
            "Ctrl+Left moves to the start of the previous word",
            "foo bar| baz",
            vec![Step::Key(Left, ctrl)],
            "foo |bar baz",
        ),
        case(
            "Ctrl+Right moves to the end of the next word",
            "fo|o bar baz",
            vec![Step::Key(Right, ctrl), Step::Key(Right, ctrl)],
            "foo bar| baz",
        ),
        case(
            "Alt+Right is the same word motion as Ctrl+Right",
            "fo|o bar baz",
            vec![Step::Key(Right, KeyModifiers::ALT)],
            "foo| bar baz",
        ),
        case(
            "Shift+End selects from the caret to the line end",
            "|hello world\nsecond",
            vec![Step::Key(End, shift)],
            "[hello world|]\nsecond",
        ),
        case(
            "Ctrl+A selects the whole document, head at its end",
            "hello |world\nsecond",
            vec![Step::Key(Char('a'), ctrl)],
            "[hello world\nsecond|]",
        ),
        case(
            "Ctrl+A does not scroll to the end of the document",
            "|0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19",
            vec![Step::Key(Char('a'), ctrl)],
            "[0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19|]",
        )
        .scrolled(0),
        case(
            "Shift+Right extends one character at a time",
            "|hello",
            vec![Step::Key(Right, shift), Step::Key(Right, shift)],
            "[he|]llo",
        ),
        case(
            "Shift+Left extends backward, the head leading",
            "he|llo",
            vec![Step::Key(Left, shift), Step::Key(Left, shift)],
            "[|he]llo",
        ),
        case(
            "a plain arrow clears the selection",
            "he|llo",
            vec![Step::Key(Left, shift), Step::Key(Left, shift), key(Left)],
            "|hello",
        ),
        case(
            "Ctrl+Alt+Down adds a caret below, which becomes primary",
            "|ab\ncd",
            vec![Step::Key(Down, ctrl | KeyModifiers::ALT)],
            "|ab\n#cd",
        ),
    ]);
}

#[test]
fn esc_collapses_extra_carets_and_keeps_editor_focus() {
    let mut rig = Rig::new("|ab\ncd");
    rig.step(Step::Cmd(Command::AddCursorBelow));
    assert!(rig.tab().editor.has_multiple_cursors());
    rig.step(key(KeyCode::Esc));
    assert!(!rig.tab().editor.has_multiple_cursors());
    assert_eq!(rig.app.keyboard.focus(), Focus::Editor);

    // With one caret Esc is a no-op that never leaves the editor.
    rig.step(key(KeyCode::Esc));
    assert_eq!(rig.app.keyboard.focus(), Focus::Editor);
}

#[test]
fn markdown_links_require_a_modifier_and_open_workspace_files() {
    let root = test_dir("markdown-links");
    write_file(&root, "README.md", b"[guide](docs/guide.md)");
    write_file(&root, "docs/guide.md", b"guide");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.focused_frame_mut().markdown_link_hits = vec![MarkdownLinkHit {
        rect: Rect::new(4, 3, 5, 1),
        target: "docs/guide.md".to_string(),
    }];
    let click = |modifiers| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 3,
        modifiers,
    };

    assert!(!app.handle_markdown_link_mouse(click(KeyModifiers::NONE)));
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("README.md").as_path())
    );
    assert!(app.handle_markdown_link_mouse(click(KeyModifiers::CONTROL)));
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("docs/guide.md").as_path())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn markdown_file_links_outside_the_workspace_require_confirmation() {
    let parent = test_dir("markdown-link-boundary");
    let root = parent.join("workspace");
    write_file(&root, "README.md", b"[outside](../outside.md)");
    write_file(&parent, "outside.md", b"outside");
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.focused_frame_mut().markdown_link_hits = vec![MarkdownLinkHit {
        rect: Rect::new(1, 1, 7, 1),
        target: "../outside.md".to_string(),
    }];

    assert!(app.handle_markdown_link_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 1,
        modifiers: KeyModifiers::SUPER,
    }));
    assert!(app.confirm.is_some(), "the escape raised a confirmation");
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("README.md").as_path()),
        "still on the file the link came from"
    );

    // The safe answer is selected, so Enter alone stays inside the workspace.
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        app.tabs[app.active].path(),
        Some(root.join("README.md").as_path())
    );

    assert!(app.handle_markdown_link_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 2,
        row: 1,
        modifiers: KeyModifiers::SUPER,
    }));
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        app.tabs[app.active].path(),
        Some(parent.join("outside.md").as_path())
    );
    let _ = std::fs::remove_dir_all(parent);
}

/// Click, multi-click, modifier-click and wheel rows (`docs/editing-behavior.md`).
#[test]
fn mouse_caret_and_selection_conformance() {
    let alt = KeyModifiers::ALT;
    let shift = KeyModifiers::SHIFT;
    let lines: &'static str =
        "|0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n";
    check(vec![
        case(
            "a click places the caret",
            "|foo bar baz",
            vec![click(text(0, 5))],
            "foo b|ar baz",
        ),
        case(
            "a click past the end of a line lands at the line end",
            "|foo\nbar",
            vec![click(At::Offset(0, 3, 4, 0))],
            "foo|\nbar",
        ),
        case(
            "a click below the last line lands at the end of the document",
            "|foo\nbar",
            vec![click(At::Offset(1, 1, 0, 2))],
            "foo\nbar|",
        ),
        case(
            "a click on the right half of a wide character lands after it",
            "|日本",
            vec![click(At::Offset(0, 0, 1, 0))],
            "日|本",
        ),
        case(
            "a click on the last cell of a tab lands after it",
            "|\tx",
            vec![click(At::Offset(0, 1, -1, 0))],
            "\t|x",
        ),
        case(
            "a click on the first cell of a tab lands before it",
            "\t|x",
            vec![click(text(0, 0))],
            "|\tx",
        ),
        case(
            "a click drops every secondary caret",
            "|foo\n#bar",
            vec![click(text(0, 2))],
            "fo|o\nbar",
        ),
        case(
            "Alt+click adds a caret, which becomes primary",
            "|foo bar baz",
            vec![click(text(0, 0)), Step::Click(text(0, 5), alt)],
            "|foo b#ar baz",
        ),
        case(
            "a double click selects the word under the pointer",
            "|foo bar baz",
            vec![click(text(0, 5)), click(text(0, 5))],
            "foo [bar|] baz",
        ),
        case(
            "Shift+click extends forward from the caret",
            "|foo bar baz",
            vec![click(text(0, 0)), Step::Click(text(0, 5), shift)],
            "[foo b|]ar baz",
        ),
        case(
            "Shift+click extends backward when it precedes the caret",
            "|foo bar baz",
            vec![click(text(0, 10)), Step::Click(text(0, 3), shift)],
            "foo[| bar ba]z",
        ),
        case(
            "the wheel scrolls without moving the caret",
            lines,
            vec![Step::WheelDown(text(0, 0), KeyModifiers::NONE)],
            "|0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n",
        )
        .scrolled(3),
        case(
            "a wheel notch up scrolls back by the same step",
            lines,
            vec![
                Step::WheelDown(text(0, 0), KeyModifiers::NONE),
                Step::WheelDown(text(3, 0), KeyModifiers::NONE),
                Step::WheelUp(text(6, 0), KeyModifiers::NONE),
            ],
            "|0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n",
        )
        .scrolled(3)
        .sized(40, 10),
    ]);
}

#[test]
fn a_click_in_another_split_moves_focus_and_only_that_panes_caret() {
    let mut rig = Rig::new("|alpha\nbeta");
    rig.step(Step::Cmd(Command::SplitRight));
    assert_eq!(rig.focused_index(), Some(1), "the new split is focused");

    rig.step(click(At::Pane(0, 1, 2)));

    assert_eq!(rig.focused_index(), Some(0));
    assert_eq!(rig.marked_in(0), "alpha\nbe|ta");
    assert_eq!(
        rig.marked_in(1),
        "|alpha\nbeta",
        "the other view keeps its caret"
    );
}

#[test]
fn tab_at_maps_columns_to_tabs_and_close() {
    let hits = vec![
        TabHit {
            start: 0,
            end: 10,
            close: 8,
        },
        TabHit {
            start: 10,
            end: 20,
            close: 18,
        },
    ];
    assert_eq!(tab_at(&hits, 3), Some((0, false)));
    assert_eq!(tab_at(&hits, 8), Some((0, true)));
    assert_eq!(tab_at(&hits, 12), Some((1, false)));
    assert_eq!(tab_at(&hits, 18), Some((1, true)));
    assert_eq!(tab_at(&hits, 25), None);
}

#[test]
fn status_segment_click_dispatches_its_command() {
    let mut app = app();
    app.last_frame.status_rect = Rect {
        x: 0,
        y: 9,
        width: 80,
        height: 1,
    };
    app.last_frame.status_hits = vec![
        (0, 9, Command::ToggleFocus),
        (12, 19, Command::OpenQuickOpen),
    ];
    assert_eq!(app.status_command_at(3), Some(Command::ToggleFocus));
    assert_eq!(app.status_command_at(15), Some(Command::OpenQuickOpen));
    assert_eq!(app.status_command_at(40), None);
    // Clicking the focus segment toggles focus.
    let before = app.keyboard.focus();
    app.handle_status_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 9,
        modifiers: KeyModifiers::NONE,
    });
    assert_ne!(app.keyboard.focus(), before);
}

#[test]
fn sidebar_header_click_switches_panel() {
    let mut app = app();
    app.last_frame.sidebar_rect = Rect {
        x: 0,
        y: 1,
        width: 30,
        height: 10,
    };
    app.last_frame.panel_hits = vec![
        (23, 25, SidebarPanel::Explorer),
        (25, 27, SidebarPanel::Search),
        (27, 29, SidebarPanel::SourceControl),
    ];
    app.handle_sidebar_click(25, 1, KeyModifiers::NONE); // header row, the "2" cell
    assert_eq!(app.sidebar_panel, SidebarPanel::Search);
}

#[test]
fn sidebar_click_selects_and_opens_scm_change() {
    let mut app = app(); // staged a.rs, working b.rs
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.last_frame.sidebar_rect = Rect {
        x: 0,
        y: 1,
        width: 30,
        height: 10,
    };
    app.last_frame.sidebar_content_rect = Rect {
        x: 0,
        y: 2,
        width: 30,
        height: 8,
    };
    // Clicks hit-test against the changes region rect.
    app.scm_ui.changes_rect = Rect {
        x: 0,
        y: 2,
        width: 30,
        height: 8,
    };
    app.scm_ui.offset = 0;
    // Display rows: 0 header, 1 a.rs(0), 2 header, 3 b.rs(1).
    app.scm_ui.row_map = vec![None, Some(0), None, Some(1)];
    app.handle_sidebar_click(2, 5, KeyModifiers::NONE); // content row 3 -> change index 1
    assert_eq!(app.scm.selection.cursor(), 1);
    assert!(app.active_is_diff());

    // Ctrl-click a second row adds it to the selection without opening a diff.
    app.handle_sidebar_click(2, 3, KeyModifiers::CONTROL); // content row 1 -> index 0
    assert_eq!(app.scm.selection.selected_indices(), vec![0, 1]);
}

#[test]
fn dragging_moves_the_active_tab() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.push_tab(code_tab("b.rs"));
    app.push_tab(code_tab("c.rs"));
    app.last_frame.panes = vec![PaneFrame {
        pane: app.focus_pane(),
        tabstrip_rect: Rect::default(),
        tab_hits: vec![
            TabHit {
                start: 0,
                end: 8,
                close: 6,
            },
            TabHit {
                start: 8,
                end: 16,
                close: 14,
            },
            TabHit {
                start: 16,
                end: 24,
                close: 22,
            },
        ],
        action_hits: Vec::new(),
        breadcrumb_rect: Rect::default(),
        breadcrumb_hits: Vec::new(),
        lsp_badge_hit: None,
        content_rect: Rect::default(),
        editor_rect: Rect::default(),
        markdown_preview_rect: Rect::default(),
        commit_file_hits: Vec::new(),
        commit_dir_hits: Vec::new(),
        commit_rail_rect: Rect::default(),
        commit_collapse_hits: Vec::new(),
        select_regions: Vec::new(),
        ..PaneFrame::empty(app.focus_pane())
    }];
    app.active = 0;
    app.tab_drag = Some(TabDrag {
        from_pane: app.focus_pane(),
        hover: None,
    });
    app.drag_tab_to(20); // over the third tab
    let titles: Vec<_> = app.tabs.iter().map(|t| t.title.clone()).collect();
    assert_eq!(titles, vec!["b.rs", "c.rs", "a.rs"]);
    assert_eq!(app.active, 2);
}

#[test]
fn pane_action_click_wins_over_the_underlying_tab_hit() {
    let mut app = app();
    app.push_tab(text_tab("README.md", "# Title\n"));
    app.last_frame.panes = vec![PaneFrame {
        pane: app.focus_pane(),
        tabstrip_rect: Rect::new(0, 0, 30, 1),
        tab_hits: vec![TabHit {
            start: 0,
            end: 30,
            close: 28,
        }],
        action_hits: vec![(24, 27, Command::MarkdownPreviewSide)],
        breadcrumb_rect: Rect::default(),
        breadcrumb_hits: Vec::new(),
        lsp_badge_hit: None,
        content_rect: Rect::default(),
        editor_rect: Rect::default(),
        markdown_preview_rect: Rect::default(),
        commit_file_hits: Vec::new(),
        commit_dir_hits: Vec::new(),
        commit_rail_rect: Rect::default(),
        commit_collapse_hits: Vec::new(),
        select_regions: Vec::new(),
        ..PaneFrame::empty(app.focus_pane())
    }];

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 25,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.pane_action_hover, Some((25, 0)));
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 25,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.tabs[app.active].markdown_preview.is_some());
    assert!(app.tab_drag.is_none());
}

#[test]
fn drop_tab_on_right_edge_creates_a_second_pane() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.push_tab(code_tab("b.rs"));
    let from = app.focus_pane();
    let dragged = app.tabs[app.active].title.clone();
    app.drop_tab_on(from, DropZone::Right);
    assert_eq!(app.layout.pane_count(), 2);
    // Focus moved to the new pane, holding the dragged tab.
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].title, dragged);
    // The origin pane survives with its remaining tab(s), in storage.
    assert!(app.stored.contains_key(&from));
}

#[test]
fn dropping_the_only_tab_on_an_edge_keeps_one_pane() {
    let mut app = app();
    app.push_tab(code_tab("only.rs"));
    let from = app.focus_pane();
    // The sole tab can't leave an empty origin pane behind, so the split
    // collapses back to a single pane holding it.
    app.drop_tab_on(from, DropZone::Bottom);
    assert_eq!(app.layout.pane_count(), 1);
    assert_eq!(app.tabs[0].title, "only.rs");
}

#[test]
fn keyboard_split_opens_a_second_view_and_focus_cycles() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    let from = app.focus_pane();
    app.dispatch(Command::SplitRight);
    assert_eq!(app.layout.pane_count(), 2);
    let new_pane = app.focus_pane();
    assert_ne!(new_pane, from);
    // The new pane holds a duplicate view of the active document.
    assert_eq!(app.tabs[0].title, "a.rs");
    // Focus cycles to the origin pane and back.
    app.dispatch(Command::FocusPrevPane);
    assert_eq!(app.focus_pane(), from);
    app.dispatch(Command::FocusNextPane);
    assert_eq!(app.focus_pane(), new_pane);
}

#[test]
fn keyboard_resize_grows_the_focused_pane_toward_the_requested_edge() {
    let mut app = app();
    app.last_frame.main_rect = Rect::new(0, 0, 80, 24);
    let left = app.layout.root_pane();
    let right = app.layout.split(left, SplitDir::Right);
    app.layout.set_focus(right);

    app.dispatch(Command::ResizePaneLeft);

    assert_eq!(
        app.layout
            .pane_rect(left, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(38)
    );
    assert_eq!(
        app.layout
            .pane_rect(right, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(42)
    );
}

#[test]
fn mouse_drag_resizes_a_pane_divider_and_releases_cleanly() {
    let mut app = app();
    app.last_frame.main_rect = Rect::new(0, 0, 80, 24);
    let left = app.layout.root_pane();
    let right = app.layout.split(left, SplitDir::Right);
    app.last_frame.pane_dividers = app.layout.dividers(app.last_frame.main_rect);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 39,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.pane_resize.is_some());
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 48,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 100,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.layout
            .pane_rect(left, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(70)
    );
    // Remaining outside the minimum-size boundary must not make it jump back.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 90,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.layout
            .pane_rect(left, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(70)
    );
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 60,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 60,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(
        app.layout
            .pane_rect(left, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(61)
    );
    assert_eq!(
        app.layout
            .pane_rect(right, app.last_frame.main_rect)
            .map(|rect| rect.width),
        Some(19)
    );
    assert!(app.pane_resize.is_none());
}

#[test]
fn drop_tab_center_on_self_is_a_noop() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    let from = app.focus_pane();
    app.drop_tab_on(from, DropZone::Center);
    assert_eq!(app.layout.pane_count(), 1);
}

#[test]
fn close_other_tabs_keeps_the_active_one() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.push_tab(code_tab("b.rs"));
    app.push_tab(code_tab("c.rs"));
    app.active = 1;
    app.close_other_tabs();
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].title, "b.rs");
    assert_eq!(app.active, 0);
}

#[test]
fn closing_remembers_path_and_reopen_restores_it() {
    let dir = std::env::temp_dir().join(format!("karet-reopen-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("a.rs");
    let _ = std::fs::write(&file, "fn main() {}\n");

    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.push_tab(workspace::open_file(&file));
    app.push_tab(code_tab("scratch"));
    app.active = 0;
    app.close_tab_at(0);
    assert_eq!(app.closed.last(), Some(&file));
    app.reopen_closed_tab();
    assert!(app.tabs.iter().any(|t| t.path() == Some(file.as_path())));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_last_tab_collapses_its_tile_when_another_remains() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.dispatch(Command::SplitRight);
    assert_eq!(app.layout.pane_count(), 2);

    app.close_tab_at(0);

    assert_eq!(app.layout.pane_count(), 1);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].title, "a.rs");
    assert_eq!(app.keyboard.focus(), Focus::Editor);
}

#[test]
fn closing_a_welcome_tab_collapses_its_tile_when_another_remains() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));
    app.dispatch(Command::SplitRight);
    app.tabs = vec![Tab::welcome()];
    app.active = 0;

    app.close_tab_at(0);

    assert_eq!(app.layout.pane_count(), 1);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].title, "a.rs");
}

#[test]
fn closing_the_only_panes_last_tab_keeps_one_welcome_tab() {
    let mut app = app();
    app.push_tab(code_tab("a.rs"));

    app.close_tab_at(0);

    assert_eq!(app.layout.pane_count(), 1);
    assert_eq!(app.tabs.len(), 1);
    assert!(matches!(app.tabs[0].kind, TabKind::Welcome));
}
