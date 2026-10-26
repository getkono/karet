    // The GitHub commands' answer for a workspace that is not an eligible
    // github.com worktree: every request is still answered on its own id, so a
    // panel waiting on one never hangs.

    #[cfg(feature = "github")]
    #[test]
    fn every_github_command_answers_an_ineligible_workspace() {
        use crate::api::GithubNewIssue;
        use crate::api::GithubNewPullRequest;
        use crate::api::GithubToken;

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let commands = [
            Command::GithubLogin {
                token: GithubToken::new("github_pat_never_sent".to_owned()),
            },
            Command::GithubSearchIssues {
                query: "is:open".to_owned(),
                page: 1,
            },
            Command::GithubSearchPullRequests {
                query: "is:open".to_owned(),
                page: 1,
            },
            Command::GithubActions { page: 1 },
            Command::GithubIssue { number: 1 },
            Command::GithubPullRequest { number: 1 },
            Command::GithubUpdatePullRequestBody {
                number: 1,
                body: String::new(),
            },
            Command::GithubCommentPullRequest {
                number: 1,
                body: String::new(),
            },
            Command::GithubMergePullRequest {
                number: 1,
                head_sha: "0".repeat(40),
            },
            Command::GithubSetPullRequestDraft {
                node_id: "PR_1".to_owned(),
                number: 1,
                draft: true,
            },
            Command::GithubIssueMetadata,
            Command::GithubCreateIssue {
                issue: GithubNewIssue::default(),
            },
            Command::GithubCreatePullRequest {
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

        session.handle(RequestId(1), Command::GithubRefresh);

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
