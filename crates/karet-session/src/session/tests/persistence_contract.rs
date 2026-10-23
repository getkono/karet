    // Crash recovery, closing, the manifest-hint guards, and the project
    // dictionary's confirmation step, all driven through `Session::handle`
    // rather than by reaching into the session's fields.

    /// A session whose swap directory already holds one swap (written by
    /// another session) for `original`, recording `unsaved`.
    fn session_with_a_left_swap(
        swapdir: &Path,
        original: &Path,
        unsaved: &str,
    ) -> Option<(Session, EventRx)> {
        SwapStore::with_dir(swapdir.to_path_buf(), 9)
            .write(original, unsaved, None, None, 1)
            .ok()?;
        let (session, events, _snaps) = Session::new(SessionConfig {
            swap_dir: Some(swapdir.to_path_buf()),
            ..SessionConfig::default()
        });
        Some((session, events))
    }

    #[test]
    fn recover_swaps_reopens_the_backed_up_buffer_as_unsaved() {
        let (Some((_dir, path)), Ok(swapdir)) =
            (write_temp("r.rs", "on disk\n"), tempfile::tempdir())
        else {
            return;
        };
        let Some((mut session, mut events)) =
            session_with_a_left_swap(swapdir.path(), &path, "recovered!\n")
        else {
            return;
        };
        while events.try_recv().is_some() {} // the SwapsFound announcement

        session.handle(RequestId(4), Command::RecoverSwaps);

        let mut opened = None;
        while let Some((id, event)) = events.try_recv() {
            if let Event::Opened { doc, .. } = event {
                opened = Some((id, doc));
            }
        }
        let Some((id, doc)) = opened else {
            unreachable!("recovery must open the document")
        };
        assert_eq!(id, Some(RequestId(4)), "the open answers the recover request");
        let view = session.document(doc);
        assert_eq!(
            view.as_ref().map(|v| v.buffer().text()).as_deref(),
            Some("recovered!\n")
        );
        assert_eq!(
            view.map(|v| v.buffer().is_dirty()),
            Some(true),
            "recovered content is unsaved"
        );
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some("on disk\n"),
            "recovery never writes the file itself"
        );
        assert!(scan(swapdir.path()).is_empty(), "the swap is consumed");

        // Recovery is one-shot: a second request finds nothing left to restore.
        session.handle(RequestId(5), Command::RecoverSwaps);
        assert!(opened_nothing(&mut events));
    }

    /// Whether no queued event opened a document.
    fn opened_nothing(events: &mut EventRx) -> bool {
        std::iter::from_fn(|| events.try_recv())
            .all(|(_, event)| !matches!(event, Event::Opened { .. }))
    }

    #[test]
    fn discard_swaps_deletes_them_without_opening_anything() {
        let (Some((_dir, path)), Ok(swapdir)) =
            (write_temp("d.rs", "on disk\n"), tempfile::tempdir())
        else {
            return;
        };
        let Some((mut session, mut events)) =
            session_with_a_left_swap(swapdir.path(), &path, "throw me away\n")
        else {
            return;
        };
        assert_eq!(scan(swapdir.path()).len(), 1);
        while events.try_recv().is_some() {}

        session.handle(RequestId(1), Command::DiscardSwaps);
        session.handle(RequestId(2), Command::RecoverSwaps);

        assert!(scan(swapdir.path()).is_empty(), "the declined swap is gone");
        assert!(
            opened_nothing(&mut events),
            "a discarded swap can no longer be recovered"
        );
        assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), Some("on disk\n"));
    }

    /// A document opened by two views closes only when the last one does, and
    /// only that close is answered with `Closed`.
    #[test]
    fn close_answers_closed_only_when_the_last_view_lets_go() {
        let Some((_dir, path)) = write_temp("c.rs", "fn c() {}\n") else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let open = || Command::OpenDocument {
            path: path.clone(),
            language: None,
        };
        session.handle(RequestId(1), open());
        session.handle(RequestId(2), open());
        let Some(doc) = opened_doc(&mut events) else {
            unreachable!("the document never opened")
        };

        let closed = |events: &mut EventRx| {
            std::iter::from_fn(|| events.try_recv())
                .filter_map(|(id, event)| match event {
                    Event::Closed { doc } => Some((id, doc)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        session.handle(RequestId(3), Command::CloseDocument { doc });
        assert!(closed(&mut events).is_empty(), "one view still holds it");
        assert!(session.document(doc).is_some());

        session.handle(RequestId(4), Command::CloseDocument { doc });
        assert_eq!(closed(&mut events), vec![(Some(RequestId(4)), doc)]);
        assert!(session.document(doc).is_none(), "the document left the store");

        // Closing what is already gone is a no-op, not a second `Closed`.
        session.handle(RequestId(5), Command::CloseDocument { doc });
        assert!(closed(&mut events).is_empty());
    }

    /// The hint worker is network-backed, so every guard in front of it must
    /// hold: nothing spawns for a disabled setting, a file that is not a
    /// `Cargo.toml`, or a document that is not open.
    #[cfg(feature = "deps")]
    #[test]
    fn manifest_hints_spawn_nothing_unless_every_guard_passes() {
        let (Some((_one, manifest)), Some((_two, other))) = (
            write_temp("Cargo.toml", "[package]\nname = \"x\"\n"),
            write_temp("notes.toml", "[package]\nname = \"x\"\n"),
        ) else {
            return;
        };
        let open = |session: &mut Session, events: &mut EventRx, path: &Path| {
            session.handle(
                RequestId(1),
                Command::OpenDocument {
                    path: path.to_path_buf(),
                    language: None,
                },
            );
            opened_doc(events)
        };

        let mut settings = crate::config::Settings::default();
        settings.deps.enabled = false;
        let (mut disabled, mut events, _snaps) = Session::new(SessionConfig {
            settings,
            ..SessionConfig::default()
        });
        let Some(doc) = open(&mut disabled, &mut events, &manifest) else {
            unreachable!("Cargo.toml never opened")
        };
        disabled.handle(RequestId(2), Command::RefreshManifestHints { doc });
        assert!(disabled.manifest_hints_worker.is_none(), "deps.enabled is off");

        let (mut enabled, mut events, _snaps) = Session::new(SessionConfig::default());
        let Some(doc) = open(&mut enabled, &mut events, &other) else {
            unreachable!("notes.toml never opened")
        };
        enabled.handle(RequestId(2), Command::RefreshManifestHints { doc });
        assert!(enabled.manifest_hints_worker.is_none(), "not a Cargo.toml");

        enabled.handle(
            RequestId(3),
            Command::RefreshManifestHints {
                doc: DocumentId(u64::MAX),
            },
        );
        assert!(enabled.manifest_hints_worker.is_none(), "no such document");
        assert!(
            std::iter::from_fn(|| events.try_recv())
                .all(|(_, e)| !matches!(e, Event::ManifestHints { .. })),
            "a guarded refresh answers nothing"
        );
    }

    /// Adding a project word must not silently create `.karet/setting.jsonc`:
    /// the first attempt asks, and only the confirmed retry writes the file.
    #[test]
    fn a_project_word_asks_before_creating_the_project_settings_file() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        if std::fs::create_dir(dir.path().join(".git")).is_err() {
            return;
        }
        let settings_file = dir.path().join(".karet").join("setting.jsonc");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        let add = |create_project| Command::AddDictionaryWord {
            word: "karetish".to_owned(),
            scope: crate::api::DictionaryScope::Project,
            create_project,
        };
        while events.try_recv().is_some() {}

        session.handle(RequestId(1), add(false));
        let asked = std::iter::from_fn(|| events.try_recv()).find_map(|(id, event)| match event {
            Event::ProjectSettingsCreationRequired { word, path } => Some((id, word, path)),
            _ => None,
        });
        assert_eq!(
            asked,
            Some((Some(RequestId(1)), "karetish".to_owned(), settings_file.clone()))
        );
        assert!(!settings_file.exists(), "nothing is created before confirmation");

        session.handle(RequestId(2), add(true));
        let added = std::iter::from_fn(|| events.try_recv()).find_map(|(id, event)| match event {
            Event::DictionaryWordAdded { word, path } => Some((id, word, path)),
            _ => None,
        });
        assert_eq!(
            added,
            Some((Some(RequestId(2)), "karetish".to_owned(), settings_file.clone()))
        );
        assert!(
            std::fs::read_to_string(&settings_file)
                .unwrap_or_default()
                .contains("karetish")
        );
    }
