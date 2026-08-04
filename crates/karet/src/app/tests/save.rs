    #[test]
    fn pending_save_drives_the_animation_tick() {
        let mut app = app();
        assert!(app.next_wake().is_none());
        app.pending_saves.insert(
            RequestId(1),
            PendingSave {
                doc: DocumentId(1),
            },
        );
        assert_eq!(app.next_wake(), Some(Duration::from_millis(100)));
    }

    #[test]
    fn save_completion_clears_the_spinner() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.tabs[app.active].saving_since = Some(Instant::now());
        app.pending_saves.insert(
            RequestId(5),
            PendingSave {
                doc: DocumentId(2),
            },
        );
        app.on_backend_event(
            Some(RequestId(5)),
            SessionEvent::Saved { doc: DocumentId(2) },
        );
        assert!(app.tabs[app.active].saving_since.is_none());
        assert!(app.pending_saves.is_empty());
    }

    #[test]
    fn duplicate_save_command_is_debounced_while_in_flight() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }

        app.save_active();
        app.save_active();

        let sent_saves = backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| matches!(command, SessionCommand::Save { .. }))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(sent_saves, 1, "only one save may be in flight per document");
        assert_eq!(
            app.status.as_deref(),
            Some("save already in progress"),
            "the second shortcut is ignored because the first save is still pending"
        );
    }

    #[test]
    fn after_delay_auto_save_debounces_to_the_newest_edit() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.settings.files.auto_save = AutoSave::AfterDelay;
        app.settings.files.auto_save_delay = 100;
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        let start = Instant::now();

        app.schedule_auto_save(DocumentId(2), 1, start);
        app.schedule_auto_save(DocumentId(2), 1, start + Duration::from_millis(50));
        app.fire_auto_save(start + Duration::from_millis(100));
        assert_eq!(saved_docs(&backend), [DocumentId(2)]);

        let request = app.pending_saves.keys().next().copied();
        if let Some(request) = request {
            app.on_backend_event(
                Some(request),
                SessionEvent::Saved { doc: DocumentId(2) },
            );
        }
        app.schedule_auto_save(DocumentId(2), 2, start + Duration::from_millis(110));
        app.schedule_auto_save(DocumentId(2), 3, start + Duration::from_millis(150));
        app.fire_auto_save(start + Duration::from_millis(249));
        assert_eq!(saved_docs(&backend).len(), 1);
        app.fire_auto_save(start + Duration::from_millis(250));
        assert_eq!(saved_docs(&backend).len(), 2);
    }

    #[test]
    fn focus_change_auto_save_only_fires_when_the_edited_document_loses_focus() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.settings.files.auto_save = AutoSave::OnFocusChange;
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.schedule_auto_save(DocumentId(2), 1, Instant::now());

        app.auto_save_context_changed(Some(DocumentId(2)));
        assert!(saved_docs(&backend).is_empty(), "the same editor keeps focus");
        app.focus = Focus::Sidebar;
        app.auto_save_context_changed(Some(DocumentId(2)));
        assert_eq!(saved_docs(&backend), [DocumentId(2)]);
    }

    #[test]
    fn a_late_dirty_snapshot_saves_when_its_document_already_lost_focus() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.settings.files.auto_save = AutoSave::OnFocusChange;
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.focus = Focus::Sidebar;

        app.schedule_auto_save(DocumentId(2), 1, Instant::now());

        assert_eq!(saved_docs(&backend), [DocumentId(2)]);
    }

    #[test]
    fn an_edit_during_an_auto_save_keeps_its_own_follow_up_deadline() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.settings.files.auto_save = AutoSave::AfterDelay;
        app.settings.files.auto_save_delay = 10;
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        let start = Instant::now();
        app.schedule_auto_save(DocumentId(2), 1, start);
        app.fire_auto_save(start + Duration::from_millis(10));
        let request = app.pending_saves.keys().next().copied();

        app.schedule_auto_save(DocumentId(2), 2, start + Duration::from_millis(11));
        app.fire_auto_save(start + Duration::from_millis(21));
        assert_eq!(saved_docs(&backend).len(), 1, "only one save may be in flight");
        if let Some(request) = request {
            app.on_backend_event(
                Some(request),
                SessionEvent::Saved { doc: DocumentId(2) },
            );
        }
        assert!(app.auto_save_pending.contains_key(&DocumentId(2)));
        app.fire_auto_save(start + Duration::from_millis(21));
        assert_eq!(saved_docs(&backend).len(), 2);
    }

    #[test]
    fn an_auto_save_conflict_keeps_the_buffer_dirty_and_warns() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);
        app.settings.files.auto_save = AutoSave::AfterDelay;
        app.settings.files.auto_save_delay = 1;
        app.push_tab(text_tab("t.rs", "local"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.tabs[app.active].dirty = true;
        let start = Instant::now();
        app.schedule_auto_save(DocumentId(2), 1, start);
        app.fire_auto_save(start + Duration::from_millis(1));
        let request = app.pending_saves.keys().next().copied();

        if let Some(request) = request {
            app.on_backend_event(
                Some(request),
                SessionEvent::ExternalConflict { doc: DocumentId(2) },
            );
        }

        assert!(app.tabs[app.active].dirty);
        assert!(app.notifications.active().iter().any(|notification| {
            notification.title.contains("file changed on disk")
                && notification.severity == Severity::Warning
        }));
    }

    #[test]
    fn save_active_marks_every_view_of_the_document_as_saving() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.split_focused(SplitDir::Right);

        app.save_active();

        assert!(app.tabs[app.active].saving_since.is_some());
        let stored_saving = app
            .stored
            .values()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.saving_since.is_some());
        assert!(
            stored_saving,
            "background split view should show save progress"
        );
    }

    #[test]
    fn quit_save_all_conflict_keeps_the_app_open() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.tabs[app.active].dirty = true;
        app.saving_close = Some(CloseRequest::Quit);
        app.pending_saves.insert(
            RequestId(5),
            PendingSave {
                doc: DocumentId(2),
            },
        );

        app.on_backend_event(
            Some(RequestId(5)),
            SessionEvent::ExternalConflict { doc: DocumentId(2) },
        );

        assert!(!app.should_quit);
        assert!(app.saving_close.is_none());
        assert!(app.tabs[app.active].dirty);
    }

    #[test]
    fn saved_event_clears_the_dirty_flag() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(1));
        }
        app.tabs[app.active].dirty = true;
        app.on_backend_event(None, SessionEvent::Saved { doc: DocumentId(1) });
        assert!(!app.tabs[app.active].dirty);
    }

    #[test]
    fn scm_log_pages_replace_then_append() {
        fn commit(hash: &str, summary: &str) -> Commit {
            Commit {
                hash: hash.to_string(),
                short_hash: hash.chars().take(7).collect(),
                summary: summary.to_string(),
                author: "a".to_string(),
                time: 0,
                parents: Vec::new(),
            }
        }
        let mut app = app();
        // The first page replaces and clears the in-flight flag.
        app.scm.log_loading = true;
        app.apply_vcs_log(0, vec![commit("aaaaaaa", "first")], true);
        assert_eq!(app.scm.log.len(), 1);
        assert!(app.scm.log_has_more);
        assert!(!app.scm.log_loading);
        // A page at the right offset appends.
        app.apply_vcs_log(1, vec![commit("bbbbbbb", "second")], false);
        assert_eq!(app.scm.log.len(), 2);
        assert!(!app.scm.log_has_more);
        // A page at the wrong offset is ignored (no duplicate/torn appends).
        app.apply_vcs_log(5, vec![commit("ccccccc", "stale")], false);
        assert_eq!(app.scm.log.len(), 2);
    }

    #[test]
    fn hover_maps_to_explorer_and_scm_rows() {
        let mut app = app();
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 10,
        };
        // Explorer: hover at y=4 with offset 0 → absolute row 2.
        app.hover = Some((5, 4));
        assert_eq!(app.hovered_explorer_row(), Some(2));
        // Above the content area → no hovered row.
        app.hover = Some((5, 1));
        assert_eq!(app.hovered_explorer_row(), None);

        // Source control: display 0 is a section header, 1 and 2 are changes. Hover
        // maps against the changes region rect.
        app.scm_changes_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 10,
        };
        app.scm_offset = 0;
        app.scm_row_map = vec![None, Some(0), Some(1)];
        app.hover = Some((5, 3)); // display = 0 + (3 - 2) = 1 → change 0
        assert_eq!(app.hovered_scm_change(), Some(0));
        app.hover = Some((5, 2)); // display 0 → header → nothing
        assert_eq!(app.hovered_scm_change(), None);
    }

    #[test]
    fn sidebar_header_hover_tracks_header_only() {
        let mut app = app();
        app.sidebar_visible = true;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 20,
            height: 8,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 7,
        };
        let moved = |column, row| MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(moved(5, 1));
        assert_eq!(app.sidebar_header_hover, Some((5, 1)));
        assert_eq!(app.hover, None);

        app.handle_mouse(moved(5, 3));
        assert_eq!(app.sidebar_header_hover, None);
        assert_eq!(app.hover, Some((5, 3)));

        app.handle_mouse(moved(30, 3));
        assert_eq!(app.sidebar_header_hover, None);
        assert_eq!(app.hover, None);
    }

    #[test]
    fn notify_makes_errors_persistent_and_info_transient() {
        let mut app = app();
        app.notify(Severity::Error, NotificationKind::Io, "save failed");
        app.notify(Severity::Information, NotificationKind::Vcs, "committed");
        let active = app.notifications.active();
        assert_eq!(active.len(), 2);
        // Newest (info) is first; it auto-expires. The error persists.
        assert!(active[0].timeout.is_some());
        assert!(active[1].timeout.is_none());
    }

    #[test]
    fn esc_dismisses_a_toast_before_normal_handling() {
        let mut app = app();
        app.notify(Severity::Error, NotificationKind::Io, "boom");
        assert!(!app.notifications.is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(app.notifications.is_empty());
        // A second Esc, with no toast left, falls through to normal handling.
        assert!(!app.should_quit);
    }

    #[test]
    fn starts_explorer_focused_with_welcome_tab() {
        let app = app();
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert!(matches!(app.tabs[0].kind, TabKind::Welcome));
    }

    #[test]
    fn focus_target_tracks_focus_and_panel() {
        let mut app = app();
        assert_eq!(app.focus_target(), FocusTarget::Explorer);
        app.sidebar_panel = SidebarPanel::SourceControl;
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);
        app.focus = Focus::Editor;
        assert_eq!(app.focus_target(), FocusTarget::Editor);
    }

    #[test]
    fn open_anyway_bypasses_the_guard_and_decodes_in_place() {
        // A .cbor file that (per its recorded length) tripped the size guard shows a
        // too-large placeholder; the override re-opens it decoded, in the same tab.
        let dir = std::env::temp_dir().join(format!("karet-anyway-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("big.cbor");
        let value = karet_cbor::CborValue::Array(vec![karet_cbor::CborValue::Integer(1)]);
        let bytes = karet_cbor::encode(&value).unwrap_or_default();
        let _ = std::fs::write(&file, &bytes);

        let mut app = app();
        let len = karet_fileview::viewer::SIZE_GUARD + 1;
        app.tabs = vec![Tab::new(
            "big.cbor",
            TabKind::Placeholder {
                path: file.clone(),
                kind: FileKind::TooLarge { len },
                dims: None,
                len,
            },
        )];
        app.active = 0;
        app.focus = Focus::Editor;
        // A too-large placeholder gets the override layer, so Enter is bound.
        assert_eq!(app.focus_target(), FocusTarget::Oversize);

        app.dispatch(Command::OpenAnyway);
        assert_eq!(
            app.tabs.len(),
            1,
            "the placeholder is replaced, not appended"
        );
        assert!(
            matches!(
                app.tabs[0].kind,
                TabKind::Code {
                    language: "CBOR",
                    ..
                }
            ),
            "open-anyway decodes the CBOR in place"
        );

        // The override is inert on an ordinary tab.
        app.dispatch(Command::OpenAnyway);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn send_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, mods));
    }

    fn commit(hash: &str, summary: &str) -> Commit {
        Commit {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            author: "T".to_string(),
            time: 0,
            parents: Vec::new(),
        }
    }

    fn commit_detail(hash: &str, summary: &str) -> CommitDetail {
        let id = karet_vcs::Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        CommitDetail {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            body: String::new(),
            author: id.clone(),
            committer: id,
            parents: Vec::new(),
            signature: None,
        }
    }
