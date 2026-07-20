    #[test]
    fn toggle_sidebar_and_focus() {
        let mut app = app();
        app.dispatch(Command::ToggleSidebar);
        assert!(!app.sidebar_visible);
        app.dispatch(Command::ToggleFocus);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn opening_same_file_focuses_existing_tab() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-open-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");
        let _ = std::fs::write(&b, "fn b() {}\n");

        let mut app = app();
        app.open_path(&a);
        assert_eq!(app.tabs.len(), 1, "first open replaces the welcome tab");
        app.open_path(&a);
        assert_eq!(
            app.tabs.len(),
            1,
            "re-opening the same file focuses, not duplicates"
        );
        app.open_path(&b);
        assert_eq!(app.tabs.len(), 2);
        app.open_path(&a); // focuses a's existing tab rather than appending
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_open_replaces_the_current_preview_tab_in_place() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");
        let _ = std::fs::write(&b, "fn b() {}\n");

        let mut app = app();
        app.open_path_preview(&a, true);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            app.tabs[0].is_preview,
            "a preview-opened file is marked preview"
        );
        assert_eq!(app.tabs[0].path(), Some(a.as_path()));

        // Navigating to a second file replaces the preview tab in place — no
        // second tab, and the old one's path is gone.
        app.open_path_preview(&b, true);
        assert_eq!(
            app.tabs.len(),
            1,
            "opening another preview must replace, not append"
        );
        assert!(app.tabs[0].is_preview);
        assert_eq!(app.tabs[0].path(), Some(b.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_open_on_an_already_open_permanent_tab_just_focuses_it() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-focus-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");

        let mut app = app();
        app.open_path(&a); // permanent open (not preview)
        assert!(!app.tabs[0].is_preview);

        app.open_path_preview(&a, true);
        assert_eq!(app.tabs.len(), 1, "must not duplicate an already-open file");
        assert!(
            !app.tabs[0].is_preview,
            "focusing an already-open permanent tab must not turn it into a preview"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_click_promotes_the_preview_tab_without_duplicating_it() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-promote-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer.ensure_built(&dir);
        app.explorer.select_visible(
            app.explorer
                .rows()
                .iter()
                .position(|r| r.label == "a.rs")
                .expect("a.rs is listed"),
        );
        app.sidebar_promote_or_open_permanent();
        assert_eq!(app.tabs.len(), 1, "not yet open: opens one permanent tab");
        assert!(!app.tabs[0].is_preview);

        // Re-open as preview, then double-click-promote the existing tab: still
        // exactly one tab, now permanent.
        app.close_all_tabs();
        app.open_path_preview(&a, true);
        assert!(app.tabs[0].is_preview);
        app.sidebar_promote_or_open_permanent();
        assert_eq!(app.tabs.len(), 1, "promoting must not open a duplicate tab");
        assert!(
            !app.tabs[0].is_preview,
            "double-click clears the preview flag"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn editing_a_preview_tab_promotes_it_permanently() {
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-edit-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "ab").expect("write temp file");

        let (session, mut events, mut snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path_preview(&path, true);
        pump(&mut app, &mut events).await;
        assert!(app.tabs[app.active].is_preview);

        app.dispatch(Command::InsertChar('x'));
        pump(&mut app, &mut events).await;
        // The dirty flag (and thus the promote-on-edit hook) is only ever set from
        // a document snapshot, not from the optimistic local apply in submit_edit.
        while let Ok(Some((doc, snap))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), snaps.recv()).await
        {
            app.on_snapshot(doc, &snap);
        }
        assert!(
            !app.tabs[app.active].is_preview,
            "the first edit must permanently promote the preview tab"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_dirty_preview_is_not_silently_replaced_by_the_next_preview() {
        // Editing a preview promotes it (see `editing_a_preview_tab_...`), so by the
        // time the next preview opens the edited tab is no longer the preview slot:
        // it survives, its document is not discarded, and the close guard (#51) is
        // never asked to drop unsaved work.
        let dir = test_dir("preview-dirty-safety");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "ab").expect("write a");
        std::fs::write(&b, "cd").expect("write b");

        let (session, mut events, mut snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);

        app.open_path_preview(&a, true);
        pump(&mut app, &mut events).await;
        assert!(app.tabs[app.active].is_preview);

        // Edit a.txt so a snapshot marks it dirty (and thus permanent).
        app.dispatch(Command::InsertChar('x'));
        pump(&mut app, &mut events).await;
        while let Ok(Some((doc, snap))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), snaps.recv()).await
        {
            app.on_snapshot(doc, &snap);
        }
        assert!(!app.tabs[app.active].is_preview, "the edit promoted a.txt");
        assert!(app.tabs[app.active].dirty, "a.txt has unsaved changes");

        // Now preview b.txt. The dirty a.txt tab must NOT be replaced — it has no
        // preview flag — so b.txt opens as a second tab and a.txt is kept safe.
        app.open_path_preview(&b, true);
        pump(&mut app, &mut events).await;
        assert_eq!(app.tabs.len(), 2, "the dirty tab is kept, not replaced");
        let a_tab = app
            .tabs
            .iter()
            .find(|t| t.path().map(canonical) == Some(canonical(&a)))
            .expect("a.txt is still open");
        assert!(a_tab.dirty, "a.txt keeps its unsaved changes");
        assert!(!a_tab.is_preview);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn explorer_row_index(app: &App, label: &str) -> usize {
        app.explorer
            .rows()
            .iter()
            .position(|r| r.label == label)
            .unwrap_or_else(|| panic!("missing explorer row {label}"))
    }

    #[test]
    fn arrowing_the_explorer_previews_files_without_stealing_focus() {
        let dir = test_dir("explorer-arrow-preview");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        let a_idx = explorer_row_index(&app, "a.rs");
        let b_idx = explorer_row_index(&app, "b.rs");
        app.explorer.select_index(a_idx);

        // Arrow onto b.rs: it opens in the pane's preview slot, and the sidebar
        // keeps keyboard focus so the user can keep arrowing.
        app.sidebar_step((b_idx as i32 - a_idx as i32).signum());
        assert_eq!(app.focus, Focus::Sidebar, "arrowing must not steal focus");
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview, "the arrowed-to file is a preview");
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("b.rs")))
        );

        // Arrow back onto a.rs: the single preview slot is reused, never appended.
        app.sidebar_step((a_idx as i32 - b_idx as i32).signum());
        assert_eq!(
            app.tabs.len(),
            1,
            "one preview slot is reused, not appended"
        );
        assert!(app.tabs[0].is_preview);
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );
        assert_eq!(app.focus, Focus::Sidebar);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wheel_scrolling_the_explorer_moves_selection_without_previewing() {
        let dir = test_dir("explorer-wheel-no-preview");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        let before = app.explorer.cursor();

        // A wheel notch moves the selection but must not open anything —
        // scrolling past files must not thrash the preview slot.
        app.sidebar_wheel(1, 3);
        assert_ne!(app.explorer.cursor(), before, "the wheel moves selection");
        assert_eq!(app.tabs.len(), 1);
        assert!(
            matches!(app.tabs[0].kind, TabKind::Welcome),
            "the wheel must not open a preview"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arrowing_onto_a_directory_row_leaves_the_editor_untouched() {
        let dir = test_dir("explorer-arrow-dir");
        write_file(&dir, "sub/nested.rs", b"fn n() {}\n");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        // Preview a.rs first, then arrow onto the `sub` directory row.
        let a_idx = explorer_row_index(&app, "a.rs");
        app.explorer.select_index(a_idx);
        app.preview_selected_explorer_row();
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);

        let sub_idx = explorer_row_index(&app, "sub");
        assert!(
            app.explorer.rows()[sub_idx].is_dir,
            "sub is a directory row"
        );
        app.explorer.select_index(sub_idx);
        app.preview_selected_explorer_row();
        // Landing on a directory changes nothing: the a.rs preview stays as-is.
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arrowing_onto_an_already_open_file_activates_it_without_a_new_tab() {
        let dir = test_dir("explorer-arrow-permanent");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        // b.rs is open as a permanent tab (not a preview).
        app.open_path(&dir.join("b.rs"));
        assert!(!app.tabs[0].is_preview);
        app.focus = Focus::Sidebar;

        // Arrow onto b.rs from a.rs: it activates the existing permanent tab
        // rather than opening a preview, and does not steal focus.
        let a_idx = explorer_row_index(&app, "a.rs");
        let b_idx = explorer_row_index(&app, "b.rs");
        app.explorer.select_index(a_idx);
        app.sidebar_step((b_idx as i32 - a_idx as i32).signum());
        assert_eq!(app.tabs.len(), 1, "must not duplicate the open file");
        assert!(
            !app.tabs[0].is_preview,
            "an already-permanent tab stays permanent"
        );
        assert_eq!(app.active, 0);
        assert_eq!(app.focus, Focus::Sidebar);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_highlights_matches_in_a_code_tab() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;

        let mut app = app();
        app.push_tab(Tab::new(
            "t.rs",
            TabKind::Code {
                path: PathBuf::from("t.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text("foo bar foo"),
                text: "foo bar foo".to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        app.dispatch(Command::OpenFind);
        if let Some(find) = app.active_find_mut() {
            find.query = "foo".to_string();
        }
        app.run_find();
        assert_eq!(app.active_find().map(|f| f.count), Some(2));
        if let TabKind::Code { decos, .. } = &app.tabs[app.active].kind {
            assert_eq!(decos.len(), 2);
        } else {
            unreachable!("active tab is a code tab");
        }
        // Closing find clears the highlights.
        app.close_find();
        if let TabKind::Code { decos, .. } = &app.tabs[app.active].kind {
            assert!(decos.is_empty());
        }
    }

    #[test]
    fn global_search_collects_matching_files() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("a.txt"), "needle here\n");
        let _ = std::fs::write(dir.join("b.txt"), "nothing\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 1);
        assert!(app.search.results[0].path.ends_with("a.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn go_to_commit_input_opens_reprompts_and_cancels() {
        let mut app = app();
        app.dispatch(Command::OpenCommitByHash);
        assert_eq!(app.rev_input.as_deref(), Some(""));
        // Submitting an empty revision re-prompts rather than closing.
        app.dispatch(Command::RevInputSubmit);
        assert_eq!(app.rev_input.as_deref(), Some(""));
        // Cancel clears the input.
        app.dispatch(Command::RevInputCancel);
        assert!(app.rev_input.is_none());
    }

    #[test]
    fn file_history_requires_an_open_file() {
        let mut app = app();
        // The Welcome tab has no path — file history has nothing to show.
        app.dispatch(Command::ShowFileHistory);
        assert_eq!(
            app.status.as_deref(),
            Some("file history: open a file first")
        );
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn commit_graph_browser_opens_fills_and_clamps_navigation() {
        let mut app = app();
        app.dispatch(Command::ShowCommitGraph);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::CommitGraph { .. }
        ));

        // Backend is None in unit tests, so feed a history page directly.
        let commit = |hash: &str, summary: &str, parents: Vec<String>| Commit {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            author: "Tester".to_string(),
            time: 0,
            parents,
        };
        app.apply_graph_log(
            0,
            vec![
                commit("aaaa", "c1", vec!["bbbb".to_string()]),
                commit("bbbb", "c0", Vec::new()),
            ],
            false,
        );
        if let TabKind::CommitGraph { commits, .. } = &app.tabs[app.active].kind {
            assert_eq!(commits.len(), 2);
        }
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 0, .. }
        ));
        // Down twice clamps at the last commit; up past the top clamps at 0.
        app.graph_select(1);
        app.graph_select(1);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 1, .. }
        ));
        app.graph_select(-5);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 0, .. }
        ));
    }

    #[test]
    fn parse_rev_range_distinguishes_two_and_three_dot() {
        assert_eq!(
            parse_rev_range("main..feature"),
            Some(("main".to_string(), "feature".to_string(), false))
        );
        assert_eq!(
            parse_rev_range("main...feature"),
            Some(("main".to_string(), "feature".to_string(), true))
        );
        // An omitted side defaults to HEAD, and whitespace is trimmed.
        assert_eq!(
            parse_rev_range("origin/main.. "),
            Some(("origin/main".to_string(), "HEAD".to_string(), false))
        );
        assert_eq!(
            parse_rev_range("...HEAD"),
            Some(("HEAD".to_string(), "HEAD".to_string(), true))
        );
        // A plain revision is not a range.
        assert_eq!(parse_rev_range("HEAD~2"), None);
        assert_eq!(parse_rev_range("abc123"), None);
    }

    #[test]
    fn open_compare_tab_builds_a_compare_tab() {
        let mut app = app();
        app.open_compare_tab(
            "main".to_string(),
            "HEAD".to_string(),
            true,
            vec![change("a.rs", StatusKind::Modified)],
        );
        match &app.tabs[app.active].kind {
            TabKind::Compare {
                base_label,
                head_label,
                merge_base,
                files,
                view,
            } => {
                assert_eq!(base_label, "main");
                assert_eq!(head_label, "HEAD");
                assert!(*merge_base);
                assert_eq!(files.len(), 1);
                assert_eq!(view.scroll, 0);
            },
            _ => panic!("expected a compare tab"),
        }
        // A compare tab scrolls via the shared pager arm.
        app.scroll_lines(2);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Compare { view, .. } if view.scroll == 2
        ));
    }

    #[test]
    fn graph_compare_requires_a_marked_base() {
        let mut app = app();
        app.dispatch(Command::ShowCommitGraph);
        app.apply_graph_log(
            0,
            vec![
                Commit {
                    hash: "aaaa".to_string(),
                    short_hash: "aaaa".to_string(),
                    summary: "c1".to_string(),
                    author: "T".to_string(),
                    time: 0,
                    parents: vec!["bbbb".to_string()],
                },
                Commit {
                    hash: "bbbb".to_string(),
                    short_hash: "bbbb".to_string(),
                    summary: "c0".to_string(),
                    author: "T".to_string(),
                    time: 0,
                    parents: Vec::new(),
                },
            ],
            false,
        );
        // Comparing before marking a base only reports a status hint.
        app.dispatch(Command::CommitGraphCompare);
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("mark a compare base")),
            "compare without a base nudges the user"
        );
        // Marking a base records it on the browser tab.
        app.dispatch(Command::CommitGraphMarkBase);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { compare_base: Some(b), .. } if b == "aaaa"
        ));
    }

    #[test]
    fn commit_view_scrolls_by_wheel_and_page_and_edges() {
        let mut app = app();
        // Build a standalone commit view with one changed file.
        let detail = CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: karet_vcs::Identity {
                name: "Tester".to_string(),
                email: "t@example.com".to_string(),
                time: 0,
                offset: 0,
            },
            committer: karet_vcs::Identity {
                name: "Tester".to_string(),
                email: "t@example.com".to_string(),
                time: 0,
                offset: 0,
            },
            parents: Vec::new(),
            signature: None,
        };
        let files = vec![FileView::new(
            change("a.rs", StatusKind::Modified),
            crate::render::Section::Staged,
            false,
        )];
        app.push_tab(Tab::commit(Box::new(detail), files));
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Commit { view, .. } if view.scroll == 0
        ));

        // A wheel notch / ScrollDown advances the offset (the draw-time clamp caps it).
        app.scroll_lines(3);
        let scrolled = match &app.tabs[app.active].kind {
            TabKind::Commit { view, .. } => view.scroll,
            _ => unreachable!(),
        };
        assert_eq!(scrolled, 3, "the commit view scrolls on a wheel notch");

        // Bottom pins to u16::MAX (clamped against content only during draw); Top returns to 0.
        app.scroll_edge(false);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Commit { view, .. } if view.scroll == u16::MAX
        ));
        app.scroll_edge(true);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Commit { view, .. } if view.scroll == 0
        ));
    }

    #[test]
    fn double_click_badge_reveals_and_wakes_to_hide() {
        let mut app = app();
        let id = || karet_vcs::Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        let detail = CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: id(),
            committer: id(),
            parents: Vec::new(),
            signature: None,
        };
        let files = vec![FileView::new(
            change("a.rs", StatusKind::Modified),
            crate::render::Section::Staged,
            false,
        )];
        app.push_tab(Tab::commit(Box::new(detail), files));
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        app.pane_frames = vec![content_frame(&app, area)];
        // Pretend the last frame placed the badge here.
        let badge = Rect {
            x: 20,
            y: 3,
            width: 8,
            height: 1,
        };
        app.commit_badge_rect = Some(badge);
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // A single click on the badge does not reveal (needs a double-click).
        app.handle_editor_click(click(22, 3));
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit {
                explain_since: None,
                ..
            }
        ));

        // A second, quick click over the same cell reveals the explanation.
        app.handle_editor_click(click(22, 3));
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit {
                explain_since: Some(_),
                ..
            }
        ));

        // The loop is now scheduled to wake within the reveal window so it can repaint
        // and hide the tooltip.
        let wake = app.next_wake().expect("a reveal is pending");
        assert!(wake <= COMMIT_REVEAL && wake > Duration::ZERO);
    }

    #[test]
    fn global_search_highlights_matches_in_an_already_open_tab() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-search-decos-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("a.txt");
        let _ = std::fs::write(&file, "needle here\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&file);

        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 1);
        match &app.tabs[app.active].kind {
            TabKind::Code { search_decos, .. } => assert_eq!(search_decos.len(), 1),
            _ => unreachable!("expected a code tab"),
        }

        // Clearing the query must clear the highlights too, not leave them stale.
        app.search.query.clear();
        app.run_global_search();
        match &app.tabs[app.active].kind {
            TabKind::Code { search_decos, .. } => assert!(search_decos.is_empty()),
            _ => unreachable!("expected a code tab"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_changed_event_reruns_a_live_global_search() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-fschanged-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 0, "no matching file exists yet");

        // A file matching the live query appears on disk...
        let file = dir.join("new.txt");
        let _ = std::fs::write(&file, "needle here\n");
        // ...and the watcher's debounced event is what tells the app to look again.
        app.on_backend_event(None, SessionEvent::FsChanged { paths: vec![file] });
        assert_eq!(
            app.search.results.len(),
            1,
            "FsChanged must re-run the live search"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blame_without_a_code_tab_reports_status() {
        let mut app = app();
        app.settings.git.blame = false;
        app.dispatch(Command::ShowBlame);
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
        assert!(app.settings.git.blame);
        assert_eq!(app.settings.git.blame_mode.as_str(), "line");
    }

    #[test]
    fn blame_command_cycles_line_semantic_and_off() {
        let mut app = app();
        app.settings.git.blame = true;
        app.settings.git.blame_mode = karet_session::config::GitBlameMode::Line;
        app.dispatch(Command::ShowBlame);
        assert_eq!(app.settings.git.blame_mode.as_str(), "semantic");
        app.dispatch(Command::ShowBlame);
        assert!(!app.settings.git.blame);
    }
