//! The shared delayed-loading policy, surface by surface: each placeholder stays
//! hidden on the fast path, reveals muted once the work outlasts
//! [`LOADING_REVEAL_DELAY`], and schedules the repaint that reveals it; and a late
//! answer — a diff for a tab that is gone or re-targeted, a GitHub reply for a
//! request nobody is waiting on — is dropped.

use std::time::Instant;

use karet_session::GithubComment;
use karet_session::GithubIssue;
use karet_session::GithubPage;
use ratatui::buffer::Buffer;

use super::github::anonymous_auth;
use super::github::dashboard_pending;
use super::github::pull_request;
use super::github::tagged_github_app;
use super::support::*;
use crate::app::*;
use crate::tab::MergeConflictState;

const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

/// Where `needle` starts on the painted screen, as a cell position.
fn locate(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let area = buffer.area;
    (0..area.height).find_map(|y| {
        let mut row = String::new();
        let mut starts = Vec::new();
        for x in 0..area.width {
            starts.push((row.len(), x));
            row.push_str(buffer[(x, y)].symbol());
        }
        let at = row.find(needle)?;
        starts
            .iter()
            .find(|(offset, _)| *offset == at)
            .map(|(_, x)| (*x, y))
    })
}

/// A Code tab backed by document `doc`, as the outline requires.
fn document_tab(doc: DocumentId) -> Tab {
    let mut tab = code_tab("a.rs");
    if let TabKind::Code { doc: slot, .. } = &mut tab.kind {
        *slot = Some(doc);
    }
    tab
}

fn show_sidebar(app: &mut App, panel: SidebarPanel) {
    app.sidebar_visible = true;
    app.sidebar_panel = panel;
}

fn github_view(app: &mut App) {
    app.view = View::GitHub;
    app.keyboard.focus_editor();
    app.apply_github_availability(Some(repository()), anonymous_auth());
}

fn stage_diff(app: &mut App, pending: Pending) {
    app.push_tab(Tab::diff(
        "a.rs".to_string(),
        PathBuf::from("a.rs"),
        Section::Working,
        None,
        ViewMode::Unified,
    ));
    if let TabKind::Diff { loading_since, .. } = &mut app.tabs[app.active].kind {
        *loading_since = Some(pending);
    }
}

fn stage_commit_loading(app: &mut App, pending: Pending) {
    app.push_tab(Tab::commit_loading("0123456789abcdef"));
    if let TabKind::CommitLoading { loading_since, .. } = &mut app.tabs[app.active].kind {
        *loading_since = pending;
    }
}

fn stage_commit_graph(app: &mut App, pending: Pending) {
    app.push_tab(Tab::commit_graph(None, "Graph"));
    if let TabKind::CommitGraph { loading_since, .. } = &mut app.tabs[app.active].kind {
        *loading_since = Some(pending);
    }
}

fn stage_conflict_side(app: &mut App, pending: Pending) {
    app.push_tab(text_tab("a.rs", "merged\n"));
    let mut conflict = MergeConflictState::loading();
    conflict.loading_since = pending;
    app.tabs[app.active].merge_conflict = Some(conflict);
}

fn stage_outline(app: &mut App, pending: Pending) {
    let doc = DocumentId(7);
    app.push_tab(document_tab(doc));
    app.outline.visible = true;
    app.docs.outline_loading.insert(doc, (0, pending));
}

fn stage_scm_repository(app: &mut App, pending: Pending) {
    show_sidebar(app, SidebarPanel::SourceControl);
    app.scm.repository = None;
    app.scm.repository_loading_since = Some(pending);
}

fn stage_scm_log(app: &mut App, pending: Pending) {
    show_sidebar(app, SidebarPanel::SourceControl);
    app.scm.log_has_more = true;
    app.scm.log_loading_since = Some(pending);
}

fn stage_search(app: &mut App, pending: Pending) {
    show_sidebar(app, SidebarPanel::Search);
    app.search.query = "needle".to_string();
    app.search.searching = Some(RequestId(1));
    app.search.started = Some(pending);
}

fn stage_github_dashboard(app: &mut App, pending: Pending) {
    github_view(app);
    if let Some(dashboard) = app.github.dashboard_mut() {
        dashboard.loading_since = Some(pending);
    }
}

fn stage_github_issue(app: &mut App, pending: Pending) {
    github_view(app);
    app.push_github_page(crate::app::github::github_issue(204, Some(RequestId(9))));
    if let Some(crate::app::github::GithubViewState::Issue { loading_since, .. }) =
        app.github.active_page_mut()
    {
        *loading_since = pending;
    }
}

fn stage_github_pull_request(app: &mut App, pending: Pending) {
    github_view(app);
    app.push_github_page(crate::app::github::github_pull_request(
        pull_request(12, false),
        true,
        Some(RequestId(9)),
    ));
    if let Some(crate::app::github::GithubViewState::PullRequest(view)) =
        app.github.active_page_mut()
    {
        view.loading_since = pending;
    }
}

/// One surface that follows the delayed-loading rule.
struct Surface {
    name: &'static str,
    /// Bring a fresh app into the surface's loading state, anchored at `pending`.
    stage: fn(&mut App, Pending),
    /// The placeholder text the surface reveals after the delay.
    placeholder: &'static str,
    /// The role the revealed placeholder is painted in.
    role: ThemeRole,
}

const SURFACES: &[Surface] = &[
    Surface {
        name: "diff tab",
        stage: stage_diff,
        placeholder: "Loading diff…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "commit card",
        stage: stage_commit_loading,
        placeholder: "details and file changes…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "commit graph",
        stage: stage_commit_graph,
        placeholder: "loading history…",
        role: ThemeRole::LineNumber,
    },
    Surface {
        name: "conflict side",
        stage: stage_conflict_side,
        placeholder: "Loading conflict side…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "outline",
        stage: stage_outline,
        placeholder: "Loading…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "SCM repository header",
        stage: stage_scm_repository,
        placeholder: "Loading repository…",
        // The placeholder stands in the branch name's slot and takes its style.
        role: ThemeRole::LineNumberActive,
    },
    Surface {
        name: "SCM commit log",
        stage: stage_scm_log,
        placeholder: "loading…",
        role: ThemeRole::LineNumber,
    },
    Surface {
        name: "search panel",
        stage: stage_search,
        placeholder: "searching…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "GitHub dashboard",
        stage: stage_github_dashboard,
        placeholder: "Loading GitHub data…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "GitHub issue",
        stage: stage_github_issue,
        placeholder: "Loading conversation…",
        role: ThemeRole::Muted,
    },
    Surface {
        name: "GitHub pull request",
        stage: stage_github_pull_request,
        placeholder: "Refreshing…",
        role: ThemeRole::Muted,
    },
];

#[test]
fn every_delayed_placeholder_is_hidden_before_the_delay_and_muted_after() {
    for surface in SURFACES {
        let mut early = app();
        (surface.stage)(&mut early, Pending::start());
        let painted = frame(&mut early, WIDTH, HEIGHT);
        assert_eq!(
            locate(&painted, surface.placeholder),
            None,
            "{}: the placeholder flashed before the reveal delay",
            surface.name
        );

        let mut late = app();
        (surface.stage)(&mut late, Pending::revealed());
        let painted = frame(&mut late, WIDTH, HEIGHT);
        let at = locate(&painted, surface.placeholder);
        assert!(
            at.is_some(),
            "{}: the placeholder is missing after the reveal delay",
            surface.name
        );
        if let Some(at) = at {
            assert_eq!(
                Some(painted[at].fg),
                late.theme.style(surface.role).fg,
                "{}: the placeholder is not painted in {:?}",
                surface.name,
                surface.role
            );
        }
    }
}

/// One delayed-loading source, opened the way the app opens it.
struct Source {
    name: &'static str,
    open: fn(&mut App),
}

const SOURCES: &[Source] = &[
    Source {
        name: "SCM commit log",
        open: |app| stage_scm_log(app, Pending::start()),
    },
    Source {
        name: "SCM repository snapshot",
        open: |app| stage_scm_repository(app, Pending::start()),
    },
    Source {
        name: "search panel",
        open: |app| stage_search(app, Pending::start()),
    },
    Source {
        name: "outline",
        open: |app| stage_outline(app, Pending::start()),
    },
    Source {
        name: "merge conflict side",
        open: |app| {
            app.push_tab(text_tab("a.rs", "merged\n"));
            app.tabs[app.active].merge_conflict = Some(MergeConflictState::loading());
        },
    },
    Source {
        name: "language servers",
        open: App::open_language_servers,
    },
    Source {
        name: "commit loading",
        open: |app| app.push_tab(Tab::commit_loading("0123456789abcdef")),
    },
    Source {
        name: "LaTeX preview",
        open: |app| app.push_tab(Tab::latex_preview(PathBuf::from("paper.tex"))),
    },
    Source {
        name: "document conversion",
        open: |app| app.push_tab(Tab::document_converting(PathBuf::from("notes.docx"))),
    },
    Source {
        name: "server log",
        open: |app| app.push_tab(Tab::server_log(LanguageServerId::RustAnalyzer)),
    },
    Source {
        name: "commit files",
        open: |app| {
            app.push_tab(Tab::commit(
                Box::new(commit_detail("0123456789abcdef", "Change")),
                CommitFiles::loading(),
            ));
        },
    },
    Source {
        name: "compare files",
        open: |app| {
            app.push_tab(Tab::compare(
                "main".to_string(),
                "topic".to_string(),
                true,
                CommitFiles::loading(),
            ));
        },
    },
    Source {
        name: "diff tab",
        open: |app| stage_diff(app, Pending::start()),
    },
    Source {
        name: "commit graph",
        open: |app| app.push_tab(Tab::commit_graph(None, "Graph")),
    },
    Source {
        name: "GitHub dashboard",
        open: |app| {
            app.backend = Some(Arc::new(RecordingBackend::new()));
            github_view(app);
        },
    },
    Source {
        name: "GitHub issue",
        open: |app| {
            github_view(app);
            if let Some(dashboard) = app.github.dashboard_mut() {
                dashboard.loading_since = None;
            }
            app.push_github_page(crate::app::github::github_issue(204, Some(RequestId(9))));
        },
    },
    Source {
        name: "GitHub pull request",
        open: |app| {
            github_view(app);
            if let Some(dashboard) = app.github.dashboard_mut() {
                dashboard.loading_since = None;
            }
            app.push_github_page(crate::app::github::github_pull_request(
                pull_request(12, false),
                true,
                Some(RequestId(9)),
            ));
        },
    },
];

#[test]
fn every_pending_source_schedules_its_reveal_within_the_delay() {
    for source in SOURCES {
        let mut app = app();
        let before = app.pendings().len();
        (source.open)(&mut app);
        let pendings = app.pendings();
        assert!(
            pendings.len() > before,
            "{}: the load is missing from pendings(), so its placeholder would wait for a key",
            source.name
        );
        let now = Instant::now();
        assert!(
            pendings
                .iter()
                .filter_map(|pending| pending.wake(now))
                .min()
                .is_some_and(|wake| wake <= LOADING_REVEAL_DELAY),
            "{}: no reveal wake within the delay",
            source.name
        );
        assert!(
            app.next_wake()
                .is_some_and(|wake| wake <= LOADING_REVEAL_DELAY),
            "{}: the event loop would not wake for the reveal",
            source.name
        );
    }
}

/// An app holding a reserved revision-diff tab whose `DiffPrepared` is in flight
/// as `request`.
fn awaiting_revision_diff(request: RequestId) -> (App, ViewId) {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app.push_tab(Tab::diff(
        "a.rs (HEAD~1 ↔ working)".to_string(),
        PathBuf::from("a.rs"),
        Section::Working,
        None,
        ViewMode::Unified,
    ));
    let view = app.tabs[app.active].view;
    app.pending_prepared_diffs.insert(request, (view, None));
    (app, view)
}

#[test]
fn a_diff_prepared_for_a_closed_diff_tab_is_dropped() {
    let request = RequestId(41);
    let (mut app, view) = awaiting_revision_diff(request);
    app.execute_close(CloseRequest::Tab { view });
    assert!(app.pending_prepared_diffs.is_empty());

    app.apply_diff_prepared(
        Some(request),
        Ok(Box::new(prepared_change("a.rs", StatusKind::Modified))),
    );
    assert!(
        !app.all_tabs().any(Tab::is_diff),
        "the late answer recreated the diff tab"
    );

    let (mut app, view) = awaiting_revision_diff(request);
    app.execute_close(CloseRequest::Tab { view });
    app.apply_diff_prepared(Some(request), Err("bad revision".to_string()));
    assert!(
        app.notifications.is_empty(),
        "a failure for a diff the user closed must stay quiet"
    );
}

#[test]
fn a_diff_prepared_for_a_retargeted_diff_tab_is_dropped() {
    let request = RequestId(41);
    let (mut app, view) = awaiting_revision_diff(request);
    // Next Changed File re-targets the same tab (same view id) at a working change.
    app.step_changed_file(1);
    let target = match &app.tabs[app.active].kind {
        TabKind::Diff {
            path, file: None, ..
        } => Some(path.clone()),
        _ => None,
    };
    assert!(target.is_some(), "the tab is re-targeted and loading");
    assert_eq!(app.tabs[app.active].view, view);

    app.apply_diff_prepared(
        Some(request),
        Ok(Box::new(prepared_change("stale.rs", StatusKind::Modified))),
    );
    assert!(
        matches!(&app.tabs[app.active].kind, TabKind::Diff { file: None, .. }),
        "the superseded revision diff filled the re-targeted tab"
    );

    // The re-targeted tab still fills with its own change.
    if let Some(path) = target {
        let staged = matches!(
            &app.tabs[app.active].kind,
            TabKind::Diff {
                section: Section::Staged,
                ..
            }
        );
        let name = path.to_string_lossy().into_owned();
        app.apply_change_prepared(
            &path,
            staged,
            Ok(Box::new(prepared_change(&name, StatusKind::Modified))),
        );
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::Diff { file: Some(file), .. } if file.change.path == path
        ));
    }
}

/// The request the issue page for `number` is waiting on, and whether it has loaded.
fn issue_page(app: &App, number: u64) -> Option<(Option<RequestId>, bool)> {
    app.github.pages().iter().find_map(|page| match page {
        crate::app::github::GithubViewState::Issue {
            number: open,
            pending,
            issue,
            ..
        } if *open == number => Some((*pending, issue.is_some())),
        _ => None,
    })
}

/// One page of issue search results holding `items`.
fn issue_page_of(items: Vec<GithubIssue>) -> GithubPage<GithubIssue> {
    let total = items.len() as u64;
    GithubPage {
        items,
        page: 1,
        next_page: None,
        total_count: Some(total),
    }
}

/// An issue's empty conversation.
fn no_comments() -> GithubPage<GithubComment> {
    GithubPage {
        items: Vec::new(),
        page: 1,
        next_page: None,
        total_count: Some(0),
    }
}

#[test]
fn a_superseded_issue_search_answer_is_dropped() {
    let mut app = tagged_github_app();
    let first = dashboard_pending(&app);
    assert!(first.is_some(), "availability requested the issue list");

    // Ctrl+R re-queries before the first answer lands.
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
    let second = dashboard_pending(&app);
    assert!(second.is_some() && second != first);

    app.apply_github_issues(first, issue_page_of(vec![issue(1)]));
    let dashboard = app.github.dashboard();
    assert_eq!(dashboard.map(|d| d.issues.items.len()), Some(0));
    assert_eq!(dashboard.and_then(|d| d.pending), second, "still waiting");
    assert!(dashboard.is_some_and(|d| d.loading_since.is_some()));

    app.apply_github_issues(second, issue_page_of(vec![issue(1), issue(2)]));
    let dashboard = app.github.dashboard();
    assert_eq!(dashboard.map(|d| d.issues.items.len()), Some(2));
    assert_eq!(dashboard.and_then(|d| d.pending), None);
    assert!(dashboard.is_some_and(|d| d.loading_since.is_none()));
}

#[test]
fn a_github_error_for_a_superseded_request_leaves_the_dashboard_loading() {
    let mut app = tagged_github_app();
    let first = dashboard_pending(&app);
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
    let second = dashboard_pending(&app);

    app.apply_github_error(
        first,
        karet_session::GithubOperation::Issues,
        "HTTP 502".to_string(),
    );
    let dashboard = app.github.dashboard();
    assert_eq!(dashboard.and_then(|d| d.error.clone()), None);
    assert_eq!(dashboard.and_then(|d| d.pending), second);
}

#[test]
fn two_issue_pages_each_take_only_their_own_answer() {
    let mut app = tagged_github_app();
    let listing = dashboard_pending(&app);
    app.apply_github_issues(listing, issue_page_of(vec![issue(1), issue(2)]));

    // Open both rows the way a reader does: Enter, back to the dashboard, next row.
    app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.github.select(0);
    app.github_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let one = issue_page(&app, 1).and_then(|(pending, _)| pending);
    let two = issue_page(&app, 2).and_then(|(pending, _)| pending);
    assert!(one.is_some() && two.is_some() && one != two);

    app.apply_github_issue(two, issue(2), no_comments());
    assert_eq!(issue_page(&app, 2), Some((None, true)));
    assert_eq!(
        issue_page(&app, 1),
        Some((one, false)),
        "issue #1 was filled by issue #2's answer"
    );

    // A reply nobody is waiting on any more changes neither page.
    app.apply_github_issue(two, issue(2), no_comments());
    assert_eq!(issue_page(&app, 1), Some((one, false)));

    app.apply_github_issue(one, issue(1), no_comments());
    assert_eq!(issue_page(&app, 1), Some((None, true)));
    let numbers: Vec<u64> = app
        .github
        .pages()
        .iter()
        .filter_map(|page| match page {
            crate::app::github::GithubViewState::Issue {
                issue: Some(issue), ..
            } => Some(issue.number),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, vec![1, 2], "each page holds its own issue");
}

#[test]
fn a_stale_issue_detail_answer_is_dropped() {
    let mut app = tagged_github_app();
    let listing = dashboard_pending(&app);
    app.apply_github_issues(listing, issue_page_of(vec![issue(1)]));
    app.github_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let first = issue_page(&app, 1).and_then(|(pending, _)| pending);
    // Ctrl+R on the issue page re-requests it.
    assert!(app.github_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)));
    let second = issue_page(&app, 1).and_then(|(pending, _)| pending);
    assert!(second.is_some() && second != first);

    app.apply_github_issue(first, issue(1), no_comments());
    assert_eq!(issue_page(&app, 1), Some((second, false)));
    app.apply_github_issue(second, issue(1), no_comments());
    assert_eq!(issue_page(&app, 1), Some((None, true)));
}
