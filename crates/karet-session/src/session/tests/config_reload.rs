    // Live reload through `handle_fs_event`: a change to a watched settings layer or
    // an `.editorconfig` re-resolves open documents' settings. A settings layer is
    // configuration, not workspace content, so it raises no `FsChanged`.

    /// Open `path` in `session`, returning its document once `Opened` arrives.
    fn open_for_reload(session: &mut Session, events: &mut EventRx, path: &Path) -> Option<DocumentId> {
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.to_path_buf(),
                language: None,
            },
        );
        let mut found = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::Opened { doc, .. } = event {
                found = Some(doc);
            }
        }
        found
    }

    /// Every event the session emitted since the last drain.
    fn drain_events(events: &mut EventRx) -> Vec<Event> {
        std::iter::from_fn(|| events.try_recv().map(|(_, event)| event)).collect()
    }

    /// The settings the last `DocumentSettingsChanged` for `doc` carried.
    fn settings_change(events: &[Event], doc: DocumentId) -> Option<crate::api::DocumentSettings> {
        events.iter().rev().find_map(|event| match event {
            Event::DocumentSettingsChanged { doc: changed, settings } if *changed == doc => Some(*settings),
            _ => None,
        })
    }

    #[test]
    fn a_settings_layer_change_reresolves_documents_without_fs_changed() -> TestResult {
        let dir = tempfile::tempdir()?;
        let settings_path = dir.path().join("setting.jsonc");
        let file = dir.path().join("a.txt");
        std::fs::write(&settings_path, r#"{ "editor": { "tabSize": 4 } }"#)?;
        std::fs::write(&file, "x\n")?;
        let loaded = crate::config::load::load_layer_reports(crate::config::load::ordered_layers(
            None,
            None,
            Some(settings_path.clone()),
        ));
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings: loaded.settings.clone(),
            loaded_config: loaded,
            ..SessionConfig::default()
        });
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        drain_events(&mut events);

        std::fs::write(&settings_path, r#"{ "editor": { "tabSize": 2 } }"#)?;
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![settings_path],
        });

        let emitted = drain_events(&mut events);
        let settings = settings_change(&emitted, doc);
        assert_eq!(settings.map(|s| s.tab_width), Some(2), "{emitted:?}");
        assert!(
            emitted.iter().any(|event| matches!(event, Event::ConfigChanged { .. })),
            "the new report must reach the client"
        );
        assert!(
            !emitted.iter().any(|event| matches!(event, Event::FsChanged { .. })),
            "a settings layer is not workspace content: {emitted:?}"
        );
        Ok(())
    }

    #[test]
    fn an_unchanged_settings_layer_rewrite_emits_nothing_for_documents() -> TestResult {
        let dir = tempfile::tempdir()?;
        let settings_path = dir.path().join("setting.jsonc");
        let file = dir.path().join("a.txt");
        let body = r#"{ "editor": { "tabSize": 3 } }"#;
        std::fs::write(&settings_path, body)?;
        std::fs::write(&file, "x\n")?;
        let loaded = crate::config::load::load_layer_reports(crate::config::load::ordered_layers(
            None,
            None,
            Some(settings_path.clone()),
        ));
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings: loaded.settings.clone(),
            loaded_config: loaded,
            ..SessionConfig::default()
        });
        open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        drain_events(&mut events);

        // A save that rewrites identical content changes nothing observable.
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![settings_path],
        });
        let emitted = drain_events(&mut events);
        assert!(
            !emitted.iter().any(|event| matches!(
                event,
                Event::ConfigChanged { .. } | Event::DocumentSettingsChanged { .. } | Event::FsChanged { .. }
            )),
            "{emitted:?}"
        );
        Ok(())
    }

    #[test]
    fn an_editorconfig_change_reresolves_open_documents() -> TestResult {
        let dir = tempfile::tempdir()?;
        let editorconfig = dir.path().join(".editorconfig");
        let file = dir.path().join("a.txt");
        std::fs::write(&editorconfig, "root = true\n")?;
        std::fs::write(&file, "x\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        drain_events(&mut events);

        std::fs::write(&editorconfig, "root = true\n\n[*]\nindent_style = tab\ntab_width = 3\n")?;
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![editorconfig],
        });

        let emitted = drain_events(&mut events);
        let settings = settings_change(&emitted, doc);
        assert_eq!(
            settings.map(|s| (s.insert_spaces, s.tab_width)),
            Some((false, 3)),
            "{emitted:?}"
        );
        Ok(())
    }
