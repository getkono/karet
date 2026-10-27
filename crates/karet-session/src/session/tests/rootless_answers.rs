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

    /// Any event at all: for rows that only need to prove the id is resolved.
    fn any_answer(_: &Event) -> bool {
        true
    }

    /// Every root- or repository-dependent request, with the answer it must get.
    #[allow(clippy::type_complexity)] // A table of (label, command, check) rows.
    fn rootless_requests() -> Vec<(&'static str, Command, fn(&Event) -> bool)> {
        vec![
            (
                "IndexSeams",
                SeamCommand::Index {
                    root: None,
                    mode: crate::api::SeamSync::Incremental,
                }
                .into(),
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            (
                "ReindexSeams",
                SeamCommand::Reindex {
                    path: PathBuf::from("lib.rs"),
                    text: String::new(),
                }
                .into(),
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            (
                "SetSeamConfiguration",
                SeamCommand::SetConfiguration {
                    name: "default".to_owned(),
                }
                .into(),
                |event| matches!(event, Event::SeamIndexFailed { .. }),
            ),
            (
                "DependencyGraph",
                WorkspaceCommand::DependencyGraph.into(),
                is_error_notice,
            ),
            (
                "Stage",
                VcsCommand::Stage {
                    paths: vec![PathBuf::from("a.txt")],
                }
                .into(),
                is_error_notice,
            ),
            (
                "Unstage",
                VcsCommand::Unstage {
                    paths: vec![PathBuf::from("a.txt")],
                }
                .into(),
                is_error_notice,
            ),
            (
                "Discard",
                VcsCommand::Discard {
                    paths: vec![PathBuf::from("a.txt")],
                }
                .into(),
                is_error_notice,
            ),
            (
                "ApplyIndexPatch",
                VcsCommand::ApplyIndexPatch {
                    patch: String::new(),
                    reverse: false,
                }
                .into(),
                is_error_notice,
            ),
            ("StageAll", VcsCommand::StageAll.into(), is_error_notice),
            ("UnstageAll", VcsCommand::UnstageAll.into(), is_error_notice),
            ("RefreshVcs", VcsCommand::Refresh.into(), |event| {
                matches!(event, Event::VcsStatus { staged, working }
                    if staged.is_empty() && working.is_empty())
            }),
            (
                "VcsLog",
                VcsCommand::Log { skip: 0, limit: 10 }.into(),
                is_error_notice,
            ),
            (
                "RepositorySnapshot",
                VcsCommand::RepositorySnapshot.into(),
                is_error_notice,
            ),
            (
                "CommitDetail",
                VcsCommand::CommitDetail {
                    rev: "HEAD".to_owned(),
                }
                .into(),
                is_error_notice,
            ),
            (
                "FileHistory",
                VcsCommand::FileHistory {
                    path: PathBuf::from("a.txt"),
                    skip: 0,
                    limit: 10,
                }
                .into(),
                is_error_notice,
            ),
            // Rows whose answer shape other tests pin; here only that one arrives.
            (
                "Commit",
                VcsCommand::Commit {
                    message: "c".to_owned(),
                }
                .into(),
                any_answer,
            ),
            (
                "PrepareChange",
                VcsCommand::PrepareChange {
                    path: PathBuf::from("a.txt"),
                    staged: false,
                }
                .into(),
                |event| matches!(event, Event::ChangePrepared { result: Err(_), .. }),
            ),
            (
                "MergeConflict",
                VcsCommand::MergeConflict {
                    path: PathBuf::from("a.txt"),
                }
                .into(),
                any_answer,
            ),
            (
                "NestedRepositoryStatus",
                VcsCommand::NestedRepositoryStatus {
                    path: PathBuf::from("nested"),
                }
                .into(),
                is_error_notice,
            ),
            (
                "PullRequests",
                VcsCommand::PullRequests {
                    remote: "origin".to_owned(),
                    page: 1,
                    per_page: 10,
                }
                .into(),
                is_error_notice,
            ),
            (
                "Search",
                WorkspaceCommand::Search {
                    query: literal_query("needle"),
                    file_limit: 10,
                    match_limit: 10,
                }
                .into(),
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
        session.handle(RequestId(2), SeamCommand::Reindex {
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
