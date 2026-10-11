    // ---- the Command/Event contract of renaming and applying workspace edits.
    //
    // Commands in, events and document state out: `ApplyWorkspaceEdit`,
    // `PrepareRename` and `Rename` answer with `WorkspaceEdit` /
    // `RenamePrepared`, a language server's own `workspace/applyEdit` lands
    // through the same path, and nothing reaches disk but file operations.
    mod workspace_edit_contract {
        use karet_core::ApplyWorkspaceEditResult;
        use karet_core::DeleteOptions;
        use karet_core::DocumentChange;
        use karet_core::DocumentEdit;
        use karet_core::FileOptions;
        use karet_core::WorkspaceEdit;

        use super::*;
        use crate::api::AppliedWorkspaceEdit;
        use crate::api::RenameTarget;
        use crate::lsp::RenameAnswer;

        type TestResult = Result<(), Box<dyn std::error::Error>>;

        fn edit(line: u32, from: u32, to: u32, text: &str) -> TextEdit {
            TextEdit {
                range: Range {
                    start: LineCol::new(line, from),
                    end: LineCol::new(line, to),
                },
                new_text: text.to_owned(),
            }
        }

        fn open(session: &mut Session, events: &mut EventRx, path: &Path) -> Option<DocumentId> {
            session.handle(
                RequestId(1),
                Command::OpenDocument {
                    path: path.to_path_buf(),
                    language: None,
                },
            );
            opened_doc(events)
        }

        /// Send `edit` and return the answer, after every other event.
        fn apply(
            session: &mut Session,
            events: &mut EventRx,
            edit: WorkspaceEdit,
        ) -> Option<AppliedWorkspaceEdit> {
            session.handle(
                RequestId(50),
                Command::ApplyWorkspaceEdit {
                    edit,
                    label: Some("contract".into()),
                },
            );
            answer(events, RequestId(50))
        }

        /// The `WorkspaceEdit` answering `id`, draining everything else.
        fn answer(events: &mut EventRx, id: RequestId) -> Option<AppliedWorkspaceEdit> {
            let mut found = None;
            while let Some((request, event)) = events.try_recv() {
                if let (Some(request), Event::WorkspaceEdit { applied }) = (request, event)
                    && request == id
                {
                    found = Some(applied);
                }
            }
            found
        }

        fn text(session: &Session, doc: DocumentId) -> Option<String> {
            Some(session.document(doc)?.buffer().text())
        }

        #[test]
        fn edits_open_and_unopened_files_as_unsaved_undoable_changes() -> TestResult {
            let dir = tempfile::tempdir()?;
            let (open_path, closed_path) = (dir.path().join("a.rs"), dir.path().join("b.rs"));
            std::fs::write(&open_path, "let old = 1;\n")?;
            std::fs::write(&closed_path, "use old;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &open_path).ok_or("open")?;

            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    changes: vec![
                        (open_path.clone(), vec![edit(0, 4, 7, "new")]),
                        (closed_path.clone(), vec![edit(0, 4, 7, "new")]),
                    ],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;

            assert_eq!(applied.result, ApplyWorkspaceEditResult::applied());
            assert_eq!(applied.label.as_deref(), Some("contract"));
            assert_eq!(applied.edited, vec![open_path.clone(), closed_path.clone()]);
            // The unopened file was opened on the client's behalf, not written.
            let [(background, opened_path)] = applied.opened.as_slice() else {
                return Err(format!("expected one opened document: {:?}", applied.opened).into());
            };
            assert_eq!(opened_path, &closed_path);
            assert_eq!(text(&session, doc).as_deref(), Some("let new = 1;\n"));
            assert_eq!(text(&session, *background).as_deref(), Some("use new;\n"));
            assert_eq!(std::fs::read_to_string(&closed_path)?, "use old;\n");
            assert_eq!(std::fs::read_to_string(&open_path)?, "let old = 1;\n");

            // Undo is per document: undoing one leaves the other renamed.
            session.handle(RequestId(60), Command::Undo { doc: *background });
            assert_eq!(text(&session, *background).as_deref(), Some("use old;\n"));
            assert_eq!(text(&session, doc).as_deref(), Some("let new = 1;\n"));
            Ok(())
        }

        #[test]
        fn a_text_only_edit_that_fails_anywhere_changes_nothing() -> TestResult {
            let dir = tempfile::tempdir()?;
            let (first, second, missing) = (
                dir.path().join("a.rs"),
                dir.path().join("b.rs"),
                dir.path().join("missing.rs"),
            );
            std::fs::write(&first, "one\n")?;
            std::fs::write(&second, "two\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &first).ok_or("open")?;

            // The first edit is fine; the second addresses a line `b.rs` lacks.
            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    changes: vec![
                        (first.clone(), vec![edit(0, 0, 3, "ONE")]),
                        (second.clone(), vec![edit(9, 0, 1, "x")]),
                    ],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;
            assert!(!applied.result.applied);
            assert!(applied.edited.is_empty() && applied.opened.is_empty());
            assert_eq!(text(&session, doc).as_deref(), Some("one\n"));

            // A file that does not exist fails the same way, at its step.
            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    document_changes: vec![
                        DocumentChange::Edit(DocumentEdit {
                            path: first.clone(),
                            version: None,
                            edits: vec![edit(0, 0, 3, "ONE")],
                        }),
                        DocumentChange::Edit(DocumentEdit {
                            path: missing.clone(),
                            version: None,
                            edits: vec![edit(0, 0, 0, "x")],
                        }),
                    ],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;
            assert_eq!(applied.result.failed_change, Some(1));
            assert_eq!(text(&session, doc).as_deref(), Some("one\n"));
            assert!(!missing.exists());
            Ok(())
        }

        #[test]
        fn a_stale_versioned_edit_is_refused() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "one\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;
            let current = session.document(doc).ok_or("doc")?.version();
            let stale = crate::lsp::version_i32(current).wrapping_add(1);
            let versioned = |version| WorkspaceEdit {
                document_changes: vec![DocumentChange::Edit(DocumentEdit {
                    path: path.clone(),
                    version: Some(version),
                    edits: vec![edit(0, 0, 3, "ONE")],
                })],
                ..WorkspaceEdit::default()
            };

            let refused = apply(&mut session, &mut events, versioned(stale)).ok_or("answer")?;
            assert_eq!(refused.result.failed_change, Some(0));
            assert_eq!(text(&session, doc).as_deref(), Some("one\n"));

            let current = crate::lsp::version_i32(current);
            let applied = apply(&mut session, &mut events, versioned(current)).ok_or("answer")?;
            assert!(applied.result.applied);
            assert_eq!(text(&session, doc).as_deref(), Some("ONE\n"));
            Ok(())
        }

        #[test]
        fn several_steps_for_one_file_are_checked_against_its_version_before_the_edit()
        -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "one two\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;
            let current = crate::lsp::version_i32(session.document(doc).ok_or("doc")?.version());
            let step = |version, from, to, text: &str| {
                DocumentChange::Edit(DocumentEdit {
                    path: path.clone(),
                    version: Some(version),
                    edits: vec![edit(0, from, to, text)],
                })
            };

            // A stale version on a later step fails the dry run: nothing lands.
            let refused = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    document_changes: vec![
                        step(current, 0, 3, "ONE"),
                        step(current.wrapping_add(1), 4, 7, "TWO"),
                    ],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("answer")?;
            assert_eq!(refused.result.failed_change, Some(1));
            assert_eq!(text(&session, doc).as_deref(), Some("one two\n"));

            // Both naming the version the edit was computed against: the
            // second is not refused for the version the first produced.
            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    document_changes: vec![step(current, 0, 3, "ONE"), step(current, 4, 7, "TWO")],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("answer")?;
            assert_eq!(applied.result, ApplyWorkspaceEditResult::applied());
            assert_eq!(text(&session, doc).as_deref(), Some("ONE TWO\n"));
            Ok(())
        }

        #[test]
        fn another_spelling_of_an_open_file_edits_its_document() -> TestResult {
            let dir = tempfile::tempdir()?;
            std::fs::create_dir(dir.path().join("sub"))?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "let old = 1;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;

            // Not the path the document was opened under, but the same file.
            let spelled = dir.path().join("sub").join("..").join("a.rs");
            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    changes: vec![(spelled, vec![edit(0, 4, 7, "new")])],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;
            assert!(applied.result.applied);
            assert!(applied.opened.is_empty(), "not opened a second time");
            assert_eq!(text(&session, doc).as_deref(), Some("let new = 1;\n"));

            // A symlink to the file is the same document too.
            #[cfg(unix)]
            {
                let link = dir.path().join("link.rs");
                std::os::unix::fs::symlink(&path, &link)?;
                let applied = apply(
                    &mut session,
                    &mut events,
                    WorkspaceEdit {
                        changes: vec![(link, vec![edit(0, 4, 7, "old")])],
                        ..WorkspaceEdit::default()
                    },
                )
                .ok_or("no answer")?;
                assert!(applied.result.applied);
                assert!(applied.opened.is_empty(), "not opened a second time");
                assert_eq!(text(&session, doc).as_deref(), Some("let old = 1;\n"));
            }
            Ok(())
        }

        #[test]
        fn renaming_a_file_onto_itself_never_deletes_it() -> TestResult {
            let dir = tempfile::tempdir()?;
            std::fs::create_dir(dir.path().join("sub"))?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "keep\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let overwrite = FileOptions {
                overwrite: true,
                ignore_if_exists: false,
            };
            // The same path, and another spelling of it: `to` is the source,
            // not a file in the way, so `overwrite` must not remove it.
            for to in [path.clone(), dir.path().join("sub").join("..").join("a.rs")] {
                let applied = apply(
                    &mut session,
                    &mut events,
                    WorkspaceEdit {
                        document_changes: vec![DocumentChange::Rename {
                            from: path.clone(),
                            to,
                            options: overwrite,
                        }],
                        ..WorkspaceEdit::default()
                    },
                )
                .ok_or("no answer")?;
                assert!(applied.result.applied, "{:?}", applied.result);
                assert_eq!(std::fs::read_to_string(&path)?, "keep\n");
            }
            Ok(())
        }

        #[test]
        fn file_operations_run_in_order_and_stop_at_the_first_failure() -> TestResult {
            let dir = tempfile::tempdir()?;
            let root = dir.path();
            let (old, moved, created, gone) = (
                root.join("old.rs"),
                root.join("sub/moved.rs"),
                root.join("new/created.rs"),
                root.join("gone.rs"),
            );
            std::fs::write(&old, "fn old() {}\n")?;
            std::fs::write(&gone, "")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &old).ok_or("open")?;

            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    document_changes: vec![
                        DocumentChange::Create {
                            path: created.clone(),
                            options: FileOptions::default(),
                        },
                        DocumentChange::Edit(DocumentEdit {
                            path: created.clone(),
                            version: None,
                            edits: vec![edit(0, 0, 0, "fn created() {}\n")],
                        }),
                        DocumentChange::Rename {
                            from: old.clone(),
                            to: moved.clone(),
                            options: FileOptions::default(),
                        },
                        DocumentChange::Delete {
                            path: gone.clone(),
                            options: DeleteOptions::default(),
                        },
                        // Fails: the file exists and nothing allows that.
                        DocumentChange::Create {
                            path: moved.clone(),
                            options: FileOptions::default(),
                        },
                        DocumentChange::Delete {
                            path: created.clone(),
                            options: DeleteOptions::default(),
                        },
                    ],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;

            assert!(!applied.result.applied);
            assert_eq!(applied.result.failed_change, Some(4));
            // Every step before the failure stands; the one after never ran.
            assert!(created.exists() && moved.exists());
            assert!(!old.exists() && !gone.exists());
            assert_eq!(applied.renamed, vec![(old.clone(), moved.clone())]);
            let [(created_doc, _)] = applied.opened.as_slice() else {
                return Err("the created file was opened to edit it".into());
            };
            assert_eq!(
                text(&session, *created_doc).as_deref(),
                Some("fn created() {}\n")
            );
            // The open document followed its file.
            session.handle(
                RequestId(70),
                Command::OpenDocument {
                    path: moved.clone(),
                    language: None,
                },
            );
            assert_eq!(opened_doc(&mut events), Some(doc));

            // `ignoreIfExists` turns the failing step into a no-op.
            let applied = apply(
                &mut session,
                &mut events,
                WorkspaceEdit {
                    document_changes: vec![DocumentChange::Create {
                        path: moved.clone(),
                        options: FileOptions {
                            overwrite: false,
                            ignore_if_exists: true,
                        },
                    }],
                    ..WorkspaceEdit::default()
                },
            )
            .ok_or("no answer")?;
            assert!(applied.result.applied);
            assert_eq!(std::fs::read_to_string(&moved)?, "fn old() {}\n");
            Ok(())
        }

        #[test]
        fn a_rename_answer_is_converted_from_utf16_and_applied() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            // The rocket is one character but two UTF-16 code units.
            std::fs::write(&path, "let \u{1f680} = value;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;
            let version = session.document(doc).ok_or("doc")?.version();

            session.apply_lsp_update(LspUpdate::WorkspaceEdit {
                generation: 0,
                request: RequestId(80),
                doc,
                version,
                // `value` starts at UTF-16 column 9, buffer column 8.
                edit: Ok(WorkspaceEdit {
                    changes: vec![(path.clone(), vec![edit(0, 9, 14, "renamed")])],
                    ..WorkspaceEdit::default()
                }),
            });
            let applied = answer(&mut events, RequestId(80)).ok_or("no answer")?;
            assert!(applied.result.applied);
            assert_eq!(
                text(&session, doc).as_deref(),
                Some("let \u{1f680} = renamed;\n")
            );

            // An answer for a version the buffer has left is refused whole.
            session.apply_lsp_update(LspUpdate::WorkspaceEdit {
                generation: 0,
                request: RequestId(81),
                doc,
                version,
                edit: Ok(WorkspaceEdit {
                    changes: vec![(path.clone(), vec![edit(0, 0, 3, "const")])],
                    ..WorkspaceEdit::default()
                }),
            });
            let refused = answer(&mut events, RequestId(81)).ok_or("no answer")?;
            assert!(!refused.result.applied);
            assert!(text(&session, doc).is_some_and(|text| text.starts_with("let ")));
            Ok(())
        }

        #[test]
        fn a_rename_the_server_refuses_answers_with_its_reason() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "let value = 1;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;
            let version = session.document(doc).ok_or("doc")?.version();

            session.apply_lsp_update(LspUpdate::WorkspaceEdit {
                generation: 0,
                request: RequestId(82),
                doc,
                version,
                edit: Err("`1x` is not a valid identifier".to_owned()),
            });
            let refused = answer(&mut events, RequestId(82)).ok_or("no answer")?;
            assert!(!refused.result.applied);
            assert_eq!(
                refused.result.failure_reason.as_deref(),
                Some("`1x` is not a valid identifier"),
                "the server's reason, not \"nothing to rename\""
            );
            assert_eq!(text(&session, doc).as_deref(), Some("let value = 1;\n"));
            Ok(())
        }

        #[test]
        fn rename_without_a_language_server_answers_refused() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("notes.txt");
            std::fs::write(&path, "word\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;

            session.handle(
                RequestId(90),
                Command::Rename {
                    doc,
                    position: LineCol::new(0, 1),
                    new_name: "other".into(),
                },
            );
            let refused = answer(&mut events, RequestId(90)).ok_or("no answer")?;
            assert!(!refused.result.applied);
            assert!(refused.result.failure_reason.is_some());

            session.handle(
                RequestId(91),
                Command::PrepareRename {
                    doc,
                    position: LineCol::new(0, 1),
                },
            );
            let mut prepared = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::RenamePrepared { target, reason, .. } = event
                    && id == Some(RequestId(91))
                {
                    prepared = Some((target, reason));
                }
            }
            let (target, reason) = prepared.ok_or("no RenamePrepared")?;
            assert_eq!(target, None);
            assert!(reason.is_some_and(|reason| reason.contains("language server")));
            assert_eq!(text(&session, doc).as_deref(), Some("word\n"));
            Ok(())
        }

        /// Adopt one prepare-rename answer and return what the client hears.
        fn prepared(
            session: &mut Session,
            events: &mut EventRx,
            doc: DocumentId,
            caret: LineCol,
            answer: RenameAnswer,
        ) -> Option<(Option<RenameTarget>, Option<String>)> {
            let version = session.document(doc)?.version();
            session.apply_lsp_update(LspUpdate::PrepareRename {
                generation: 0,
                request: RequestId(100),
                doc,
                version,
                caret,
                answer,
            });
            let mut found = None;
            while let Some((_, event)) = events.try_recv() {
                if let Event::RenamePrepared { target, reason, .. } = event {
                    found = Some((target, reason));
                }
            }
            found
        }

        #[test]
        fn prepare_rename_answers_become_targets_in_buffer_columns() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "let \u{1f680} = some_value;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;
            let span = |start, end| Range {
                start: LineCol::new(0, start),
                end: LineCol::new(0, end),
            };

            // The server's span is UTF-16; the placeholder defaults to its text.
            let (target, _) = prepared(
                &mut session,
                &mut events,
                doc,
                LineCol::new(0, 8),
                RenameAnswer::Span(karet_core::PrepareRename::Range {
                    range: span(9, 19),
                    placeholder: None,
                }),
            )
            .ok_or("answer")?;
            assert_eq!(
                target,
                Some(RenameTarget {
                    range: span(8, 18),
                    placeholder: "some_value".into(),
                })
            );

            // Left to the client: the identifier around the caret, which may
            // sit just past its end.
            for caret in [8, 12, 18] {
                let (target, _) = prepared(
                    &mut session,
                    &mut events,
                    doc,
                    LineCol::new(0, caret),
                    RenameAnswer::Identifier,
                )
                .ok_or("answer")?;
                assert_eq!(
                    target.map(|target| (target.range, target.placeholder)),
                    Some((span(8, 18), "some_value".into())),
                    "caret at {caret}"
                );
            }

            // Not on an identifier, and a server's refusal, both say why.
            let (target, reason) = prepared(
                &mut session,
                &mut events,
                doc,
                LineCol::new(0, 6),
                RenameAnswer::Identifier,
            )
            .ok_or("answer")?;
            assert_eq!(target, None);
            assert!(reason.is_some());
            let (target, reason) = prepared(
                &mut session,
                &mut events,
                doc,
                LineCol::new(0, 0),
                RenameAnswer::Refused(Some("cannot rename a keyword".into())),
            )
            .ok_or("answer")?;
            assert_eq!(target, None);
            assert_eq!(reason.as_deref(), Some("cannot rename a keyword"));
            Ok(())
        }

        /// One frame of the fake server's side of an in-memory LSP wire.
        async fn read_json(
            reader: &mut tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        ) -> Option<serde_json::Value> {
            let bytes = karet_lsp::codec::read_frame(reader).await.ok()??;
            serde_json::from_slice(&bytes).ok()
        }

        #[tokio::test]
        async fn a_server_apply_edit_is_applied_answered_and_announced() -> TestResult {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("a.rs");
            std::fs::write(&path, "let \u{1f680} = old;\n")?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            let doc = open(&mut session, &mut events, &path).ok_or("open")?;

            let (client_end, server_end) = tokio::io::duplex(1 << 16);
            let (client_read, client_write) = tokio::io::split(client_end);
            let (server_read, mut server_write) = tokio::io::split(server_end);
            let uri = format!("file://{}", path.display());
            let server = tokio::spawn(async move {
                let mut reader = tokio::io::BufReader::new(server_read);
                let init = read_json(&mut reader).await?;
                let reply = serde_json::json!({"jsonrpc": "2.0", "id": init["id"],
                                               "result": {"capabilities": {}}});
                karet_lsp::codec::write_frame(&mut server_write, reply.to_string().as_bytes())
                    .await
                    .ok()?;
                read_json(&mut reader).await?; // initialized
                // UTF-16 columns: `old` starts at 9 behind the rocket.
                let request = serde_json::json!({
                    "jsonrpc": "2.0", "id": 5, "method": "workspace/applyEdit",
                    "params": {"label": "fix", "edit": {"changes": {uri: [{
                        "range": {"start": {"line": 0, "character": 9},
                                  "end": {"line": 0, "character": 12}},
                        "newText": "new"
                    }]}}}
                });
                karet_lsp::codec::write_frame(&mut server_write, request.to_string().as_bytes())
                    .await
                    .ok()?;
                read_json(&mut reader).await
            });
            let client =
                karet_lsp::LspClient::connect(client_read, client_write, dir.path()).await?;
            let mut requests = client.apply_edit_requests().ok_or("stream")?;
            let request = requests.recv().await.ok_or("no request")?;
            session.adopt_server_edit(request);

            let reply = server.await?.ok_or("no reply")?;
            assert_eq!(reply["id"], serde_json::json!(5));
            assert_eq!(reply["result"]["applied"], serde_json::json!(true));
            assert_eq!(
                text(&session, doc).as_deref(),
                Some("let \u{1f680} = new;\n")
            );
            // The client hears about it unrequested, with the server's label.
            let mut announced = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::WorkspaceEdit { applied } = event {
                    announced = Some((id, applied));
                }
            }
            let (id, applied) = announced.ok_or("no WorkspaceEdit event")?;
            assert_eq!(id, None);
            assert_eq!(applied.label.as_deref(), Some("fix"));
            assert_eq!(applied.edited, vec![path]);
            Ok(())
        }
    }
