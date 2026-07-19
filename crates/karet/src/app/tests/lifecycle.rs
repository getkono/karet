    #[test]
    fn quit_with_unsaved_changes_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::Quit);
        assert_eq!(
            app.pending_close,
            Some(CloseRequest::Quit),
            "unsaved changes arm the quit prompt"
        );
        assert!(!app.should_quit);
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::CloseConfirm)
        );

        // Discarding exits.
        app.dispatch(Command::CloseConfirmDiscard);
        assert!(app.pending_close.is_none());
        assert!(app.should_quit);
    }

    #[test]
    fn quit_without_unsaved_changes_exits_immediately() {
        let mut app = app();
        app.dispatch(Command::Quit);
        assert!(app.pending_close.is_none());
        assert!(app.should_quit);
    }

    #[test]
    fn quit_prompt_disabled_by_confirm_on_exit_setting() {
        let mut app = app();
        app.settings.files.confirm_on_exit = false;
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::Quit);
        assert!(
            app.should_quit,
            "confirmOnExit=false quits without prompting"
        );
    }

    #[test]
    fn quit_save_all_with_nothing_dirty_exits() {
        let mut app = app();
        app.pending_close = Some(CloseRequest::Quit);
        app.dispatch(Command::CloseConfirmSave);
        assert!(app.should_quit);
        assert!(app.saving_close.is_none(), "no saves in flight");
    }

    /// Push a dirty code tab backed by `doc`, returning its stable view id. The tab
    /// becomes the focused pane's active tab.
    fn dirty_doc_tab(app: &mut App, name: &str, doc: u64) -> ViewId {
        app.push_tab(text_tab(name, "x"));
        let idx = app.active;
        if let TabKind::Code { doc: d, .. } = &mut app.tabs[idx].kind {
            *d = Some(DocumentId(doc));
        }
        app.tabs[idx].dirty = true;
        app.tabs[idx].view
    }

    /// The documents a backend was asked to save, in order.
    fn saved_docs(backend: &RecordingBackend) -> Vec<DocumentId> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(_, command)| match command {
                        SessionCommand::Save { doc } => Some(*doc),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn close_tab_with_unsaved_changes_arms_the_prompt_and_does_not_close() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        // The close is deferred behind the confirmation, and the tab is untouched.
        assert!(matches!(app.pending_close, Some(CloseRequest::Tab { .. })));
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));
        assert!(app.tabs[0].dirty);
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::CloseConfirm)
        );
    }

    #[test]
    fn close_tab_confirm_discard_closes_and_discards() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        app.dispatch(Command::CloseConfirmDiscard);
        assert!(app.pending_close.is_none());
        // The last tab collapses to a Welcome tab; the dirty buffer is discarded.
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_tab_unbound_key_cancels_and_keeps_the_tab() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        // Any key that is not s/d aborts (the default), leaving the tab open.
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert!(app.pending_close.is_none());
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));
        assert_eq!(app.status.as_deref(), Some("close cancelled"));
    }

    #[test]
    fn close_tab_save_parks_request_then_closes_when_saves_drain() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        dirty_doc_tab(&mut app, "t.rs", 7);
        app.dispatch(Command::CloseTab);

        app.dispatch(Command::CloseConfirmSave);
        // The request is parked mid-save; exactly the at-risk doc is saved, and the
        // tab stays open until the save answers.
        assert!(matches!(app.saving_close, Some(CloseRequest::Tab { .. })));
        assert_eq!(saved_docs(&backend), vec![DocumentId(7)]);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));

        // The save drains → the parked close runs.
        let save_id = *app
            .pending_saves
            .keys()
            .next()
            .expect("a save is in flight");
        app.on_backend_event(Some(save_id), SessionEvent::Saved { doc: DocumentId(7) });
        assert!(app.saving_close.is_none());
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_other_tabs_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "keep.rs", 1);
        dirty_doc_tab(&mut app, "other.rs", 2);
        // Keep the first tab active; the dirty second tab would be dropped.
        app.active = 0;
        app.dispatch(Command::CloseOtherTabs);
        assert_eq!(app.pending_close, Some(CloseRequest::OtherTabs));
        assert_eq!(app.tabs.len(), 2, "nothing closes while the prompt is up");
    }

    #[test]
    fn close_tabs_to_right_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "left.rs", 1);
        dirty_doc_tab(&mut app, "right.rs", 2);
        app.active = 0;
        app.dispatch(Command::CloseTabsToRight);
        assert_eq!(app.pending_close, Some(CloseRequest::TabsToRight));
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn close_all_tabs_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "a.rs", 1);
        dirty_doc_tab(&mut app, "b.rs", 2);
        app.dispatch(Command::CloseAllTabs);
        assert_eq!(app.pending_close, Some(CloseRequest::AllTabs));
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn clean_tab_closes_without_prompting() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(1));
        }
        // Not dirty → close runs immediately.
        app.dispatch(Command::CloseTab);
        assert!(app.pending_close.is_none());
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_tab_does_not_prompt_when_doc_open_in_another_tab() {
        let mut app = app();
        // Two tabs of the same dirty document; closing one leaves the other.
        let keep = dirty_doc_tab(&mut app, "dup.rs", 5);
        let drop = dirty_doc_tab(&mut app, "dup.rs", 5);
        assert_ne!(keep, drop);
        app.dispatch(Command::CloseTab); // closes the active (second) view
        assert!(
            app.pending_close.is_none(),
            "the document survives in the first tab, so no data is lost"
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].view, keep);
    }

    #[test]
    fn close_tab_does_not_prompt_when_doc_open_in_another_pane() {
        let mut app = app();
        dirty_doc_tab(&mut app, "shared.rs", 9);
        // Split: the duplicate (same doc) becomes the focused pane; the dirty original
        // moves into a stored pane and keeps the document referenced.
        app.split_focused(SplitDir::Right);
        app.dispatch(Command::CloseTab);
        assert!(
            app.pending_close.is_none(),
            "the dirty document still lives in the other pane"
        );
    }

    #[test]
    fn close_save_targets_only_the_at_risk_documents() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        // Two independent dirty docs; only the one being dropped should be saved.
        dirty_doc_tab(&mut app, "keep.rs", 1);
        let drop = dirty_doc_tab(&mut app, "drop.rs", 2);
        app.guarded_close(CloseRequest::Tab { view: drop });
        app.dispatch(Command::CloseConfirmSave);
        assert_eq!(
            saved_docs(&backend),
            vec![DocumentId(2)],
            "only the at-risk document is saved, not every dirty document"
        );
    }

    #[test]
    fn close_tab_save_revalidates_index_after_drain() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        let _a = dirty_doc_tab(&mut app, "a.rs", 1); // scaffold at index 0, cleaned below
        app.tabs[0].dirty = false;
        let target = dirty_doc_tab(&mut app, "target.rs", 2); // index 1
        let c = dirty_doc_tab(&mut app, "c.rs", 3);
        app.tabs[2].dirty = false;

        app.guarded_close(CloseRequest::Tab { view: target });
        app.dispatch(Command::CloseConfirmSave);
        assert!(matches!(app.saving_close, Some(CloseRequest::Tab { .. })));

        // A tab list mutation before the save drains shifts `target` from index 1 to 0.
        app.tabs.remove(0);

        let save_id = *app
            .pending_saves
            .keys()
            .next()
            .expect("a save is in flight");
        app.on_backend_event(Some(save_id), SessionEvent::Saved { doc: DocumentId(2) });

        // The view-id lookup closes `target` (not whatever now sits at the old index).
        let views: Vec<ViewId> = app.tabs.iter().map(|t| t.view).collect();
        assert!(!views.contains(&target), "the intended tab was closed");
        assert!(views.contains(&c), "the other tab is untouched");
    }

    #[test]
    fn non_code_tab_never_prompts_even_with_other_dirty_docs() {
        let mut app = app();
        dirty_doc_tab(&mut app, "dirty.rs", 1); // a dirty doc lives elsewhere
        app.push_tab(Tab::welcome()); // a non-code tab, now active
        app.dispatch(Command::CloseTab);
        assert!(
            app.pending_close.is_none(),
            "closing a doc-less tab risks no data"
        );
        // The dirty code tab is still open.
        assert!(app.all_tabs().any(|t| t.dirty));
    }

    #[test]
    fn recover_swaps_opens_a_tab_for_each_backed_up_file() {
        let path = std::env::temp_dir().join(format!("karet-recover-{}.rs", std::process::id()));
        if std::fs::write(&path, "fn main() {}\n").is_err() {
            return;
        }
        let mut app = app();
        app.pending_swaps = Some(vec![SwapInfo {
            original: path.clone(),
            updated_unix_ms: 0,
            conflict: false,
        }]);
        app.dispatch(Command::RecoverSwaps);
        assert!(app.pending_swaps.is_none());
        assert!(
            app.all_tabs().any(|t| t.path().is_some_and(|p| p == path)),
            "recovery opens a tab for the backed-up file"
        );
    }

    #[test]
    fn swaps_found_arms_the_recovery_prompt() {
        let mut app = app();
        app.on_backend_event(
            None,
            SessionEvent::SwapsFound {
                swaps: vec![SwapInfo {
                    original: PathBuf::from("/work/a.rs"),
                    updated_unix_ms: 0,
                    conflict: false,
                }],
            },
        );
        assert!(app.pending_swaps.is_some());
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::SwapRecover)
        );
    }

