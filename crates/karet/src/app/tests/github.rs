use karet_session::GithubAuth;
use karet_session::GithubAuthSource;
use karet_session::GithubCheckRun;
use karet_session::GithubIssue;
use karet_session::GithubPage;
use karet_session::GithubPullRequest;
use karet_session::GithubPullRequestActivity;
use karet_session::GithubPullRequestCommit;
use karet_session::GithubRepository;
use karet_session::GithubWorkflow;
use karet_session::GithubWorkflowRun;

fn repository() -> GithubRepository {
    GithubRepository {
        owner: "getkono".to_string(),
        repo: "karet".to_string(),
    }
}

fn anonymous_auth() -> GithubAuth {
    GithubAuth {
        source: GithubAuthSource::Anonymous,
        can_write: false,
        viewer_id: None,
        viewer_login: None,
    }
}

fn issue(number: u64) -> GithubIssue {
    GithubIssue {
        number,
        title: format!("Issue {number}"),
        body: Some("description".to_string()),
        state: "open".to_string(),
        creator: Some("octocat".to_string()),
        creator_id: Some(1),
        created_unix: 1,
        updated_unix: 2,
        labels: Vec::new(),
        blocked: false,
        html_url: format!("https://github.com/getkono/karet/issues/{number}"),
    }
}

fn pull_request(number: u64, draft: bool) -> GithubPullRequest {
    GithubPullRequest {
        number,
        title: format!("Pull request {number}"),
        body: Some("PR description".to_string()),
        state: "open".to_string(),
        creator: Some("octocat".to_string()),
        creator_id: Some(1),
        created_unix: 1,
        updated_unix: 2,
        labels: Vec::new(),
        draft,
        node_id: "PR_node".to_string(),
        head_sha: "bbbbbbbb".to_string(),
        base_sha: "aaaaaaaa".to_string(),
        mergeable: Some(true),
        merged: false,
        html_url: format!("https://github.com/getkono/karet/pull/{number}"),
    }
}

#[test]
fn github_dashboard_is_singleton_leftmost_and_uncloseable() {
    let mut app = app();
    let repository = repository();
    let auth = anonymous_auth();
    app.apply_github_availability(Some(repository.clone()), auth.clone());
    app.apply_github_availability(Some(repository), auth);

    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].is_github_dashboard());
    app.request_close_active_tab();
    assert!(app.tabs[0].is_github_dashboard());

    app.push_tab(Tab::welcome());
    assert!(app.tabs[0].is_github_dashboard());
    app.move_tab(0, 1);
    assert!(app.tabs[0].is_github_dashboard());
    app.close_all_tabs();
    assert_eq!(app.tabs.len(), 1);
    assert!(app.tabs[0].is_github_dashboard());
}

#[test]
fn github_dashboard_disappears_when_repository_becomes_ineligible() {
    let mut app = app();
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.push_tab(Tab::welcome());
    app.apply_github_availability(None, anonymous_auth());

    assert!(!app.all_tabs().any(Tab::is_github_dashboard));
    assert!(!app.tabs.is_empty());
}

#[test]
fn github_dashboard_opens_a_masked_in_tui_sign_in_control() {
    let mut app = app();
    app.apply_github_availability(Some(repository()), anonymous_auth());

    assert!(app.github_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)));

    let dashboard = app.tabs.first().and_then(|tab| match &tab.kind {
        TabKind::Github(crate::app::github::GithubViewState::Dashboard(dashboard)) => {
            Some(dashboard)
        },
        _ => None,
    });
    assert!(dashboard.is_some_and(|dashboard| dashboard.login_editing));
    assert_eq!(
        dashboard.map(|dashboard| dashboard.login_token.as_str()),
        Some("se")
    );
}

#[test]
fn github_issue_table_supports_keyboard_multi_selection() {
    let mut app = app();
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.apply_github_issues(
        None,
        GithubPage {
            items: vec![issue(1), issue(2), issue(3)],
            page: 1,
            next_page: None,
            total_count: Some(3),
        },
    );

    app.github_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.github_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.github_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let selected = app.tabs.first().and_then(|tab| match &tab.kind {
        TabKind::Github(crate::app::github::GithubViewState::Dashboard(state)) => {
            Some(state.selected.clone())
        },
        _ => None,
    });
    assert_eq!(selected, Some(BTreeSet::from([0, 1])));
}

#[test]
fn github_shift_click_appends_focused_range_across_card_rows() {
    let mut app = app();
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.apply_github_issues(
        None,
        GithubPage {
            items: (1..=5).map(issue).collect(),
            page: 1,
            next_page: None,
            total_count: Some(5),
        },
    );
    if let Some(TabKind::Github(crate::app::github::GithubViewState::Dashboard(dashboard))) =
        app.tabs.first_mut().map(|tab| &mut tab.kind)
    {
        dashboard.cursor = 1;
        dashboard.selected = BTreeSet::from([0]);
        dashboard.table_rect = Rect::new(0, 10, 80, 15);
    }

    assert!(app.github_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 4,
        row: 22,
        modifiers: KeyModifiers::SHIFT,
    }));

    let state = app.tabs.first().and_then(|tab| match &tab.kind {
        TabKind::Github(crate::app::github::GithubViewState::Dashboard(state)) => Some(state),
        _ => None,
    });
    assert_eq!(state.map(|state| state.cursor), Some(4));
    assert_eq!(
        state.map(|state| state.selected.clone()),
        Some(BTreeSet::from([0, 1, 2, 3, 4]))
    );
}

#[test]
fn github_section_labels_are_clickable_and_actions_rows_open() {
    let mut app = app();
    app.apply_github_availability(Some(repository()), anonymous_auth());
    if let Some(TabKind::Github(crate::app::github::GithubViewState::Dashboard(dashboard))) =
        app.tabs.first_mut().map(|tab| &mut tab.kind)
    {
        dashboard.section_hits = vec![
            (
                crate::app::github::GithubSection::PullRequests,
                Rect::new(10, 2, 20, 1),
            ),
            (
                crate::app::github::GithubSection::Actions,
                Rect::new(30, 2, 12, 1),
            ),
        ];
    }
    let click = |column| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: 2,
        modifiers: KeyModifiers::NONE,
    };
    assert!(app.github_mouse(click(12)));
    assert!(matches!(
        app.tabs.first().map(|tab| &tab.kind),
        Some(TabKind::Github(
            crate::app::github::GithubViewState::Dashboard(crate::app::github::GithubDashboard {
                section: crate::app::github::GithubSection::PullRequests,
                ..
            })
        ))
    ));
    assert!(app.github_mouse(click(32)));

    app.apply_github_actions(
        None,
        GithubPage {
            items: vec![GithubWorkflow {
                id: 7,
                name: "CI".to_string(),
                path: ".github/workflows/ci.yml".to_string(),
                state: "active".to_string(),
                updated_unix: 1,
            }],
            page: 1,
            next_page: None,
            total_count: Some(1),
        },
        GithubPage {
            items: vec![GithubWorkflowRun {
                id: 9,
                workflow_id: 7,
                title: "Tests".to_string(),
                branch: Some("main".to_string()),
                head_sha: "abc123".to_string(),
                event: "push".to_string(),
                status: Some("completed".to_string()),
                conclusion: Some("success".to_string()),
                actor: Some("octocat".to_string()),
                run_number: 42,
                created_unix: 1,
                html_url: "https://github.com/getkono/karet/actions/runs/9".to_string(),
            }],
            page: 1,
            next_page: None,
            total_count: Some(1),
        },
    );
    assert!(app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(matches!(
        app.tabs.last().map(|tab| &tab.kind),
        Some(TabKind::Github(
            crate::app::github::GithubViewState::WorkflowRun { .. }
        ))
    ));
}

#[test]
fn ctrl_r_refreshes_every_github_page_that_loads_remote_data() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.apply_github_availability(Some(repository()), anonymous_auth());
    if let Ok(mut sent) = backend.sent.lock() {
        sent.clear();
    }
    let refresh = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.github_key(refresh));

    app.push_tab(Tab::github_issue(repository(), 4, None));
    assert!(app.github_key(refresh));
    app.push_tab(Tab::github_pull_request(
        repository(),
        pull_request(5, false),
        true,
        None,
    ));
    assert!(app.github_key(refresh));
    app.push_tab(Tab::github_workflow_run(
        repository(),
        None,
        GithubWorkflowRun {
            id: 9,
            workflow_id: 7,
            title: "Tests".to_string(),
            branch: Some("main".to_string()),
            head_sha: "abc123".to_string(),
            event: "push".to_string(),
            status: Some("completed".to_string()),
            conclusion: Some("success".to_string()),
            actor: Some("octocat".to_string()),
            run_number: 42,
            created_unix: 1,
            html_url: "https://github.com/getkono/karet/actions/runs/9".to_string(),
        },
    ));
    assert!(app.github_key(refresh));

    let commands = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .map(|(_, command)| std::mem::discriminant(command))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(commands.contains(&std::mem::discriminant(
        &SessionCommand::GithubSearchIssues {
            query: String::new(),
            page: 1,
        }
    )));
    assert!(commands.contains(&std::mem::discriminant(
        &SessionCommand::GithubIssue { number: 4 }
    )));
    assert!(commands.contains(&std::mem::discriminant(
        &SessionCommand::GithubPullRequest { number: 5 }
    )));
    assert!(commands.contains(&std::mem::discriminant(
        &SessionCommand::GithubActions { page: 1 }
    )));
}

#[test]
fn pull_request_body_comment_merge_and_readiness_controls_submit_typed_commands() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(Tab::github_pull_request(
        repository(),
        pull_request(12, false),
        true,
        None,
    ));
    if let TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) =
        &mut app.tabs[app.active].kind
    {
        view.body_rect = Rect::new(2, 3, 40, 5);
    }
    assert!(app.github_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 4,
        row: 4,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('!'),
        KeyModifiers::NONE
    )));
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL
    )));
    if let TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) =
        &mut app.tabs[app.active].kind
    {
        view.pending = None;
        view.editor = None;
    }
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('m'),
        KeyModifiers::NONE
    )));
    if let TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) =
        &mut app.tabs[app.active].kind
    {
        view.pending = None;
    }
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('d'),
        KeyModifiers::NONE
    )));
    if let TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) =
        &mut app.tabs[app.active].kind
    {
        view.pending = None;
    }
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::NONE
    )));
    for character in "Looks good".chars() {
        assert!(app.github_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE
        )));
    }
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL
    )));

    let sent = backend.sent.lock();
    assert!(sent.as_ref().is_ok_and(|sent| sent.iter().any(|(_, command)| {
        matches!(
            command,
            SessionCommand::GithubUpdatePullRequestBody { number: 12, body }
                if body == "PR description!"
        )
    })));
    assert!(sent.as_ref().is_ok_and(|sent| sent.iter().any(|(_, command)| {
        matches!(command, SessionCommand::GithubMergePullRequest { number: 12, .. })
    })));
    assert!(sent.as_ref().is_ok_and(|sent| sent.iter().any(|(_, command)| {
        matches!(
            command,
            SessionCommand::GithubSetPullRequestDraft {
                number: 12,
                draft: true,
                ..
            }
        )
    })));
    assert!(sent.as_ref().is_ok_and(|sent| sent.iter().any(|(_, command)| {
        matches!(
            command,
            SessionCommand::GithubCommentPullRequest { number: 12, body }
                if body == "Looks good"
        )
    })));
}

#[test]
fn pull_request_tabs_use_commits_and_existing_range_diff_paths() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(Tab::github_pull_request(
        repository(),
        pull_request(12, false),
        true,
        None,
    ));
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('2'),
        KeyModifiers::NONE
    )));
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Github(crate::app::github::GithubViewState::PullRequest(view))
            if view.section == crate::app::github::GithubPullRequestSection::Commits
    ));
    assert!(app.github_key(KeyEvent::new(
        KeyCode::Char('3'),
        KeyModifiers::NONE
    )));
    assert!(backend.sent.lock().as_ref().is_ok_and(|sent| sent.iter().any(
        |(_, command)| matches!(
            command,
            SessionCommand::RangeChanges {
                spec: RangeSpec::Between {
                    base,
                    head,
                    merge_base: true,
                }
            } if base == "aaaaaaaa" && head == "bbbbbbbb"
        )
    )));
}

#[test]
fn pull_request_conversation_renders_github_familiar_controls_and_success_colours(
) -> Result<(), String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    let mut app = app();
    app.push_tab(Tab::github_pull_request(
        repository(),
        pull_request(12, false),
        true,
        None,
    ));
    app.apply_github_pull_request(
        None,
        pull_request(12, false),
        GithubPage {
            items: Vec::new(),
            page: 1,
            next_page: None,
            total_count: Some(0),
        },
        crate::app::github::GithubPullRequestSupplement {
            commits: vec![GithubPullRequestCommit {
                sha: "bbbbbbbb".to_string(),
                summary: "Add the feature".to_string(),
                author: "Octo Cat".to_string(),
                committed_unix: 1,
                parents: vec!["aaaaaaaa".to_string()],
                html_url: "https://github.com/getkono/karet/commit/bbbbbbbb".to_string(),
            }],
            checks: vec![GithubCheckRun {
                id: 9,
                name: "CI / tests".to_string(),
                status: "completed".to_string(),
                conclusion: Some("success".to_string()),
                html_url: "https://github.com/getkono/karet/actions/runs/9".to_string(),
            }],
            activity: vec![GithubPullRequestActivity {
                id: Some(3),
                kind: "head_ref_force_pushed".to_string(),
                actor: Some("octocat".to_string()),
                commit_id: None,
                before: Some("11111111".to_string()),
                after: Some("22222222".to_string()),
                created_unix: Some(2),
            }],
            activity_error: None,
        },
    );
    let mut terminal =
        Terminal::new(TestBackend::new(120, 32)).map_err(|error| error.to_string())?;
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .map_err(|error| error.to_string())?;
    let buffer = terminal.backend().buffer();
    let painted = (0..32)
        .map(|y| {
            (0..120)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(painted.contains("Conversation"));
    assert!(painted.contains("Commits"));
    assert!(painted.contains("Files changed"));
    assert!(!painted.contains("Checks  "));
    assert!(painted.contains("force-pushed"));
    assert!(painted.contains("All checks have passed"));
    assert!(painted.contains("CI / tests"));
    assert!(painted.contains("Merge pull request"));
    assert!(painted.contains("Convert to draft"));
    assert!(painted.contains("Leave a comment · Markdown"));
    let merge_rect = match &app.tabs[app.active].kind {
        TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) => view.merge_rect,
        _ => Rect::default(),
    };
    assert_eq!(buffer[(merge_rect.x, merge_rect.y)].bg, Color::Green);
    Ok(())
}
