    /// Push a minimal empty code tab and open Find over it, for tests that only
    /// exercise the find-bar state machine (not real match content).
    fn app_with_find_open() -> App {
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
                buffer: TextBuffer::from_text(""),
                text: String::new(),
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
        app
    }

    #[test]
    fn find_bar_toggle_field_reveals_and_switches_replace() {
        let mut app = app_with_find_open();
        assert!(app.active_find().is_some_and(|f| !f.replace_visible));
        app.find_toggle_field();
        assert!(
            app.active_find()
                .is_some_and(|f| f.replace_visible && f.field == SearchField::Replace)
        );
        app.find_toggle_field();
        assert!(
            app.active_find()
                .is_some_and(|f| f.field == SearchField::Find)
        );
    }

    #[test]
    fn find_input_edits_the_active_field() {
        let mut app = app_with_find_open();
        app.find_input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.find_toggle_field(); // switch to the replace field
        app.find_input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "a" && f.replace == "b")
        );
    }

    #[test]
    fn find_toggle_option_flips_the_flags() {
        let mut app = app_with_find_open();
        app.find_toggle_option(SearchOption::Regex);
        app.find_toggle_option(SearchOption::Word);
        assert!(
            app.active_find()
                .is_some_and(|f| f.regex && !f.case_sensitive && f.whole_word)
        );
    }

    #[test]
    fn find_state_survives_esc_and_is_restored_on_reopen() {
        // Regression: closing Find (Esc) used to discard the query/toggles;
        // reopening Find on the same tab must restore them instead of starting
        // blank.
        let mut app = app_with_find_open();
        if let Some(find) = app.active_find_mut() {
            find.query = "needle".to_string();
            find.regex = true;
        }
        app.close_find();
        assert_eq!(
            app.input_context().modal,
            None,
            "closing find must hide the bar"
        );
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "needle" && f.regex),
            "the tab's find data must survive Esc"
        );

        app.open_find();
        assert_eq!(app.input_context().modal, Some(crate::keymap::Modal::Find));
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "needle" && f.regex),
            "reopening find on the same tab must restore the prior query/toggles"
        );
    }

    #[test]
    fn switching_tabs_does_not_show_a_stale_find_bar() {
        let mut app = app_with_find_open();
        app.push_tab(Tab::new(
            "u.rs",
            TabKind::Code {
                path: PathBuf::from("u.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: karet_text::TextBuffer::from_text(""),
                text: String::new(),
                highlights: karet_syntax::Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        assert_eq!(
            app.input_context().modal,
            None,
            "opening a second tab must not carry the first tab's open find bar over"
        );
        app.select_tab(0);
        assert_eq!(
            app.input_context().modal,
            None,
            "switching back must not resurrect the find bar either — only reopening it should"
        );
    }

    #[test]
    fn search_toggle_field_reveals_and_switches_replace() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        assert_eq!(app.search.field, SearchField::Find);
        // Collapse the replace field, then Tab reveals it and moves focus to it.
        app.search_toggle_replace();
        assert!(!app.search.replace_visible);
        app.search_toggle_field();
        assert!(app.search.replace_visible);
        assert_eq!(app.search.field, SearchField::Replace);
        assert!(app.search.input);
        // Tab again returns to the find field.
        app.search_toggle_field();
        assert_eq!(app.search.field, SearchField::Find);
    }

    #[test]
    fn search_edit_targets_the_active_field() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.search.field = SearchField::Find;
        app.search_edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.search.field = SearchField::Replace;
        app.search_edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(app.search.query, "a");
        assert_eq!(app.search.replace, "b");
    }

    #[test]
    fn search_replace_all_rewrites_matching_files() {
        let dir = std::env::temp_dir().join(format!("karet-replace-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("a.txt"), "needle and needle\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Search;
        app.search.query = "needle".to_string();
        app.search.case_sensitive = true;
        app.search.replace = "pin".to_string();
        app.search_replace_all();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap_or_default(),
            "pin and pin\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_option_toggle_button_click_dispatches() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Search;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        // A "regex" toggle button on row 2, columns 20..22.
        app.search_action_hits = vec![(20, 22, 2, Command::SearchToggleRegex)];
        assert!(!app.search.regex);
        app.handle_sidebar_click(20, 2, KeyModifiers::NONE);
        assert!(app.search.regex);
    }

    // --- full-stack Source-Control action tests ------------------------------
    //
    // These drive the real `Session` + `local()` backend over a temp git repo, so
    // they exercise the whole key → focus/layer → dispatch → backend actor → git2 →
    // VcsStatus → apply loop that unit tests skip.

    /// A temp directory removed on drop, so a panicking test can't leak it.
    struct TempRepo {
        path: PathBuf,
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Run `git` in `dir`, returning whether it succeeded.
    fn git(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A git repo in a fresh temp dir holding a single untracked file, or `None`
    /// when `git` is unavailable (so the test skips rather than fails).
    fn init_test_repo() -> Option<TempRepo> {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "karet-scm-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).ok()?;
        let repo = TempRepo { path };
        if !git(&repo.path, &["init", "-q"])
            || !git(&repo.path, &["config", "user.email", "test@example.com"])
            || !git(&repo.path, &["config", "user.name", "karet test"])
        {
            return None;
        }
        std::fs::write(repo.path.join("new.rs"), "fn main() {}\n").ok()?;
        Some(repo)
    }

    /// A bare key press (no modifiers).
    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Drain backend events into `app`, waiting briefly for the spawned actor.
    async fn pump(app: &mut App, events: &mut EventRx) {
        while let Ok(Some((id, ev))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
        {
            app.on_backend_event(id, ev);
        }
    }

    /// Build an app wired to a real session + local backend, focused on the SCM pane.
    fn scm_app(root: PathBuf) -> (App, EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(root, Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        (app, events)
    }

    fn code_tab_text(app: &App) -> String {
        match &app.tabs[app.active].kind {
            TabKind::Code { text, .. } => text.clone(),
            _ => panic!("expected the active tab to be a code tab"),
        }
    }

    #[tokio::test]
    async fn backspace_and_insert_apply_to_the_local_buffer_without_waiting_for_a_snapshot() {
        // Regression for "editor jumps back / skips characters on backspace": edits
        // used to only move the caret optimistically while the displayed text
        // waited on an async snapshot echo, so a fast burst of keys raced ahead of
        // what was actually applied. Every edit below is dispatched back-to-back
        // with no `pump` in between, so this fails if `submit_edit` regresses to
        // only updating the caret again.
        let dir =
            std::env::temp_dir().join(format!("karet-edit-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "ab").expect("write temp file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await; // registers the doc so submit_edit can act

        app.dispatch(Command::InsertChar('x'));
        assert_eq!(code_tab_text(&app), "xab");
        app.dispatch(Command::InsertChar('y'));
        assert_eq!(code_tab_text(&app), "xyab");
        app.dispatch(Command::DeleteBackward);
        assert_eq!(code_tab_text(&app), "xab");
        app.dispatch(Command::DeleteBackward);
        assert_eq!(
            code_tab_text(&app),
            "ab",
            "two backspaces fired before any snapshot arrives must still land on the \
             locally-applied text"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn paste_while_find_is_open_targets_the_find_query_not_the_editor() {
        // Regression: paste used to always land in the main editor buffer
        // regardless of which text field was actually focused, so pasting while
        // Find was open silently replaced the editor's content/selection instead
        // of the find query.
        let dir = std::env::temp_dir().join(format!(
            "karet-pastefind-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "hello world").expect("write temp file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await;

        app.open_find();
        assert!(app.find_open);

        app.handle_paste("needle".to_string());

        assert_eq!(
            app.active_find().map(|f| f.query.as_str()),
            Some("needle"),
            "pasted text must land in the find query"
        );
        assert_eq!(
            code_tab_text(&app),
            "hello world",
            "paste while Find is open must not touch the editor buffer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_non_utf8_file_opens_read_only_instead_of_a_silently_dead_tab() {
        let dir =
            std::env::temp_dir().join(format!("karet-nonutf8-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("bad.rs");
        // A long valid-ASCII prefix (longer than the classifier's 8 KiB head sample)
        // followed by one invalid byte: the workspace-level classifier sees only
        // clean text and opens a normal code tab, but the session's full-file strict
        // UTF-8 load then fails — this is exactly the "misses a genuinely non-UTF-8
        // file" gap this fallback exists for, not the earlier (already-handled)
        // "obviously binary within the head sample" case.
        let mut bytes = vec![b'a'; 9000];
        bytes.push(0xff);
        std::fs::write(&path, &bytes).expect("write invalid-utf8 file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await;

        assert!(
            matches!(app.tabs[app.active].kind, TabKind::Hex { .. }),
            "a non-UTF-8 file must fall back to the read-only hex view, not a dead \
             code tab with doc: None"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn scm_stage_key_stages_through_the_backend() {
        let Some(repo) = init_test_repo() else {
            return;
        };
        let (mut app, mut events) = scm_app(repo.path.clone());

        // The seeded status lists the untracked file.
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Untracked);

        // Pressing 's' in the focused SCM pane stages it, end to end.
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
    }

    #[tokio::test]
    async fn scm_stage_still_works_after_previewing_a_diff() {
        // Regression for "actions do nothing after opening a diff": browsing the
        // change list (arrow moves) previews each diff *without* stealing focus
        // from the Source-Control pane, so the staging keys stay live. (Enter is
        // the explicit "commit into the view" action and does move focus.)
        let Some(repo) = init_test_repo() else {
            return;
        };
        let (mut app, mut events) = scm_app(repo.path.clone());
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 1);

        // Arrow-browse onto the change: its diff previews, focus stays on SCM.
        app.dispatch(Command::SidebarDown);
        assert!(app.active_is_diff());
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);

        // Staging still works while the preview is up.
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
    }

    #[tokio::test]
    async fn scm_stages_a_multi_file_selection() {
        let Some(repo) = init_test_repo() else {
            return;
        };
        if std::fs::write(repo.path.join("second.rs"), b"fn second() {}\n").is_err() {
            return;
        }
        let (mut app, mut events) = scm_app(repo.path.clone());
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 2);

        // Select both changed files, then stage the whole selection at once.
        app.scm.selection.select_all();
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 2);
        assert!(
            app.scm
                .changes
                .iter()
                .all(|c| c.status == StatusKind::Added)
        );
    }

    // --- completion UI (issue #57) -----------------------------------------

    fn completion_item_labeled(label: &str, insert: &str) -> karet_core::CompletionItem {
        karet_core::CompletionItem {
            label: label.to_owned(),
            kind: karet_core::CompletionKind::Function,
            detail: None,
            documentation: None,
            insert_text: insert.to_owned(),
            edit: None,
            sort_text: None,
            deprecated: false,
        }
    }

    /// A focused editor over `text` (doc 9) wired to a recording backend, with
    /// the caret at `caret`.
    fn completion_app(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App) {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.push_tab(text_tab("main.rs", text));
        app.focus = Focus::Editor;
        let idx = app.active;
        if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
            *doc = Some(DocumentId(9));
        }
        app.tabs[idx].editor.set_carets(&[caret]);
        (backend, app)
    }

    /// The completion requests a backend received, as `(id, position)`.
    fn completion_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(id, command)| match command {
                        SessionCommand::Completion { position, .. } => Some((*id, *position)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn open_popup(app: &mut App, items: Vec<karet_core::CompletionItem>, anchor: LineCol) {
        app.completion = Some(crate::completion::CompletionUi {
            items,
            list: karet_widgets::CompletionState::default(),
            doc: DocumentId(9),
            anchor,
            last_filter: String::new(),
            mode: crate::completion::CompletionMode::Filtered,
        });
    }

    #[test]
    fn ctrl_space_requests_completions_at_the_caret() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let sent = completion_requests(&backend);
        assert_eq!(sent.len(), 1, "one Completion command");
        assert_eq!(sent[0].1, LineCol::new(0, 2));
        let pending = app.pending_completion.expect("a pending request");
        assert_eq!(pending.id, sent[0].0, "answer correlates by request id");
        assert_eq!(pending.anchor, LineCol::new(0, 0), "anchored at word start");
    }

    #[test]
    fn completion_enablement_resolves_against_the_tab_language() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor = serde_json::from_str(
            r#"{
                "completion": { "enabled": true },
                "[rust]": { "completion": { "enabled": false } }
            }"#,
        )
        .unwrap_or_default();

        app.trigger_completion(true);
        assert!(completion_requests(&backend).is_empty());

        if let TabKind::Code { language, .. } = &mut app.tabs[app.active].kind {
            *language = "Python";
        }
        app.trigger_completion(true);
        assert_eq!(completion_requests(&backend).len(), 1);
    }

    #[test]
    fn auto_trigger_fires_on_word_chars_but_the_error_gate_blocks_it() {
        let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 0));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "a typed identifier char auto-triggers"
        );

        // A syntax error intersecting the caret line suppresses auto-trigger.
        app.dismiss_completion();
        let idx = app.active;
        if let TabKind::Code { syntax_errors, .. } = &mut app.tabs[idx].kind {
            *syntax_errors = vec![(0, 0)];
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "the gate holds while the line has an outright error"
        );
    }

    #[test]
    fn manual_trigger_bypasses_the_error_gate() {
        let (backend, mut app) = completion_app("broken(\n", LineCol::new(0, 7));
        let idx = app.active;
        if let TabKind::Code { syntax_errors, .. } = &mut app.tabs[idx].kind {
            *syntax_errors = vec![(0, 3)];
        }
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "Ctrl+Space ignores the gate"
        );
    }

    #[test]
    fn trigger_characters_re_request_at_the_boundary() {
        // `.` triggers with an empty prefix …
        let (backend, mut app) = completion_app("self\n", LineCol::new(0, 4));
        app.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::empty()));
        let sent = completion_requests(&backend);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1, LineCol::new(0, 5), "requested after the dot");
        let pending = app.pending_completion.expect("pending");
        assert_eq!(
            pending.anchor,
            LineCol::new(0, 5),
            "empty prefix at a boundary"
        );

        // … a lone `:` does not, the second `:` of `::` does.
        let (backend, mut app) = completion_app("std\n", LineCol::new(0, 3));
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        assert!(
            completion_requests(&backend).is_empty(),
            "single colon is not a boundary"
        );
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        assert_eq!(completion_requests(&backend).len(), 1, "`::` re-requests");
    }

    #[test]
    fn completion_settings_disable_the_paths() {
        // enabled = false kills both manual and automatic completion.
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor.completion.enabled = false;
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(completion_requests(&backend).is_empty());

        // autoTrigger = false keeps manual completion working.
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor.completion.auto_trigger = false;
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(completion_requests(&backend).is_empty(), "no auto-trigger");
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(completion_requests(&backend).len(), 1, "manual still works");
    }

    #[test]
    fn stale_completions_are_ignored_and_fresh_ones_open_the_popup() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let (id, _) = completion_requests(&backend)[0];

        // An answer to a different (superseded) request id is dropped.
        app.on_backend_event(
            Some(RequestId(id.0 + 100)),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("stale", "stale")],
            },
        );
        assert!(
            app.completion.is_none(),
            "stale answers never open the popup"
        );
        assert!(
            app.pending_completion.is_some(),
            "still awaiting the real one"
        );

        // The matching answer opens the popup.
        app.on_backend_event(
            Some(id),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("foobar", "foobar")],
            },
        );
        let ui = app.completion.as_ref().expect("popup open");
        assert_eq!(ui.items.len(), 1);
        assert!(app.pending_completion.is_none());
    }

    #[test]
    fn a_moved_caret_drops_late_completions() {
        let (backend, mut app) = completion_app("fo\nbar\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let (id, _) = completion_requests(&backend)[0];
        // The caret leaves the anchor line before the answer arrives.
        let idx = app.active;
        app.tabs[idx].editor.set_carets(&[LineCol::new(1, 0)]);
        app.on_backend_event(
            Some(id),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("foobar", "foobar")],
            },
        );
        assert!(
            app.completion.is_none(),
            "late answers for a moved caret drop"
        );
    }

    #[test]
    fn accepting_replaces_the_typed_prefix() {
        let (_backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        open_popup(
            &mut app,
            vec![completion_item_labeled("foobar", "foobar")],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(code_tab_text(&app), "foobar\n", "the prefix was replaced");
        let idx = app.active;
        assert_eq!(app.tabs[idx].editor.cursor(), LineCol::new(0, 6));
        assert!(app.completion.is_none(), "accepting closes the popup");
    }

    #[test]
    fn popup_keys_navigate_and_escape_dismisses() {
        let (_backend, mut app) = completion_app("\n", LineCol::new(0, 0));
        open_popup(
            &mut app,
            vec![
                completion_item_labeled("alpha", "alpha"),
                completion_item_labeled("beta", "beta"),
            ],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(
            app.completion.as_ref().map(|ui| ui.list.selected),
            Some(1),
            "Down moves the selection"
        );
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
        assert_eq!(app.completion.as_ref().map(|ui| ui.list.selected), Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(app.completion.is_none(), "Esc dismisses");
    }

    #[test]
    fn backspacing_past_the_anchor_dismisses_the_popup() {
        let (_backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
        open_popup(
            &mut app,
            vec![completion_item_labeled("foo", "foo")],
            LineCol::new(0, 1),
        );
        // Deleting the char before the anchor moves the caret to (0,0) < anchor.
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert!(app.completion.is_none(), "the popup follows its anchor");
    }

    #[test]
    fn typing_keeps_the_popup_filtering_without_a_new_request() {
        let (backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
        open_popup(
            &mut app,
            vec![
                completion_item_labeled("foobar", "foobar"),
                completion_item_labeled("other", "other"),
            ],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        assert!(
            completion_requests(&backend).is_empty(),
            "word chars refilter client-side while open"
        );
        assert!(app.completion.is_some(), "the popup stays open");
        let ranked = app.completion_ranked().unwrap_or_default();
        assert_eq!(
            ranked,
            vec![0],
            "only the matching candidate survives \"fo\""
        );
    }

    #[test]
    fn the_popup_paints_near_the_caret() {
        let (_backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        open_popup(
            &mut app,
            vec![completion_item_labeled("frobnicate", "frobnicate")],
            LineCol::new(0, 0),
        );
        let painted = screen(&mut app, 80, 16).join("\n");
        assert!(
            painted.contains("frobnicate"),
            "the popup row is painted: {painted}"
        );
    }
