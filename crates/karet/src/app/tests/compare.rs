use super::support::*;
use crate::app::*;

/// An app wired to a recording backend, so range requests get request ids.
fn compare_app() -> (App, Arc<RecordingBackend>) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    (app, backend)
}

fn range_ready(base: &str, head: &str, changes: Vec<PreparedChange>) -> SessionEvent {
    SessionEvent::RangeReady {
        base_label: base.to_string(),
        head_label: head.to_string(),
        merge_base: true,
        changes,
    }
}

fn compare_tabs(app: &App) -> usize {
    app.all_tabs()
        .filter(|tab| matches!(tab.kind, TabKind::Compare { .. }))
        .count()
}

/// The compare tab showing `view`, as `(title, base, head, files, loading, error)`.
fn compare_state(
    app: &App,
    view: ViewId,
) -> Option<(String, String, String, usize, bool, Option<String>)> {
    app.all_tabs()
        .find(|tab| tab.view == view)
        .and_then(|tab| match &tab.kind {
            TabKind::Compare {
                base_label,
                head_label,
                files,
                ..
            } => Some((
                tab.title.clone(),
                base_label.clone(),
                head_label.clone(),
                files.files.len(),
                files.loading_since.is_some(),
                files.error.clone(),
            )),
            _ => None,
        })
}

#[test]
fn open_compare_tab_builds_a_compare_tab() {
    let mut app = app();
    app.open_compare_tab(
        "main".to_string(),
        "HEAD".to_string(),
        true,
        vec![prepared_change("a.rs", StatusKind::Modified)],
    );
    match &app.tabs[app.active].kind {
        TabKind::Compare {
            base_label,
            head_label,
            merge_base,
            files,
            view,
        } => {
            assert_eq!(base_label, "main");
            assert_eq!(head_label, "HEAD");
            assert!(*merge_base);
            assert_eq!(files.files.len(), 1);
            assert_eq!(view.scroll, 0);
        },
        _ => panic!("expected a compare tab"),
    }
    // A compare tab scrolls via the shared pager arm.
    app.scroll_lines(2);
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Compare { view, .. } if view.scroll == 2
    ));
}

#[test]
fn a_range_request_reserves_its_tab_and_the_answer_fills_it_in_place() {
    let (mut app, backend) = compare_app();
    app.dispatch(Command::DiffSinceBase);

    // The tab exists before any answer, loading, under the requested labels.
    let view = app.tabs[app.active].view;
    assert_eq!(
        compare_state(&app, view),
        Some((
            "\u{21c4} base\u{2026}HEAD".to_string(),
            "base".to_string(),
            "HEAD".to_string(),
            0,
            true,
            None,
        ))
    );
    let sent = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(id, command)| {
                    *id == RequestId(1) && matches!(command, SessionCommand::RangeChanges { .. })
                })
                .count()
        })
        .unwrap_or_default();
    assert_eq!(
        sent, 1,
        "the range is requested once, under the reserved id"
    );

    app.on_backend_event(
        Some(RequestId(1)),
        range_ready(
            "origin/main",
            "HEAD",
            vec![prepared_change("a.rs", StatusKind::Modified)],
        ),
    );
    // The same tab now carries the resolved labels and the files; no second tab.
    assert_eq!(compare_tabs(&app), 1);
    assert_eq!(
        compare_state(&app, view),
        Some((
            "\u{21c4} origin/main\u{2026}HEAD".to_string(),
            "origin/main".to_string(),
            "HEAD".to_string(),
            1,
            false,
            None,
        ))
    );
}

#[test]
fn quick_requests_fill_their_own_tabs_and_stale_answers_are_dropped() {
    let (mut app, _backend) = compare_app();
    app.dispatch(Command::DiffUnpushed);
    let first = app.tabs[app.active].view;
    app.dispatch(Command::DiffSinceBase);
    let second = app.tabs[app.active].view;
    assert_eq!(compare_tabs(&app), 2);

    // Answers arrive out of order; each fills the tab its request reserved.
    app.on_backend_event(
        Some(RequestId(2)),
        range_ready(
            "main",
            "HEAD",
            vec![prepared_change("b.rs", StatusKind::Added)],
        ),
    );
    app.on_backend_event(
        Some(RequestId(1)),
        range_ready(
            "origin/feature",
            "HEAD",
            vec![
                prepared_change("a.rs", StatusKind::Modified),
                prepared_change("c.rs", StatusKind::Deleted),
            ],
        ),
    );
    assert!(
        compare_state(&app, first)
            .is_some_and(|(_, base, _, files, ..)| base == "origin/feature" && files == 2)
    );
    assert!(
        compare_state(&app, second)
            .is_some_and(|(_, base, _, files, ..)| base == "main" && files == 1)
    );

    // A duplicate answer, and one for a request that never reserved a tab, are stale.
    app.on_backend_event(Some(RequestId(1)), range_ready("x", "y", Vec::new()));
    app.on_backend_event(Some(RequestId(99)), range_ready("x", "y", Vec::new()));
    assert_eq!(compare_tabs(&app), 2);
    assert!(compare_state(&app, first).is_some_and(|(_, base, ..)| base == "origin/feature"));
}

#[test]
fn an_unresolvable_range_marks_its_reserved_tab_failed() {
    let (mut app, _backend) = compare_app();
    app.dispatch(Command::DiffUnpushed);
    let view = app.tabs[app.active].view;

    app.on_backend_event(
        Some(RequestId(1)),
        SessionEvent::Notification {
            severity: Severity::Warning,
            kind: NotificationKind::Vcs,
            message: "no upstream branch is set for the current branch".to_string(),
        },
    );
    assert!(
        compare_state(&app, view).is_some_and(|(.., loading, error)| {
            !loading && error.as_deref() == Some("no upstream branch is set for the current branch")
        })
    );

    // The request is settled, so a late answer cannot overwrite the failure.
    app.on_backend_event(
        Some(RequestId(1)),
        range_ready(
            "origin/main",
            "HEAD",
            vec![prepared_change("a.rs", StatusKind::Modified)],
        ),
    );
    assert!(
        compare_state(&app, view)
            .is_some_and(|(_, _, _, files, _, error)| { files == 0 && error.is_some() })
    );
    assert_eq!(compare_tabs(&app), 1);
}

#[test]
fn closing_a_reserved_compare_tab_cancels_it_and_a_late_answer_stays_closed() {
    let (mut app, backend) = compare_app();
    app.dispatch(Command::DiffSinceBase);
    let view = app.tabs[app.active].view;

    app.request_close_active_tab();

    assert!(!app.all_tabs().any(|tab| tab.view == view));
    let cancelled = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter().any(|(_, command)| {
                matches!(command, SessionCommand::Cancel { request } if *request == RequestId(1))
            })
        })
        .unwrap_or_default();
    assert!(cancelled, "closing sends cooperative cancellation");

    app.on_backend_event(
        Some(RequestId(1)),
        range_ready(
            "main",
            "HEAD",
            vec![prepared_change("a.rs", StatusKind::Modified)],
        ),
    );
    assert_eq!(compare_tabs(&app), 0);
}

#[test]
fn an_empty_range_fills_its_tab_and_says_why_it_is_empty() {
    let (mut app, _backend) = compare_app();
    app.dispatch(Command::DiffSinceBase);
    let view = app.tabs[app.active].view;

    app.on_backend_event(Some(RequestId(1)), range_ready("main", "HEAD", Vec::new()));
    assert!(
        compare_state(&app, view).is_some_and(|(_, _, _, files, loading, error)| files == 0
            && !loading
            && error.is_none())
    );
    assert!(
        last_message(&app)
            .as_deref()
            .is_some_and(|message| message.contains("no changes between main and HEAD"))
    );
}

#[test]
fn without_a_backend_no_compare_tab_is_reserved() {
    let mut app = app();
    app.dispatch(Command::DiffSinceBase);
    assert_eq!(
        compare_tabs(&app),
        0,
        "nothing would ever answer the request"
    );
}
