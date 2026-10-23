    // The destructive half of workspace search: `SearchReplaceAll` rewrites
    // files on disk, so its reach (every root, never an excluded path) and its
    // answer (`SearchReplaced` counts) are pinned at the seam.

    /// Wait (5s) for the `SearchReplaced` answering `id`.
    fn wait_replaced(events: &mut EventRx, id: RequestId) -> Option<(usize, usize)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match events.try_recv() {
                Some((
                    tag,
                    Event::SearchReplaced {
                        files_changed,
                        replacements,
                    },
                )) if tag == Some(id) => return Some((files_changed, replacements)),
                Some(_) => {},
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        None
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
                }
            ))
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
                }
            ))
        ));
        Ok(())
    }
