//! The open-pull-request picker: listed through the GitHub worker, filled page by
//! page, and retired cleanly when the listing fails.

use karet_session::GithubCommand;
use karet_session::GithubOperation;
use karet_session::GithubPage;

use super::github::pull_request;
use super::support::*;
use crate::app::*;
use crate::overlay::PickerKind;

/// An app with a recording backend and a repository whose branch tracks
/// `upstream`, so the picker has a preferred remote that is not `origin`.
fn picker_app() -> (App, Arc<RecordingBackend>) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    let mut snapshot = RepositorySnapshot::default();
    snapshot.state.upstream = Some("upstream/main".to_string());
    snapshot.remotes = ["origin", "upstream"]
        .into_iter()
        .map(|name| karet_vcs::Remote {
            name: name.to_string(),
            url: Some(format!("https://github.com/{name}/karet.git")),
        })
        .collect();
    app.scm.repository = Some(snapshot);
    (app, backend)
}

/// Every open-pull-request listing the app has sent, as `(id, remote, page)`.
fn listings(backend: &RecordingBackend) -> Vec<(RequestId, String, u32)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Github(GithubCommand::ListOpenPullRequests {
                        remote,
                        page,
                    }) => Some((*id, remote.clone(), *page)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn page(numbers: &[u64], next_page: Option<u32>) -> SessionEvent {
    SessionEvent::GithubPullRequests {
        page: GithubPage {
            items: numbers.iter().map(|n| pull_request(*n, false)).collect(),
            page: 1,
            next_page,
            total_count: None,
        },
    }
}

#[test]
fn the_picker_lists_the_upstream_remote_through_github_and_opens_on_the_last_page() {
    let (mut app, backend) = picker_app();

    app.open_pull_request_picker();
    let first = listings(&backend);
    assert_eq!(first, [(RequestId(1), "upstream".to_string(), 1)]);

    // A full page asks for the next one under a fresh id, for the same remote.
    app.on_backend_event(Some(RequestId(1)), page(&[1, 2], Some(2)));
    assert!(app.overlay.is_none(), "the picker waits for the last page");
    assert_eq!(
        listings(&backend).last(),
        Some(&(RequestId(2), "upstream".to_string(), 2))
    );

    app.on_backend_event(Some(RequestId(2)), page(&[3], None));
    assert_eq!(
        app.overlay.as_ref().and_then(Overlay::picker_kind),
        Some(PickerKind::PullRequests)
    );
    assert_eq!(app.pending_pull_requests, None);
    assert!(app.pull_request_items.is_empty());
    assert_eq!(app.pull_request_remote, None);
}

#[test]
fn a_page_for_another_request_is_left_to_the_github_view() {
    let (mut app, backend) = picker_app();
    app.open_pull_request_picker();

    app.on_backend_event(Some(RequestId(99)), page(&[1], None));

    assert!(app.overlay.is_none());
    assert_eq!(app.pending_pull_requests, Some(RequestId(1)));
    assert_eq!(listings(&backend).len(), 1);
}

#[test]
fn an_empty_listing_says_so_instead_of_opening_an_empty_picker() {
    let (mut app, _backend) = picker_app();
    app.open_pull_request_picker();

    app.on_backend_event(Some(RequestId(1)), page(&[], None));

    assert!(app.overlay.is_none());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("upstream: no open pull requests")
    );
}

#[test]
fn a_failed_listing_retires_the_picker_and_reports_why() {
    let (mut app, _backend) = picker_app();
    app.open_pull_request_picker();
    app.on_backend_event(Some(RequestId(1)), page(&[1], Some(2)));

    app.on_backend_event(
        Some(RequestId(2)),
        SessionEvent::GithubError {
            operation: GithubOperation::PullRequests,
            message: "remote upstream is not hosted on github.com".to_string(),
        },
    );

    assert!(app.overlay.is_none());
    assert_eq!(app.pending_pull_requests, None);
    assert!(app.pull_request_items.is_empty());
    assert_eq!(app.pull_request_remote, None);
    let message = last_message(&app).unwrap_or_default();
    assert!(
        message.contains("Could not load pull requests")
            && message.contains("not hosted on github.com"),
        "{message}"
    );
}
