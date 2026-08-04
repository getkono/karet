    #[test]
    fn toggle_fold_collapses_at_cursor_and_relocates_caret() {
        use karet_treesitter::ParserPool;
        use karet_treesitter::SyntaxTree;
        use karet_treesitter::language_id_from_path;

        let Some(lang) = language_id_from_path(Path::new("f.rs")) else {
            return; // rust grammar not compiled in
        };
        let src = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src).expect("parse");
        let regions = karet_syntax::fold(&tree);
        let start = regions.regions()[0].start;
        assert_eq!(start, 0, "the function body folds from line 0");

        let mut app = app();
        app.push_tab(text_tab("f.rs", src));
        if let TabKind::Code { folds, .. } = &mut app.tabs[app.active].kind {
            *folds = regions;
        }
        // Cursor inside the region: toggling collapses it and moves the caret to the
        // (still visible) header line.
        app.tabs[app.active].editor.place_caret(LineCol::new(1, 0));
        app.toggle_fold();
        assert_eq!(app.tabs[app.active].editor.cursor().line, 0);
        if let TabKind::Code { folded, .. } = &app.tabs[app.active].kind {
            assert!(folded.contains(&0));
        }
        // Toggling again (cursor now on the header) expands it.
        app.toggle_fold();
        if let TabKind::Code { folded, .. } = &app.tabs[app.active].kind {
            assert!(!folded.contains(&0));
        }
    }

    #[test]
    fn prepended_commits_dedupe_and_preserve_scroll() {
        let mut app = app();
        app.scm.log = vec![commit("aaaaaaa1", "old top"), commit("bbbbbbb2", "older")];
        app.scm_commits_offset = 5;
        // A genuinely-new commit plus a duplicate of the current top: only the new
        // one prepends, and the viewport shifts down by that one inserted row.
        app.apply_vcs_commits_prepended(vec![
            commit("ccccccc3", "new"),
            commit("aaaaaaa1", "old top"),
        ]);
        assert_eq!(app.scm.log.len(), 3);
        assert_eq!(app.scm.log[0].summary, "new");
        assert_eq!(app.scm.log[1].summary, "old top");
        assert_eq!(app.scm_commits_offset, 6);
    }

    #[test]
    fn scm_wheel_scrolls_the_region_under_the_pointer() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        // Changes region on top (rows 0..10), commit-log region below (rows 10..15).
        app.scm_changes_rect = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        };
        app.scm_total_rows = 20;
        app.scm_commits_rect = Rect {
            x: 0,
            y: 10,
            width: 20,
            height: 5,
        };
        app.scm_commits_total = 12;

        // Wheeling over the changes region scrolls it, clamped to total - height.
        app.sidebar_wheel(5, 3);
        assert_eq!(app.scm_offset, 5);
        app.sidebar_wheel(100, 3);
        assert_eq!(app.scm_offset, 10);
        app.sidebar_wheel(-100, 3);
        assert_eq!(app.scm_offset, 0);

        // Wheeling over the commit-log region scrolls it independently.
        app.sidebar_wheel(4, 11);
        assert_eq!(app.scm_commits_offset, 4);
        assert_eq!(app.scm_offset, 0); // changes untouched
        app.sidebar_wheel(100, 11);
        assert_eq!(app.scm_commits_offset, 7); // clamps to 12 - 5
    }

    #[test]
    fn source_control_commit_click_opens_pending_commit_tab_immediately() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_rect = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 12,
        };
        app.scm_commits_rect = Rect {
            x: 0,
            y: 4,
            width: 30,
            height: 6,
        };
        app.scm_commits_offset = 0;
        app.scm.log = vec![commit("aaaaaaa111", "first")];

        app.handle_sidebar_click(2, 5, KeyModifiers::NONE);

        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitLoading { rev, .. } if rev == "aaaaaaa111"
        ));
        let sent = backend
            .sent
            .lock()
            .map(|sent| sent.len())
            .unwrap_or_default();
        assert_eq!(sent, 1, "the detail request is lazy and asynchronous");
    }

    #[test]
    fn closing_a_loading_commit_cancels_it_and_late_results_stay_closed() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;

        app.request_close_active_tab();

        assert!(!app.all_tabs().any(|tab| tab.view == view));
        let cancelled = backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter().any(|(_, command)| {
                    matches!(command, SessionCommand::Cancel { request } if *request == RequestId(1))
                })
            })
            .unwrap_or_default();
        assert!(cancelled, "closing sends cooperative cancellation");

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitDetailReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
            },
        );
        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: vec![change("a.rs", StatusKind::Modified)],
            },
        );
        assert!(!app
            .all_tabs()
            .any(|tab| matches!(tab.kind, TabKind::Commit { .. } | TabKind::CommitLoading { .. })));
    }

    #[test]
    fn closing_while_commit_diffs_prepare_off_thread_stays_closed() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);
        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: (0..64)
                    .map(|index| {
                        change(&format!("src/file-{index}.rs"), StatusKind::Modified)
                    })
                    .collect(),
            },
        );
        assert!(app.pending_commit_preparation.contains_key(&RequestId(1)));

        app.request_close_active_tab();

        assert!(!app.all_tabs().any(|tab| tab.view == view));
        assert!(app.pending_commit_preparation.is_empty());
        if let Some(rx) = app.prepare_rx.as_mut()
            && let Ok(result) = rx.try_recv()
        {
            app.on_prepare_result(result);
        }
        assert!(!app.all_tabs().any(|tab| tab.view == view));
    }

    #[test]
    fn commit_detail_response_fills_the_pending_tab_in_place() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);

        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::CommitLoading { .. }
        ));

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: vec![change("a.rs", StatusKind::Modified)],
            },
        );
        finish_preparation(&mut app);

        assert_eq!(app.tabs[app.active].view, view);
        assert_eq!(app.tabs[app.active].title, "Commit aaaaaaa");
        assert!(!app.tabs[app.active].dirty);
        match &app.tabs[app.active].kind {
            TabKind::Commit { detail, files, .. } => {
                assert_eq!(detail.hash, "aaaaaaa111");
                assert_eq!(files.len(), 1);
            },
            _ => panic!("pending tab should become a loaded commit view"),
        }
    }

    #[test]
    fn commit_metadata_response_progressively_fills_pending_tab() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);

        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitDetailReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
            },
        );

        assert_eq!(app.tabs[app.active].view, view);
        assert_eq!(app.tabs[app.active].title, "Commit aaaaaaa");
        assert!(!app.tabs[app.active].dirty);
        match &app.tabs[app.active].kind {
            TabKind::Commit {
                detail,
                files,
                files_loading_since,
                ..
            } => {
                assert_eq!(detail.hash, "aaaaaaa111");
                assert!(files.is_empty());
                assert!(files_loading_since.is_some());
            },
            _ => panic!("pending tab should show commit metadata while files load"),
        }

        app.apply_commit_verification(
            "aaaaaaa111",
            GithubVerification {
                verified: true,
                reason: "valid".to_string(),
                signer: Some("Tester".to_string()),
            },
        );

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: vec![change("a.rs", StatusKind::Modified)],
            },
        );
        finish_preparation(&mut app);

        match &app.tabs[app.active].kind {
            TabKind::Commit {
                files,
                files_loading_since,
                verification,
                ..
            } => {
                assert_eq!(files.len(), 1);
                assert!(files_loading_since.is_none());
                assert!(verification.as_ref().is_some_and(|v| v.verified));
            },
            _ => panic!("metadata tab should become a complete commit view"),
        }
    }

    #[test]
    fn command_palette_keys_route_through_the_overlay_layer() {
        let mut app = app();
        app.dispatch(Command::OpenCommandPalette);
        assert!(app.overlay.is_some());
        // A printable is a fall-through into the query; Esc resolves to
        // OverlayCancel via the Overlay layer and dismisses the overlay.
        send_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.overlay.is_some());
        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn loaded_config_command_opens_read_only_tab_without_backend() {
        let mut app = app().with_loaded_config(karet_session::LoadedConfig::from_settings(
            Settings::default(),
        ));
        app.dispatch(Command::ShowLoadedConfig);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::LoadedConfig { .. }
        ));
        assert_eq!(app.tabs[app.active].title, "Loaded Settings");
    }

    #[test]
    fn search_modal_switches_between_input_and_list() {
        let mut app = app();
        app.dispatch(Command::OpenGlobalSearch);
        assert!(app.search.input, "global search starts in query input");
        // Esc in the input modal stops editing (SearchEndInput), it does not quit.
        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.search.input);
        assert!(!app.should_quit);
        // `/` from the results list re-enters input (SearchBeginInput).
        send_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(app.search.input);
        // A Ctrl-chord still resolves globally while in the Search modal.
        send_key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert!(
            !app.sidebar_visible,
            "Ctrl+B toggled the sidebar from Search"
        );
    }

    #[test]
    fn discard_prompt_confirms_and_cancels_through_the_keymap() {
        let mut app = app();
        // A bound confirm key (Enter) resolves to ConfirmDiscard and clears the arm.
        app.pending_discard = Some(vec![PathBuf::from("a.rs")]);
        send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_discard.is_none());
        // Any unbound key at the prompt cancels (the documented fall-through).
        app.pending_discard = Some(vec![PathBuf::from("a.rs")]);
        send_key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.pending_discard.is_none());
    }

    #[test]
    fn scm_range_selection_collects_both_paths() {
        // `app()` seeds one staged (a.rs) and one working (b.rs) change.
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SelectExtendDown);
        assert_eq!(app.scm.selection.selected_indices(), vec![0, 1]);
        assert_eq!(app.scm.selected_paths().len(), 2);
    }

    #[test]
    fn scm_plain_move_collapses_range() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SelectExtendDown);
        assert!(app.scm.selection.anchor().is_some());
        // A non-extending move in the SCM panel clears the range.
        app.dispatch(Command::SidebarDown);
        assert!(app.scm.selection.anchor().is_none());
        assert_eq!(app.scm.selection.selected_indices(), vec![1]);
    }

    #[test]
    fn vcs_status_event_repopulates_panel() {
        let mut app = app();
        app.apply_vcs_status(
            vec![change("x.rs", StatusKind::Added)],
            vec![
                change("y.rs", StatusKind::Untracked),
                change("z.rs", StatusKind::Modified),
            ],
        );
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes.len(), 3);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
        assert_eq!(app.scm.selection.anchor(), None);
        assert_eq!(app.scm.selection.len(), 3);
    }

    #[test]
    fn scm_cursor_display_row_accounts_for_both_permanent_headers() {
        let mut app = app();
        app.apply_vcs_status(
            vec![
                change("a.rs", StatusKind::Added),
                change("b.rs", StatusKind::Added),
            ],
            vec![
                change("c.rs", StatusKind::Modified),
                change("d.rs", StatusKind::Modified),
            ],
        );
        // Layout rows: 0 "STAGED CHANGES", 1-2 staged, 3 "CHANGES", 4-5 working.
        let rows: Vec<usize> = (0..4)
            .map(|i| {
                app.scm.selection.move_to(i);
                app.scm_cursor_display_row()
            })
            .collect();
        assert_eq!(rows, vec![1, 2, 4, 5]);
    }

    #[test]
    fn scm_cursor_display_row_reserves_line_for_empty_staged_section() {
        let mut app = app();
        // With nothing staged, the staged section still reserves its header plus one
        // placeholder line, so the first working row lands at display row 3.
        app.apply_vcs_status(
            Vec::new(),
            vec![
                change("c.rs", StatusKind::Modified),
                change("d.rs", StatusKind::Modified),
            ],
        );
        app.scm.selection.move_to(0);
        assert_eq!(app.scm_cursor_display_row(), 3);
        app.scm.selection.move_to(1);
        assert_eq!(app.scm_cursor_display_row(), 4);
    }

    #[test]
    fn permanent_commit_input_focuses_even_before_changes_are_staged() {
        let mut app = app();
        app.dispatch(Command::ScmCommit);
        assert!(app.commit_input.focused);

        // Drafting is always available; only submission requires staged changes.
        app.apply_vcs_status(Vec::new(), vec![change("b.rs", StatusKind::Modified)]);
        app.commit_cancel();
        app.dispatch(Command::ScmCommit);
        assert!(app.commit_input.focused);
        app.commit_input.text = "draft".to_string();
        app.commit_input.cursor = app.commit_input.text.len();
        app.commit_submit();
        assert!(app.status.is_some());
        assert_eq!(app.commit_input.text, "draft");
    }

    #[test]
    fn commit_editor_supports_multiline_navigation_paste_and_submit() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.dispatch(Command::ScmCommit);
        for key in [
            KeyCode::Char('s'),
            KeyCode::Char('u'),
            KeyCode::Char('b'),
            KeyCode::Char('j'),
            KeyCode::Char('e'),
            KeyCode::Char('c'),
            KeyCode::Char('t'),
            KeyCode::Enter,
        ] {
            app.commit_edit(KeyEvent::new(key, KeyModifiers::NONE));
        }
        app.commit_paste("body\r\nmore");
        assert_eq!(app.commit_input.text, "subject\nbody\nmore");
        app.commit_edit(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.commit_edit(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        app.commit_edit(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
        assert_eq!(app.commit_input.text, "subject\n>body\nmore");

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert!(app.commit_input.pending.is_some());
        assert_eq!(app.commit_input.text, "subject\n>body\nmore");
        let sent = backend
            .sent
            .lock()
            .map(|sent| sent.clone())
            .unwrap_or_default();
        assert!(sent.iter().any(|(_, command)| matches!(
            command,
            SessionCommand::Commit { message } if message == "subject\n>body\nmore"
        )));

        let pending = app.commit_input.pending;
        app.on_backend_event(
            pending,
            SessionEvent::Notification {
                severity: Severity::Error,
                kind: NotificationKind::Vcs,
                message: "identity missing".to_string(),
            },
        );
        assert_eq!(app.commit_input.pending, None);
        assert_eq!(app.commit_input.text, "subject\n>body\nmore");

        app.commit_submit();
        let pending = app.commit_input.pending;
        app.on_backend_event(
            pending,
            SessionEvent::Committed {
                oid: "1234567890abcdef".to_string(),
            },
        );
        assert!(app.commit_input.text.is_empty());
        assert_eq!(app.commit_input.pending, None);
    }

    #[test]
    fn opening_a_diff_keeps_source_control_focused() {
        // The contract: browsing (arrow moves) previews each change's diff while
        // the SCM pane keeps focus, so stage/unstage/discard/commit and the
        // selection keys keep working; Enter is the explicit "commit into the
        // view" action that focuses the diff editor (see
        // `enter_on_a_change_materializes_and_focuses_the_diff`).
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarDown); // cursor 0 → 1: previews b.rs
        assert!(app.active_is_diff(), "the diff preview is shown");
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.tabs[app.active].title, "b.rs");
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);
        assert_eq!(app.tabs.len(), 1, "welcome tab is replaced, not appended");
        // Arrowing back retargets the SAME preview slot — never one tab per
        // visited change.
        app.dispatch(Command::SidebarUp); // cursor 1 → 0: previews a.rs
        assert_eq!(
            app.tabs.len(),
            1,
            "the preview slot is reused, not appended"
        );
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.tabs[app.active].title, "a.rs");
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn enter_on_a_change_materializes_and_focuses_the_diff() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        // Browse first: the diff shows as a preview without stealing focus.
        app.dispatch(Command::SidebarDown);
        assert!(app.tabs[app.active].is_preview);
        let view = app.tabs[app.active].view;
        // Enter: the SAME previewed view is materialized and focused — the
        // reported bug was a brand-new duplicate diff tab on every Enter.
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "Enter must reuse the previewed diff");
        assert!(!app.tabs[app.active].is_preview, "Enter materializes");
        assert_eq!(app.tabs[app.active].view, view, "the same view, not a copy");
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);
        // Enter again (back from the sidebar): re-focuses, never duplicates.
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "repeat Enter must not duplicate");
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn enter_on_a_focused_diff_opens_the_file_at_its_first_changed_line() {
        let dir = test_dir("diff-enter-into-file");
        write_file(&dir, "a.rs", b"fn a() {}\nfn added() {}\nfn c() {}\n");
        let changed = FileChange {
            path: PathBuf::from("a.rs"),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: "fn a() {}\nfn c() {}\n".to_string(),
            new: "fn a() {}\nfn added() {}\nfn c() {}\n".to_string(),
        };
        let mut app = App::new(dir.clone(), Vec::new(), vec![changed], false);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate); // materialize + focus the diff
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

        // Enter on the focused diff drops into the file, caret on the first
        // changed line (line 2, 0-based 1) — keyboard parity with the mouse.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(app.tabs[app.active].kind, TabKind::Code { .. }),
            "a normal, editable editor tab"
        );
        assert_eq!(
            app.tabs[app.active].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(
            app.tabs[app.active].editor.cursor().line,
            1,
            "caret lands on the first changed line"
        );
        assert_eq!(app.tabs.len(), 2, "the diff stays open alongside the file");

        // Enter again from the diff re-focuses the existing file tab — never a
        // duplicate.
        let file_idx = app.active;
        let diff_idx = app
            .tabs
            .iter()
            .position(Tab::is_diff)
            .expect("the diff tab is still open");
        app.select_tab(diff_idx);
        app.dispatch(Command::OpenDiffFile);
        assert_eq!(app.tabs.len(), 2, "no duplicate editor tab");
        assert_eq!(app.active, file_idx);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enter_on_a_deleted_files_diff_reports_instead_of_opening() {
        let dir = test_dir("diff-enter-deleted");
        let deleted = FileChange {
            path: PathBuf::from("gone.rs"),
            old_path: None,
            status: StatusKind::Deleted,
            is_binary: false,
            old: "fn gone() {}\n".to_string(),
            new: String::new(),
        };
        let mut app = App::new(dir.clone(), Vec::new(), vec![deleted], false);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

        // The file is gone from the working tree: Enter degrades to a status
        // message — no dead tab, no panic.
        app.dispatch(Command::OpenDiffFile);
        assert_eq!(app.tabs.len(), 1, "nothing new opens for a deleted file");
        assert!(app.active_is_diff(), "the diff stays active");
        assert!(
            app.status.as_deref().is_some_and(|s| s.contains("gone.rs")),
            "a status message names the missing file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scm_double_click_materializes_the_previewed_diff_without_duplicating() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_visible = true;
        // Seed the layout state a render would have produced: the changes region
        // starts at row 2, whose first display row is change index 0.
        app.sidebar_rect = Rect::new(0, 0, 20, 20);
        app.scm_changes_rect = Rect::new(0, 2, 20, 10);
        app.scm_row_map = vec![Some(0), Some(1)];

        // First click of the double-click: the diff opens as a preview and the
        // panel keeps focus (a plain single click is a browse).
        app.handle_sidebar_click(3, 2, KeyModifiers::NONE);
        assert!(app.active_is_diff());
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.focus, Focus::Sidebar);
        let view = app.tabs[app.active].view;

        // Second click: the SAME view is materialized and focused — the bug was
        // a separate duplicate view on double-click.
        app.handle_sidebar_click(3, 2, KeyModifiers::NONE);
        assert_eq!(
            app.tabs.len(),
            1,
            "double-click must not duplicate the diff"
        );
        assert!(
            !app.tabs[app.active].is_preview,
            "double-click materializes"
        );
        assert_eq!(app.tabs[app.active].view, view, "the same view, not a copy");
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);
    }

    #[test]
    fn enter_on_an_explorer_file_materializes_it() {
        let dir = test_dir("explorer-enter-materialize");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        select_explorer_path(&mut app, &dir.join("a.rs"));

        // Enter opens the file materialized (not a preview) and focuses it.
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            !app.tabs[0].is_preview,
            "Enter materializes, never previews"
        );
        assert_eq!(app.focus, Focus::Editor);

        // Enter again re-focuses the same tab — no duplicate.
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "repeat Enter must not duplicate");
        assert_eq!(app.focus, Focus::Editor);

        // And Enter on a file currently in the preview slot materializes that
        // same tab in place.
        app.close_all_tabs();
        app.open_path_preview(&dir.join("a.rs"), false);
        assert!(app.tabs[0].is_preview);
        let view = app.tabs[0].view;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tabs[0].is_preview);
        assert_eq!(app.tabs[0].view, view, "the same view, not a copy");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_preview_and_a_diff_preview_share_the_panes_one_slot() {
        let dir = test_dir("shared-preview-slot");
        write_file(&dir, "c.rs", b"fn c() {}\n");
        let mut app = App::new(
            dir.clone(),
            vec![change("a.rs", StatusKind::Modified)],
            Vec::new(),
            false,
        );
        // A previewed file occupies the slot…
        app.open_path_preview(&dir.join("c.rs"), false);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);
        assert!(!app.tabs[0].is_diff());

        // …a previewed diff replaces it in place…
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.preview_selected_diff();
        assert_eq!(app.tabs.len(), 1, "one preview slot per pane, any content");
        assert!(app.tabs[0].is_preview);
        assert!(app.tabs[0].is_diff());

        // …and a previewed file takes it back.
        app.open_path_preview(&dir.join("c.rs"), false);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);
        assert!(!app.tabs[0].is_diff());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stepping_changed_files_walks_the_scm_list() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate); // opens a.rs (index 0)
        app.dispatch(Command::NextChangedFile);
        assert_eq!(app.scm.selection.cursor(), 1);
        app.dispatch(Command::PrevChangedFile);
        assert_eq!(app.scm.selection.cursor(), 0);
    }

    #[test]
    fn toggle_diff_layout_flips_view() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate);
        let before = matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::Unified,
                ..
            }
        );
        app.dispatch(Command::ToggleDiffLayout);
        let after = matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::SideBySide,
                ..
            }
        );
        assert!(before && after);
        // The choice persists: the next opened diff adopts the remembered layout.
        assert_eq!(app.diff_layout, ViewMode::SideBySide);
        app.scm.selection.move_to(1);
        app.dispatch(Command::SidebarActivate);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::SideBySide,
                ..
            }
        ));
    }
