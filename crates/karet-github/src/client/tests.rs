use super::*;

fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        owner: "getkono".to_string(),
        repo: "karet".to_string(),
    }
}

#[test]
fn search_query_is_forced_to_repository_and_kind() -> Result<(), GitHubError> {
    let query = scoped_query(&repository(), SearchKind::Issue, "label:bug is:open")?;
    assert_eq!(query, "repo:getkono/karet is:issue label:bug is:open");
    Ok(())
}

#[test]
fn search_query_rejects_scope_escape_but_not_quoted_text() -> Result<(), GitHubError> {
    assert!(matches!(
        scoped_query(&repository(), SearchKind::Issue, "repo:elsewhere/project"),
        Err(GitHubError::QueryScope(_))
    ));
    let query = scoped_query(
        &repository(),
        SearchKind::PullRequest,
        "\"repo:mentioned/in prose\" review:required",
    )?;
    assert!(query.contains("is:pr"));
    Ok(())
}

#[test]
fn parses_next_link_and_rate_limit_headers() -> Result<(), GitHubError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "link",
        HeaderValue::from_static(
            "<https://api.github.com/search/issues?page=2>; rel=\"next\", <https://api.github.com/search/issues?page=8>; rel=\"last\"",
        ),
    );
    headers.insert("x-ratelimit-limit", HeaderValue::from_static("30"));
    headers.insert("x-ratelimit-remaining", HeaderValue::from_static("29"));
    assert_eq!(parse_next_page(&headers), Some(2));
    assert_eq!(rate_limit(&headers).remaining, Some(29));
    Ok(())
}

#[test]
fn issue_blocked_state_uses_total_or_page_count() -> Result<(), GitHubError> {
    let mut issue: IssueSummary = serde_json::from_str(
        r#"{
            "number": 1,
            "title": "Blocked",
            "state": "open",
            "user": { "id": 42, "login": "octocat" },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "issue_dependencies_summary": {
                "blocked_by": 0,
                "blocking": 0,
                "total_blocked_by": 2,
                "total_blocking": 0
            }
        }"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    assert!(issue.is_blocked());
    assert_eq!(issue.user.as_ref().map(|user| user.id), Some(42));
    issue.issue_dependencies_summary = None;
    assert!(!issue.is_blocked());
    Ok(())
}

#[test]
fn actions_responses_accept_github_rfc3339_timestamps() -> Result<(), GitHubError> {
    let workflows: WorkflowResponse = serde_json::from_str(
        r#"{
            "total_count": 1,
            "workflows": [{
                "id": 7,
                "name": "CI",
                "path": ".github/workflows/ci.yml",
                "state": "active",
                "html_url": "https://github.com/o/r/actions/workflows/ci.yml",
                "updated_at": "2026-07-20T14:59:58Z"
            }]
        }"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    assert_eq!(workflows.workflows[0].name, "CI");

    let runs: WorkflowRunsResponse = serde_json::from_str(
        r#"{
            "total_count": 1,
            "workflow_runs": [{
                "id": 9,
                "workflow_id": 7,
                "display_title": "Check the build",
                "head_branch": "main",
                "head_sha": "abc123",
                "event": "push",
                "status": "completed",
                "conclusion": "success",
                "actor": {"id": 42, "login": "octocat"},
                "run_number": 11,
                "created_at": "2026-07-20T14:56:46Z",
                "updated_at": "2026-07-20T14:59:58Z",
                "html_url": "https://github.com/o/r/actions/runs/9"
            }]
        }"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    assert_eq!(runs.workflow_runs[0].run_number, 11);
    Ok(())
}

#[test]
fn pull_request_detail_accepts_github_rfc3339_timestamps() -> Result<(), GitHubError> {
    let response: PullRequestResponse = serde_json::from_str(
        r#"{
            "number": 12,
            "title": "Readable detail",
            "body": "**Markdown**",
            "state": "open",
            "user": {"id": 42, "login": "octocat"},
            "created_at": "2026-07-20T14:56:46Z",
            "updated_at": "2026-07-20T14:59:58Z",
            "labels": [],
            "draft": false,
            "node_id": "PR_node",
            "head": {"sha": "bbbbbbbb"},
            "base": {"sha": "aaaaaaaa"},
            "mergeable": true,
            "merged": false,
            "html_url": "https://github.com/o/r/pull/12"
        }"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    let pull = response.into_summary();
    assert_eq!(pull.creator.as_deref(), Some("octocat"));
    assert_eq!(pull.number, 12);
    assert_eq!(pull.node_id, "PR_node");
    assert_eq!(pull.head_sha, "bbbbbbbb");
    assert_eq!(pull.base_sha, "aaaaaaaa");
    assert_eq!(pull.mergeable, Some(true));
    assert!(!pull.merged);
    Ok(())
}

#[test]
fn pull_request_conversation_support_shapes_are_strictly_typed() -> Result<(), GitHubError> {
    let commits: Vec<PullRequestCommitResponse> = serde_json::from_str(
        r#"[{
            "sha": "bbbbbbbb",
            "commit": {
                "message": "Add feature\n\nDetails",
                "author": {"name": "Octo Cat", "date": "2026-07-20T14:56:46Z"},
                "committer": null
            },
            "parents": [{"sha": "aaaaaaaa"}],
            "html_url": "https://github.com/o/r/commit/bbbbbbbb"
        }]"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    let checks: CheckRunsResponse = serde_json::from_str(
        r#"{
            "total_count": 1,
            "check_runs": [{
                "id": 9,
                "name": "CI / tests",
                "status": "completed",
                "conclusion": "success",
                "html_url": "https://github.com/o/r/runs/9"
            }]
        }"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    let timeline: Vec<TimelineEventResponse> = serde_json::from_str(
        r#"[{
            "id": 3,
            "event": "head_ref_force_pushed",
            "actor": {"id": 42, "login": "octocat"},
            "before": "11111111",
            "after": "22222222",
            "created_at": "2026-07-20T14:59:58Z"
        }]"#,
    )
    .map_err(|error| GitHubError::Decode(error.to_string()))?;
    assert_eq!(
        commits[0].commit.message.lines().next(),
        Some("Add feature")
    );
    assert_eq!(checks.total_count, 1);
    assert_eq!(checks.check_runs[0].conclusion.as_deref(), Some("success"));
    assert_eq!(timeline[0].event.as_deref(), Some("head_ref_force_pushed"));
    assert_eq!(timeline[0].before.as_deref(), Some("11111111"));
    Ok(())
}

#[tokio::test]
async fn pull_request_mutations_require_authentication_before_transport() -> Result<(), GitHubError>
{
    let client = GitHubClient::with_token(None)?;
    let repository = repository();
    assert!(matches!(
        client
            .update_pull_request_body(&repository, 12, "body".to_string())
            .await,
        Err(GitHubError::AuthenticationRequired)
    ));
    assert!(matches!(
        client
            .create_pull_request_comment(&repository, 12, "comment".to_string())
            .await,
        Err(GitHubError::AuthenticationRequired)
    ));
    assert!(matches!(
        client
            .merge_pull_request(&repository, 12, "bbbbbbbb".to_string())
            .await,
        Err(GitHubError::AuthenticationRequired)
    ));
    assert!(matches!(
        client
            .set_pull_request_draft("PR_node".to_string(), true)
            .await,
        Err(GitHubError::AuthenticationRequired)
    ));
    Ok(())
}
