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
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    #[test]
    fn scan_finishes_immediately_when_spellcheck_is_disabled() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        // The default settings have `spellcheck.enabled` off.
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });

        session.handle(RequestId(1), Command::ScanWorkspaceSpelling { limit: 100 });

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
    }

    #[test]
    fn scan_finishes_immediately_for_an_unsupported_spelling_locale() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let mut settings = spellcheck_settings();
        settings.spellcheck.language = "fr_FR".to_owned();
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });

        session.handle(RequestId(1), Command::ScanWorkspaceSpelling { limit: 100 });

        assert!(matches!(
            events.try_recv().map(|(_, event)| event),
            Some(Event::SpellingScanFinished {
                files_scanned: 0,
                ..
            })
        ));
    }

    #[test]
    fn scan_answers_open_documents_from_their_live_buffers() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("notes.md");
        if std::fs::write(&path, "hello world\n").is_err() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings: spellcheck_settings(),
            ..SessionConfig::default()
        });
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
        let Some(version) = session.document(doc).map(|view| view.version()) else {
            return;
        };
        // Stand in for the debounce worker: the buffer's *live* spell layer, which
        // an unsaved edit would put out of step with the file on disk.
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![spell_diagnostic(0, 6, 11, "world")],
            error: None,
        });
        while events.try_recv().is_some() {}

        session.handle(RequestId(9), Command::ScanWorkspaceSpelling { limit: 100 });

        // The seed batch is emitted synchronously, before the worker even starts.
        let seeded = events.try_recv();
        let Some((Some(RequestId(9)), Event::SpellingScanProgress { hits, .. })) = seeded else {
            unreachable!("expected a seeded progress batch, got {seeded:?}")
        };
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].path, path);
        assert_eq!(hits[0].word, "world");
        assert_eq!(
            hits[0].line_text, "hello world",
            "the hit carries its line as list context"
        );
    }

    #[test]
    fn adding_a_dictionary_word_re_runs_the_spell_layer_of_every_open_document() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        // A project write needs a git root with an existing settings file.
        if std::fs::create_dir(dir.path().join(".git")).is_err()
            || std::fs::create_dir(dir.path().join(".karet")).is_err()
            || std::fs::write(dir.path().join(".karet/setting.jsonc"), "{}\n").is_err()
        {
            return;
        }
        let path = dir.path().join("notes.md");
        if std::fs::write(&path, "the wrod ends\n").is_err() {
            return;
        }
        // Spell-checking is off, so the re-run resolves no dictionary and the
        // stale layer is dropped — the observable proof that the document was
        // rescheduled without any filesystem event arriving.
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        let Some(version) = session.document(doc).map(|view| view.version()) else {
            return;
        };
        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![spell_diagnostic(0, 4, 8, "wrod")],
            error: None,
        });
        while events.try_recv().is_some() {}

        session.handle(
            RequestId(2),
            Command::AddDictionaryWord {
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
        assert_eq!(
            republished,
            Some(Vec::new()),
            "the open document's spell layer is recomputed, not left stale"
        );
        assert!(
            session
                .config
                .settings
                .spellcheck
                .words
                .iter()
                .any(|word| word == "wrod"),
            "the word is live for the next check"
        );
    }
