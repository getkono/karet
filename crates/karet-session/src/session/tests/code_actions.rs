    // ---- the Command/Event contract of listing and running code actions.
    //
    // `CodeActions` answers with `CodeActions` in buffer columns;
    // `ApplyCodeAction` answers with one `WorkspaceEdit`, labelled with the
    // action's title, after resolving, applying, and running its command --
    // or with the reason it stopped. Server answers are injected as the task
    // would send them; the live round trip is in `lsp::tests`.
    mod code_action_contract {
        use karet_core::CodeAction;
        use karet_core::WorkspaceEdit;

        use super::*;
        use crate::api::AppliedWorkspaceEdit;
        use crate::api::CodeActionTrigger;
        use crate::lsp::CodeActionAnswer;

        type TestResult = Result<(), Box<dyn std::error::Error>>;

        fn edit(from: u32, to: u32, text: &str) -> TextEdit {
            TextEdit {
                range: Range {
                    start: LineCol::new(0, from),
                    end: LineCol::new(0, to),
                },
                new_text: text.to_owned(),
            }
        }

        fn action(title: &str, path: &Path, edit: Option<TextEdit>) -> CodeAction {
            CodeAction {
                title: title.to_owned(),
                edit: edit.map(|edit| WorkspaceEdit {
                    changes: vec![(path.to_path_buf(), vec![edit])],
                    ..WorkspaceEdit::default()
                }),
                ..CodeAction::default()
            }
        }

        /// A fresh session with one file open: the directory holding it, its
        /// path, the session, its events, and the document.
        type Fixture = (tempfile::TempDir, PathBuf, Session, EventRx, DocumentId);

        /// Open `text` as `name` in a fresh session.
        fn session_with(name: &str, text: &str) -> Result<Fixture, Box<dyn std::error::Error>> {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join(name);
            std::fs::write(&path, text)?;
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            session.handle(
                RequestId(1),
                DocumentCommand::Open {
                    path: path.clone(),
                    language: None,
                },
            );
            let doc = opened_doc(&mut events).ok_or("open")?;
            Ok((dir, path, session, events, doc))
        }

        /// Every event answering `id`, draining the rest.
        fn answers(events: &mut EventRx, id: RequestId) -> Vec<Event> {
            let mut found = Vec::new();
            while let Some((request, event)) = events.try_recv() {
                if request == Some(id) {
                    found.push(event);
                }
            }
            found
        }

        fn applied(events: &mut EventRx, id: RequestId) -> Option<AppliedWorkspaceEdit> {
            answers(events, id).into_iter().find_map(|event| match event {
                Event::WorkspaceEdit { applied } => Some(applied),
                _ => None,
            })
        }

        fn text(session: &Session, doc: DocumentId) -> Option<String> {
            Some(session.document(doc)?.buffer().text())
        }

        #[test]
        fn listed_actions_reach_the_client_in_buffer_columns() -> TestResult {
            // The rocket is one character but two UTF-16 code units.
            let (_dir, path, mut session, mut events, doc) =
                session_with("a.rs", "let \u{1f680} = value;\n")?;
            let version = session.document(doc).ok_or("doc")?.version();
            session.apply_lsp_update(LspUpdate::CodeAction {
                generation: 0,
                answer: CodeActionAnswer::Listed {
                    request: RequestId(10),
                    doc,
                    version,
                    // `value` is UTF-16 9..14, buffer 8..13.
                    actions: Ok(vec![action("Rename", &path, Some(edit(9, 14, "v")))]),
                },
            });
            let answer = answers(&mut events, RequestId(10));
            let Some(Event::CodeActions {
                version: listed,
                actions,
                reason: None,
                ..
            }) = answer.first()
            else {
                return Err(format!("expected CodeActions, got {answer:?}").into());
            };
            assert_eq!(*listed, version);
            let edit = actions[0].edit.as_ref().ok_or("edit")?;
            assert_eq!(edit.changes[0].1[0].range.start, LineCol::new(0, 8));
            assert_eq!(edit.changes[0].1[0].range.end, LineCol::new(0, 13));

            // A server's refusal travels as the reason, with no actions.
            session.apply_lsp_update(LspUpdate::CodeAction {
                generation: 0,
                answer: CodeActionAnswer::Listed {
                    request: RequestId(11),
                    doc,
                    version,
                    actions: Err("index not ready".into()),
                },
            });
            let answer = answers(&mut events, RequestId(11));
            assert!(matches!(
                answer.first(),
                Some(Event::CodeActions { actions, reason: Some(reason), .. })
                    if actions.is_empty() && reason == "index not ready"
            ));
            Ok(())
        }

        #[test]
        fn an_action_applies_its_edit_unless_stale_or_disabled() -> TestResult {
            let (_dir, path, mut session, mut events, doc) = session_with("a.rs", "let a = 1;\n")?;
            let version = session.document(doc).ok_or("doc")?.version();

            let mut disabled = action("Inline", &path, Some(edit(4, 5, "b")));
            disabled.disabled = Some("nothing to inline".into());
            session.handle(RequestId(20), LspCommand::ApplyCodeAction {
                doc,
                version,
                action: disabled,
            });
            let refused = applied(&mut events, RequestId(20)).ok_or("no answer")?;
            assert!(!refused.result.applied);
            assert_eq!(refused.result.failure_reason.as_deref(), Some("nothing to inline"));

            let rename = action("Rename to b", &path, Some(edit(4, 5, "b")));
            session.handle(RequestId(21), LspCommand::ApplyCodeAction {
                doc,
                version,
                action: rename.clone(),
            });
            let done = applied(&mut events, RequestId(21)).ok_or("no answer")?;
            assert!(done.result.applied, "{done:?}");
            assert_eq!(done.label.as_deref(), Some("Rename to b"));
            assert_eq!(done.edited, vec![path.clone()]);
            assert_eq!(text(&session, doc).as_deref(), Some("let b = 1;\n"));

            // Listed at the version before that edit: its ranges are stale.
            session.handle(RequestId(22), LspCommand::ApplyCodeAction {
                doc,
                version,
                action: rename,
            });
            let stale = applied(&mut events, RequestId(22)).ok_or("no answer")?;
            assert!(!stale.result.applied);
            assert_eq!(text(&session, doc).as_deref(), Some("let b = 1;\n"));
            Ok(())
        }

        #[test]
        fn a_resolved_action_applies_in_utf16_and_a_failed_step_says_why() -> TestResult {
            let (_dir, path, mut session, mut events, doc) =
                session_with("a.rs", "let \u{1f680} = value;\n")?;
            let version = session.document(doc).ok_or("doc")?.version();
            session.apply_lsp_update(LspUpdate::CodeAction {
                generation: 0,
                answer: CodeActionAnswer::Resolved {
                    request: RequestId(30),
                    doc,
                    version,
                    action: Ok(action("Fix", &path, Some(edit(9, 14, "fixed")))),
                },
            });
            let done = applied(&mut events, RequestId(30)).ok_or("no answer")?;
            assert!(done.result.applied, "{done:?}");
            assert_eq!(
                text(&session, doc).as_deref(),
                Some("let \u{1f680} = fixed;\n")
            );

            session.apply_lsp_update(LspUpdate::CodeAction {
                generation: 0,
                answer: CodeActionAnswer::Resolved {
                    request: RequestId(31),
                    doc,
                    version,
                    action: Err("resolve failed".into()),
                },
            });
            let refused = applied(&mut events, RequestId(31)).ok_or("no answer")?;
            assert_eq!(refused.result.failure_reason.as_deref(), Some("resolve failed"));

            // A command that fails after its edit applied reports the edit and
            // the failure together.
            let report = AppliedWorkspaceEdit {
                label: Some("Fix".into()),
                edited: vec![path.clone()],
                ..AppliedWorkspaceEdit::default()
            };
            session.apply_lsp_update(LspUpdate::CodeAction {
                generation: 0,
                answer: CodeActionAnswer::Executed {
                    request: RequestId(32),
                    applied: Box::new(report),
                    result: Err("command crashed".into()),
                },
            });
            let failed = applied(&mut events, RequestId(32)).ok_or("no answer")?;
            assert!(!failed.result.applied);
            assert_eq!(failed.result.failure_reason.as_deref(), Some("command crashed"));
            assert_eq!(failed.edited, vec![path]);
            Ok(())
        }

        #[test]
        fn without_a_language_server_every_step_still_answers() -> TestResult {
            let (_dir, path, mut session, mut events, doc) = session_with("notes.txt", "word\n")?;
            let version = session.document(doc).ok_or("doc")?.version();

            session.handle(RequestId(40), LspCommand::CodeActions {
                doc,
                range: Range::default(),
                kinds: Vec::new(),
                trigger: CodeActionTrigger::Automatic,
            });
            let answer = answers(&mut events, RequestId(40));
            assert!(matches!(
                answer.first(),
                Some(Event::CodeActions { actions, reason: Some(reason), .. })
                    if actions.is_empty() && reason.contains("language server")
            ));

            // A deferred edit cannot be resolved without its server.
            let deferred = CodeAction {
                title: "Fix".into(),
                payload: Some("{}".into()),
                ..CodeAction::default()
            };
            session.handle(RequestId(41), LspCommand::ApplyCodeAction {
                doc,
                version,
                action: deferred,
            });
            let refused = applied(&mut events, RequestId(41)).ok_or("no answer")?;
            assert!(!refused.result.applied);

            // An inline edit applies; the command after it cannot run, and
            // the answer says so beside what the edit changed.
            let mut both = action("Fix", &path, Some(edit(0, 4, "text")));
            both.command = Some(karet_core::CommandId("fix".into()));
            session.handle(RequestId(42), LspCommand::ApplyCodeAction {
                doc,
                version,
                action: both,
            });
            let partial = applied(&mut events, RequestId(42)).ok_or("no answer")?;
            assert!(!partial.result.applied);
            assert!(
                partial
                    .result
                    .failure_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("language server"))
            );
            assert_eq!(partial.edited, vec![path]);
            assert_eq!(text(&session, doc).as_deref(), Some("text\n"));
            Ok(())
        }
    }
