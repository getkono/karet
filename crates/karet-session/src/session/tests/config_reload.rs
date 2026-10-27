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

    #[test]
    fn files_encoding_decides_the_byte_order_mark_on_save() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".editorconfig"), "root = true\n")?;
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "\u{feff}x\n")?;
        let mut settings = crate::config::Settings::default();
        settings.files.encoding = crate::config::schema::Encoding::Utf8;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            loaded_config: crate::config::LoadedConfig::from_settings(settings),
            ..SessionConfig::default()
        });
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;

        session.handle(
            RequestId(2),
            DocumentCommand::Save {
                doc,
                cause: SaveCause::Manual,
            },
        );

        assert_eq!(std::fs::read(&file)?, b"x\n", "`utf-8` drops the file's BOM");
        Ok(())
    }

    /// A `Cargo.lock` document resolves its settings under the `[toml]` override,
    /// the selector its file type carries, not its display name. Falsified by
    /// resolving the document's settings against `"Cargo lockfile"` (#617).
    #[test]
    fn a_cargo_lock_document_takes_the_toml_override() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".editorconfig"), "root = true\n")?;
        let file = dir.path().join("Cargo.lock");
        std::fs::write(&file, "version = 4\n")?;
        let mut settings = crate::config::Settings::default();
        settings.editor.tab_size = 4;
        let selector = crate::config::schema::LanguageSelector::from_language("toml")
            .ok_or("no toml selector")?;
        settings.editor.language_overrides.insert(
            selector,
            crate::config::schema::EditorOverride {
                tab_size: Some(2),
                ..crate::config::schema::EditorOverride::default()
            },
        );
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            loaded_config: crate::config::LoadedConfig::from_settings(settings),
            ..SessionConfig::default()
        });
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        let doc = session.store.docs.get(&doc).ok_or("opened document not stored")?;
        assert_eq!(doc.file_type.name(), "Cargo lockfile");
        assert_eq!(doc.language_selector, Some("toml"));
        assert_eq!(doc.settings.indent_size, 2);
        assert_eq!(doc.settings.tab_width, 2);
        Ok(())
    }

    /// Global `tabSize` 4 with a `[selector]` patch setting it to `tab_size`.
    fn tab_size_override(selector: &str, tab_size: u8) -> Option<crate::config::Settings> {
        let mut settings = crate::config::Settings::default();
        settings.editor.tab_size = 4;
        settings.editor.language_overrides.insert(
            crate::config::schema::LanguageSelector::from_language(selector)?,
            crate::config::schema::EditorOverride {
                tab_size: Some(tab_size),
                ..crate::config::schema::EditorOverride::default()
            },
        );
        Some(settings)
    }

    /// A rename re-types the document, so the new language's override applies to
    /// the settings and syntax work that read `file_type`. Falsified by a
    /// `retarget` that leaves the file type stale, keeping the old language's
    /// override (or none).
    #[test]
    fn a_rename_retypes_the_document_for_its_settings() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".editorconfig"), "root = true\n")?;
        let file = dir.path().join("main.rs");
        std::fs::write(&file, "x = 1\n")?;
        let settings = tab_size_override("python", 2).ok_or("no python selector")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            loaded_config: crate::config::LoadedConfig::from_settings(settings),
            ..SessionConfig::default()
        });
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        let renamed = dir.path().join("main.py");
        session.handle(
            RequestId(2),
            DocumentCommand::Retarget {
                doc,
                path: renamed,
            },
        );
        let stored = session.store.docs.get(&doc).ok_or("document not stored")?;
        assert_eq!(stored.file_type.config_selector(), Some("python"));
        assert_eq!(stored.language_selector, Some("python"));
        assert_eq!(stored.settings.indent_size, 2);
        let resolved = session.config.settings().editor.for_language(Some(stored.file_type));
        assert_eq!(resolved.tab_size(), 2);
        Ok(())
    }

    /// A CBOR document recognized by content alone (the self-describe tag, no
    /// `.cbor` extension) is typed CBOR, as the client's tab is, so `[cbor]`
    /// reaches its session-side settings; a rename keeps it CBOR. Falsified by
    /// typing it from the path, which finds no language at all.
    #[cfg(feature = "cbor")]
    #[test]
    fn a_cbor_document_recognized_by_content_takes_the_cbor_override() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".editorconfig"), "root = true\n")?;
        let file = dir.path().join("payload");
        // The self-describe tag 55799 (0xd9d9f7), then the array [1, 2].
        std::fs::write(&file, [0xd9u8, 0xd9, 0xf7, 0x82, 0x01, 0x02])?;
        let settings = tab_size_override("cbor", 2).ok_or("no cbor selector")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            loaded_config: crate::config::LoadedConfig::from_settings(settings),
            ..SessionConfig::default()
        });
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        let stored = session.store.docs.get(&doc).ok_or("document not stored")?;
        assert_eq!(stored.language_selector, Some("cbor"));
        assert_eq!(stored.settings.indent_size, 2);

        let renamed = dir.path().join("payload.json");
        session.handle(
            RequestId(2),
            DocumentCommand::Retarget {
                doc,
                path: renamed,
            },
        );
        let stored = session.store.docs.get(&doc).ok_or("document not stored")?;
        assert_eq!(stored.language_selector, Some("cbor"), "a rename keeps it CBOR");
        assert_eq!(stored.settings.indent_size, 2);
        Ok(())
    }

    /// A text document renamed to a `.cbor` path is still text (the session saves
    /// it as text), so it keeps the language it had, as the client's tab does.
    /// Falsified by re-typing it from the new path alone.
    #[test]
    fn a_text_document_renamed_to_cbor_keeps_its_language() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("notes.toml");
        std::fs::write(&file, "a = 1\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let doc = open_for_reload(&mut session, &mut events, &file).ok_or("no Opened event")?;
        session.handle(
            RequestId(2),
            DocumentCommand::Retarget {
                doc,
                path: dir.path().join("notes.cbor"),
            },
        );
        let stored = session.store.docs.get(&doc).ok_or("document not stored")?;
        assert_eq!(stored.language_selector, Some("toml"));
        assert_eq!(stored.file_type.config_selector(), Some("toml"));
        Ok(())
    }
