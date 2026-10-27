    // The GitHub commands' answer for a workspace that is not an eligible
    // github.com worktree: every request is still answered on its own id, so a
    // panel waiting on one never hangs.

    #[cfg(feature = "github")]
    #[test]
    fn every_github_command_answers_an_ineligible_workspace() {
        use crate::api::GithubCommand;
        use crate::api::GithubNewIssue;
        use crate::api::GithubNewPullRequest;
        use crate::api::GithubToken;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let commands = [
            GithubCommand::Login {
                token: GithubToken::new("github_pat_never_sent".to_owned()),
            },
            GithubCommand::SearchIssues {
                query: "is:open".to_owned(),
                page: 1,
            },
            GithubCommand::SearchPullRequests {
                query: "is:open".to_owned(),
                page: 1,
            },
            GithubCommand::ListOpenPullRequests {
                remote: "origin".to_owned(),
                page: 1,
            },
            GithubCommand::Actions { page: 1 },
            GithubCommand::Issue { number: 1 },
            GithubCommand::PullRequest { number: 1 },
            GithubCommand::UpdatePullRequestBody {
                number: 1,
                body: String::new(),
            },
            GithubCommand::CommentPullRequest {
                number: 1,
                body: String::new(),
            },
            GithubCommand::MergePullRequest {
                number: 1,
                head_sha: "0".repeat(40),
            },
            GithubCommand::SetPullRequestDraft {
                node_id: "PR_1".to_owned(),
                number: 1,
                draft: true,
            },
            GithubCommand::IssueMetadata,
            GithubCommand::CreateIssue {
                issue: GithubNewIssue::default(),
            },
            GithubCommand::CreatePullRequest {
                pull_request: GithubNewPullRequest::default(),
            },
        ];
        let sent = commands.len();
        for (n, command) in commands.into_iter().enumerate() {
            session.handle(RequestId(n as u64 + 1), command);
        }

        let answers: Vec<_> = std::iter::from_fn(|| events.try_recv()).collect();
        assert_eq!(answers.len(), sent, "{answers:?}");
        for (n, (id, event)) in answers.into_iter().enumerate() {
            assert_eq!(id, Some(RequestId(n as u64 + 1)));
            assert!(
                matches!(
                    &event,
                    Event::GithubError { operation, message }
                        if *operation == crate::api::GithubOperation::Eligibility && message.contains("not an eligible")
                ),
                "{event:?}"
            );
            // The token must never come back out, even inside an error.
            assert!(!format!("{event:?}").contains("github_pat_never_sent"));
        }
    }

    /// Refreshing an ineligible workspace is an availability answer, not an
    /// error: the pinned view hides rather than raising a notification.
    #[cfg(feature = "github")]
    #[test]
    fn refreshing_an_ineligible_workspace_reports_it_unavailable() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());

        session.handle(RequestId(1), crate::api::GithubCommand::Refresh);

        let answer = events.try_recv();
        assert!(
            matches!(
                &answer,
                Some((
                    Some(RequestId(1)),
                    Event::GithubAvailability {
                        repository: None,
                        auth: crate::api::GithubAuth {
                            source: crate::api::GithubAuthSource::Anonymous,
                            can_write: false,
                            ..
                        },
                    }
                ))
            ),
            "{answer:?}"
        );
        assert!(events.try_recv().is_none(), "exactly one answer");
    }

    /// A build without the `github` feature answers every GitHub command, on
    /// its own id, with a `FeatureDisabled` error -- the one
    /// `Command::Github(_)` arm, so a panel waiting on any of them never hangs.
    #[cfg(not(feature = "github"))]
    #[test]
    fn every_github_command_answers_feature_disabled_without_the_feature() {
        use crate::api::GithubCommand;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let commands = [
            GithubCommand::Refresh,
            GithubCommand::Issue { number: 1 },
            GithubCommand::IssueMetadata,
        ];
        let sent = commands.len();
        for (n, command) in commands.into_iter().enumerate() {
            session.handle(RequestId(n as u64 + 1), command);
        }

        let answers: Vec<_> = std::iter::from_fn(|| events.try_recv()).collect();
        assert_eq!(answers.len(), sent, "{answers:?}");
        for (n, (id, event)) in answers.into_iter().enumerate() {
            assert_eq!(id, Some(RequestId(n as u64 + 1)));
            assert!(
                matches!(
                    &event,
                    Event::GithubError { operation, .. }
                        if *operation == crate::api::GithubOperation::FeatureDisabled
                ),
                "{event:?}"
            );
        }
    }
