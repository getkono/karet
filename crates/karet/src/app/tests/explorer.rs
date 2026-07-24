    #[test]
    fn explorer_header_toolbar_click_begins_new_file() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.header_action_hits = vec![
            (20, 22, Command::ExplorerNewFile),
            (22, 24, Command::ExplorerNewFolder),
            (24, 26, Command::ExplorerRefresh),
            (26, 28, Command::ExplorerCollapseAll),
        ];
        // Clicking the "new file" button on the header row starts an inline edit.
        app.handle_sidebar_click(20, 1, KeyModifiers::NONE);
        assert!(app.explorer.is_editing());
    }

    #[test]
    fn explorer_blank_area_click_does_not_open_the_last_row() {
        let dir = test_dir("blank-click");
        write_file(&dir, "a.txt", b"a");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_rect = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 7,
        };
        app.explorer.ensure_built(&dir);

        app.handle_sidebar_click(1, 5, KeyModifiers::NONE);

        assert!(
            !app.tabs
                .iter()
                .any(|tab| tab.path() == Some(dir.join("a.txt").as_path()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_commit_edit_creates_a_file() {
        let dir = std::env::temp_dir().join(format!("karet-newfile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer_begin_new(false);
        for c in "hello.txt".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();
        assert!(dir.join("hello.txt").exists());
        assert!(!app.explorer.is_editing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_explorer_create_keeps_inline_name_for_retry() {
        let dir = std::env::temp_dir().join(format!("karet-newfile-fail-{}", std::process::id()));
        let existing = dir.join("existing");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&existing, "already here");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer_begin_new(true);
        for c in "existing".chars() {
            app.explorer.edit_push(c);
        }

        app.explorer_commit_edit();

        assert!(app.explorer.is_editing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_copy_paste_file_uses_copy_suffix() {
        let dir = test_dir("copy-file");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        app.dispatch(Command::Copy);
        app.dispatch(Command::Paste);

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_copy_paste_directory_recursively_into_selected_directory() {
        let dir = test_dir("copy-dir");
        write_file(&dir, "src/nested/file.txt", b"nested");
        write_file(&dir, "src/marker.txt", b"marker");
        let _ = std::fs::create_dir_all(dir.join("dst"));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("src"));
        app.dispatch(Command::Copy);
        select_explorer_path(&mut app, &dir.join("dst"));
        app.dispatch(Command::Paste);

        assert_eq!(
            std::fs::read(dir.join("dst/src/nested/file.txt")).unwrap_or_default(),
            b"nested"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_cut_paste_moves_files_and_clears_clipboard() {
        let dir = test_dir("cut-file");
        write_file(&dir, "move.txt", b"move");
        let _ = std::fs::create_dir_all(dir.join("dst"));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("move.txt"));
        app.dispatch(Command::Cut);
        select_explorer_path(&mut app, &dir.join("dst"));
        app.dispatch(Command::Paste);

        assert!(!dir.join("move.txt").exists());
        assert_eq!(
            std::fs::read(dir.join("dst/move.txt")).unwrap_or_default(),
            b"move"
        );
        assert!(app.explorer_clipboard.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_duplicate_file_uses_copy_suffix() {
        let dir = test_dir("duplicate-file");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("a.txt"));
        app.dispatch(Command::ExplorerDuplicate);

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_expands_ancestors_and_selects_nested_file() {
        let dir = test_dir("reveal-nested");
        write_file(&dir, "a/b/c.rs", b"code");
        write_file(&dir, "a/note.txt", b"note");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        // Start from a different panel/focus to prove the reveal switches them.
        app.sidebar_panel = SidebarPanel::Search;
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        let target = dir.join("a/b/c.rs");
        app.reveal_in_explorer(&target);

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_selects_a_directory() {
        let dir = test_dir("reveal-dir");
        write_file(&dir, "a/b/c.rs", b"code");
        write_file(&dir, "a/note.txt", b"note");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        let target = dir.join("a/b");
        app.reveal_in_explorer(&target);

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_outside_root_is_noop_with_status() {
        let dir = test_dir("reveal-outside");
        write_file(&dir, "inside.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        let outside = dir
            .parent()
            .map(|p| p.join("elsewhere.txt"))
            .unwrap_or_else(|| PathBuf::from("/elsewhere.txt"));
        app.reveal_in_explorer(&outside);

        // Nothing changes but a status note.
        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::Editor);
        assert!(
            app.status.as_deref().is_some_and(|s| s.contains("outside")),
            "status: {:?}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_missing_path_reports_status() {
        let dir = test_dir("reveal-missing");
        write_file(&dir, "inside.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        app.reveal_in_explorer(&dir.join("does-not-exist.txt"));

        // A path under the root but absent from the tree does not steal focus.
        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::Editor);
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("not in the explorer")),
            "status: {:?}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_scrolls_selection_into_view() {
        let dir = test_dir("reveal-scroll");
        for i in 0..30 {
            write_file(&dir, &format!("d/f{i:02}.txt"), b"x");
        }
        write_file(&dir, "d/target.txt", b"needle");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        let target = dir.join("d/target.txt");
        app.reveal_in_explorer(&target);
        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));

        // Render a short terminal: the tree clamps its offset to the cursor, so the
        // revealed row scrolls into view even though it sits far below the fold.
        let painted = screen(&mut app, 100, 12).join("\n");
        assert!(
            painted.contains("target.txt"),
            "revealed row not scrolled into view:\n{painted}"
        );
        assert!(
            app.explorer.offset() > 0,
            "the tree did not scroll to reveal the selection"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_of_the_root_focuses_the_explorer_without_reselecting() {
        let dir = test_dir("reveal-root");
        write_file(&dir, "top.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        select_explorer_path(&mut app, &dir.join("top.txt"));
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        // The root has no row of its own: revealing it shows and focuses the
        // Explorer but leaves the selection where it was.
        app.reveal_in_explorer(&dir);

        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(
            app.explorer.selected_path(),
            Some(dir.join("top.txt").as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A frame whose breadcrumb row is at `y = 1` (columns 10..50) with one
    /// clickable segment at columns 12..15 resolving to `segment`, over a content
    /// rect that deliberately overlaps the breadcrumb row — so a swallowed click
    /// is distinguishable from one that fell through to the editor.
    fn breadcrumb_frame(app: &App, segment: PathBuf) -> PaneFrame {
        PaneFrame {
            pane: app.focus_pane(),
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            action_hits: Vec::new(),
            breadcrumb_rect: Rect {
                x: 10,
                y: 1,
                width: 40,
                height: 1,
            },
            breadcrumb_hits: vec![BreadcrumbHit {
                start: 12,
                end: 15,
                path: segment,
            }],
            content_rect: Rect {
                x: 10,
                y: 1,
                width: 40,
                height: 10,
            },
            commit_file_hits: Vec::new(),
        }
    }

    #[test]
    fn clicking_a_breadcrumb_segment_reveals_its_path_in_the_explorer() {
        let dir = test_dir("breadcrumb-click");
        write_file(&dir, "a/b/c.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;
        let target = dir.join("a/b");
        app.pane_frames = vec![breadcrumb_frame(&app, target.clone())];

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 13,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_breadcrumb_gap_click_is_swallowed_not_forwarded_to_the_editor() {
        let dir = test_dir("breadcrumb-gap");
        write_file(&dir, "a/b/c.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.focus = Focus::Sidebar;
        app.pane_frames = vec![breadcrumb_frame(&app, dir.join("a/b"))];

        // Column 16 is past the segment (a separator gap): the click lands on the
        // breadcrumb row but maps to no segment. Had it fallen through, the editor
        // click handler (whose content rect overlaps the row) would steal focus.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 16,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.focus, Focus::Sidebar, "the gap click fell through");
        assert_eq!(app.explorer.selected_path(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_frame_records_breadcrumb_hits_only_within_the_workspace() {
        let dir = test_dir("breadcrumb-frame");
        write_file(&dir, "a/b.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&dir.join("a/b.rs"));

        let painted = screen(&mut app, 200, 20).join("\n");
        assert!(
            painted.contains('\u{203a}'),
            "the breadcrumb separator did not paint:\n{painted}"
        );

        let frame = app.pane_frames.first().expect("a pane frame");
        assert_eq!(frame.breadcrumb_rect.height, 1);
        let paths: Vec<_> = frame
            .breadcrumb_hits
            .iter()
            .map(|h| h.path.clone())
            .collect();
        // Segments above the workspace root ("/", "tmp", …) are inert: only the
        // root itself and the components below it are recorded.
        assert_eq!(paths, vec![dir.clone(), dir.join("a"), dir.join("a/b.rs")]);
        // Spans are ordered, non-overlapping, and inside the breadcrumb row.
        for pair in frame.breadcrumb_hits.windows(2) {
            assert!(pair[0].end < pair[1].start, "segments overlap or touch");
        }
        for hit in &frame.breadcrumb_hits {
            assert!(hit.start >= frame.breadcrumb_rect.x);
            assert!(hit.end <= frame.breadcrumb_rect.right());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_delete_requires_confirmation() {
        let dir = test_dir("delete-file");
        write_file(&dir, "gone.txt", b"delete");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("gone.txt"));
        app.dispatch(Command::ExplorerDelete);
        assert!(dir.join("gone.txt").exists());
        assert!(app.pending_explorer_delete.is_some());

        app.dispatch(Command::ConfirmExplorerDelete);
        assert!(!dir.join("gone.txt").exists());
        assert!(app.pending_explorer_delete.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_context_menu_accepts_the_selected_file_command() {
        let dir = test_dir("context-duplicate");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer.ensure_built(&dir);
        let Some(row) = app
            .explorer
            .rows()
            .iter()
            .position(|row| row.path == dir.join("a.txt"))
        else {
            return;
        };

        app.open_context_menu(2, 2, Some(row));
        let Some(menu) = app.context_menu.as_mut() else {
            return;
        };
        let Some(duplicate) = menu
            .entries
            .iter()
            .position(|entry| entry.command() == Some(Command::ExplorerDuplicate))
        else {
            return;
        };
        menu.selected = duplicate;
        app.accept_context_menu();

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(app.context_menu.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_right_click_selects_the_row_and_offers_path_commands() {
        let dir = test_dir("context-copy-path");
        let target = dir.join("a.txt");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = true;
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_rect = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 7,
        };
        app.explorer.ensure_built(&dir);
        let Some(row) = app
            .explorer
            .rows()
            .iter()
            .position(|row| row.path == target)
        else {
            return;
        };

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 2,
            row: app.sidebar_content_rect.y + row as u16,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        let Some(menu) = app.context_menu.as_ref() else {
            return;
        };
        let has = |command| {
            menu.entries
                .iter()
                .any(|entry| entry.command() == Some(command))
        };
        assert!(has(Command::ExplorerCopyPath));
        assert!(has(Command::ExplorerCopyRelativePath));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_keyboard_context_menu_uses_blank_items_when_empty() {
        let dir = test_dir("context-empty");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };

        app.open_context_menu_for_selection();

        let Some(menu) = app.context_menu.as_ref() else {
            return;
        };
        let has = |cmd: Command| {
            menu.entries
                .iter()
                .any(|entry| entry.command() == Some(cmd))
        };
        assert!(has(Command::ExplorerNewFile));
        assert!(has(Command::ExplorerNewFolder));
        assert!(!has(Command::SidebarActivate));
        assert!(!has(Command::ExplorerRename));
        assert!(
            menu.entries.iter().all(|entry| entry.enabled),
            "explorer menu entries stay enabled"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_menu_opens_on_the_first_enabled_entry_and_skips_disabled_on_nav() {
        let dir = test_dir("context-skip-disabled");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.context_menu = Some(ContextMenu::new(
            2,
            2,
            vec![
                ContextMenuEntry::disabled(Command::CopyPath, "no file"),
                ContextMenuEntry::enabled(Command::CopyRelativePath),
                ContextMenuEntry::disabled(Command::Quit, "blocked"),
                ContextMenuEntry::enabled(Command::ExplorerRefresh),
            ],
        ));
        let selected = |app: &App| app.context_menu.as_ref().map(|m| m.selected);
        // The initial selection lands on the first enabled row, not row 0.
        assert_eq!(selected(&app), Some(1));
        // Down skips the disabled row 2 and lands on 3; another Down stays put.
        app.dispatch(Command::ContextMenuDown);
        assert_eq!(selected(&app), Some(3));
        app.dispatch(Command::ContextMenuDown);
        assert_eq!(selected(&app), Some(3));
        // Up skips row 2 back to 1; another Up stays (row 0 is disabled).
        app.dispatch(Command::ContextMenuUp);
        assert_eq!(selected(&app), Some(1));
        app.dispatch(Command::ContextMenuUp);
        assert_eq!(selected(&app), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_repository_status_is_requested_once_and_rendered_when_non_clean() {
        let dir = test_dir("nested-repository-status");
        write_file(&dir, "nested/.git/config", b"[core]\n");
        write_file(&dir, "nested/src/lib.rs", b"pub fn example() {}\n");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;

        app.request_nested_repository_statuses();
        app.request_nested_repository_statuses();

        let requests: Vec<_> = backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(id, command)| match command {
                        SessionCommand::NestedRepositoryStatus { path } => {
                            Some((*id, path.clone()))
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(requests, vec![(RequestId(1), dir.join("nested"))]);
        assert!(app.nested_repository_badges(Instant::now()).is_empty());

        app.on_backend_event(
            Some(requests[0].0),
            SessionEvent::NestedRepositoryStatus {
                path: requests[0].1.clone(),
                summary: RepositorySummary {
                    ahead: 1,
                    behind: 2,
                    added: 3,
                    removed: 4,
                },
            },
        );
        let badges = app.nested_repository_badges(Instant::now());
        assert_eq!(badges.len(), 1);
        assert_eq!(badges[0].0, dir.join("nested"));
        assert!(badges[0].1.contains("1"));
        assert!(badges[0].1.contains("2"));
        assert!(badges[0].1.contains("+3"));
        assert!(badges[0].1.contains("-4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_repository_loading_badge_respects_the_shared_reveal_delay() {
        let dir = test_dir("nested-repository-loading");
        let nested = dir.join("nested");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.nested_repository_pending.insert(
            RequestId(7),
            (nested.clone(), Instant::now() - Duration::from_millis(250)),
        );

        let now = Instant::now();
        assert_eq!(app.nested_repository_badges(now).len(), 1);
        assert_eq!(
            app.nested_repository_next_wake(now),
            Some(Duration::from_millis(100))
        );

        app.nested_repository_pending.clear();
        app.nested_repository_status
            .insert(nested, RepositorySummary::default());
        assert!(app.nested_repository_badges(now).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
