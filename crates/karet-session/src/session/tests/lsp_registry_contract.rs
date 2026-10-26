    // The managed language-server registry at the seam: each worker update's
    // event, the install refusal commands, and a retired formatter answering a
    // save twice.

    use crate::api::LanguageServerId;
    use crate::lsp_registry::RegistryUpdate;

    /// Every event the session has queued, with its tag.
    fn queued(events: &mut EventRx) -> Vec<(Option<RequestId>, Event)> {
        std::iter::from_fn(|| events.try_recv()).collect()
    }

    #[test]
    fn each_registry_update_is_answered_with_its_own_event() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let change = crate::api::LanguageServerChange {
            server: LanguageServerId::Ruff,
            current: Some("0.1.0".to_owned()),
            target: "0.2.0".to_owned(),
            download_bytes: Some(1024),
        };

        session.apply_lsp_registry_update(RegistryUpdate::Plan {
            request: RequestId(1),
            plan: crate::api::LanguageServerPlanId(7),
            changes: vec![change.clone()],
        });
        assert!(matches!(
            queued(&mut events).as_slice(),
            [(
                Some(RequestId(1)),
                Event::LanguageServerUpdatePlan { plan, changes },
            )] if plan.0 == 7 && *changes == vec![change.clone()]
        ));

        session.apply_lsp_registry_update(RegistryUpdate::Progress {
            server: LanguageServerId::Ruff,
            downloaded: 10,
            total: Some(1024),
        });
        assert!(matches!(
            queued(&mut events).as_slice(),
            [(
                None,
                Event::LanguageServerProgress {
                    server,
                    downloaded: 10,
                    total: Some(1024),
                },
            )] if *server == LanguageServerId::Ruff
        ));

        // Nothing is running, so neither a first install nor an upgrade asks
        // for a restart.
        for (request, was_installed) in [(RequestId(2), false), (RequestId(3), true)] {
            session.apply_lsp_registry_update(RegistryUpdate::Changed {
                request,
                server: LanguageServerId::Ruff,
                version: "0.2.0".to_owned(),
                was_installed,
            });
            let changed: Vec<_> = queued(&mut events)
                .into_iter()
                .filter_map(|(id, event)| match event {
                    Event::LanguageServerChanged {
                        server,
                        version,
                        restart_required,
                    } => Some((id, server, version, restart_required)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                changed,
                vec![(
                    Some(request),
                    LanguageServerId::Ruff,
                    "0.2.0".to_owned(),
                    false
                )]
            );
        }

        session.apply_lsp_registry_update(RegistryUpdate::Removed {
            request: RequestId(4),
            server: LanguageServerId::Ruff,
            cleanup_pending: true,
        });
        let removed: Vec<_> = queued(&mut events)
            .into_iter()
            .filter_map(|(id, event)| match event {
                Event::LanguageServerRemoved {
                    server,
                    cleanup_pending,
                } => Some((id, server, cleanup_pending)),
                _ => None,
            })
            .collect();
        assert_eq!(
            removed,
            vec![(Some(RequestId(4)), LanguageServerId::Ruff, true)]
        );

        session.apply_lsp_registry_update(RegistryUpdate::Complete {
            request: RequestId(5),
        });
        assert!(matches!(
            queued(&mut events).as_slice(),
            [(
                Some(RequestId(5)),
                Event::Notification {
                    severity: Severity::Information,
                    kind: NotificationKind::Lsp,
                    message,
                },
            )] if message == "language-server update plan applied"
        ));
    }

    fn registry_session(registry: &Path) -> (Session, EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            lsp_registry_dir: Some(registry.to_path_buf()),
            ..SessionConfig::default()
        });
        (session, events)
    }

    #[test]
    fn declining_an_install_records_it_and_undeclining_clears_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut session, mut events) = registry_session(dir.path());
        let declined =
            || crate::lsp_registry::store::read_declined(Some(dir.path()), &LanguageServerId::Pyright);

        session.handle(
            RequestId(1),
            Command::DeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        assert!(declined().is_some(), "the refusal is on disk");
        assert!(
            crate::lsp_registry::store::read_declined(Some(dir.path()), &LanguageServerId::Ruff).is_none(),
            "and scoped to the provider declined"
        );

        session.handle(
            RequestId(2),
            Command::UndeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        assert!(declined().is_none(), "the provider may be offered again");

        // Clearing a refusal that is not there is the state asked for already.
        session.handle(
            RequestId(3),
            Command::UndeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        assert!(queued(&mut events).is_empty(), "success answers nothing");
        Ok(())
    }

    #[test]
    fn a_refusal_that_cannot_be_written_says_so() -> TestResult {
        let dir = tempfile::tempdir()?;
        // A file where the registry directory should be: nothing can go under it.
        let registry = dir.path().join("registry");
        std::fs::write(&registry, "")?;
        let (mut session, mut events) = registry_session(&registry);

        session.handle(
            RequestId(1),
            Command::DeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        assert!(matches!(
            queued(&mut events).as_slice(),
            [(
                Some(RequestId(1)),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Lsp,
                    message,
                },
            )] if message.starts_with("could not record the refusal")
        ));
        Ok(())
    }

    /// With no registry directory there is nowhere to record anything; both
    /// commands are consumed quietly rather than reaching the main dispatch.
    #[test]
    fn refusals_without_a_registry_directory_are_consumed_quietly() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::DeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        session.handle(
            RequestId(2),
            Command::UndeclineLanguageServer {
                server: LanguageServerId::Pyright,
            },
        );
        assert!(queued(&mut events).is_empty());
    }

    /// A retired generation's formatting answer finishes its save — once. A
    /// duplicate of that answer (the same request settled again) must neither
    /// write the file a second time nor answer the request twice.
    #[test]
    fn a_retired_formatting_answer_arriving_twice_saves_once() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "original\n")?;
        let (mut session, doc, mut events, request) =
            parked_save(&path).ok_or("the save was never parked on a formatting request")?;
        let version = session
            .document(doc)
            .map(|d| d.version())
            .ok_or("opened document is not registered")?;
        let retired = || crate::lsp::LspUpdate::Formatting {
            generation: u64::MAX,
            request,
            doc,
            version,
            formatted: true,
            edits: Vec::new(),
            scope: crate::lsp::FormattingScope::Save,
        };
        let saves = |events: &mut EventRx| {
            queued(events)
                .into_iter()
                .filter(|(_, event)| matches!(event, Event::Saved { .. }))
                .count()
        };

        session.apply_lsp_update(retired());
        assert_eq!(saves(&mut events), 1);
        assert_eq!(std::fs::read_to_string(&path)?, "edited\n");

        // Someone else writes the file; the duplicate must not clobber it.
        std::fs::write(&path, "external\n")?;
        session.apply_lsp_update(retired());
        assert_eq!(saves(&mut events), 0, "the request was already answered");
        assert_eq!(std::fs::read_to_string(&path)?, "external\n");
        assert!(session.pending_format_saves.is_empty());
        Ok(())
    }
