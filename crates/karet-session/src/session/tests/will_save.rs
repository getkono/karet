    // ---- a parked save's will-save edits (#379).
    //
    // Seeded by hand, as the format-on-save tests above are: every ending of
    // the wait, which a live server cannot be made to produce on cue.

    /// A will-save answer to the save `request` parks, at `version`.
    fn will_save_answer(
        (request, doc, version): (RequestId, DocumentId, u64),
        generation: u64,
        then_format: bool,
    ) -> crate::lsp::LspUpdate {
        crate::lsp::LspUpdate::Editing {
            generation,
            answer: crate::lsp::EditingAnswer {
                request,
                doc,
                version,
                reply: crate::lsp::EditingReply::WillSave {
                    edits: vec![TextEdit {
                        range: Range {
                            start: LineCol::new(0, 0),
                            end: LineCol::new(0, 0),
                        },
                        new_text: "// saved\n".to_string(),
                    }],
                    then_format,
                },
            },
        }
    }

    /// With no server left to format after them, the will-save edits are
    /// still written, and the save completes.
    #[test]
    fn will_save_edits_are_written_when_no_formatter_is_left() {
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

        session.apply_lsp_update(will_save_answer((request, doc, version), 0, true));

        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "// saved\nedited\n"
        );
        assert!(session.pending_format_saves.is_empty());
    }

    /// Edits for text that has since changed would land in the wrong place:
    /// the save is written without them.
    #[test]
    fn will_save_edits_for_a_superseded_version_are_dropped() {
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

        let stale = version.saturating_sub(1);
        session.apply_lsp_update(will_save_answer((request, doc, stale), 0, false));

        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "edited\n");
    }

    /// A save that already gave up (cancelled, closed, or written by the
    /// deadline) is not waiting: a late answer changes nothing.
    #[test]
    fn will_save_edits_for_a_save_no_longer_waiting_change_nothing() {
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
        session.pending_format_saves.clear();

        session.apply_lsp_update(will_save_answer((request, doc, version), 0, false));

        assert!(!saved(&mut events));
        assert_eq!(
            session.document(doc).map(|d| d.version()),
            Some(version),
            "the buffer must not take edits for a save nobody waits on"
        );
    }

    /// An answer fenced out by reconfiguration still finishes its save,
    /// without the edits a retired server made.
    #[test]
    fn a_retired_will_save_answer_still_finishes_its_save() {
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

        session.apply_lsp_update(will_save_answer((request, doc, version), 999, true));

        assert!(saved(&mut events));
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), "edited\n");
        assert!(session.pending_format_saves.is_empty());
    }

    /// A save that does not format, answered by a server task with no
    /// connection, is written as is: the built-in formatter is not a
    /// fallback for a save the user asked not to format.
    #[cfg(feature = "toml-format")]
    #[test]
    fn an_unformatted_save_never_falls_back_on_the_builtin_formatter() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("Cargo.toml");
        if std::fs::write(&path, "[package]\nname=\"x\"\n").is_err() {
            return;
        }
        let Some((mut session, doc, mut events)) = format_on_save_session(&path) else {
            return;
        };
        let request = RequestId(3);
        let pending = crate::session::PendingFormatSave { doc, issued_ms: 0 };
        session.pending_format_saves.insert(request, pending);
        let version = session.document(doc).map(|d| d.version()).unwrap_or(0);
        // Falsifier: the built-in formatter does rewrite this file.
        assert!(session.builtin_format_edits(doc).is_some_and(|edits| !edits.is_empty()));

        session.apply_lsp_update(crate::lsp::LspUpdate::Formatting {
            generation: 0,
            request,
            doc,
            version,
            formatted: false,
            edits: Vec::new(),
            scope: crate::lsp::FormattingScope::SaveUnformatted,
        });

        assert!(saved(&mut events));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "[package]\nname=\"x\"\n"
        );
    }
