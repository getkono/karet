    // The workspace search's Command/Event contract. The preview transform is
    // covered hermetically in `crate::search_worker::preview`; these cover the
    // seam: streaming, the caps, error reporting, and the configured excludes.

    /// Drain events until `SearchFinished` arrives, or the deadline passes.
    ///
    /// The walk runs on its own thread, so a bare `try_recv` would race it.
    fn drain_search(events: &mut EventRx) -> (Vec<crate::api::SearchHit>, Option<Event>) {
        let mut hits = Vec::new();
        let finished = await_event(events, EVENT_DEADLINE, |_, event| match event {
            Event::SearchProgress { hits: batch, .. } => {
                hits.extend(batch);
                None
            },
            finished @ Event::SearchFinished { .. } => Some(finished),
            _ => None,
        });
        (hits, finished)
    }

    fn search_session(dir: &std::path::Path) -> (Session, EventRx, SnapshotRx) {
        Session::new(SessionConfig {
            roots: vec![dir.to_path_buf()],
            ..SessionConfig::default()
        })
    }

    fn literal_query(pattern: &str) -> karet_search::SearchQuery {
        karet_search::SearchQuery {
            pattern: pattern.to_owned(),
            case_sensitive: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_search_streams_hits_carrying_their_matched_line() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.rs"), "\tlet needle = 1;\n")?;
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, finished) = drain_search(&mut events);

        assert_eq!(hits.len(), 1, "{hits:?}");
        let m = &hits[0].matches[0];
        // Indentation is trimmed and the offsets follow it.
        assert_eq!(m.line_text, "let needle = 1;");
        assert_eq!(
            m.line_text
                .get(m.preview_start as usize..m.preview_end as usize),
            Some("needle")
        );
        assert!(matches!(
            finished,
            Some(Event::SearchFinished {
                matches_found: 1,
                truncated: false,
                cancelled: false,
                error: None,
                ..
            })
        ));
        Ok(())
    }

    /// The cap must stop the *walk*, not merely trim the list — that is the whole
    /// point on a large repository.
    #[test]
    fn a_search_stops_at_the_match_limit_and_reports_truncation() -> TestResult {
        let dir = tempfile::tempdir()?;
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "needle\nneedle\n")?;
        }
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 3,
            },
        );
        let (_, finished) = drain_search(&mut events);

        let Some(Event::SearchFinished {
            truncated,
            files_scanned,
            ..
        }) = finished
        else {
            return Err("expected SearchFinished".into());
        };
        assert!(truncated, "the cap was hit, so the panel must be told");
        assert!(
            files_scanned < 5,
            "the walk stopped early rather than reading all 5 files: {files_scanned}"
        );
        Ok(())
    }

    #[test]
    fn a_search_stops_at_the_file_limit_too() -> TestResult {
        let dir = tempfile::tempdir()?;
        for i in 0..4 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "needle\n")?;
        }
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("needle"),
                file_limit: 2,
                match_limit: 1000,
            },
        );
        let (hits, finished) = drain_search(&mut events);

        assert_eq!(hits.len(), 2);
        assert!(matches!(
            finished,
            Some(Event::SearchFinished {
                truncated: true,
                ..
            })
        ));
        Ok(())
    }

    /// An invalid regex used to be swallowed and read as "no matches".
    #[test]
    fn an_invalid_pattern_reports_an_error_rather_than_emptiness() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.rs"), "needle\n")?;
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: karet_search::SearchQuery {
                    pattern: "(".to_owned(),
                    regex: true,
                    ..Default::default()
                },
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, finished) = drain_search(&mut events);

        assert!(hits.is_empty());
        let Some(Event::SearchFinished { error, .. }) = finished else {
            return Err("expected SearchFinished".into());
        };
        assert!(error.is_some(), "a bad pattern must say so");
        Ok(())
    }

    #[test]
    fn an_empty_pattern_answers_without_walking() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query(""),
                file_limit: 100,
                match_limit: 100,
            },
        );

        assert!(matches!(
            events.try_recv(),
            Some((
                Some(RequestId(1)),
                Event::SearchFinished {
                    files_scanned: 0,
                    error: None,
                    ..
                }
            )),
        ));
        Ok(())
    }

    /// The query's own excludes are additive to the configured ones, so narrowing
    /// a search never widens it past the project's settings.
    #[test]
    fn configured_excludes_apply_alongside_the_query_s_own() -> TestResult {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir_all(dir.path().join("vendor"))?;
        std::fs::write(dir.path().join("vendor/v.rs"), "needle\n")?;
        std::fs::write(dir.path().join("a.rs"), "needle\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            settings: {
                let mut settings = crate::config::Settings::default();
                settings.search.exclude = vec!["vendor/**".to_owned()];
                settings
            },
            ..SessionConfig::default()
        });

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, _) = drain_search(&mut events);

        assert_eq!(hits.len(), 1, "vendor is excluded by settings: {hits:?}");
        assert!(hits[0].path.ends_with("a.rs"));
        Ok(())
    }

    /// Every root is searched; the dispatch used to take `roots.first()` only.
    #[test]
    fn a_search_covers_every_workspace_root() -> TestResult {
        let (one, two) = (tempfile::tempdir()?, tempfile::tempdir()?);
        std::fs::write(one.path().join("a.rs"), "needle\n")?;
        std::fs::write(two.path().join("b.rs"), "needle\n")?;
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![one.path().to_path_buf(), two.path().to_path_buf()],
            ..SessionConfig::default()
        });

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, _) = drain_search(&mut events);

        assert_eq!(hits.len(), 2, "both roots contribute: {hits:?}");
        Ok(())
    }

    /// The match cap must bound the previews actually *built*, not merely trim the
    /// answer afterwards. One 10 MiB minified line can hold a million matches, and
    /// converting them all first allocates a preview `String` per match before the
    /// cap ever gets a look in.
    #[test]
    fn the_match_cap_bounds_the_previews_built_for_a_single_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        // One line, 20_000 matches — the shape of a minified bundle.
        std::fs::write(dir.path().join("bundle.js"), "a".repeat(20_000))?;
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            WorkspaceCommand::Search {
                query: literal_query("a"),
                file_limit: 100,
                match_limit: 10,
            },
        );
        let (hits, finished) = drain_search(&mut events);

        let built: usize = hits.iter().map(|hit| hit.matches.len()).sum();
        assert!(
            built <= 10,
            "only the budgeted previews are built, got {built}"
        );
        let Some(Event::SearchFinished {
            matches_found,
            truncated,
            ..
        }) = finished
        else {
            return Err("expected SearchFinished".into());
        };
        assert!(truncated);
        assert_eq!(
            matches_found, 20_000,
            "the count stays honest even though the previews are capped"
        );
        Ok(())
    }
