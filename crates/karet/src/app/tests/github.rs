use karet_session::GithubAuth;
use karet_session::GithubAuthSource;
use karet_session::GithubCheckRun;
use karet_session::GithubPage;
use karet_session::GithubPullRequest;
use karet_session::GithubPullRequestActivity;
use karet_session::GithubPullRequestCommit;
use karet_session::GithubWorkflow;
use karet_session::GithubWorkflowRun;

use super::support::*;
use crate::app::*;
pub(super) fn anonymous_auth() -> GithubAuth {
    GithubAuth {
        source: GithubAuthSource::Anonymous,
        can_write: false,
        viewer_id: None,
        viewer_login: None,
    }
}
pub(super) fn pull_request(number: u64, draft: bool) -> GithubPullRequest {
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

/// An app sitting in the GitHub view with an eligible repository, which is where
/// every GitHub key and click below is aimed. `push_tab` used to set `Focus::Editor`
/// as a side effect; a page pushed onto the surface does not, so it is set here.
fn github_app() -> App {
    let mut app = app();
    app.view = View::GitHub;
    app.focus = Focus::Editor;
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app
}

#[test]
fn every_github_page_names_itself_from_its_own_state() {
    // The strip and the tab both label a page with `title()`, and a creation form
    // *becomes* the resource it created (`apply_github_issue`) rather than being
    // replaced — so the name has to follow the state, not be stored beside it.
    use crate::app::github::GithubViewState;

    let dashboard = GithubViewState::dashboard(repository(), anonymous_auth());
    assert_eq!(dashboard.title(), "GitHub");

    let issue = crate::app::github::github_issue(204, None);
    assert_eq!(issue.title(), "Issue #204");

    let review = crate::app::github::github_pull_request(pull_request(262, false), true, None);
    assert_eq!(review.title(), "Pull Request #262");

    let form = crate::app::github::github_new_issue(repository(), None);
    assert_eq!(form.title(), "New GitHub Issue");
}

/// Availability is re-emitted on every `GithubJob::Refresh`, so the install path
/// has to recognise a dashboard parked in a pane that is not the focused one.
#[test]
fn github_dashboard_opens_a_masked_in_tui_sign_in_control() {
    let mut app = github_app();
    // Installing the dashboard no longer grabs focus, so drive it as a user does:
    // select the tab first.

    assert!(app.github_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)));

    let dashboard = app.github.dashboard();
    assert!(dashboard.is_some_and(|dashboard| dashboard.login_editing));
    assert_eq!(
        dashboard.map(|dashboard| dashboard.login_token.as_str()),
        Some("se")
    );
}

#[test]
fn github_issue_table_supports_keyboard_multi_selection() {
    let mut app = github_app();
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
    let selected = app.github.dashboard().map(|state| state.selected.clone());
    assert_eq!(selected, Some(BTreeSet::from([0, 1])));
}

#[test]
fn github_shift_click_appends_focused_range_across_card_rows() {
    let mut app = github_app();
    app.apply_github_issues(
        None,
        GithubPage {
            items: (1..=5).map(issue).collect(),
            page: 1,
            next_page: None,
            total_count: Some(5),
        },
    );
    if let Some(dashboard) = app.github.dashboard_mut() {
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

    let state = app.github.dashboard();
    assert_eq!(state.map(|state| state.cursor), Some(4));
    assert_eq!(
        state.map(|state| state.selected.clone()),
        Some(BTreeSet::from([0, 1, 2, 3, 4]))
    );
}

#[test]
fn github_section_labels_are_clickable_and_actions_rows_open() {
    let mut app = github_app();
    if let Some(dashboard) = app.github.dashboard_mut() {
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
        app.github.active_page(),
        Some(crate::app::github::GithubViewState::Dashboard(
            crate::app::github::GithubDashboard {
                section: crate::app::github::GithubSection::PullRequests,
                ..
            }
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
        app.github.active_page(),
        Some(crate::app::github::GithubViewState::WorkflowRun { .. })
    ));
}

#[test]
fn ctrl_r_refreshes_every_github_page_that_loads_remote_data() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.view = View::GitHub;
    app.focus = Focus::Editor;
    app.apply_github_availability(Some(repository()), anonymous_auth());
    if let Ok(mut sent) = backend.sent.lock() {
        sent.clear();
    }
    let refresh = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(app.github_key(refresh));

    app.push_github_page(crate::app::github::github_issue(4, None));
    assert!(app.github_key(refresh));
    app.push_github_page(crate::app::github::github_pull_request(
        pull_request(5, false),
        true,
        None,
    ));
    assert!(app.github_key(refresh));
    app.push_github_page(crate::app::github::github_workflow_run(
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
    assert!(
        commands.contains(&std::mem::discriminant(&SessionCommand::GithubIssue {
            number: 4
        }))
    );
    assert!(commands.contains(&std::mem::discriminant(
        &SessionCommand::GithubPullRequest { number: 5 }
    )));
    assert!(
        commands.contains(&std::mem::discriminant(&SessionCommand::GithubActions {
            page: 1
        }))
    );
}

#[test]
fn pull_request_body_comment_merge_and_readiness_controls_submit_typed_commands() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.view = View::GitHub;
    app.focus = Focus::Editor;
    // A detail page stacks on the dashboard; the surface refuses to hold one without
    // it, since `Esc` would then have nothing to fall back to.
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.push_github_page(crate::app::github::github_pull_request(
        pull_request(12, false),
        true,
        None,
    ));
    if let Some(crate::app::github::GithubViewState::PullRequest(view)) =
        app.github.active_page_mut()
    {
        view.body_rect = Rect::new(2, 3, 40, 5);
    }
    assert!(app.github_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 4,
        row: 4,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)));
    assert!(app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));
    if let Some(crate::app::github::GithubViewState::PullRequest(view)) =
        app.github.active_page_mut()
    {
        view.pending = None;
        view.editor = None;
    }
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)));
    if let Some(crate::app::github::GithubViewState::PullRequest(view)) =
        app.github.active_page_mut()
    {
        view.pending = None;
    }
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)));
    if let Some(crate::app::github::GithubViewState::PullRequest(view)) =
        app.github.active_page_mut()
    {
        view.pending = None;
    }
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
    for character in "Looks good".chars() {
        assert!(app.github_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)));
    }
    assert!(app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));

    let sent = backend.sent.lock();
    assert!(
        sent.as_ref()
            .is_ok_and(|sent| sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::GithubUpdatePullRequestBody { number: 12, body }
                        if body == "PR description!"
                )
            }))
    );
    assert!(
        sent.as_ref()
            .is_ok_and(|sent| sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::GithubMergePullRequest { number: 12, .. }
                )
            }))
    );
    assert!(
        sent.as_ref()
            .is_ok_and(|sent| sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::GithubSetPullRequestDraft {
                        number: 12,
                        draft: true,
                        ..
                    }
                )
            }))
    );
    assert!(
        sent.as_ref()
            .is_ok_and(|sent| sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::GithubCommentPullRequest { number: 12, body }
                        if body == "Looks good"
                )
            }))
    );
}

#[test]
fn pull_request_tabs_use_commits_and_existing_range_diff_paths() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.view = View::GitHub;
    app.focus = Focus::Editor;
    // A detail page stacks on the dashboard; the surface refuses to hold one without
    // it, since `Esc` would then have nothing to fall back to.
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.push_github_page(crate::app::github::github_pull_request(
        pull_request(12, false),
        true,
        None,
    ));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE)));
    assert!(matches!(
        app.github.active_page(),
        Some(crate::app::github::GithubViewState::PullRequest(view))
            if view.section == crate::app::github::GithubPullRequestSection::Commits
    ));
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE)));
    assert!(
        backend
            .sent
            .lock()
            .as_ref()
            .is_ok_and(|sent| sent.iter().any(|(_, command)| matches!(
                command,
                SessionCommand::RangeChanges {
                    spec: RangeSpec::Between {
                        base,
                        head,
                        merge_base: true,
                    }
                } if base == "aaaaaaaa" && head == "bbbbbbbb"
            )))
    );
}

#[test]
fn pull_request_conversation_renders_github_familiar_controls_and_success_colours()
-> Result<(), String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    // Drawn through the real `ui::draw`, so the page has to be where the view layer
    // will look for it: on the GitHub surface, under the GitHub view.
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_pull_request(
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
    let merge_rect = match app.github.active_page() {
        Some(crate::app::github::GithubViewState::PullRequest(view)) => view.merge_rect,
        _ => Rect::default(),
    };
    assert_eq!(buffer[(merge_rect.x, merge_rect.y)].bg, Color::Green);
    Ok(())
}

#[test]
fn a_failed_commit_verification_stays_quiet() {
    // The signature lookup fires for whatever commit is on screen, so a commit
    // the forge does not know — unpushed, a fork, no GitHub remote — is an
    // ordinary outcome and must not raise an error toast.
    let mut app = app();
    app.apply_github_error(
        Some(RequestId(1)),
        "commit verification".to_owned(),
        "GitHub returned HTTP 422: No commit found for SHA: deadbeef".to_owned(),
    );
    assert!(
        app.notifications.is_empty(),
        "a speculative enrichment must not notify"
    );
}

#[test]
fn other_github_failures_still_reach_the_user() {
    // The quiet path is scoped to verification; an action the user asked for
    // must still report when it fails.
    let mut app = app();
    app.apply_github_error(
        Some(RequestId(1)),
        "merge pull request".to_owned(),
        "GitHub returned HTTP 405: not mergeable".to_owned(),
    );
    let active = app.notifications.active();
    assert_eq!(active.len(), 1);
    let rendered = format!("{active:?}");
    assert!(
        rendered.contains("Could not merge the pull request"),
        "{rendered}"
    );
}

#[test]
fn the_surface_holds_one_dashboard_and_never_closes_it() {
    // What the pinned-tab guards used to enforce across every pane is now a property
    // of the surface: availability can arrive any number of times and there is still
    // exactly one dashboard, sitting at the bottom of the stack.
    let mut app = github_app();
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.apply_github_availability(Some(repository()), anonymous_auth());
    assert_eq!(app.github.pages().len(), 1);

    // Esc on the dashboard declines rather than emptying the view.
    assert!(!app.close_github_page());
    assert_eq!(app.github.pages().len(), 1);
}

#[test]
fn a_detail_page_stacks_on_the_dashboard_and_esc_pops_it() {
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(204, None));
    assert_eq!(app.github.pages().len(), 2);
    assert_eq!(app.github.active(), 1);

    assert!(app.github_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert_eq!(app.github.pages().len(), 1);
    assert_eq!(app.github.active(), 0);
}

#[test]
fn opening_the_same_issue_twice_focuses_the_page_already_open() {
    // A stack the user pops with Esc would otherwise accumulate duplicates, and
    // unlike a tab strip there is no "close all" to dig out of it.
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(204, None));
    app.github.select(0);
    app.push_github_page(crate::app::github::github_issue(204, None));

    assert_eq!(app.github.pages().len(), 2);
    assert_eq!(app.github.active(), 1);
}

#[test]
fn a_detail_page_leaves_the_dashboard_keys_alone() {
    // `n` and 1/2/3 belong to the dashboard. They must not act on it through a detail
    // page that happens to be in front, which is what a single `pages[0]` accessor
    // would have allowed.
    let mut app = github_app();
    let before = app
        .github
        .dashboard()
        .map(|dashboard| dashboard.section)
        .expect("a dashboard");
    app.push_github_page(crate::app::github::github_issue(204, None));

    app.github_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    app.github_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

    assert_eq!(
        app.github.dashboard().map(|dashboard| dashboard.section),
        Some(before)
    );
    assert_eq!(app.github.pages().len(), 2, "no form was opened");
}

#[test]
fn config_churn_keeps_open_pages_and_the_typed_query() {
    // Availability is re-emitted on every refresh, `.git/config` watch churn
    // included. Rebuilding the surface there would throw away the user's work each
    // time git touched its own config.
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(204, None));
    if let Some(dashboard) = app.github.dashboard_mut() {
        dashboard.query = "is:open author:@me".to_string();
        dashboard.cursor = 3;
    }

    app.apply_github_availability(Some(repository()), anonymous_auth());

    assert_eq!(app.github.pages().len(), 2, "the issue page survived");
    let dashboard = app.github.dashboard().expect("a dashboard");
    assert_eq!(dashboard.query, "is:open author:@me");
    assert_eq!(dashboard.cursor, 3);
}

#[test]
fn the_surface_withdraws_when_the_repository_becomes_ineligible() {
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(204, None));

    app.apply_github_availability(None, anonymous_auth());

    assert!(!app.github.is_active());
    assert!(app.github.dashboard().is_none());
    // The editor keeps whatever it had; only the GitHub view emptied.
    assert!(!app.tabs.is_empty());
}

#[test]
fn a_detail_page_scrolls_itself_rather_than_the_document_behind_it() {
    // `scroll_lines` walks the active *tab*. Under the GitHub view that tab is a
    // document drawn over, so a wheel or a `j` reaching it would move something
    // invisible.
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(204, None));
    let editor_before = app.tabs[app.active].editor.scroll_line;

    app.github_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.github_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 4,
        row: 8,
        modifiers: KeyModifiers::NONE,
    }));

    assert!(matches!(
        app.github.active_page(),
        Some(crate::app::github::GithubViewState::Issue { scroll, .. }) if *scroll > 0
    ));
    assert_eq!(
        app.tabs[app.active].editor.scroll_line, editor_before,
        "the hidden document did not move"
    );
}

#[test]
fn opening_a_tab_from_the_github_view_shows_it() {
    // A pull request's "Files changed" opens the range diff as an editor tab. Without
    // the view switch the user presses the button and, from inside the GitHub view,
    // nothing appears to happen at all.
    let mut app = github_app();
    app.push_tab(Tab::welcome());

    assert_eq!(app.view, View::Editor);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn the_github_view_names_a_workspace_with_no_github_behind_it() -> Result<(), String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // Reaching the view in a non-GitHub checkout is ordinary — `--view github`, or
    // `Ctrl+K 2` out of habit. It has to say why it is empty rather than just be.
    let mut app = app();
    app.view = View::GitHub;

    let mut terminal =
        Terminal::new(TestBackend::new(100, 24)).map_err(|error| error.to_string())?;
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .map_err(|error| error.to_string())?;
    let buffer = terminal.backend().buffer();
    let painted = (0..24)
        .map(|y| {
            (0..100)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(painted.contains("not a GitHub repository"));
    assert!(
        !painted.contains("not available yet"),
        "the surface exists; it is the repository that does not"
    );
    Ok(())
}

#[test]
fn nothing_stacks_on_a_workspace_with_no_github_behind_it() {
    // The surface is empty or the dashboard is its floor — never a detail page on its
    // own, which `Esc` could not get out of and the next availability would replace
    // wholesale.
    let mut app = app();
    app.view = View::GitHub;

    app.push_github_page(crate::app::github::github_issue(204, None));

    assert!(!app.github.is_active());
    assert!(app.github.pages().is_empty());
}

#[test]
fn closing_a_page_behind_the_one_in_front_leaves_the_reader_alone() {
    // The strip closes by index. Reading page 3 and dismissing page 1 must keep you
    // on what you were reading — it just shifts left — rather than dropping you onto
    // whatever sat under the page you dismissed.
    let mut app = github_app();
    app.push_github_page(crate::app::github::github_issue(1, None));
    app.push_github_page(crate::app::github::github_issue(2, None));
    assert_eq!(app.github.active(), 2);

    assert!(app.github.close_at(1));

    assert_eq!(app.github.pages().len(), 2);
    assert_eq!(app.github.active(), 1, "still reading issue #2");
    assert!(matches!(
        app.github.active_page(),
        Some(crate::app::github::GithubViewState::Issue { number: 2, .. })
    ));
}

#[test]
fn selecting_an_existing_tab_from_the_github_view_shows_it() {
    // `select_tab` sets focus without the view, which is what strands a caller from
    // another view: the tab takes the keyboard while staying invisible, and
    // `FocusTarget::from` then routes the next keys to the view still on screen.
    let mut app = github_app();
    app.push_tab(Tab::welcome());
    app.dispatch(Command::SelectView(View::GitHub));

    app.select_tab(0);

    assert_eq!(app.view, View::Editor);
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn previewing_from_the_sidebar_does_not_yank_you_out_of_the_github_view() {
    // Selection-follows-preview is passive: arrowing a file tree must not rip the
    // user out of the view they are reading. Only a focus-stealing open does that.
    let mut app = github_app();
    app.focus = Focus::Sidebar;

    app.install_preview_tab(Tab::welcome(), false);

    assert_eq!(app.view, View::GitHub);
    assert_eq!(app.focus, Focus::Sidebar);
}

#[test]
fn reopening_an_issue_focuses_it_without_orphaning_a_request() {
    // `push` drops the page it is handed when one for the same resource is open, and
    // with it that page's request id. Sending first would leave a reply nobody owns —
    // and an error for it would surface as a toast the user never asked for.
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.view = View::GitHub;
    app.focus = Focus::Editor;
    app.apply_github_availability(Some(repository()), anonymous_auth());
    app.apply_github_issues(
        None,
        GithubPage {
            items: vec![issue(1)],
            page: 1,
            next_page: None,
            total_count: Some(1),
        },
    );
    app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.github.pages().len(), 2);
    if let Ok(mut sent) = backend.sent.lock() {
        sent.clear();
    }

    // Back to the dashboard, then open the same row again.
    app.github.select(0);
    app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.github.pages().len(), 2, "focused, not stacked");
    assert_eq!(app.github.active(), 1);
    let issued = backend.sent.lock().is_ok_and(|sent| {
        sent.iter()
            .any(|(_, command)| matches!(command, SessionCommand::GithubIssue { .. }))
    });
    assert!(!issued, "no request without a page to own its reply");
}
