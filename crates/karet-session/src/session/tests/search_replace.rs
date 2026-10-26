    // The destructive half of workspace search: `SearchReplaceAll` rewrites
    // files on disk, so its reach (every root, never an excluded path) and its
    // answer (`SearchReplaced` counts) are pinned at the seam.

    /// The `SearchReplaced` answering `id`, or `None` past [`EVENT_DEADLINE`].
    fn wait_replaced(events: &mut EventRx, id: RequestId) -> Option<(usize, usize)> {
        wait_replaced_fully(events, id).and_then(|(files, replacements, failures, error)| {
            (failures == 0 && error.is_none()).then_some((files, replacements))
        })
    }

    /// The whole `SearchReplaced` answering `id`: counts, how many files failed to
    /// be written, and the error, or `None` past [`EVENT_DEADLINE`].
    fn wait_replaced_fully(
        events: &mut EventRx,
        id: RequestId,
    ) -> Option<(usize, usize, usize, Option<String>)> {
        await_event(events, EVENT_DEADLINE, |tag, event| match event {
            Event::SearchReplaced {
                files_changed,
                replacements,
                failures,
                error,
            } if tag == Some(id) => Some((files_changed, replacements, failures.len(), error)),
            _ => None,
        })
    }

    fn replace_all(pattern: &str, replacement: &str) -> Command {
        Command::SearchReplaceAll {
            query: karet_search::SearchQuery {
                pattern: pattern.to_owned(),
                case_sensitive: true,
                ..Default::default()
            },
            replacement: replacement.to_owned(),
        }
    }

    #[test]
    fn replace_all_rewrites_every_root_and_spares_excluded_files() -> TestResult {
        let (one, two) = (tempfile::tempdir()?, tempfile::tempdir()?);
        let excluded = one.path().join("vendor").join("v.rs");
        std::fs::create_dir_all(one.path().join("vendor"))?;
        std::fs::write(one.path().join("a.rs"), "needle needle\n")?;
        std::fs::write(two.path().join("b.rs"), "needle\n")?;
        std::fs::write(&excluded, "needle\n")?;
        let mut settings = crate::config::Settings::default();
        settings.search.exclude = vec!["vendor/**".to_owned()];
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![one.path().to_path_buf(), two.path().to_path_buf()],
            settings,
            ..SessionConfig::default()
        });

        session.handle(RequestId(1), replace_all("needle", "pin"));

        assert_eq!(
            wait_replaced(&mut events, RequestId(1)),
            Some((2, 3)),
            "two files written, three replacements, across both roots"
        );
        assert_eq!(std::fs::read_to_string(one.path().join("a.rs"))?, "pin pin\n");
        assert_eq!(std::fs::read_to_string(two.path().join("b.rs"))?, "pin\n");
        assert_eq!(
            std::fs::read_to_string(excluded)?,
            "needle\n",
            "search.exclude must hold for the destructive path too"
        );
        Ok(())
    }

    /// An empty pattern would match everywhere; it must answer without walking,
    /// and with a session that has no roots at all, the same zero answer.
    #[test]
    fn replace_all_with_no_pattern_or_no_root_answers_zero_without_writing() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "needle\n")?;
        let (mut session, mut events, _snaps) = search_session(dir.path());
        session.handle(RequestId(1), replace_all("", "pin"));
        assert!(matches!(
            events.try_recv(),
            Some((
                Some(RequestId(1)),
                Event::SearchReplaced {
                    files_changed: 0,
                    replacements: 0,
                    ref failures,
                    error: None,
                }
            )) if failures.is_empty()
        ));
        assert_eq!(std::fs::read_to_string(&file)?, "needle\n");

        let (mut rootless, mut events, _snaps) = Session::new(SessionConfig::default());
        rootless.handle(RequestId(2), replace_all("needle", "pin"));
        assert!(matches!(
            events.try_recv(),
            Some((
                Some(RequestId(2)),
                Event::SearchReplaced {
                    files_changed: 0,
                    replacements: 0,
                    ref failures,
                    error: None,
                }
            )) if failures.is_empty()
        ));
        Ok(())
    }

    /// A replace whose pattern cannot compile answers with the reason, not with
    /// a "0 replacements" indistinguishable from a query that matched nothing.
    #[test]
    fn replace_all_with_an_invalid_regex_answers_with_the_error() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "needle\n")?;
        let (mut session, mut events, _snaps) = search_session(dir.path());
        session.handle(
            RequestId(1),
            Command::SearchReplaceAll {
                query: karet_search::SearchQuery {
                    pattern: "(".to_owned(),
                    regex: true,
                    ..Default::default()
                },
                replacement: "pin".to_owned(),
            },
        );
        assert_eq!(
            wait_replaced_fully(&mut events, RequestId(1)),
            Some((0, 0, 0, Some("invalid search pattern".to_owned())))
        );
        assert_eq!(std::fs::read_to_string(&file)?, "needle\n");
        Ok(())
    }
