    // ---- the undo cap and persisted undo (#355).
    //
    // `files.undoLimit` bounds every buffer's undo stack, and
    // `files.persistentUndo` carries a saved file's history into the next
    // session through the swap directory, but only onto unchanged content.

    type UndoTestResult = Result<(), Box<dyn std::error::Error>>;

    /// A session over `swapdir` with `files` tweaked by `configure`.
    fn undo_session(
        swapdir: &std::path::Path,
        configure: impl FnOnce(&mut crate::config::schema::Files),
    ) -> (Session, EventRx) {
        let mut settings = crate::config::Settings::default();
        configure(&mut settings.files);
        let (session, events, _snaps) = Session::new(SessionConfig {
            settings,
            swap_dir: Some(swapdir.to_path_buf()),
            ..SessionConfig::default()
        });
        (session, events)
    }

    fn open_in(session: &mut Session, events: &mut EventRx, path: &Path) -> Option<DocumentId> {
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.to_path_buf(),
                language: None,
            },
        );
        opened_doc(events)
    }

    /// Append `text` at the start of `doc` as its own undo step.
    fn edit_start(session: &mut Session, doc: DocumentId, text: &str) {
        let version = session.document(doc).map_or(0, |d| d.version());
        let change = Change::new(
            version,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: text.to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
    }

    fn doc_text(session: &Session, doc: DocumentId) -> String {
        session
            .document(doc)
            .map(|d| d.buffer().text())
            .unwrap_or_default()
    }

    fn save_and_close(session: &mut Session, doc: DocumentId) {
        session.handle(
            RequestId(3),
            Command::Save {
                doc,
                cause: SaveCause::Manual,
            },
        );
        session.handle(RequestId(4), Command::CloseDocument { doc });
    }

    fn undo_files(swapdir: &Path) -> usize {
        std::fs::read_dir(swapdir).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "karet-undo"))
                .count()
        })
    }

    #[test]
    fn undo_survives_a_restart_onto_the_saved_file() -> UndoTestResult {
        let dir = tempfile::tempdir()?;
        let swapdir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "base\n")?;

        let (mut first, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut first, &mut events, &path).ok_or("not opened")?;
        edit_start(&mut first, doc, "one ");
        edit_start(&mut first, doc, "two ");
        save_and_close(&mut first, doc);
        assert_eq!(std::fs::read_to_string(&path)?, "two one base\n");
        assert_eq!(undo_files(swapdir.path()), 1);

        let (mut second, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut second, &mut events, &path).ok_or("not reopened")?;
        assert!(
            second.document(doc).is_some_and(|d| !d.buffer().is_dirty()),
            "restoring history leaves the reopened file clean"
        );
        second.handle(RequestId(5), Command::Undo { doc });
        assert_eq!(doc_text(&second, doc), "one base\n");
        second.handle(RequestId(6), Command::Undo { doc });
        assert_eq!(doc_text(&second, doc), "base\n");
        second.handle(RequestId(7), Command::Redo { doc });
        assert_eq!(doc_text(&second, doc), "one base\n");
        Ok(())
    }

    #[test]
    fn a_file_changed_since_its_history_was_kept_gets_none() -> UndoTestResult {
        let dir = tempfile::tempdir()?;
        let swapdir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "base\n")?;

        let (mut first, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut first, &mut events, &path).ok_or("not opened")?;
        edit_start(&mut first, doc, "one ");
        save_and_close(&mut first, doc);
        // Edited by something else between the two sessions.
        std::fs::write(&path, "rewritten elsewhere\n")?;

        let (mut second, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut second, &mut events, &path).ok_or("not reopened")?;
        second.handle(RequestId(5), Command::Undo { doc });
        assert_eq!(
            doc_text(&second, doc),
            "rewritten elsewhere\n",
            "history for other content must not replay"
        );
        assert_eq!(undo_files(swapdir.path()), 0, "the stale history is deleted");
        Ok(())
    }

    #[test]
    fn without_persistent_undo_no_history_is_kept_and_old_history_goes() -> UndoTestResult {
        let dir = tempfile::tempdir()?;
        let swapdir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "base\n")?;

        let (mut off, mut events) = undo_session(swapdir.path(), |_| {});
        let doc = open_in(&mut off, &mut events, &path).ok_or("not opened")?;
        edit_start(&mut off, doc, "one ");
        save_and_close(&mut off, doc);
        assert_eq!(undo_files(swapdir.path()), 0, "persistence is opt-in");

        let (mut on, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut on, &mut events, &path).ok_or("not opened")?;
        edit_start(&mut on, doc, "two ");
        save_and_close(&mut on, doc);
        assert_eq!(undo_files(swapdir.path()), 1);

        // Switched back off: the next save deletes what was kept.
        let (mut off, mut events) = undo_session(swapdir.path(), |_| {});
        let doc = open_in(&mut off, &mut events, &path).ok_or("not opened")?;
        off.handle(RequestId(5), Command::Undo { doc });
        assert_eq!(doc_text(&off, doc), "two one base\n", "nothing is restored when off");
        save_and_close(&mut off, doc);
        assert_eq!(undo_files(swapdir.path()), 0);
        Ok(())
    }

    #[test]
    fn closing_dirty_keeps_the_history_of_the_last_save() -> UndoTestResult {
        let dir = tempfile::tempdir()?;
        let swapdir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "base\n")?;

        let (mut first, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut first, &mut events, &path).ok_or("not opened")?;
        edit_start(&mut first, doc, "one ");
        first.handle(
            RequestId(3),
            Command::Save {
                doc,
                cause: SaveCause::Manual,
            },
        );
        // An unsaved edit, discarded by closing.
        edit_start(&mut first, doc, "lost ");
        first.handle(RequestId(4), Command::CloseDocument { doc });

        let (mut second, mut events) = undo_session(swapdir.path(), |f| f.persistent_undo = true);
        let doc = open_in(&mut second, &mut events, &path).ok_or("not reopened")?;
        second.handle(RequestId(5), Command::Undo { doc });
        assert_eq!(doc_text(&second, doc), "base\n");
        Ok(())
    }

    #[test]
    fn the_undo_limit_caps_open_buffers_and_follows_a_settings_change() -> UndoTestResult {
        let dir = tempfile::tempdir()?;
        let swapdir = tempfile::tempdir()?;
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "base\n")?;

        let (mut session, mut events) = undo_session(swapdir.path(), |f| f.undo_limit = 2);
        let doc = open_in(&mut session, &mut events, &path).ok_or("not opened")?;
        for word in ["a ", "b ", "c "] {
            edit_start(&mut session, doc, word);
        }
        let undo_len = |session: &Session| session.document(doc).map_or(0, |d| d.buffer().undo_len());
        assert_eq!(undo_len(&session), 2, "the oldest step was forgotten");

        let mut settings = session.config.settings.clone();
        settings.files.undo_limit = 1;
        session.apply_config_report(crate::config::LoadedConfig::from_settings(settings));
        assert_eq!(undo_len(&session), 1, "a lowered cap applies to open buffers");
        session.handle(RequestId(5), Command::Undo { doc });
        session.handle(RequestId(6), Command::Undo { doc });
        assert_eq!(doc_text(&session, doc), "b a base\n");
        Ok(())
    }
