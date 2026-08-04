    #[test]
    fn blame_without_a_code_tab_reports_status() {
        let mut app = app();
        app.settings.git.blame = false;
        app.dispatch(Command::ToggleInlineBlame);
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
        assert!(app.settings.git.blame);
        assert_eq!(app.status.as_deref(), Some("inline blame: on"));
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
        let decoration = app.live_blame.as_ref().and_then(LiveBlame::decoration);
        assert!(decoration.is_some());
        let Some(decoration) = decoration else { return };
        assert_eq!(decoration.role, Some(ThemeRole::Muted));
        assert!(matches!(
            decoration.kind,
            DecorationKind::InlineText { ref text, before: false }
                if text == "  Ada just now"
        ));

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
    fn clicking_blame_opens_detail_without_editor_fallthrough() {
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
        assert!(app.handle_blame_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 22,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitLoading { rev, .. } if rev == "abc123"
        ));
    }
