use super::support::*;
use crate::app::*;

#[test]
fn pending_save_drives_the_animation_tick() {
    let mut app = app();
    assert!(app.next_wake().is_none());
    app.pending_saves
        .insert(RequestId(1), PendingSave { doc: DocumentId(1) });
    assert_eq!(app.next_wake(), Some(Duration::from_millis(100)));
}

#[test]
fn save_completion_clears_the_spinner() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.tabs[app.active].saving_since = Some(Instant::now());
    app.pending_saves
        .insert(RequestId(5), PendingSave { doc: DocumentId(2) });
    app.on_backend_event(
        Some(RequestId(5)),
        SessionEvent::Saved { doc: DocumentId(2) },
    );
    assert!(app.tabs[app.active].saving_since.is_none());
    assert!(app.pending_saves.is_empty());
}

#[test]
fn duplicate_save_command_is_debounced_while_in_flight() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }

    app.save_active();
    app.save_active();

    let sent_saves = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| matches!(command, SessionCommand::Save { .. }))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(sent_saves, 1, "only one save may be in flight per document");
    assert_eq!(
        last_message(&app).as_deref(),
        Some("save already in progress"),
        "the second shortcut is ignored because the first save is still pending"
    );
}

#[test]
fn after_delay_auto_save_debounces_to_the_newest_edit() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.files.auto_save = AutoSave::AfterDelay;
    app.settings.files.auto_save_delay = 100;
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    let start = Instant::now();

    app.schedule_auto_save(DocumentId(2), 1, start);
    app.schedule_auto_save(DocumentId(2), 1, start + Duration::from_millis(50));
    app.fire_auto_save(start + Duration::from_millis(100));
    assert_eq!(saved_docs(&backend), [DocumentId(2)]);

    let request = app.pending_saves.keys().next().copied();
    if let Some(request) = request {
        app.on_backend_event(Some(request), SessionEvent::Saved { doc: DocumentId(2) });
    }
    app.schedule_auto_save(DocumentId(2), 2, start + Duration::from_millis(110));
    app.schedule_auto_save(DocumentId(2), 3, start + Duration::from_millis(150));
    app.fire_auto_save(start + Duration::from_millis(249));
    assert_eq!(saved_docs(&backend).len(), 1);
    app.fire_auto_save(start + Duration::from_millis(250));
    assert_eq!(saved_docs(&backend).len(), 2);
}

#[test]
fn focus_change_auto_save_only_fires_when_the_edited_document_loses_focus() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.files.auto_save = AutoSave::OnFocusChange;
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.schedule_auto_save(DocumentId(2), 1, Instant::now());

    app.auto_save_context_changed(Some(DocumentId(2)));
    assert!(
        saved_docs(&backend).is_empty(),
        "the same editor keeps focus"
    );
    app.focus = Focus::Sidebar;
    app.auto_save_context_changed(Some(DocumentId(2)));
    assert_eq!(saved_docs(&backend), [DocumentId(2)]);
}

#[test]
fn a_late_dirty_snapshot_saves_when_its_document_already_lost_focus() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.files.auto_save = AutoSave::OnFocusChange;
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.focus = Focus::Sidebar;

    app.schedule_auto_save(DocumentId(2), 1, Instant::now());

    assert_eq!(saved_docs(&backend), [DocumentId(2)]);
}

#[test]
fn an_edit_during_an_auto_save_keeps_its_own_follow_up_deadline() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.files.auto_save = AutoSave::AfterDelay;
    app.settings.files.auto_save_delay = 10;
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    let start = Instant::now();
    app.schedule_auto_save(DocumentId(2), 1, start);
    app.fire_auto_save(start + Duration::from_millis(10));
    let request = app.pending_saves.keys().next().copied();

    app.schedule_auto_save(DocumentId(2), 2, start + Duration::from_millis(11));
    app.fire_auto_save(start + Duration::from_millis(21));
    assert_eq!(
        saved_docs(&backend).len(),
        1,
        "only one save may be in flight"
    );
    if let Some(request) = request {
        app.on_backend_event(Some(request), SessionEvent::Saved { doc: DocumentId(2) });
    }
    assert!(app.auto_save_pending.contains_key(&DocumentId(2)));
    app.fire_auto_save(start + Duration::from_millis(21));
    assert_eq!(saved_docs(&backend).len(), 2);
}

#[test]
fn an_auto_save_conflict_keeps_the_buffer_dirty_and_warns() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.settings.files.auto_save = AutoSave::AfterDelay;
    app.settings.files.auto_save_delay = 1;
    app.push_tab(text_tab("t.rs", "local"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.tabs[app.active].dirty = true;
    let start = Instant::now();
    app.schedule_auto_save(DocumentId(2), 1, start);
    app.fire_auto_save(start + Duration::from_millis(1));
    let request = app.pending_saves.keys().next().copied();

    if let Some(request) = request {
        app.on_backend_event(
            Some(request),
            SessionEvent::ExternalConflict { doc: DocumentId(2) },
        );
    }

    assert!(app.tabs[app.active].dirty);
    assert!(app.notifications.active().iter().any(|notification| {
        notification.title.contains("file changed on disk")
            && notification.severity == Severity::Warning
    }));
}

#[test]
fn save_active_marks_every_view_of_the_document_as_saving() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.split_focused(SplitDir::Right);

    app.save_active();

    assert!(app.tabs[app.active].saving_since.is_some());
    let stored_saving = app
        .stored
        .values()
        .flat_map(|pane| pane.tabs.iter())
        .any(|tab| tab.saving_since.is_some());
    assert!(
        stored_saving,
        "background split view should show save progress"
    );
}

#[test]
fn quit_save_all_conflict_keeps_the_app_open() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(2));
    }
    app.tabs[app.active].dirty = true;
    app.saving_close = Some(CloseRequest::Quit);
    app.pending_saves
        .insert(RequestId(5), PendingSave { doc: DocumentId(2) });

    app.on_backend_event(
        Some(RequestId(5)),
        SessionEvent::ExternalConflict { doc: DocumentId(2) },
    );

    assert!(!app.should_quit);
    assert!(app.saving_close.is_none());
    assert!(app.tabs[app.active].dirty);
}

#[test]
fn saved_event_clears_the_dirty_flag() {
    let mut app = app();
    app.push_tab(text_tab("t.rs", "x"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(1));
    }
    app.tabs[app.active].dirty = true;
    app.on_backend_event(None, SessionEvent::Saved { doc: DocumentId(1) });
    assert!(!app.tabs[app.active].dirty);
}

#[test]
fn scm_log_pages_replace_then_append() {
    let mut app = app();
    // The first page replaces and clears the in-flight flag.
    app.scm.log_loading = true;
    app.apply_vcs_log(0, vec![commit("aaaaaaa", "first")], true);
    assert_eq!(app.scm.log.len(), 1);
    assert!(app.scm.log_has_more);
    assert!(!app.scm.log_loading);
    // A page at the right offset appends.
    app.apply_vcs_log(1, vec![commit("bbbbbbb", "second")], false);
    assert_eq!(app.scm.log.len(), 2);
    assert!(!app.scm.log_has_more);
    // A page at the wrong offset is ignored (no duplicate/torn appends).
    app.apply_vcs_log(5, vec![commit("ccccccc", "stale")], false);
    assert_eq!(app.scm.log.len(), 2);
}

#[test]
fn hover_maps_to_explorer_and_scm_rows() {
    let mut app = app();
    app.sidebar_content_rect = Rect {
        x: 0,
        y: 2,
        width: 20,
        height: 10,
    };
    // Explorer: hover at y=4 with offset 0 → absolute row 2.
    app.hover = Some((5, 4));
    assert_eq!(app.hovered_explorer_row(), Some(2));
    // Above the content area → no hovered row.
    app.hover = Some((5, 1));
    assert_eq!(app.hovered_explorer_row(), None);

    // Source control: display 0 is a section header, 1 and 2 are changes. Hover
    // maps against the changes region rect.
    app.scm_ui.changes_rect = Rect {
        x: 0,
        y: 2,
        width: 20,
        height: 10,
    };
    app.scm_ui.offset = 0;
    app.scm_ui.row_map = vec![None, Some(0), Some(1)];
    app.hover = Some((5, 3)); // display = 0 + (3 - 2) = 1 → change 0
    assert_eq!(app.hovered_scm_change(), Some(0));
    app.hover = Some((5, 2)); // display 0 → header → nothing
    assert_eq!(app.hovered_scm_change(), None);
}

#[test]
fn sidebar_header_hover_tracks_header_only() {
    let mut app = app();
    app.sidebar_visible = true;
    app.sidebar_rect = Rect {
        x: 0,
        y: 1,
        width: 20,
        height: 8,
    };
    app.sidebar_content_rect = Rect {
        x: 0,
        y: 2,
        width: 20,
        height: 7,
    };
    let moved = |column, row| MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };

    app.handle_mouse(moved(5, 1));
    assert_eq!(app.sidebar_header_hover, Some((5, 1)));
    assert_eq!(app.hover, None);

    app.handle_mouse(moved(5, 3));
    assert_eq!(app.sidebar_header_hover, None);
    assert_eq!(app.hover, Some((5, 3)));

    app.handle_mouse(moved(30, 3));
    assert_eq!(app.sidebar_header_hover, None);
    assert_eq!(app.hover, None);
}

#[test]
fn notify_makes_errors_persistent_and_info_transient() {
    let mut app = app();
    app.notify(Report::Failure, NotificationKind::Io, "save failed");
    app.notify(Report::Outcome, NotificationKind::Vcs, "committed");
    let active = app.notifications.active();
    assert_eq!(active.len(), 2);
    // Newest (info) is first; it auto-expires. The error persists.
    assert!(active[0].timeout.is_some());
    assert!(active[1].timeout.is_none());
}

#[test]
fn esc_dismisses_a_toast_before_normal_handling() {
    let mut app = app();
    app.notify(Report::Failure, NotificationKind::Io, "boom");
    assert!(!app.notifications.is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
    assert!(app.notifications.is_empty());
    // A second Esc, with no toast left, falls through to normal handling.
    assert!(!app.should_quit);
}

#[test]
fn starts_explorer_focused_with_welcome_tab() {
    let app = app();
    assert_eq!(app.focus, Focus::Sidebar);
    assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
    assert!(matches!(app.tabs[0].kind, TabKind::Welcome));
}

#[test]
fn focus_target_tracks_focus_and_panel() {
    let mut app = app();
    assert_eq!(app.focus_target(), FocusTarget::Explorer);
    app.sidebar_panel = SidebarPanel::SourceControl;
    assert_eq!(app.focus_target(), FocusTarget::SourceControl);
    app.focus = Focus::Editor;
    assert_eq!(app.focus_target(), FocusTarget::Editor);
}

#[test]
fn open_anyway_bypasses_the_guard_and_decodes_in_place() {
    // A .cbor file that (per its recorded length) tripped the size guard shows a
    // too-large placeholder; the override re-opens it decoded, in the same tab.
    let dir = std::env::temp_dir().join(format!("karet-anyway-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("big.cbor");
    // 0x81 0x01: the CBOR array [1].
    let _ = std::fs::write(&file, [0x81u8, 0x01]);

    let mut app = app();
    let len = karet_fileview::viewer::SIZE_GUARD + 1;
    app.tabs = vec![Tab::new(
        "big.cbor",
        TabKind::Placeholder {
            path: file.clone(),
            kind: FileKind::TooLarge { len },
            dims: None,
            len,
        },
    )];
    app.active = 0;
    app.focus = Focus::Editor;
    // A too-large placeholder gets the override layer, so Enter is bound.
    assert_eq!(app.focus_target(), FocusTarget::Oversize);

    app.dispatch(Command::OpenAnyway);
    assert_eq!(
        app.tabs.len(),
        1,
        "the placeholder is replaced, not appended"
    );
    assert!(
        matches!(
            app.tabs[0].kind,
            TabKind::Code {
                language: "CBOR",
                ..
            }
        ),
        "open-anyway decodes the CBOR in place"
    );

    // The override is inert on an ordinary tab.
    app.dispatch(Command::OpenAnyway);
    assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_message_no_longer_takes_the_hint_bar_or_its_click_targets() {
    // The defect this replaced: a message rendered into the hint region, which
    // skipped the pass that records `status_hits` — so while one showed, the bar
    // named no shortcuts and every shortcut on it stopped answering clicks.
    let mut app = app();
    let _ = frame(&mut app, 120, 8);
    let hints_before = app.status_hits.len();
    assert!(hints_before > 0, "the bar starts with clickable hints");

    app.notify(Report::Failure, NotificationKind::Io, "save failed");
    let rows = screen(&mut app, 120, 8).join("\n");

    assert_eq!(
        app.status_hits.len(),
        hints_before,
        "the hints and their click targets survive an active notification"
    );
    assert!(
        rows.contains("save failed"),
        "the message is on screen:\n{rows}"
    );
}

#[test]
fn a_failure_is_painted_in_the_error_role_rather_than_the_status_bar_style() {
    // The other half of the defect: the status line drew every message in the
    // bar's own style, so an error was exactly the colour of ordinary chrome.
    let mut app = app();
    app.notify(Report::Failure, NotificationKind::Io, "save failed");
    let error = app.theme.role(ThemeRole::DiagnosticError).to_ratatui();
    let chrome = app.theme.role(ThemeRole::StatusBarForeground).to_ratatui();
    assert_ne!(
        error, chrome,
        "the roles must differ for this to prove anything"
    );

    let buffer = frame(&mut app, 120, 8);
    assert!(
        buffer.content().iter().any(|cell| cell.fg == error),
        "the card is painted in the error role"
    );
}

#[test]
fn a_refusal_clears_itself_while_a_failure_waits_to_be_read() {
    // Lifetime is the axis that separates them: a refusal the user provoked goes
    // away on its own, a failure stays until it is dismissed.
    let mut app = app();
    app.notify(
        Report::Refusal,
        NotificationKind::Io,
        "save: open a text file",
    );
    app.notify(Report::Failure, NotificationKind::Io, "save failed");

    let active = app.notifications.active();
    let refusal = active
        .iter()
        .find(|note| note.title.starts_with("save: open"))
        .expect("the refusal is active");
    let failure = active
        .iter()
        .find(|note| note.title == "save failed")
        .expect("the failure is active");
    assert!(refusal.timeout.is_some(), "a refusal expires");
    assert!(failure.timeout.is_none(), "a failure waits");
    assert_eq!(refusal.severity, Severity::Warning);
    assert_eq!(failure.severity, Severity::Error);
}

#[test]
fn a_save_batch_card_is_retired_when_the_batch_finishes() {
    // A progress card carries no timeout, so whatever raises one owes it a
    // retirement on *every* exit. This is the success exit: the saves land, the
    // parked close runs, and the card must not outlive the batch.
    let dir = test_dir("save-batch-card");
    write_file(&dir, "a.rs", b"fn main() {}\n");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app.notify_progress(
        NotificationKind::Io,
        App::SAVE_BATCH_TAG.to_string(),
        "saving 1 file(s) before quitting…",
        None,
    );
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|note| note.tag.as_deref() == Some("save.batch")),
        "the batch card is up"
    );

    // Nothing is pending, so the next backend event settles the batch.
    app.on_backend_event(
        None,
        SessionEvent::VcsStatus {
            staged: Vec::new(),
            working: Vec::new(),
        },
    );

    assert!(
        !app.notifications
            .active()
            .iter()
            .any(|note| note.tag.as_deref() == Some("save.batch")),
        "the card is retired once no save is pending"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dependency_check_card_is_retired_when_the_hints_arrive() {
    // The hints are the answer to an explicit re-check, so they retire its card.
    let mut app = app();
    app.notify_progress(
        NotificationKind::System,
        App::DEPS_CHECK_TAG.to_string(),
        "re-checking dependencies",
        None,
    );
    assert!(
        app.notifications
            .active()
            .iter()
            .any(|note| note.tag.as_deref() == Some("deps.check")),
        "the check card is up"
    );

    app.on_backend_event(
        None,
        SessionEvent::ManifestHints {
            doc: DocumentId(1),
            version: 1,
            hints: Vec::new(),
        },
    );

    assert!(
        !app.notifications
            .active()
            .iter()
            .any(|note| note.tag.as_deref() == Some("deps.check")),
        "the card does not outlive the check it describes"
    );
}
