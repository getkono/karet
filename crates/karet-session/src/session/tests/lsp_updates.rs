    // Column conversion when adopting an LSP result. Servers answer in UTF-16
    // columns; the session owes every downstream client buffer columns, for the
    // file the answer *names* — which for a definition is usually not the file the
    // request came from.
    mod lsp_updates {
        use karet_core::LineCol;
        use karet_core::Location;
        use karet_core::Range;

        use crate::api::Command;
        use crate::api::Event;
        use crate::api::LocationKind;
        use crate::lsp::LspUpdate;
        use crate::session::Session;
        use crate::session::SessionConfig;

        /// A line whose leading emoji is one `char` but *two* UTF-16 code units, so
        /// a column that is not converted lands one place too far right.
        const NON_BMP: &str = "let \u{1f680} = target;\n";

        fn range(line: u32, col: u32) -> Range {
            Range {
                start: LineCol::new(line, col),
                end: LineCol::new(line, col),
            }
        }

        /// Open `path` in a fresh session and return the session plus its id.
        fn session_with(
            path: &std::path::Path,
        ) -> Option<(Session, crate::api::DocumentId, crate::session::EventRx)> {
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            session.handle(
                crate::api::RequestId(1),
                Command::OpenDocument {
                    path: path.to_path_buf(),
                    language: None,
                },
            );
            let mut doc = None;
            while let Some((_, event)) = events.try_recv() {
                if let Event::Opened { doc: id, .. } = event {
                    doc = Some(id);
                }
            }
            Some((session, doc?, events))
        }

        /// Drive one `Definitions` update and return the locations it emits.
        fn adopt(
            session: &mut Session,
            events: &mut crate::session::EventRx,
            doc: crate::api::DocumentId,
            locations: Vec<Location>,
        ) -> Option<Vec<Location>> {
            let version = session.document(doc)?.version();
            session.apply_lsp_update(LspUpdate::Definitions {
                generation: 0,
                request: crate::api::RequestId(2),
                doc,
                version,
                kind: None,
                locations,
            });
            while let Some((_, event)) = events.try_recv() {
                if let Event::Definitions { locations } = event {
                    return Some(locations);
                }
            }
            None
        }

        #[test]
        fn a_navigation_with_no_server_is_answered_empty_with_its_kind() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let path = dir.path().join("notes.txt");
            if std::fs::write(&path, "plain text\n").is_err() {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&path) else {
                return;
            };
            session.handle(
                crate::api::RequestId(9),
                Command::FindLocations {
                    doc,
                    position: LineCol::new(0, 0),
                    kind: LocationKind::Implementation,
                },
            );
            let mut answer = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::Locations { kind, locations } = event {
                    answer = Some((id, kind, locations.len()));
                }
            }
            // Never left hanging: the caller is owed an answer even with
            // nobody to ask.
            assert_eq!(
                answer,
                Some((Some(crate::api::RequestId(9)), LocationKind::Implementation, 0))
            );
        }

        #[test]
        fn signature_help_with_no_server_is_answered_empty() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let path = dir.path().join("notes.txt");
            if std::fs::write(&path, "plain(text\n").is_err() {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&path) else {
                return;
            };
            session.handle(
                crate::api::RequestId(9),
                Command::SignatureHelp {
                    doc,
                    position: LineCol::new(0, 6),
                    manual: true,
                },
            );
            let mut answer = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::SignatureHelp {
                    doc: asked,
                    help,
                    options,
                } = event
                {
                    answer = Some((id, asked, help, options));
                }
            }
            assert_eq!(
                answer,
                Some((
                    Some(crate::api::RequestId(9)),
                    doc,
                    None,
                    karet_core::SignatureHelpOptions::default()
                ))
            );
        }

        #[test]
        fn a_navigation_answer_is_converted_and_keeps_its_kind() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let path = dir.path().join("here.rs");
            if std::fs::write(&path, NON_BMP).is_err() {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&path) else {
                return;
            };
            let Some(version) = session.document(doc).map(|doc| doc.version()) else {
                return;
            };
            session.apply_lsp_update(LspUpdate::Definitions {
                generation: 0,
                request: crate::api::RequestId(2),
                doc,
                version,
                kind: Some(LocationKind::References),
                locations: vec![Location {
                    path,
                    range: range(0, 6),
                }],
            });
            let mut answer = None;
            while let Some((_, event)) = events.try_recv() {
                assert!(
                    !matches!(event, Event::Definitions { .. }),
                    "a references answer must not read as a definition"
                );
                if let Event::Locations { kind, locations } = event {
                    answer = Some((kind, locations));
                }
            }
            assert_eq!(
                answer.as_ref().map(|(kind, _)| *kind),
                Some(LocationKind::References)
            );
            // UTF-16 column 6 is buffer column 5, as for a definition.
            let column = answer.and_then(|(_, l)| l.first().map(|l| l.range.start.col));
            assert_eq!(column, Some(5));
        }

        #[test]
        fn a_definition_in_the_requesting_file_is_converted() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let path = dir.path().join("here.rs");
            if std::fs::write(&path, NON_BMP).is_err() {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&path) else {
                return;
            };

            // UTF-16 column 6 is buffer column 5: the rocket counts twice.
            let out = adopt(
                &mut session,
                &mut events,
                doc,
                vec![Location {
                    path: path.clone(),
                    range: range(0, 6),
                }],
            );
            assert_eq!(out.and_then(|l| l.first().map(|l| l.range.start.col)), Some(5));
        }

        #[test]
        fn a_definition_in_another_open_file_is_converted_against_that_file() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let (here, there) = (dir.path().join("here.rs"), dir.path().join("there.rs"));
            if std::fs::write(&here, "fn caller() {}\n").is_err()
                || std::fs::write(&there, NON_BMP).is_err()
            {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&here) else {
                return;
            };
            session.handle(
                crate::api::RequestId(3),
                Command::OpenDocument {
                    path: there.clone(),
                    language: None,
                },
            );
            while events.try_recv().is_some() {}

            let out = adopt(
                &mut session,
                &mut events,
                doc,
                vec![Location {
                    path: there,
                    range: range(0, 6),
                }],
            );
            assert_eq!(out.and_then(|l| l.first().map(|l| l.range.start.col)), Some(5));
        }

        #[test]
        fn a_definition_in_a_file_that_is_not_open_is_converted_from_disk() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let (here, there) = (dir.path().join("here.rs"), dir.path().join("there.rs"));
            if std::fs::write(&here, "fn caller() {}\n").is_err()
                || std::fs::write(&there, NON_BMP).is_err()
            {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&here) else {
                return;
            };

            let out = adopt(
                &mut session,
                &mut events,
                doc,
                vec![Location {
                    path: there,
                    range: range(0, 6),
                }],
            );
            assert_eq!(out.and_then(|l| l.first().map(|l| l.range.start.col)), Some(5));
        }

        #[test]
        fn many_hits_across_files_are_each_converted_against_their_own_file() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let (here, there) = (dir.path().join("here.rs"), dir.path().join("there.rs"));
            let plain = dir.path().join("plain.rs");
            if std::fs::write(&here, NON_BMP).is_err()
                || std::fs::write(&there, "let \u{1f680} = a;\nlet \u{1f680}\u{1f680} = b;\n")
                    .is_err()
                || std::fs::write(&plain, "let x = c;\n").is_err()
            {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&here) else {
                return;
            };
            let at = |path: &std::path::Path, line, col| Location {
                path: path.to_path_buf(),
                range: range(line, col),
            };
            // Hits interleave the open file with two unopened ones, and repeat
            // one unopened file on different lines, so a buffer cached for one
            // path must never be used to convert another's columns.
            let out = adopt(&mut session, &mut events, doc, vec![
                at(&there, 0, 6),
                at(&here, 0, 6),
                at(&plain, 0, 6),
                at(&there, 1, 8),
            ]);
            let columns: Option<Vec<u32>> =
                out.map(|l| l.iter().map(|l| l.range.start.col).collect());
            assert_eq!(columns, Some(vec![5, 5, 6, 6]));
        }

        #[test]
        fn an_unreadable_definition_target_is_passed_through_rather_than_dropped() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let here = dir.path().join("here.rs");
            if std::fs::write(&here, "fn caller() {}\n").is_err() {
                return;
            }
            let Some((mut session, doc, mut events)) = session_with(&here) else {
                return;
            };

            // An approximate column beats a dropped result: `goto` clamps anyway,
            // and the line number is still right.
            let missing = dir.path().join("gone.rs");
            let out = adopt(
                &mut session,
                &mut events,
                doc,
                vec![Location {
                    path: missing.clone(),
                    range: range(3, 6),
                }],
            );
            let first = out.and_then(|l| l.first().cloned());
            assert_eq!(first.as_ref().map(|l| l.path.clone()), Some(missing));
            assert_eq!(first.map(|l| l.range.start), Some(LineCol::new(3, 6)));
        }

        fn workspace_symbol(
            name: &str,
            path: &std::path::Path,
            range: Option<Range>,
        ) -> karet_core::WorkspaceSymbol {
            karet_core::WorkspaceSymbol {
                name: name.to_string(),
                kind: karet_core::SymbolKind::Function,
                container_name: None,
                path: path.to_path_buf(),
                range,
            }
        }

        #[test]
        fn workspace_symbols_keep_their_files_and_open_ones_are_converted() {
            let Ok(dir) = tempfile::tempdir() else { return };
            let here = dir.path().join("here.rs");
            let elsewhere = dir.path().join("elsewhere.rs");
            if std::fs::write(&here, NON_BMP).is_err()
                || std::fs::write(&elsewhere, NON_BMP).is_err()
            {
                return;
            }
            let Some((mut session, _doc, mut events)) = session_with(&here) else {
                return;
            };
            session.apply_lsp_update(LspUpdate::WorkspaceSymbols {
                generation: 0,
                request: crate::api::RequestId(5),
                symbols: vec![
                    workspace_symbol("open", &here, Some(range(0, 6))),
                    workspace_symbol("closed", &elsewhere, Some(range(0, 6))),
                    workspace_symbol("lazy", &elsewhere, None),
                ],
            });
            let mut answer = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::WorkspaceSymbols { symbols } = event {
                    answer = Some((id, symbols));
                }
            }
            let Some((id, symbols)) = answer else {
                unreachable!("the answer is emitted")
            };
            assert_eq!(id, Some(crate::api::RequestId(5)));
            let found: Vec<_> = symbols
                .iter()
                .map(|s| (s.name.as_str(), s.path.clone(), s.range.map(|r| r.start)))
                .collect();
            assert_eq!(
                found,
                vec![
                    // UTF-16 column 6 is buffer column 5 in the open file...
                    ("open", here, Some(LineCol::new(0, 5))),
                    // ...while a file that is not open is not read to convert.
                    ("closed", elsewhere.clone(), Some(LineCol::new(0, 6))),
                    ("lazy", elsewhere, None),
                ]
            );
        }

        #[test]
        fn a_workspace_symbol_search_with_no_server_is_answered_empty() {
            let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
            session.handle(
                crate::api::RequestId(7),
                Command::WorkspaceSymbols {
                    query: "run".to_string(),
                },
            );
            let mut answer = None;
            while let Some((id, event)) = events.try_recv() {
                if let Event::WorkspaceSymbols { symbols } = event {
                    answer = Some((id, symbols.len()));
                }
            }
            assert_eq!(answer, Some((Some(crate::api::RequestId(7)), 0)));
        }
    }

    // Asking to install a language server spends the user's bandwidth, so the
    // prompt is raised at most once per provider. These pin the two facts that
    // decide it, both of which live outside settings on purpose.
    mod install_prompt {
        use crate::api::Event;
        use crate::api::LanguageServerId;
        use crate::lsp::LspUpdate;
        use crate::lsp_registry::Declined;
        use crate::lsp_registry::write_declined;
        use crate::session::Session;
        use crate::session::SessionConfig;

        /// A session whose registry state lives under `root`, with the default
        /// `managedDownloads: "prompt"` policy.
        fn session_rooted_at(root: &std::path::Path) -> (Session, crate::session::EventRx) {
            let (session, events, _snaps) = Session::new(SessionConfig {
                lsp_registry_dir: Some(root.to_path_buf()),
                ..SessionConfig::default()
            });
            (session, events)
        }

        /// Whether the session offered to install anything.
        fn offered(events: &mut crate::session::EventRx) -> bool {
            let mut offered = false;
            while let Some((_, event)) = events.try_recv() {
                if matches!(event, Event::LanguageServerInstallRequired { .. }) {
                    offered = true;
                }
            }
            offered
        }

        fn install_required(session: &mut Session) {
            session.apply_lsp_update(LspUpdate::InstallRequired {
                generation: 0,
                server: LanguageServerId::Texlab,
                language: "latex".into(),
            });
        }

        #[test]
        fn a_provider_never_offered_raises_the_prompt() {
            let Ok(dir) = tempfile::tempdir() else {
                return;
            };
            let (mut session, mut events) = session_rooted_at(dir.path());
            install_required(&mut session);
            assert!(offered(&mut events));
        }

        #[test]
        fn a_declined_provider_is_never_offered_again() {
            let Ok(dir) = tempfile::tempdir() else {
                return;
            };
            let declined = Declined::now();
            if write_declined(dir.path(), &LanguageServerId::Texlab, &declined).is_err() {
                return;
            }
            let (mut session, mut events) = session_rooted_at(dir.path());
            install_required(&mut session);
            assert!(
                !offered(&mut events),
                "the user has already answered this question"
            );
        }
    }

