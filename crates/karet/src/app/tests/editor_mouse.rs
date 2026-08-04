    #[test]
    fn selection_text_slices_the_source() {
        use karet_text::TextBuffer;
        let src = "foo bar\nbaz";
        let buffer = TextBuffer::from_text(src);
        let range = Range {
            start: LineCol::new(0, 4),
            end: LineCol::new(1, 3),
        };
        assert_eq!(
            selection_text(&buffer, src, range).as_deref(),
            Some("bar\nbaz")
        );
    }

    #[test]
    fn active_indentation_uses_the_backend_resolved_document_settings() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", ""));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(9));
        }
        app.document_settings.insert(
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
        assert_eq!(
            text,
            "| a   | long |\n| --- | :--: |\n| x   |  y   |\n"
        );
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
                        matches!(command, SessionCommand::ApplyChange { .. })
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
        app.focus = Focus::Editor;
        app.dispatch(Command::SelectRight);
        app.dispatch(Command::Copy);
        assert_eq!(app.status.as_deref(), Some("copied selection"));
    }

    #[test]
    fn select_line_end_then_select_all_dispatch_in_editor() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello world\nsecond"));
        app.focus = Focus::Editor;
        // Shift+End selects from the caret to the end of the line.
        app.dispatch(Command::SelectLineEnd);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 11),
            })
        );
        // Ctrl+A selects the whole buffer.
        app.dispatch(Command::EditorSelectAll);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(1, 6),
            })
        );
    }

    #[test]
    fn caret_line_end_moves_without_selecting() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello"));
        app.focus = Focus::Editor;
        app.dispatch(Command::CaretLineEnd);
        assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 5));
        assert_eq!(app.tabs[app.active].editor.selection_range(), None);
    }

    #[test]
    fn add_cursor_below_then_esc_collapses() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "ab\ncd"));
        app.focus = Focus::Editor;
        app.dispatch(Command::AddCursorBelow);
        assert!(app.tabs[app.active].editor.has_multiple_cursors());
        // Esc with several carets collapses to the primary, keeping editor focus.
        app.dispatch(Command::CollapseCarets);
        assert!(!app.tabs[app.active].editor.has_multiple_cursors());
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn esc_with_a_single_caret_keeps_editor_focus() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "ab"));
        app.focus = Focus::Editor;
        app.dispatch(Command::CollapseCarets);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn markdown_links_require_a_modifier_and_open_workspace_files() {
        let root = test_dir("markdown-links");
        write_file(&root, "README.md", b"[guide](docs/guide.md)");
        write_file(&root, "docs/guide.md", b"guide");
        let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&root.join("README.md"));
        app.markdown_link_hits = vec![MarkdownLinkHit {
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
        assert_eq!(app.tabs[app.active].path(), Some(root.join("README.md").as_path()));
        assert!(app.handle_markdown_link_mouse(click(KeyModifiers::CONTROL)));
        assert_eq!(
            app.tabs[app.active].path(),
            Some(root.join("docs/guide.md").as_path())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn markdown_file_links_outside_the_workspace_require_typed_confirmation() {
        let parent = test_dir("markdown-link-boundary");
        let root = parent.join("workspace");
        write_file(&root, "README.md", b"[outside](../outside.md)");
        write_file(&parent, "outside.md", b"outside");
        let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&root.join("README.md"));
        app.markdown_link_hits = vec![MarkdownLinkHit {
            rect: Rect::new(1, 1, 7, 1),
            target: "../outside.md".to_string(),
        }];

        assert!(app.handle_markdown_link_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::SUPER,
        }));
        assert!(app.overlay.is_some());
        assert_eq!(app.tabs[app.active].path(), Some(root.join("README.md").as_path()));
        if let Some(overlay) = app.overlay.as_mut() {
            overlay.push_str("open");
        }
        app.overlay_accept();
        assert_eq!(
            app.tabs[app.active].path(),
            Some(parent.join("outside.md").as_path())
        );
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn alt_click_adds_a_second_caret() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        let click = |col, mods| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: mods,
        };
        app.handle_editor_click(click(3, KeyModifiers::NONE));
        app.handle_editor_click(click(8, KeyModifiers::ALT));
        assert!(app.tabs[app.active].editor.has_multiple_cursors());
    }

    #[test]
    fn double_click_selects_the_word() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        // Two quick clicks over the 'a' of "bar" (buffer col 5 -> screen col 8).
        let click = |col| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_editor_click(click(8));
        app.handle_editor_click(click(8));
        let sel = app.tabs[app.active].editor.selection_range();
        assert_eq!(
            sel,
            Some(Range {
                start: LineCol::new(0, 4),
                end: LineCol::new(0, 7),
            })
        );
    }

    #[test]
    fn shift_click_extends_selection_to_the_click() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        let click = |col, shift| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: if shift {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            },
        };
        // Place the caret at buffer col 0 (screen col 3 past the gutter), then
        // Shift+click at buffer col 5 (screen col 8) to extend the selection.
        app.handle_editor_click(click(3, false));
        app.handle_editor_click(click(8, true));
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 5),
            })
        );
    }

    #[test]
    fn shift_arrow_extends_then_plain_arrow_clears() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello"));
        app.focus = Focus::Editor;
        app.dispatch(Command::SelectRight);
        app.dispatch(Command::SelectRight);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 2),
            })
        );
        app.dispatch(Command::CaretLeft);
        assert_eq!(app.tabs[app.active].editor.selection_range(), None);
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
        app.status_rect = Rect {
            x: 0,
            y: 9,
            width: 80,
            height: 1,
        };
        app.status_hits = vec![
            (0, 9, Command::ToggleFocus),
            (12, 19, Command::OpenQuickOpen),
        ];
        assert_eq!(app.status_command_at(3), Some(Command::ToggleFocus));
        assert_eq!(app.status_command_at(15), Some(Command::OpenQuickOpen));
        assert_eq!(app.status_command_at(40), None);
        // Clicking the focus segment toggles focus.
        let before = app.focus;
        app.handle_status_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 9,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(app.focus, before);
    }

    #[test]
    fn sidebar_header_click_switches_panel() {
        let mut app = app();
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.panel_hits = vec![
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
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 30,
            height: 8,
        };
        // Clicks hit-test against the changes region rect.
        app.scm_changes_rect = Rect {
            x: 0,
            y: 2,
            width: 30,
            height: 8,
        };
        app.scm_offset = 0;
        // Display rows: 0 header, 1 a.rs(0), 2 header, 3 b.rs(1).
        app.scm_row_map = vec![None, Some(0), None, Some(1)];
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
        app.pane_frames = vec![PaneFrame {
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
            content_rect: Rect::default(),
            commit_file_hits: Vec::new(),
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
        app.pane_frames = vec![PaneFrame {
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
            content_rect: Rect::default(),
            commit_file_hits: Vec::new(),
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
        app.main_rect = Rect::new(0, 0, 80, 24);
        let left = app.layout.root_pane();
        let right = app.layout.split(left, SplitDir::Right);
        app.layout.set_focus(right);

        app.dispatch(Command::ResizePaneLeft);

        assert_eq!(
            app.layout.pane_rect(left, app.main_rect).map(|rect| rect.width),
            Some(38)
        );
        assert_eq!(
            app.layout.pane_rect(right, app.main_rect).map(|rect| rect.width),
            Some(42)
        );
    }

    #[test]
    fn mouse_drag_resizes_a_pane_divider_and_releases_cleanly() {
        let mut app = app();
        app.main_rect = Rect::new(0, 0, 80, 24);
        let left = app.layout.root_pane();
        let right = app.layout.split(left, SplitDir::Right);
        app.pane_dividers = app.layout.dividers(app.main_rect);

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
            app.layout.pane_rect(left, app.main_rect).map(|rect| rect.width),
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
            app.layout.pane_rect(left, app.main_rect).map(|rect| rect.width),
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
            app.layout.pane_rect(left, app.main_rect).map(|rect| rect.width),
            Some(61)
        );
        assert_eq!(
            app.layout.pane_rect(right, app.main_rect).map(|rect| rect.width),
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
        assert_eq!(app.focus, Focus::Editor);
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
