    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::TextEdit;

    use super::*;
    use crate::api::Command;

    fn write_temp(name: &str, body: &str) -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join(name);
        std::fs::write(&path, body).ok()?;
        Some((dir, path))
    }

    fn opened_doc(events: &mut EventRx) -> Option<DocumentId> {
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Opened { doc, .. } = ev {
                found = Some(doc);
            }
        }
        found
    }

    #[test]
    fn session_constructs_with_streams() {
        let (_session, _events, _snaps) = Session::new(SessionConfig::default());
    }

    #[test]
    fn session_new_does_not_walk_large_tree_on_caller_thread() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        for i in 0..1200 {
            let path = dir.path().join(format!("src/{i}/nested"));
            if std::fs::create_dir_all(path).is_err() {
                return;
            }
        }

        let started = std::time::Instant::now();
        let (_session, _events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..SessionConfig::default()
        });

        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "Session::new must not synchronously enumerate large trees"
        );
    }

    #[test]
    fn opening_a_non_utf8_file_reports_not_utf8_instead_of_a_generic_error() {
        let Some((_dir, path)) = write_temp("bad.rs", "") else {
            return;
        };
        if std::fs::write(&path, [0x66, 0x6e, 0xff, 0x00]).is_err() {
            return;
        }
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let mut not_utf8_path = None;
        let mut opened = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::NotUtf8 { path } => not_utf8_path = Some(path),
                Event::Opened { .. } => opened = true,
                _ => {},
            }
        }
        assert_eq!(not_utf8_path, Some(path));
        assert!(!opened, "a non-UTF-8 file must not report as Opened");
        assert!(
            snaps.try_recv().is_none(),
            "no document was registered, so no snapshot should follow"
        );
    }

    #[test]
    fn spell_results_publish_only_for_the_current_document_version() {
        let Some((_dir, path)) = write_temp("notes.md", "hello wrld\n") else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
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
        let diagnostic = karet_core::Diagnostic {
            range: Range {
                start: LineCol::new(0, 6),
                end: LineCol::new(0, 10),
            },
            severity: Severity::Warning,
            message: "Unknown word “wrld”".to_owned(),
            source: Some("karet-spell".to_owned()),
            code: Some("en_US".to_owned()),
            tags: Vec::new(),
            related: Vec::new(),
        };

        session.apply_spell_result(SpellResult {
            doc,
            version: version.saturating_add(1),
            diagnostics: vec![diagnostic.clone()],
            error: None,
        });
        assert!(
            events.try_recv().is_none(),
            "a result for a different version is stale"
        );

        session.apply_spell_result(SpellResult {
            doc,
            version,
            diagnostics: vec![diagnostic.clone()],
            error: None,
        });
        let published = events.try_recv().map(|(_, event)| event);
        assert!(matches!(
            published,
            Some(Event::DiagnosticsPublished {
                doc: published_doc,
                diagnostics,
            }) if published_doc == doc && diagnostics == vec![diagnostic]
        ));
    }

    #[test]
    fn unsupported_spell_language_is_reported_instead_of_silently_disabling() {
        let mut settings = crate::config::Settings::default();
        settings.spellcheck.enabled = true;
        settings.spellcheck.language = "fr_FR".to_owned();

        let (resolved, error) = resolve_document_settings(
            std::path::Path::new("notes.md"),
            Some("Markdown"),
            &settings,
        );

        assert_eq!(resolved.spelling_language, None);
        assert!(error.is_some_and(|message| {
            message.contains("en_US") && message.contains("en_GB")
        }));
    }

    #[test]
    fn editorconfig_settings_are_reported_and_applied_on_save() {
        let Some((dir, path)) = write_temp("main.rs", "let x = 1;  \n") else {
            return;
        };
        if std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.rs]\nindent_style = tab\nindent_size = 6\n\
             tab_width = 4\nend_of_line = crlf\ncharset = utf-8-bom\n\
             trim_trailing_whitespace = true\ninsert_final_newline = true\n",
        )
        .is_err()
        {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let opened = events.try_recv().and_then(|(_, event)| match event {
            Event::Opened { doc, .. } => Some(doc),
            _ => None,
        });
        let settings = events.try_recv().and_then(|(_, event)| match event {
            Event::DocumentSettingsChanged { settings, .. } => Some(settings),
            _ => None,
        });
        let (Some(doc), Some(settings)) = (opened, settings) else {
            return;
        };
        assert_eq!(settings.indent_size, 6);
        assert_eq!(settings.tab_width, 4);
        assert!(!settings.insert_spaces);

        session.handle(RequestId(2), Command::Save { doc });

        let bytes = std::fs::read(&path).unwrap_or_default();
        assert_eq!(bytes, b"\xef\xbb\xbflet x = 1;\r\n");
        assert!(session.document(doc).is_some_and(|view| {
            view.buffer().text() == "let x = 1;\n" && !view.buffer().is_dirty()
        }));
    }

    #[test]
    fn editorconfig_re_resolves_for_open_documents() {
        let Some((dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let config = dir.path().join(".editorconfig");
        if std::fs::write(&config, "root=true\n[*.rs]\nindent_size=2\n").is_err() {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events);
        let Some(doc) = doc else { return };
        if std::fs::write(&config, "root=true\n[*.rs]\nindent_size=8\n").is_err() {
            return;
        }

        session.refresh_document_settings(&[doc]);

        let mut changed = None;
        while let Some((_, event)) = events.try_recv() {
            if let Event::DocumentSettingsChanged { doc: changed_doc, settings } = event
                && changed_doc == doc
            {
                changed = Some(settings);
            }
        }
        assert_eq!(changed.map(|settings| settings.indent_size), Some(8));
    }

    #[test]
    fn final_newline_does_not_make_an_empty_file_nonempty() {
        let Some((dir, path)) = write_temp("empty.txt", "") else {
            return;
        };
        if std::fs::write(
            dir.path().join(".editorconfig"),
            "root=true\n[*]\ninsert_final_newline=true\n",
        )
        .is_err()
        {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
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

        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"");
    }

    #[test]
    fn disabled_final_newline_does_not_remove_an_existing_newline() {
        let Some((dir, path)) = write_temp("note.txt", "kept\n") else {
            return;
        };
        if std::fs::write(
            dir.path().join(".editorconfig"),
            "root=true\n[*]\ninsert_final_newline=false\n",
        )
        .is_err()
        {
            return;
        }
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
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

        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"kept\n");
    }

    #[test]
    fn open_apply_save_undo_flow() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events);
        assert!(doc.is_some(), "expected an Opened event");
        let Some(doc) = doc else { return };
        assert!(snaps.try_recv().is_some(), "open publishes a snapshot");

        // Insert "!" after the body's closing brace position (line 0, col 11).
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 12),
                    end: LineCol::new(0, 12),
                },
                new_text: "\nfn x() {}".to_string(),
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
        // Applied event with version 1.
        let mut applied_version = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Applied { version, .. } = ev {
                applied_version = Some(version);
            }
        }
        assert_eq!(applied_version, Some(1));
        // A fresh snapshot reflects the edit.
        let mut last_snap = None;
        while let Some((_, s)) = snaps.try_recv() {
            last_snap = Some(s);
        }
        assert!(last_snap.is_some(), "expected a snapshot after apply");
        let Some(snap) = last_snap else { return };
        assert_eq!(snap.version, 1);
        assert!(snap.dirty);
        // "fn main() {}\n" + inserted "\nfn x() {}" → three lines.
        assert_eq!(snap.buffer.line_count(), 3);

        // Save: the file on disk reflects the edit and the doc goes clean.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Saved { .. } = ev {
                saved = true;
            }
        }
        assert!(saved);
        assert!(
            session
                .document(doc)
                .is_some_and(|v| !v.buffer().is_dirty())
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap_or_default()
                .contains("fn x()")
        );
        let mut clean_snapshot = false;
        while let Some((_, snap)) = snaps.try_recv() {
            clean_snapshot = clean_snapshot || !snap.dirty;
        }
        assert!(clean_snapshot, "save should publish a clean snapshot");

        // Undo restores the original content ("fn main() {}\n" → two lines).
        session.handle(RequestId(4), Command::Undo { doc });
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().line_count() == 2)
        );
    }

    #[test]
    fn retarget_document_updates_the_save_destination() {
        let Some((_dir, path)) = write_temp("old.txt", "old\n") else {
            return;
        };
        let new_path = path.with_file_name("new.txt");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

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
        if std::fs::rename(&path, &new_path).is_err() {
            return;
        }

        session.handle(
            RequestId(2),
            Command::RetargetDocument {
                doc,
                path: new_path.clone(),
            },
        );
        let mut retargeted = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Retargeted { doc, path } = ev {
                retargeted = Some((doc, path));
            }
        }
        assert_eq!(retargeted, Some((doc, new_path.clone())));

        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 3),
                    end: LineCol::new(0, 3),
                },
                new_text: " moved".to_string(),
            }],
        );
        session.handle(
            RequestId(3),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}
        session.handle(RequestId(4), Command::Save { doc });

        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap_or_default(),
            "old moved\n"
        );
        assert!(!path.exists());
    }

    #[test]
    fn save_refuses_to_overwrite_a_file_changed_on_disk_since_it_was_read() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
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
        let _ = snaps.try_recv();

        // Dirty the in-memory buffer without touching the file yet.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "// edited\n".to_string(),
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
        let _ = snaps.try_recv();

        // Someone else changes the file on disk before we save.
        if std::fs::write(&path, "fn main() { /* external */ }\n").is_err() {
            return;
        }

        session.handle(RequestId(3), Command::Save { doc });
        let mut conflict = false;
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::ExternalConflict { .. } => conflict = true,
                Event::Saved { .. } => saved = true,
                _ => {},
            }
        }
        assert!(
            conflict,
            "save must report an ExternalConflict, not just fail silently"
        );
        assert!(!saved, "a conflicting save must not report as Saved");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "fn main() { /* external */ }\n",
            "a refused save must not overwrite the externally-changed file"
        );
        // The in-memory edit is still there (unsaved, not discarded).
        assert!(session.document(doc).is_some_and(|v| v.buffer().is_dirty()));
    }

    #[test]
    fn apply_against_a_stale_version_resyncs_instead_of_dropping_silently() {
        // Regression: a client whose local speculative version has diverged from
        // the backend's (e.g. after a dropped/duplicate message) used to have its
        // edit silently discarded with no way to recover — every subsequent edit
        // on that document would then fail the same way forever. It must instead
        // be told and get a fresh snapshot back so it can resync.
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
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
        let _ = snaps.try_recv(); // drain the open snapshot

        // Base the change on a version that doesn't exist yet (the real base is 0).
        let change = Change::new(
            7,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "!".to_string(),
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

        let mut notified = false;
        let mut applied = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Notification { .. } => notified = true,
                Event::Applied { .. } => applied = true,
                _ => {},
            }
        }
        assert!(notified, "a stale-version conflict must notify the client");
        assert!(!applied, "the rejected edit must not report as Applied");
        assert!(
            snaps.try_recv().is_some(),
            "the client must still get a fresh snapshot to resync from, not be left stuck"
        );
        // The document itself is untouched by the rejected edit.
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().text() == "fn main() {}\n")
        );
    }

    #[test]
    fn undo_redo_snapshot_carries_caret_but_edits_do_not() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
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

        // Helper: drain the snapshot stream and return the most recent snapshot.
        fn drain(snaps: &mut SnapshotRx) -> Option<std::sync::Arc<DocSnapshot>> {
            let mut last = None;
            while let Some((_, s)) = snaps.try_recv() {
                last = Some(s);
            }
            last
        }
        let _ = drain(&mut snaps); // discard the open snapshot

        // An ordinary edit publishes a snapshot with no caret (the UI owns the caret).
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(1, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "fn x() {}\n".to_string(),
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
        assert_eq!(
            drain(&mut snaps).and_then(|s| s.cursor.clone()),
            None,
            "an ordinary edit must not carry a caret"
        );

        // Undo publishes a snapshot that carries the caret to jump to.
        session.handle(RequestId(3), Command::Undo { doc });
        assert!(
            drain(&mut snaps).is_some_and(|s| s.cursor.is_some()),
            "undo must carry a caret so the editor jumps to the change"
        );

        // Redo (which records no cursor) still carries a derived caret at the edit.
        session.handle(RequestId(4), Command::Redo { doc });
        assert!(
            drain(&mut snaps).is_some_and(|s| s.cursor.is_some()),
            "redo must carry a caret derived from the re-applied edit"
        );
    }

    #[test]
    fn cbor_opens_decoded_and_save_reencodes() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("data.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let Ok(bytes) = karet_cbor::encode(&original) else {
            return;
        };
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }

        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
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
        // The buffer holds decoded diagnostic notation, not the raw CBOR bytes.
        let text = session.document(doc).map(|v| v.buffer().text());
        assert_eq!(text.as_deref(), Some("[\n  1,\n  2\n]"));
        while snaps.try_recv().is_some() {}

        // Edit the "2" (line 2, col 2) to "3".
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(2, 2),
                    end: LineCol::new(2, 3),
                },
                new_text: "3".to_string(),
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

        // Save re-encodes to CBOR; the file on disk decodes to the edited value.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Saved { .. } = ev {
                saved = true;
            }
        }
        assert!(saved, "a cbor save should succeed");
        let disk = std::fs::read(&path).unwrap_or_default();
        let expected = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(3),
        ]);
        assert_eq!(karet_cbor::decode(&disk).ok(), Some(expected));
    }

    #[test]
    fn cbor_save_of_malformed_edit_leaves_file_untouched() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("bad.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let Ok(bytes) = karet_cbor::encode(&original) else {
            return;
        };
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
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

        // Delete the closing ']' (line 3, col 0), making the text un-parseable.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(3, 0),
                    end: LineCol::new(3, 1),
                },
                new_text: String::new(),
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

        // Save fails to encode; no Saved event, and the file is unchanged.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        let mut failed = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Saved { .. } => saved = true,
                Event::Notification {
                    severity: Severity::Error,
                    ..
                } => failed = true,
                _ => {},
            }
        }
        assert!(!saved, "a malformed cbor buffer must not save");
        assert!(
            failed,
            "the failure should surface as an error notification"
        );
        assert_eq!(
            std::fs::read(&path).unwrap_or_default(),
            bytes,
            "the file is untouched"
        );
    }

    #[test]
    fn external_change_reloads_clean_buffer() {
        let Some((_dir, path)) = write_temp("ext.txt", "one\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
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
        while snaps.try_recv().is_some() {}

        // The file changes on disk (the buffer is clean, so this should reload).
        let _ = std::fs::write(&path, "one\ntwo\n");
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![path],
        });

        let mut reloaded = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Reloaded { .. } = ev {
                reloaded = true;
            }
        }
        assert!(reloaded, "a clean external change should reload");
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().line_count() == 3)
        );
        // The reload bumped the version (kept monotonic) and a snapshot was published.
        assert!(snaps.try_recv().is_some());
    }

    #[test]
    fn open_dedups_by_path_and_refcounts_close() {
        let Some((_dir, path)) = write_temp("a.txt", "hi\n") else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
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
        // Second open of the same path reuses the document.
        session.handle(
            RequestId(2),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let same = opened_doc(&mut events);
        assert_eq!(same, Some(doc));
        // Two opens → two refs; one close keeps it, the second drops it.
        session.handle(RequestId(3), Command::CloseDocument { doc });
        assert!(session.document(doc).is_some());
        session.handle(RequestId(4), Command::CloseDocument { doc });
        assert!(session.document(doc).is_none());
    }
