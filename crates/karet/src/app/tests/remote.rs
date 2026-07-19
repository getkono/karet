    /// A committed git repo (one tracked `new.rs`) with `origin` set to `remote_url`,
    /// or `None` when `git` is unavailable (the test then skips).
    fn repo_with_remote(remote_url: &str) -> Option<TempRepo> {
        let repo = init_test_repo()?;
        if !git(&repo.path, &["add", "."])
            || !git(&repo.path, &["commit", "-q", "-m", "init"])
            || !git(&repo.path, &["remote", "add", "origin", remote_url])
        {
            return None;
        }
        Some(repo)
    }

    #[test]
    fn pane_context_menu_lists_file_actions_and_disables_links_outside_a_repo() {
        let dir = test_dir("pane-menu-norepo");
        write_file(&dir, "a.rs", b"x\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(dir.join("a.rs").to_string_lossy().as_ref()));

        app.open_pane_context_menu(3, 3);

        let Some(menu) = app.context_menu.as_ref() else {
            panic!("a file-backed tab opens a pane menu");
        };
        let commands: Vec<Command> = menu.entries.iter().map(|e| e.command).collect();
        assert_eq!(
            commands,
            vec![
                Command::CopyPath,
                Command::CopyRelativePath,
                Command::RevealActiveInExplorer,
                Command::CopyRemoteFileUrl,
                Command::OpenChangesWithPrevious,
                Command::OpenChangesWithRevision,
                Command::OpenChangesWithBranch,
                Command::CopyGithubPermalink,
                Command::CopyGithubHeadLink,
            ]
        );
        assert!(menu.entries[..3].iter().all(|e| e.enabled));
        for entry in &menu.entries[3..] {
            assert!(
                !entry.enabled,
                "{:?} is disabled outside a repo",
                entry.command
            );
            assert_eq!(entry.note.as_deref(), Some("not in a git repository"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_context_menu_does_not_open_for_a_pathless_tab() {
        let dir = test_dir("pane-menu-welcome");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));

        app.open_pane_context_menu(3, 3);

        assert!(app.context_menu.is_none(), "a pathless tab opens no menu");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn right_click_opens_the_pane_menu_from_the_tab_strip_and_the_content_area() {
        let dir = test_dir("pane-menu-mouse");
        write_file(&dir, "a.rs", b"x\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(dir.join("a.rs").to_string_lossy().as_ref()));
        let mut frame = content_frame(&app, Rect::new(0, 1, 40, 10));
        frame.tabstrip_rect = Rect::new(0, 0, 40, 1);
        frame.tab_hits = vec![TabHit {
            start: 0,
            end: 12,
            close: 11,
        }];
        app.pane_frames = vec![frame];
        let right = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // Over the tab in the strip: selects it and opens the menu.
        app.handle_mouse(right(4, 0));
        assert!(app.context_menu.is_some(), "tab-strip right-click opens");
        app.context_menu = None;

        // In the content area: opens for the pane's active tab.
        app.handle_mouse(right(5, 5));
        assert!(app.context_menu.is_some(), "content right-click opens");
        app.context_menu = None;

        // On the strip's empty tail (past the tab): consumed, no menu.
        app.handle_mouse(right(20, 0));
        assert!(app.context_menu.is_none(), "strip tail opens nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_menu_enables_github_links_for_a_tracked_file_on_github() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("new.rs"));
        assert!(
            entries.iter().all(|e| e.enabled),
            "github + tracked enables every row: {entries:?}"
        );
    }

    #[test]
    fn pane_menu_disables_github_links_for_a_gitlab_remote_with_a_note() {
        let Some(repo) = repo_with_remote("https://gitlab.com/owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("new.rs"));
        let by_cmd = |cmd: Command| entries.iter().find(|e| e.command == cmd);
        // The generic remote URL still works on GitLab…
        assert!(by_cmd(Command::CopyRemoteFileUrl).is_some_and(|e| e.enabled));
        // …while both GitHub links are disabled and name the detected forge.
        for cmd in [Command::CopyGithubPermalink, Command::CopyGithubHeadLink] {
            let Some(entry) = by_cmd(cmd) else {
                panic!("{cmd:?} is listed");
            };
            assert!(!entry.enabled);
            let note = entry.note.as_deref().unwrap_or_default();
            assert!(
                note.contains("GitLab") && note.contains("github.com"),
                "note names the forge: {note}"
            );
        }
    }

    #[test]
    fn pane_menu_disables_links_for_an_untracked_file() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        std::fs::write(repo.path.join("untracked.rs"), "y\n").unwrap_or_default();
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("untracked.rs"));
        for cmd in [
            Command::CopyRemoteFileUrl,
            Command::CopyGithubPermalink,
            Command::CopyGithubHeadLink,
            Command::OpenChangesWithPrevious,
            Command::OpenChangesWithRevision,
            Command::OpenChangesWithBranch,
        ] {
            let Some(entry) = entries.iter().find(|e| e.command == cmd) else {
                panic!("{cmd:?} is listed");
            };
            assert!(!entry.enabled, "{cmd:?} is disabled for an untracked file");
            assert!(
                entry
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("not tracked"),
                "note explains the untracked state"
            );
        }
    }

    #[test]
    fn remote_facts_reads_the_repository_state() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let Ok(facts) = app.remote_facts(&repo.path.join("new.rs")) else {
            panic!("facts resolve inside a repo with an origin");
        };
        assert_eq!(facts.remote.kind, crate::remote::ForgeKind::GitHub);
        assert_eq!(facts.rel_path, PathBuf::from("new.rs"));
        assert!(facts.tracked);
        assert!(facts.head.is_some());
        assert!(facts.branch.is_some());
    }

    #[test]
    fn copy_github_permalink_reports_success_on_a_github_repo() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(
            repo.path.join("new.rs").to_string_lossy().as_ref(),
        ));
        app.dispatch(Command::CopyGithubPermalink);
        assert_eq!(app.status.as_deref(), Some("copied GitHub permalink"));
    }

    /// A git repo whose `new.rs` is committed (no remote), or `None` when `git`
    /// is unavailable (the test then skips).
    fn committed_repo() -> Option<TempRepo> {
        let repo = init_test_repo()?;
        if !git(&repo.path, &["add", "."]) || !git(&repo.path, &["commit", "-q", "-m", "init"]) {
            return None;
        }
        Some(repo)
    }

    /// A code tab over `path` whose live buffer holds `text`.
    fn code_tab_with_text(path: &Path, text: &str) -> Tab {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        Tab::new(
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string(),
            TabKind::Code {
                path: path.to_path_buf(),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text(text),
                text: text.to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        )
    }

    #[test]
    fn open_changes_with_previous_diffs_head_against_the_live_buffer() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        // The live buffer differs from both HEAD and the (unchanged) disk file,
        // proving the working side comes from the buffer, not disk.
        app.push_tab(code_tab_with_text(&path, "fn main() { edited }\n"));

        app.dispatch(Command::OpenChangesWithPrevious);

        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("open changes opens a diff tab, got none");
        };
        assert_eq!(title, "new.rs (HEAD \u{2194} working)");
        assert_eq!(
            file.change.old, "fn main() {}\n",
            "old side is HEAD content"
        );
        assert_eq!(
            file.change.new, "fn main() { edited }\n",
            "new side is the live buffer"
        );
    }

    #[test]
    fn open_changes_with_revision_picks_from_the_file_history() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        // A second commit changes the file, so its history has two entries.
        std::fs::write(&path, "fn main() { v1 }\n").unwrap_or_default();
        if !git(&repo.path, &["commit", "-qam", "v1"]) {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "working\n"));

        app.dispatch(Command::OpenChangesWithRevision);
        let Some(overlay) = app.overlay.as_ref() else {
            panic!("the revision picker opens");
        };
        assert_eq!(overlay.title(), "Open Changes: With Revision");
        let rows: Vec<String> = overlay.rows().iter().map(ToString::to_string).collect();
        assert_eq!(rows.len(), 2, "both commits touch the file: {rows:?}");
        assert!(rows[0].contains("v1"), "newest first: {rows:?}");

        // Choose the older commit (the initial content).
        app.dispatch(Command::OverlayDown);
        app.dispatch(Command::OverlayAccept);

        assert!(app.overlay.is_none(), "accept closes the picker");
        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("accepting a revision opens a diff tab");
        };
        assert_eq!(
            file.change.old, "fn main() {}\n",
            "old side is the picked revision's content"
        );
        assert_eq!(file.change.new, "working\n");
        assert!(
            title.contains("\u{2194} working"),
            "title names the comparison: {title}"
        );
    }

    #[test]
    fn open_changes_with_branch_diffs_against_the_branch_tip() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        // A `feature` branch changes the file; we come back to the default branch.
        if !git(&repo.path, &["checkout", "-q", "-b", "feature"]) {
            return;
        }
        std::fs::write(&path, "fn main() { feature }\n").unwrap_or_default();
        if !git(&repo.path, &["commit", "-qam", "feature change"])
            || !git(&repo.path, &["checkout", "-q", "-"])
        {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "working\n"));

        app.dispatch(Command::OpenChangesWithBranch);
        let Some(overlay) = app.overlay.as_ref() else {
            panic!("the branch picker opens");
        };
        assert_eq!(overlay.title(), "Open Changes: With Branch");
        let rows: Vec<String> = overlay.rows().iter().map(ToString::to_string).collect();
        assert!(
            rows.iter().any(|r| r.ends_with("(current)")),
            "the checked-out branch is marked: {rows:?}"
        );
        // Branches are sorted by name, so `feature` is first regardless of whether
        // the default branch is `main` or `master`.
        assert_eq!(rows[0], "feature");
        app.dispatch(Command::OverlayAccept);

        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("accepting a branch opens a diff tab");
        };
        assert_eq!(title, "new.rs (feature \u{2194} working)");
        assert_eq!(
            file.change.old, "fn main() { feature }\n",
            "old side is the branch tip's content"
        );
        assert_eq!(file.change.new, "working\n");
    }

    #[test]
    fn open_changes_reports_a_file_absent_at_the_revision() {
        let Some(repo) = committed_repo() else {
            return;
        };
        // `other.rs` only exists in the second commit, so HEAD~1 has no blob for it.
        let path = repo.path.join("other.rs");
        std::fs::write(&path, "x\n").unwrap_or_default();
        if !git(&repo.path, &["add", "."]) || !git(&repo.path, &["commit", "-qm", "add other"]) {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "x\n"));

        let before = app.tabs.len();
        app.open_changes_with("HEAD~1", "HEAD~1");

        assert_eq!(app.tabs.len(), before, "no diff tab is opened");
        assert_eq!(
            app.status.as_deref(),
            Some("open changes: file does not exist at HEAD~1")
        );
    }

    #[test]
    fn context_menu_refuses_a_disabled_entry_and_surfaces_its_note() {
        let dir = test_dir("context-disabled-accept");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.context_menu = Some(ContextMenu::new(
            2,
            2,
            vec![
                ContextMenuEntry::disabled(Command::ExplorerNewFile, "not available here"),
                ContextMenuEntry::enabled(Command::ExplorerRefresh),
            ],
        ));
        // Force the selection onto the disabled row (as a mouse click would).
        if let Some(menu) = app.context_menu.as_mut() {
            menu.selected = 0;
        }
        app.dispatch(Command::ContextMenuAccept);
        // The command did not run, the menu stays open, and the note is surfaced.
        assert!(!app.explorer.is_editing(), "disabled command must not run");
        assert!(app.context_menu.is_some(), "menu stays open on refusal");
        assert_eq!(app.status.as_deref(), Some("not available here"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_paste_rejects_directory_into_its_descendant() {
        let dir = test_dir("copy-into-self");
        write_file(&dir, "src/child/file.txt", b"child");
        write_file(&dir, "src/marker.txt", b"marker");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("src"));
        app.dispatch(Command::Copy);
        app.explorer.expand(&dir.join("src"));
        app.explorer.ensure_built(&dir);
        select_explorer_path(&mut app, &dir.join("src/child"));
        app.dispatch(Command::Paste);

        assert!(!dir.join("src/child/src").exists());
        assert_eq!(app.status.as_deref(), Some("paste failed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_rename_refreshes_vcs_status() {
        let dir = test_dir("rename-refresh");
        write_file(&dir, "old.txt", b"old");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("old.txt"));
        app.explorer_begin_rename();
        for c in "new".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();

        assert!(dir.join("new.txt").exists());
        assert_eq!(refresh_count(&backend), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_rename_retargets_open_code_tabs() {
        let dir = test_dir("rename-retarget");
        let old = dir.join("old.txt");
        let new = dir.join("new.txt");
        write_file(&dir, "old.txt", b"old");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;
        let mut tab = code_tab("old.txt");
        if let TabKind::Code { path, doc, .. } = &mut tab.kind {
            *path = old.clone();
            *doc = Some(DocumentId(42));
        }
        app.push_tab(tab);

        select_explorer_path(&mut app, &old);
        app.explorer_begin_rename();
        for c in "new".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();

        assert!(
            app.tabs
                .iter()
                .any(|tab| tab.title == "new.txt" && tab.path() == Some(new.as_path()))
        );
        assert_eq!(retarget_commands(&backend), vec![(DocumentId(42), new)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_paste_refreshes_vcs_status_after_success() {
        let dir = test_dir("paste-refresh");
        write_file(&dir, "a.txt", b"a");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;

        app.dispatch(Command::Copy);
        app.dispatch(Command::Paste);

        assert!(dir.join("a copy.txt").exists());
        assert_eq!(refresh_count(&backend), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

