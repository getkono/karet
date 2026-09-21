    use crate::config::schema::EditorOverride;
    use crate::config::schema::LanguageSelector;

    fn saved(events: &mut EventRx) -> bool {
        let mut found = false;
        while let Some((_, ev)) = events.try_recv() {
            found |= matches!(ev, Event::Saved { .. });
        }
        found
    }

    #[test]
    fn format_on_save_without_a_server_still_writes() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "fn  main() {}\n").is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events), "save must complete without a formatter");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "fn  main() {}\n"
        );
    }

    #[cfg(feature = "toml-format")]
    #[test]
    fn format_on_save_applies_the_builtin_toml_formatter() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        if std::fs::write(&path, "[package]\nname=\"x\"\n").is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[package]\nname = \"x\"\n"
        );
    }

    #[cfg(feature = "toml-format")]
    #[test]
    fn language_override_can_disable_format_on_save() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        let messy = "[package]\nname=\"x\"\n";
        if std::fs::write(&path, messy).is_err() {
            return;
        }
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        if let Some(selector) = LanguageSelector::from_language("toml") {
            settings.editor.language_overrides.insert(
                selector,
                EditorOverride {
                    format_on_save: Some(false),
                    ..EditorOverride::default()
                },
            );
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
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
        session.handle(RequestId(2), Command::Save { doc });
        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), messy);
    }

    // ---- the deferred half: a save parked on a `textDocument/formatting` reply.
    //
    // `begin_format_on_save` only parks a save when a server actually took the
    // request, which needs a live server. These drive the resolution side
    // directly instead: seed `pending_format_saves` the way a dispatched request
    // would, then deliver (or destroy) the answer it is waiting on. What is
    // under test is that every one of those endings still puts the file on disk.

    /// Open `path` in a session with `editor.formatOnSave` on.
    fn format_on_save_session(path: &std::path::Path) -> Option<(Session, DocumentId, EventRx)> {
        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            settings,
            ..SessionConfig::default()
        });
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.to_path_buf(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events)?;
        Some((session, doc, events))
    }

    /// Open a document, dirty it, and park a save on a formatting request that
    /// will never be answered normally. Returns the session, the document, the
    /// event stream, and the request id the parked save is keyed by.
    fn parked_save(path: &std::path::Path) -> Option<(Session, DocumentId, EventRx, RequestId)> {
        let (mut session, doc, mut events) = format_on_save_session(path)?;
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "edited\n".to_string(),
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
        while events.try_recv().is_some() {}
        let request = RequestId(3);
        session.pending_format_saves.insert(
            request,
            crate::session::PendingFormatSave {
                doc,
                issued_ms: 0,
            },
        );
        Some((session, doc, events, request))
    }

    /// Every warning notification the session emitted.
    fn warnings(events: &mut EventRx) -> Vec<String> {
        let mut found = Vec::new();
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Notification {
                severity: Severity::Warning,
                message,
                ..
            } = ev
            {
                found.push(message);
            }
        }
        found
    }

    /// The defect this guards: an `lsp` settings change retires the server
    /// generation, and every answer stamped with the old one is then dropped by
    /// `accepts`. A save waiting on one used to be stranded forever — the file
    /// never written, nothing answering the request, and the client's own
    /// pending-save entry never cleared.
    #[test]
    fn retiring_the_servers_writes_the_saves_that_were_waiting_on_them() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, _doc, mut events, _request)) = parked_save(&path) else {
            return;
        };

        let mut settings = crate::config::Settings::default();
        settings.editor.format_on_save = true;
        // Any `lsp` change retires the generation the parked request belongs to.
        settings.lsp.enabled = false;
        session.apply_config_report(crate::config::LoadedConfig::from_settings(settings));

        assert!(
            saved(&mut events),
            "retiring the servers must not strand the save"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "edited\n",
            "the buffer must reach disk unformatted rather than not at all"
        );
        assert!(
            session.pending_format_saves.is_empty(),
            "the parked entry must not leak"
        );
    }

    /// The same strand reached from the other side: the answer arrives after the
    /// generation moved, so `accepts` rejects it. It must still finish its save.
    #[test]
    fn an_answer_from_a_retired_generation_still_finishes_its_save() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, request)) = parked_save(&path) else {
            return;
        };
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 999, // never the live one
            request,
            doc,
            version,
            supported: true,
            edits: Vec::new(),
        });

        assert!(saved(&mut events), "a rejected answer must not eat the save");
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "edited\n");
        assert!(session.pending_format_saves.is_empty());
    }

    /// The ordinary path: the server answers in time, its edits are applied to
    /// the buffer, and the formatted text is what lands on disk.
    #[test]
    fn a_formatting_answer_is_applied_before_the_write() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, request)) = parked_save(&path) else {
            return;
        };
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            supported: true,
            edits: vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "formatted\n".to_string(),
            }],
        });

        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "formatted\n"
        );
        assert!(session.pending_format_saves.is_empty());
    }

    /// Edits computed against a version the buffer has moved past would corrupt
    /// it. They are dropped — but the save they were holding up still lands.
    #[test]
    fn a_stale_formatting_answer_writes_the_buffer_unformatted() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, request)) = parked_save(&path) else {
            return;
        };
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version: version + 7, // an answer for a buffer that no longer exists
            supported: true,
            edits: vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "formatted\n".to_string(),
            }],
        });

        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "edited\n",
            "stale edits are dropped, the save is not"
        );
        assert!(session.pending_format_saves.is_empty());
    }

    /// A server with nothing to change answers with an empty edit list. That is
    /// success, not failure: write the buffer as it stands.
    #[test]
    fn an_empty_formatting_answer_writes_the_buffer_as_it_stands() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, request)) = parked_save(&path) else {
            return;
        };
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            supported: true,
            edits: Vec::new(),
        });

        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "edited\n");
        assert!(session.pending_format_saves.is_empty());
    }

    /// Closing the document abandons the save deliberately, and says so. The
    /// file keeps whatever was last written to it.
    #[test]
    fn closing_a_document_cancels_its_pending_format_save() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, _request)) = parked_save(&path) else {
            return;
        };

        session.handle(RequestId(4), Command::CloseDocument { doc });

        assert!(
            warnings(&mut events)
                .iter()
                .any(|message| message.contains("save cancelled")),
            "an abandoned save must say so"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "original\n",
            "a cancelled save writes nothing"
        );
        assert!(session.pending_format_saves.is_empty());
    }

    /// A server may accept the request and never answer. Without a deadline of
    /// its own the save would wait out the JSON-RPC request timeout — tens of
    /// seconds of a file not being on disk, with only a spinner to show for it.
    #[test]
    fn a_formatter_that_never_answers_does_not_hold_the_file_forever() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, _doc, mut events, _request)) = parked_save(&path) else {
            return;
        };

        // Just short of the deadline the save is still the formatter's to finish.
        session.expire_format_on_save(crate::session::FORMAT_ON_SAVE_DEADLINE_MS - 1);
        assert!(!saved(&mut events), "the deadline must not fire early");
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "original\n");
        assert_eq!(session.pending_format_saves.len(), 1);

        session.expire_format_on_save(crate::session::FORMAT_ON_SAVE_DEADLINE_MS);

        assert!(saved(&mut events), "past the deadline the save must land");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "edited\n",
            "an unanswered formatter costs the formatting, not the save"
        );
        assert!(session.pending_format_saves.is_empty());
    }

    /// A server can hold a TOML document without offering to format it. The
    /// built-in taplo formatter exists for exactly that case, and the docs
    /// promise it — but it used to be skipped whenever *any* server had the
    /// file open, because nothing asked what the server could actually do.
    #[cfg(feature = "toml-format")]
    #[test]
    fn a_server_that_does_not_format_hands_toml_back_to_the_builtin() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        if std::fs::write(&path, "[package]\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events)) = format_on_save_session(&path) else {
            return;
        };
        // Dirty the buffer into something the formatter will want to change.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(1, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "name=\"x\"\n".to_string(),
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
        while events.try_recv().is_some() {}
        let request = RequestId(3);
        session.pending_format_saves.insert(
            request,
            crate::session::PendingFormatSave {
                doc,
                issued_ms: 0,
            },
        );
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            supported: false,
            edits: Vec::new(),
        });

        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[package]\nname = \"x\"\n",
            "the built-in formatter must run when the server offers nothing"
        );
    }

    /// The same answer for a language with no built-in formatter is simply a
    /// save: there is nothing to fall back to, and nothing to fail over.
    #[test]
    fn a_server_that_does_not_format_still_writes_other_languages() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("main.rs");
        if std::fs::write(&path, "original\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events, request)) = parked_save(&path) else {
            return;
        };
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            supported: false,
            edits: Vec::new(),
        });

        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "edited\n");
        assert!(session.pending_format_saves.is_empty());
    }
