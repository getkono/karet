    // The workspace search's Command/Event contract. The preview transform is
    // covered hermetically in `crate::search_worker::preview`; these cover the
    // seam: streaming, the caps, error reporting, and the configured excludes.

    /// Drain events until `SearchFinished` arrives, or the deadline passes.
    ///
    /// The walk runs on its own thread, so a bare `try_recv` would race it.
    fn drain_search(events: &mut EventRx) -> (Vec<crate::api::SearchHit>, Option<Event>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut hits = Vec::new();
        while std::time::Instant::now() < deadline {
            match events.try_recv() {
                Some((_, Event::SearchProgress { hits: batch, .. })) => hits.extend(batch),
                Some((_, finished @ Event::SearchFinished { .. })) => return (hits, Some(finished)),
                Some(_) => {},
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        (hits, None)
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
    fn a_search_streams_hits_carrying_their_matched_line() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let _ = std::fs::write(dir.path().join("a.rs"), "\tlet needle = 1;\n");
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            Command::Search {
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
    }

    /// The cap must stop the *walk*, not merely trim the list — that is the whole
    /// point on a large repository.
    #[test]
    fn a_search_stops_at_the_match_limit_and_reports_truncation() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        for i in 0..5 {
            let _ = std::fs::write(dir.path().join(format!("f{i}.rs")), "needle\nneedle\n");
        }
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            Command::Search {
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
            unreachable!("expected SearchFinished")
        };
        assert!(truncated, "the cap was hit, so the panel must be told");
        assert!(
            files_scanned < 5,
            "the walk stopped early rather than reading all 5 files: {files_scanned}"
        );
    }

    #[test]
    fn a_search_stops_at_the_file_limit_too() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        for i in 0..4 {
            let _ = std::fs::write(dir.path().join(format!("f{i}.rs")), "needle\n");
        }
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            Command::Search {
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
    }

    /// An invalid regex used to be swallowed and read as "no matches".
    #[test]
    fn an_invalid_pattern_reports_an_error_rather_than_emptiness() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let _ = std::fs::write(dir.path().join("a.rs"), "needle\n");
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            Command::Search {
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
            unreachable!("expected SearchFinished")
        };
        assert!(error.is_some(), "a bad pattern must say so");
    }

    #[test]
    fn an_empty_pattern_answers_without_walking() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let (mut session, mut events, _snaps) = search_session(dir.path());

        session.handle(
            RequestId(1),
            Command::Search {
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
    }

    /// The query's own excludes are additive to the configured ones, so narrowing
    /// a search never widens it past the project's settings.
    #[test]
    fn configured_excludes_apply_alongside_the_query_s_own() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let _ = std::fs::create_dir_all(dir.path().join("vendor"));
        let _ = std::fs::write(dir.path().join("vendor/v.rs"), "needle\n");
        let _ = std::fs::write(dir.path().join("a.rs"), "needle\n");
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
            Command::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, _) = drain_search(&mut events);

        assert_eq!(hits.len(), 1, "vendor is excluded by settings: {hits:?}");
        assert!(hits[0].path.ends_with("a.rs"));
    }

    /// Every root is searched; the dispatch used to take `roots.first()` only.
    #[test]
    fn a_search_covers_every_workspace_root() {
        let (Ok(one), Ok(two)) = (tempfile::tempdir(), tempfile::tempdir()) else {
            return;
        };
        let _ = std::fs::write(one.path().join("a.rs"), "needle\n");
        let _ = std::fs::write(two.path().join("b.rs"), "needle\n");
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![one.path().to_path_buf(), two.path().to_path_buf()],
            ..SessionConfig::default()
        });

        session.handle(
            RequestId(1),
            Command::Search {
                query: literal_query("needle"),
                file_limit: 100,
                match_limit: 100,
            },
        );
        let (hits, _) = drain_search(&mut events);

        assert_eq!(hits.len(), 2, "both roots contribute: {hits:?}");
    }
