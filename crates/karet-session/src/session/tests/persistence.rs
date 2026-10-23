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
    fn backup_tick_writes_a_swap_for_a_dirty_doc_and_save_removes_it() -> TestResult {
        let (_dir, path) =
            write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let swapdir = tempfile::tempdir()?;
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
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
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
        session.handle(RequestId(3), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });
        assert!(scan(swapdir.path()).is_empty(), "save removes the swap");
        Ok(())
    }

    #[test]
    fn recover_swaps_restores_a_dirty_buffer() -> TestResult {
        let (_dir, path) = write_temp("r.rs", "on disk\n").ok_or("temp file not written")?;
        let swapdir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 9);
        // A swap left by a previous session holds unsaved content.
        store.write(&path, "recovered!\n", None, None, 1)?;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.swaps = Some(SwapStore::with_dir(swapdir.path().to_path_buf(), 9));
        session.pending_swaps = scan(swapdir.path());
        assert_eq!(session.pending_swaps.len(), 1);

        session.recover_swaps(RequestId(1));
        let doc = opened_doc(&mut events).ok_or("recovery reported no Opened event")?;
        let document = session
            .store
            .docs
            .get(&doc)
            .ok_or("recovered document is not registered")?;
        assert_eq!(document.buffer.text(), "recovered!\n");
        assert!(document.buffer.is_dirty(), "recovered content is unsaved");
        // The swap is consumed once recovered.
        assert!(scan(swapdir.path()).is_empty());
        Ok(())
    }

    #[test]
    fn loaded_config_command_returns_in_memory_report() -> TestResult {
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
        let Some((Some(RequestId(42)), Event::LoadedConfig { report })) = received else {
            return Err(
                format!("loaded config event should answer request, got {received:?}").into(),
            );
        };
        assert_eq!(report.settings.editor.tab_size, 2);
        Ok(())
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
    fn new_session_announces_swaps_left_in_its_swap_dir() -> TestResult {
        let swapdir = tempfile::tempdir()?;
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 5);
        store.write(Path::new("/work/x.rs"), "unsaved\n", None, None, 1)?;
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
        let swaps = found.ok_or("startup announces recoverable swaps")?;
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].original, PathBuf::from("/work/x.rs"));
        Ok(())
    }
