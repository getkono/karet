    /// A whole-buffer insertion at the start of the document (base version 0).
    fn insert_change(text: &str) -> Change {
        Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: text.to_string(),
            }],
        )
    }

    #[test]
    fn backup_tick_writes_a_swap_for_a_dirty_doc_and_save_removes_it() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let mut settings = crate::config::Settings::default();
        settings.files.backup_interval = 0; // any dirty doc is immediately due
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: Vec::new(),
            settings,
            swap_dir: None,
            ..SessionConfig::default()
        });
        // Redirect swaps to a temp directory instead of the real data dir.
        session.swaps = Some(SwapStore::with_dir(swapdir.path().to_path_buf(), 1));

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change: insert_change("x"),
                cause: EditCause::Replace,
            },
        );

        // No swap until the tick decides the doc is due.
        assert!(scan(swapdir.path()).is_empty());
        session.backup_tick();
        assert_eq!(scan(swapdir.path()).len(), 1, "dirty doc backed up");

        // A successful save clears the swap.
        session.handle(RequestId(3), Command::Save { doc });
        assert!(scan(swapdir.path()).is_empty(), "save removes the swap");
    }

    #[test]
    fn recover_swaps_restores_a_dirty_buffer() {
        let Some((_dir, path)) = write_temp("r.rs", "on disk\n") else {
            return;
        };
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 9);
        // A swap left by a previous session holds unsaved content.
        if store.write(&path, "recovered!\n", None, None, 1).is_err() {
            return;
        }

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.swaps = Some(SwapStore::with_dir(swapdir.path().to_path_buf(), 9));
        session.pending_swaps = scan(swapdir.path());
        assert_eq!(session.pending_swaps.len(), 1);

        session.recover_swaps(RequestId(1));
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        let Some(document) = session.store.docs.get(&doc) else {
            return;
        };
        assert_eq!(document.buffer.text(), "recovered!\n");
        assert!(document.buffer.is_dirty(), "recovered content is unsaved");
        // The swap is consumed once recovered.
        assert!(scan(swapdir.path()).is_empty());
    }

    #[test]
    fn loaded_config_command_returns_in_memory_report() {
        let mut settings = crate::config::Settings::default();
        settings.editor.tab_size = 2;
        let report = crate::config::LoadedConfig::from_settings(settings.clone());
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
            loaded_config: report,
            ..SessionConfig::default()
        });

        session.handle(RequestId(42), Command::LoadedConfig);
        let received = events.try_recv();
        assert!(
            matches!(
                received,
                Some((Some(RequestId(42)), Event::LoadedConfig { .. }))
            ),
            "loaded config event should answer request, got {received:?}"
        );
        let Some((_, Event::LoadedConfig { report })) = received else {
            return;
        };
        assert_eq!(report.settings.editor.tab_size, 2);
    }

    #[test]
    fn applying_live_config_updates_the_snapshot_and_emits_the_contract_event() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let mut settings = crate::config::Settings::default();
        settings.editor.tab_size = 7;
        let report = crate::config::LoadedConfig::from_settings(settings);

        session.apply_config_report(report);

        assert_eq!(session.config.settings.editor.tab_size, 7);
        let received = events.try_recv();
        assert!(
            matches!(
                &received,
                Some((None, Event::ConfigChanged { report }))
                    if report.settings.editor.tab_size == 7
            ),
            "live reload should emit its active report, got {received:?}"
        );
    }

    #[test]
    fn new_session_announces_swaps_left_in_its_swap_dir() {
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 5);
        if store
            .write(Path::new("/work/x.rs"), "unsaved\n", None, None, 1)
            .is_err()
        {
            return;
        }
        // A session pointed at that swap dir scans it on construction and announces.
        let (_session, mut events, _snaps) = Session::new(SessionConfig {
            roots: Vec::new(),
            settings: crate::config::Settings::default(),
            swap_dir: Some(swapdir.path().to_path_buf()),
            ..SessionConfig::default()
        });
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::SwapsFound { swaps } = ev {
                found = Some(swaps);
            }
        }
        assert!(found.is_some(), "startup announces recoverable swaps");
        if let Some(swaps) = found {
            assert_eq!(swaps.len(), 1);
            assert_eq!(swaps[0].original, PathBuf::from("/work/x.rs"));
        }
    }
