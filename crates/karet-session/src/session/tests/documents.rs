    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::TextEdit;

    use super::*;
    use crate::api::Command;

    /// What a session test returns: a missing or wrong answer is an `Err`, so it fails
    /// instead of returning early to a pass. Shared by every file `tests.rs` includes.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// How long a test waits for an answer from one of the session's worker threads.
    ///
    /// It bounds only a failure: [`await_event`] returns the moment the answer arrives,
    /// so a generous bound costs nothing on a pass and keeps a loaded machine from
    /// turning a slow answer into a missing one.
    const EVENT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

    /// The first event `pick` accepts, with the request it answers, or `None` once
    /// `deadline` passes or the stream ends. Shared by every file `tests.rs` includes.
    ///
    /// `pick` sees every event in arrival order, so one that folds a stream (search
    /// hits, a commit's output) keeps its own state and yields once the stream closes.
    /// Async, for a test already inside a runtime; [`await_event`] is the blocking form.
    async fn next_event<T>(
        events: &mut EventRx,
        deadline: std::time::Duration,
        mut pick: impl FnMut(Option<RequestId>, Event) -> Option<T>,
    ) -> Option<T> {
        let accepted = async {
            while let Some((id, event)) = events.recv().await {
                if let Some(value) = pick(id, event) {
                    return Some(value);
                }
            }
            None
        };
        tokio::time::timeout(deadline, accepted).await.ok().flatten()
    }

    /// [`next_event`] for a synchronous test: blocks on the event stream itself rather
    /// than polling it, so the answer is taken the moment a worker thread sends it.
    ///
    /// Panics if called inside a Tokio runtime (it starts its own); an async test
    /// awaits [`next_event`] instead.
    fn await_event<T>(
        events: &mut EventRx,
        deadline: std::time::Duration,
        pick: impl FnMut(Option<RequestId>, Event) -> Option<T>,
    ) -> Option<T> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .ok()?;
        runtime.block_on(next_event(events, deadline, pick))
    }

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

    /// `Session::new` leaves the tree walk to [`Session::start`]: the initial status,
    /// a full `git status` over every file, is not computed or queued on the caller's
    /// thread, and arrives only once the session is started.
    ///
    /// Observed through the event stream rather than a stopwatch, which a loaded
    /// machine could fail on a correct build: the VCS worker runs its jobs in order,
    /// so a status queued by `Session::new` would arrive before the answer to a
    /// snapshot requested after it. Falsified by emitting the initial status from
    /// `Session::new`.
    #[test]
    fn session_new_does_not_walk_large_tree_on_caller_thread() -> TestResult {
        let Some((_dir, root, _file)) = init_git_repo()? else {
            // Skip: `git` is not on PATH.
            return Ok(());
        };
        for i in 0..1200 {
            std::fs::create_dir_all(root.join(format!("src/{i}/nested")))?;
        }

        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });

        // A sentinel behind anything `Session::new` queued on the VCS worker.
        session.handle(RequestId(1), Command::RepositorySnapshot);
        let mut walked = false;
        await_event(&mut events, EVENT_DEADLINE, |id, event| {
            walked |= matches!(event, Event::VcsStatus { .. });
            (id == Some(RequestId(1))).then_some(())
        })
        .ok_or("the snapshot sentinel was never answered")?;
        assert!(
            !walked,
            "Session::new must not walk the tree for the initial status"
        );
        session.start();
        await_event(&mut events, EVENT_DEADLINE, |_, event| {
            matches!(event, Event::VcsStatus { .. }).then_some(())
        })
        .ok_or("start() never delivered the initial status")?;
        Ok(())
    }

    #[test]
    fn opening_a_non_utf8_file_reports_not_utf8_instead_of_a_generic_error() -> TestResult {
        let (_dir, path) = write_temp("bad.rs", "").ok_or("temp file not written")?;
        std::fs::write(&path, [0x66, 0x6e, 0xff, 0x00])?;
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
        Ok(())
    }

    #[test]
    fn spell_results_publish_only_for_the_current_document_version() -> TestResult {
        let (_dir, path) = write_temp("notes.md", "hello wrld\n").ok_or("temp file not written")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        let version = session
            .document(doc)
            .map(|view| view.version())
            .ok_or("opened document is not registered")?;
        let diagnostic = karet_core::Diagnostic {
            range: Range {
                start: LineCol::new(0, 6),
                end: LineCol::new(0, 10),
            },
            severity: Severity::Warning,
            message: "Unknown word “wrld”".to_owned(),
            source: Some("karet-spell".to_owned()),
            code: Some("en_US".to_owned()),
            code_href: None,
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
        Ok(())
    }

    #[test]
    fn unsupported_spell_language_is_reported_instead_of_silently_disabling() -> TestResult {
        let (_dir, path) = write_temp("notes.md", "").ok_or("temp file not written")?;
        let mut settings = crate::config::Settings::default();
        settings.spellcheck.enabled = true;
        settings.spellcheck.language = "fr_FR".to_owned();

        let (resolved, error) =
            resolve_document_settings(&path, Some("Markdown"), &settings);

        assert_eq!(resolved.spelling_language, None);
        assert!(error.is_some_and(|message| {
            message.contains("en_US") && message.contains("en_GB")
        }));
        Ok(())
    }

    #[test]
    fn editorconfig_settings_are_reported_and_applied_on_save() -> TestResult {
        let (dir, path) = write_temp("main.rs", "let x = 1;  \n").ok_or("temp file not written")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root = true\n[*.rs]\nindent_style = tab\nindent_size = 6\n\
             tab_width = 4\nend_of_line = crlf\ncharset = utf-8-bom\n\
             trim_trailing_whitespace = true\ninsert_final_newline = true\n",
        )?;
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
        let doc = opened.ok_or("first event was not Opened")?;
        let settings = settings.ok_or("second event was not DocumentSettingsChanged")?;
        assert_eq!(settings.indent_size, 6);
        assert_eq!(settings.tab_width, 4);
        assert!(!settings.insert_spaces);

        session.handle(RequestId(2), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        let bytes = std::fs::read(&path).unwrap_or_default();
        assert_eq!(bytes, b"\xef\xbb\xbflet x = 1;\r\n");
        assert!(session.document(doc).is_some_and(|view| {
            view.buffer().text() == "let x = 1;\n" && !view.buffer().is_dirty()
        }));
        Ok(())
    }

    #[test]
    fn editorconfig_re_resolves_for_open_documents() -> TestResult {
        let (dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let config = dir.path().join(".editorconfig");
        std::fs::write(&config, "root=true\n[*.rs]\nindent_size=2\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        std::fs::write(&config, "root=true\n[*.rs]\nindent_size=8\n")?;

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
        Ok(())
    }

    #[test]
    fn final_newline_does_not_make_an_empty_file_nonempty() -> TestResult {
        let (dir, path) = write_temp("empty.txt", "").ok_or("temp file not written")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root=true\n[*]\ninsert_final_newline=true\n",
        )?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;

        session.handle(RequestId(2), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"");
        Ok(())
    }

    #[test]
    fn disabled_final_newline_does_not_remove_an_existing_newline() -> TestResult {
        let (dir, path) = write_temp("note.txt", "kept\n").ok_or("temp file not written")?;
        std::fs::write(
            dir.path().join(".editorconfig"),
            "root=true\n[*]\ninsert_final_newline=false\n",
        )?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;

        session.handle(RequestId(2), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"kept\n");
        Ok(())
    }

    #[test]
    fn wakatime_disabled_never_spawns_a_worker_or_reads_its_config() -> TestResult {
        // The privacy contract: with `wakatime.enabled` false (the default),
        // editing must not start the worker thread — which is what reads
        // `.wakatime.cfg` and opens the network — no matter how much typing
        // funnels through the heartbeat call site.
        let (_dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        assert!(!session.config.settings.wakatime.enabled, "off by default");

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        for i in 0..5 {
            let change = Change::new(
                i,
                vec![TextEdit {
                    range: Range {
                        start: LineCol::new(0, 0),
                        end: LineCol::new(0, 0),
                    },
                    new_text: "x".to_string(),
                }],
            );
            session.handle(
                RequestId(2 + i),
                Command::ApplyChange {
                    doc,
                    change,
                    cause: EditCause::Replace,
                },
            );
        }

        assert!(
            session.wakatime_worker.is_none(),
            "a disabled tracker must not start its worker"
        );
        assert!(
            session.wakatime_last.is_none(),
            "a disabled tracker records nothing"
        );
        Ok(())
    }

    #[test]
    fn open_apply_save_undo_flow() -> TestResult {
        let (_dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("expected an Opened event")?;
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
        let snap = last_snap.ok_or("expected a snapshot after apply")?;
        assert_eq!(snap.version, 1);
        assert!(snap.dirty);
        // "fn main() {}\n" + inserted "\nfn x() {}" → three lines.
        assert_eq!(snap.buffer.line_count(), 3);

        // Save: the file on disk reflects the edit and the doc goes clean.
        session.handle(RequestId(3), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });
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
        Ok(())
    }

    #[test]
    fn retarget_document_updates_the_save_destination() -> TestResult {
        let (_dir, path) = write_temp("old.txt", "old\n").ok_or("temp file not written")?;
        let new_path = path.with_file_name("new.txt");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        std::fs::rename(&path, &new_path)?;

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
        session.handle(RequestId(4), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });

        assert_eq!(
            std::fs::read_to_string(&new_path).unwrap_or_default(),
            "old moved\n"
        );
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn save_refuses_to_overwrite_a_file_changed_on_disk_since_it_was_read() -> TestResult {
        let (_dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
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
        std::fs::write(&path, "fn main() { /* external */ }\n")?;

        session.handle(RequestId(3), Command::Save {
                doc,
                cause: SaveCause::Manual,
            });
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
        Ok(())
    }

    #[test]
    fn apply_against_a_stale_version_resyncs_instead_of_dropping_silently() -> TestResult {
        // Regression: a client whose local speculative version has diverged from
        // the backend's (e.g. after a dropped/duplicate message) used to have its
        // edit silently discarded with no way to recover — every subsequent edit
        // on that document would then fail the same way forever. It must instead
        // be told and get a fresh snapshot back so it can resync.
        let (_dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
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
        Ok(())
    }

    #[test]
    fn undo_redo_snapshot_carries_caret_but_edits_do_not() -> TestResult {
        let (_dir, path) = write_temp("main.rs", "fn main() {}\n").ok_or("temp file not written")?;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;

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
        Ok(())
    }

    #[test]
    fn external_change_reloads_clean_buffer() -> TestResult {
        let (_dir, path) = write_temp("ext.txt", "one\n").ok_or("temp file not written")?;
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
        while snaps.try_recv().is_some() {}

        // The file changes on disk (the buffer is clean, so this should reload).
        std::fs::write(&path, "one\ntwo\n")?;
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
        Ok(())
    }

    #[test]
    fn open_dedups_by_path_and_refcounts_close() -> TestResult {
        let (_dir, path) = write_temp("a.txt", "hi\n").ok_or("temp file not written")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events).ok_or("no Opened event")?;
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
        Ok(())
    }

    #[test]
    fn a_plaintext_snapshot_folds_by_indentation() -> TestResult {
        let (_dir, path) =
            write_temp("notes.txt", "list\n  one\n  two\nend\n").ok_or("temp file not written")?;
        let (mut session, _events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let mut folds = None;
        while let Some((_, snap)) = snaps.try_recv() {
            folds = Some(snap.folds.regions().iter().map(|r| (r.start, r.end)).collect::<Vec<_>>());
        }
        assert_eq!(folds, Some(vec![(0, 2)]));
        Ok(())
    }
