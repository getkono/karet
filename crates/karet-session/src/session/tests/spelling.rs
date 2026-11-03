    // The workspace spelling scan's Command/Event contract. The walk itself is
    // covered hermetically in `crate::spell_scan`; these cover the session's half:
    // the disabled short-circuit and the live-buffer seeding.

    /// Settings with spell-checking on for a supported locale.
    fn spellcheck_settings() -> crate::config::Settings {
        let mut settings = crate::config::Settings::default();
        settings.spellcheck.enabled = true;
        settings.spellcheck.language = "en_US".to_owned();
        settings
    }

    fn spell_diagnostic(line: u32, start_col: u32, end_col: u32, word: &str) -> karet_core::Diagnostic {
        karet_core::Diagnostic {
            range: Range {
                start: LineCol::new(line, start_col),
                end: LineCol::new(line, end_col),
            },
            severity: Severity::Warning,
            message: format!("Unknown word “{word}”"),
            source: Some("karet-spell".to_owned()),
            code: Some("en_US".to_owned()),
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    #[test]
    fn scan_finishes_immediately_when_spellcheck_is_disabled() -> TestResult {
        let dir = tempfile::tempdir()?;
        // The default settings have `spellcheck.enabled` off.
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });

        session.handle(RequestId(1), WorkspaceCommand::ScanSpelling { limit: 100 });

        let finished = events.try_recv();
        assert!(
            matches!(
                finished,
                Some((
                    Some(RequestId(1)),
                    Event::SpellingScanFinished {
                        files_scanned: 0,
                        truncated: false,
                        cancelled: false,
                    }
                ))
            ),
            "a disabled scan must still finish so the client leaves its loading state: {finished:?}"
        );
        Ok(())
    }

    #[test]
    fn scan_finishes_immediately_for_an_unsupported_spelling_locale() -> TestResult {
        let dir = tempfile::tempdir()?;
        let mut settings = spellcheck_settings();
        settings.spellcheck.language = "fr_FR".to_owned();
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            loaded_config: crate::config::LoadedConfig::from_settings(settings),
            ..SessionConfig::default()
        });

        session.handle(RequestId(1), WorkspaceCommand::ScanSpelling { limit: 100 });

        assert!(matches!(
            events.try_recv().map(|(_, event)| event),
            Some(Event::SpellingScanFinished {
                files_scanned: 0,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn scan_answers_open_documents_from_their_live_buffers() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "hello world\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            loaded_config: crate::config::LoadedConfig::from_settings(spellcheck_settings()),
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|view| view.version())
            .ok_or("opened document is not registered")?;
        // Stand in for the debounce worker: the buffer's *live* spell layer, which
        // an unsaved edit would put out of step with the file on disk.
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![spell_diagnostic(0, 6, 11, "world")],
            error: None,
        });
        while events.try_recv().is_some() {}

        session.handle(RequestId(9), WorkspaceCommand::ScanSpelling { limit: 100 });

        // The seed batch is emitted synchronously, before the worker even starts.
        let seeded = events.try_recv();
        let Some((Some(RequestId(9)), Event::SpellingScanProgress { hits, .. })) = seeded else {
            return Err(format!("expected a seeded progress batch, got {seeded:?}").into());
        };
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, path);
        assert_eq!(hits[0].word, "world");
        assert_eq!(
            hits[0].line_text, "hello world",
            "the hit carries its line as list context"
        );
        Ok(())
    }

    #[test]
    fn a_changed_spell_layer_publishes_the_documents_hits() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "hello world\nthe wrld ends\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            loaded_config: crate::config::LoadedConfig::from_settings(spellcheck_settings()),
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|view| view.version())
            .ok_or("opened document is not registered")?;
        while events.try_recv().is_some() {}

        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![spell_diagnostic(1, 4, 8, "wrld")],
            error: None,
        });

        let mut published = None;
        while let Some((id, event)) = events.try_recv() {
            if let Event::SpellingUpdated { path, hits } = event {
                assert_eq!(id, None, "the document is not answering a scan");
                published = Some((path, hits));
            }
        }
        let (updated_path, hits) = published.ok_or("expected a SpellingUpdated event")?;
        assert_eq!(updated_path, path);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].word, "wrld");
        assert_eq!(hits[0].line_text, "the wrld ends");

        // Fixing the file publishes the empty layer, so a client's list can drop
        // the row rather than keeping a word the editor no longer marks.
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: Vec::new(),
            error: None,
        });
        let mut cleared = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::SpellingUpdated { hits, .. } = event {
                cleared = Some(hits);
            }
        }
        assert_eq!(cleared, Some(Vec::new()));
        Ok(())
    }

    #[test]
    fn seeded_open_document_hits_spend_the_same_budget_as_the_walk() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "the wrld ends here\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            loaded_config: crate::config::LoadedConfig::from_settings(spellcheck_settings()),
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|view| view.version())
            .ok_or("opened document is not registered")?;
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![
                spell_diagnostic(0, 4, 8, "wrld"),
                spell_diagnostic(0, 14, 18, "here"),
            ],
            error: None,
        });
        while events.try_recv().is_some() {}

        session.handle(RequestId(9), WorkspaceCommand::ScanSpelling { limit: 2 });

        let mut seeded = 0;
        let mut finished = None;
        while let Some((_, event)) = events.try_recv() {
            match event {
                Event::SpellingScanProgress { hits, .. } => seeded += hits.len(),
                Event::SpellingScanFinished { truncated, .. } => finished = Some(truncated),
                _ => {},
            }
        }
        assert_eq!(seeded, 2, "the seed filled the cap");
        assert_eq!(
            finished,
            Some(true),
            "a list cut off at the cap must say so rather than inviting the walk \
             to add another `limit` hits on top"
        );
        Ok(())
    }

    #[test]
    fn adding_a_dictionary_word_re_runs_the_spell_layer_of_every_open_document() -> TestResult {
        let dir = tempfile::tempdir()?;
        // A project write needs a git root with an existing settings file.
        std::fs::create_dir(dir.path().join(".git"))?;
        std::fs::create_dir(dir.path().join(".karet"))?;
        std::fs::write(dir.path().join(".karet/setting.jsonc"), "{}\n")?;
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "the wrod ends\n")?;
        // Spell-checking is off, so the re-run resolves no dictionary and the
        // stale layer is dropped — the observable proof that the document was
        // rescheduled without any filesystem event arriving.
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            DocumentCommand::Open {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|view| view.version())
            .ok_or("opened document is not registered")?;
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![spell_diagnostic(0, 4, 8, "wrod")],
            error: None,
        });
        while events.try_recv().is_some() {}

        session.handle(
            RequestId(2),
            WorkspaceCommand::AddDictionaryWord {
                word: "wrod".to_owned(),
                scope: crate::api::DictionaryScope::Project,
                create_project: false,
            },
        );

        let mut republished = None;
        let mut added = false;
        while let Some((_, event)) = events.try_recv() {
            match event {
                Event::DiagnosticsPublished { diagnostics, .. } => {
                    republished = Some(diagnostics);
                },
                Event::DictionaryWordAdded { .. } => added = true,
                _ => {},
            }
        }
        assert!(added, "the word was written");
        // Other producers (markdownlint) may keep their layers; only the
        // spell layer must have been recomputed away.
        let spell: Option<Vec<_>> = republished.map(|diagnostics| {
            diagnostics
                .into_iter()
                .filter(|d| d.source.as_deref() == Some("karet-spell"))
                .collect()
        });
        assert_eq!(
            spell,
            Some(Vec::new()),
            "the open document's spell layer is recomputed, not left stale"
        );
        assert!(
            session
                .config
                .settings()
                .spellcheck
                .words
                .iter()
                .any(|word| word == "wrod"),
            "the word is live for the next check"
        );
        // The report the client inspects is the same settings, not a copy the
        // write left behind.
        session.handle(RequestId(3), WorkspaceCommand::LoadedConfig);
        let report = std::iter::from_fn(|| events.try_recv())
            .find_map(|(_, event)| match event {
                Event::LoadedConfig { report } => Some(report),
                _ => None,
            })
            .ok_or("no LoadedConfig answer")?;
        assert!(
            report.settings.spellcheck.words.iter().any(|word| word == "wrod"),
            "LoadedConfig reports the adopted word: {:?}",
            report.settings.spellcheck.words
        );
        Ok(())
    }
