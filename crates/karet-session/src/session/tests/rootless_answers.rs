    // The `Backend` answer rule (see "Which commands answer" on the trait): a
    // request is answered even when the session has no workspace root and no
    // repository, so a client never waits on an id nobody will resolve.

    /// An error notification, whatever its text.
    fn is_error_notice(event: &Event) -> bool {
        matches!(event, Event::Notification {
            severity: Severity::Error,
            ..
        })
    }

    /// Every root- or repository-dependent request, with the answer it must get.
    #[allow(clippy::type_complexity)] // A table of (label, command, check) rows.
    fn rootless_requests() -> Vec<(&'static str, Command, fn(&Event) -> bool)> {
        vec![
            (
                "IndexSeams",
                Command::IndexSeams {
                    root: None,
                    mode: crate::api::SeamSync::Incremental,
                },
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            (
                "ReindexSeams",
                Command::ReindexSeams {
                    path: PathBuf::from("lib.rs"),
                    text: String::new(),
                },
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            (
                "SetSeamConfiguration",
                Command::SetSeamConfiguration {
                    name: "default".to_owned(),
                },
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            ("DependencyGraph", Command::DependencyGraph, is_error_notice),
            (
                "Stage",
                Command::Stage {
                    paths: vec![PathBuf::from("a.txt")],
                },
                is_error_notice,
            ),
            (
                "Unstage",
                Command::Unstage {
                    paths: vec![PathBuf::from("a.txt")],
                },
                is_error_notice,
            ),
            (
                "Discard",
                Command::Discard {
                    paths: vec![PathBuf::from("a.txt")],
                },
                is_error_notice,
            ),
            (
                "ApplyIndexPatch",
                Command::ApplyIndexPatch {
                    patch: String::new(),
                    reverse: false,
                },
                is_error_notice,
            ),
            ("StageAll", Command::StageAll, is_error_notice),
            ("UnstageAll", Command::UnstageAll, is_error_notice),
            ("RefreshVcs", Command::RefreshVcs, |event| {
                matches!(event, Event::VcsStatus { staged, working }
                    if staged.is_empty() && working.is_empty())
            }),
            (
                "VcsLog",
                Command::VcsLog { skip: 0, limit: 10 },
                is_error_notice,
            ),
            ("RepositorySnapshot", Command::RepositorySnapshot, is_error_notice),
            (
                "CommitDetail",
                Command::CommitDetail {
                    rev: "HEAD".to_owned(),
                },
                is_error_notice,
            ),
            (
                "FileHistory",
                Command::FileHistory {
                    path: PathBuf::from("a.txt"),
                    skip: 0,
                    limit: 10,
                },
                is_error_notice,
            ),
            (
                "Search",
                Command::Search {
                    query: literal_query("needle"),
                    file_limit: 10,
                    match_limit: 10,
                },
                |event| matches!(event, Event::SearchFinished { .. }),
            ),
        ]
    }

    #[test]
    fn every_request_answers_without_a_root_or_a_repository() -> TestResult {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let mut unanswered = Vec::new();
        for (index, (label, command, check)) in rootless_requests().into_iter().enumerate() {
            let id = RequestId(index as u64 + 1);
            session.handle(id, command);
            let answer = wait_answer(&mut events, id, |event| check(&event).then_some(()));
            if answer.is_none() {
                unanswered.push(label);
            }
        }
        assert!(
            unanswered.is_empty(),
            "left unanswered without a root or repository: {unanswered:?}"
        );
        Ok(())
    }

    #[test]
    fn a_re_index_that_cannot_parse_answers_with_the_standing_tree() -> TestResult {
        let (mut session, mut events, dir, first) = indexed_session()?;
        let Event::SeamIndexed { nodes: before, .. } = first else {
            return Err(format!("expected SeamIndexed, got {first:?}").into());
        };
        // No language maps this file, so the re-index fails; the request must
        // still answer, with the tree as it stood.
        session.handle(RequestId(2), Command::ReindexSeams {
            path: dir.path().join("notes.unmapped"),
            text: "not source\n".to_owned(),
        });
        let event = next_seam_answer(&mut events, "standing SeamIndexed")?;
        let Event::SeamIndexed { nodes: after, .. } = event else {
            return Err(format!("expected SeamIndexed, got {event:?}").into());
        };
        let ids = |nodes: &[crate::api::SeamNodeView]| {
            nodes.iter().map(|node| node.id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(&before), ids(&after));
        Ok(())
    }
