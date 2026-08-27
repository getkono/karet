use super::support::*;
use crate::app::*;

#[test]
fn enter_on_a_focused_diff_opens_the_file_at_its_first_changed_line() {
    let dir = test_dir("diff-enter-into-file");
    write_file(&dir, "a.rs", b"fn a() {}\nfn added() {}\nfn c() {}\n");
    let changed = ChangeSummary {
        path: PathBuf::from("a.rs"),
        old_path: None,
        status: StatusKind::Modified,
        is_binary: false,
        added: 1,
        removed: 0,
    };
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), vec![changed], false);
    app.backend = Some(backend);
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.focus = Focus::Sidebar;
    app.dispatch(Command::SidebarActivate); // reserve + focus the diff
    assert_eq!(app.focus_target(), FocusTarget::DiffEditor);
    // The backend answers with the prepared diff (line 2 is the addition).
    app.on_backend_event(
        None,
        SessionEvent::ChangePrepared {
            path: PathBuf::from("a.rs"),
            staged: false,
            result: Ok(Box::new(prepared_from_texts(
                "a.rs",
                StatusKind::Modified,
                "fn a() {}\nfn c() {}\n",
                "fn a() {}\nfn added() {}\nfn c() {}\n",
            ))),
        },
    );

    // Enter on the focused diff drops into the file, caret on the first
    // changed line (line 2, 0-based 1) — keyboard parity with the mouse.
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    deliver_content(
        &mut app,
        &dir.join("a.rs"),
        "fn a() {}\nfn added() {}\nfn c() {}\n",
    );
    assert!(
        matches!(app.tabs[app.active].kind, TabKind::Code { .. }),
        "a normal, editable editor tab"
    );
    assert_eq!(
        app.tabs[app.active].path().map(canonical),
        Some(canonical(&dir.join("a.rs")))
    );
    assert_eq!(app.focus, Focus::Editor);
    assert_eq!(
        app.tabs[app.active].editor.cursor().line,
        1,
        "caret lands on the first changed line"
    );
    assert_eq!(app.tabs.len(), 2, "the diff stays open alongside the file");

    // Enter again from the diff re-focuses the existing file tab — never a
    // duplicate.
    let file_idx = app.active;
    let diff_idx = app
        .tabs
        .iter()
        .position(Tab::is_diff)
        .expect("the diff tab is still open");
    app.select_tab(diff_idx);
    app.dispatch(Command::OpenDiffFile);
    assert_eq!(app.tabs.len(), 2, "no duplicate editor tab");
    assert_eq!(app.active, file_idx);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn enter_on_a_deleted_files_diff_reports_instead_of_opening() {
    let dir = test_dir("diff-enter-deleted");
    let deleted = ChangeSummary {
        path: PathBuf::from("gone.rs"),
        old_path: None,
        status: StatusKind::Deleted,
        is_binary: false,
        added: 0,
        removed: 1,
    };
    let mut app = App::new(dir.clone(), Vec::new(), vec![deleted], false);
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.focus = Focus::Sidebar;
    app.dispatch(Command::SidebarActivate);
    assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

    // The file is gone from the working tree: Enter degrades to a status
    // message — no dead tab, no panic.
    app.dispatch(Command::OpenDiffFile);
    assert_eq!(app.tabs.len(), 1, "nothing new opens for a deleted file");
    assert!(app.active_is_diff(), "the diff stays active");
    assert!(
        app.status.as_deref().is_some_and(|s| s.contains("gone.rs")),
        "a status message names the missing file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stage_hunk_applies_the_hunk_patch_and_reloads_the_diff() {
    let dir = test_dir("diff-stage-hunk");
    write_file(&dir, "a.rs", b"fn a() {}\nfn added() {}\nfn c() {}\n");
    let changed = ChangeSummary {
        path: PathBuf::from("a.rs"),
        old_path: None,
        status: StatusKind::Modified,
        is_binary: false,
        added: 1,
        removed: 0,
    };
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), vec![changed], false);
    app.backend = Some(backend.clone());
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.focus = Focus::Sidebar;
    app.dispatch(Command::SidebarActivate);
    app.on_backend_event(
        None,
        SessionEvent::ChangePrepared {
            path: PathBuf::from("a.rs"),
            staged: false,
            result: Ok(Box::new(prepared_from_texts(
                "a.rs",
                StatusKind::Modified,
                "fn a() {}\nfn c() {}\n",
                "fn a() {}\nfn added() {}\nfn c() {}\n",
            ))),
        },
    );
    assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

    // `u` on a working-tree diff is a hint, not an inverted apply.
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
    assert!(
        !backend.sent.lock().is_ok_and(|sent| sent
            .iter()
            .any(|(_, c)| matches!(c, SessionCommand::ApplyIndexPatch { .. }))),
        "mismatched verb sends nothing"
    );

    // `s` stages the hunk under the viewport top.
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let patch = backend.sent.lock().ok().and_then(|sent| {
        sent.iter().find_map(|(_, command)| match command {
            SessionCommand::ApplyIndexPatch { patch, reverse } => Some((patch.clone(), *reverse)),
            _ => None,
        })
    });
    let (patch, reverse) = patch.expect("the hunk patch was sent");
    assert!(!reverse);
    assert!(
        patch.contains("+fn added() {}"),
        "patch carries the hunk: {patch}"
    );
    assert!(patch.contains("--- a/a.rs") && patch.contains("+++ b/a.rs"));
    // The tab reserved its loading state and re-requested the diff.
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::Diff {
            file: None,
            loading_since: Some(_),
            ..
        }
    ));
    let requested = backend.sent.lock().is_ok_and(|sent| {
        sent.iter().any(
            |(_, c)| matches!(c, SessionCommand::PrepareChange { path, staged: false } if path == Path::new("a.rs")),
        )
    });
    assert!(requested, "the diff is re-requested after the apply");

    let _ = std::fs::remove_dir_all(&dir);
}
