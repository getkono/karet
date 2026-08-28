//! The confirmation dialog: what it offers, what a key does to it, and the
//! structural guarantee that an unread dialog cannot destroy anything.

use super::support::*;
use crate::app::confirm::ConfirmAction;
use crate::app::confirm::ConfirmChoice;
use crate::app::confirm::ConfirmDialog;
use crate::app::*;

/// An app whose backend records what the confirmed action actually sent.
fn recording_app() -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    (backend, app)
}

fn sent(backend: &RecordingBackend) -> Vec<SessionCommand> {
    backend
        .sent
        .lock()
        .map(|sent| sent.iter().map(|(_, c)| c.clone()).collect())
        .unwrap_or_default()
}

fn dialog() -> ConfirmDialog {
    ConfirmDialog::new(
        "Discard changes to 1 file(s)?",
        "This cannot be undone.",
        vec![
            ConfirmChoice::custom("Keep changes", ConfirmAction::Cancel),
            ConfirmChoice::custom(
                "Discard",
                ConfirmAction::DiscardPaths(vec![PathBuf::from("a.rs")]),
            ),
        ],
    )
}

fn selected(app: &App) -> Option<ConfirmAction> {
    app.confirm
        .as_ref()
        .and_then(|d| d.selected_choice())
        .map(|choice| choice.action.clone())
}

#[test]
fn a_new_confirmation_selects_the_safe_first_answer() {
    let mut app = app();
    app.confirm(dialog());
    assert_eq!(selected(&app), Some(ConfirmAction::Cancel));
}

#[test]
fn enter_on_an_unread_confirmation_takes_the_safe_answer() {
    let (backend, mut app) = recording_app();
    app.confirm(dialog());
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.confirm.is_none(), "the dialog closes");
    assert!(
        sent(&backend).is_empty(),
        "the destructive command never reached the backend"
    );
}

#[test]
fn an_unbound_key_cancels_the_confirmation() {
    let (backend, mut app) = recording_app();
    app.confirm(dialog());
    send_key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    assert!(app.confirm.is_none());
    assert!(sent(&backend).is_empty());
}

#[test]
fn escape_cancels_the_confirmation() {
    let mut app = app();
    app.confirm(dialog());
    send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.confirm.is_none());
}

#[test]
fn stepping_down_then_accepting_runs_the_destructive_answer() {
    let (backend, mut app) = recording_app();
    app.confirm(dialog());
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(matches!(
        selected(&app),
        Some(ConfirmAction::DiscardPaths(_))
    ));
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.confirm.is_none());
    assert!(
        sent(&backend)
            .iter()
            .any(|c| matches!(c, SessionCommand::Discard { .. })),
        "the discard reached the backend only after a deliberate step"
    );
}

#[test]
fn the_selection_stops_at_the_ends_rather_than_wrapping_onto_the_destructive_row() {
    let mut app = app();
    app.confirm(dialog());
    // Holding "up" at the top must not teleport the cursor onto "Discard".
    for _ in 0..5 {
        send_key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    }
    assert_eq!(selected(&app), Some(ConfirmAction::Cancel));
}

#[test]
fn a_confirmation_shadows_the_editor_underneath_it() {
    let mut app = app();
    app.tabs.push(text_tab("a.rs", "alpha"));
    app.active = app.tabs.len() - 1;
    let before = code_tab_text(&app);
    app.confirm(dialog());
    // A printable key is the modal's business, not the buffer's.
    send_key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    assert_eq!(
        code_tab_text(&app),
        before,
        "no text leaked into the editor"
    );
}

#[test]
fn discarding_from_source_control_asks_before_it_acts() {
    let (backend, mut app) = recording_app();
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.focus = Focus::Sidebar;
    app.dispatch(Command::ScmDiscard);
    assert!(app.confirm.is_some(), "the discard raised a confirmation");
    assert!(
        sent(&backend).is_empty(),
        "nothing was discarded before the answer"
    );
    let title = app
        .confirm
        .as_ref()
        .map(|d| d.title.clone())
        .unwrap_or_default();
    assert!(title.contains("Discard"), "{title}");
}

/// The answer a dialog runs when the user presses Enter without reading it.
fn safe_answer(app: &App) -> Option<ConfirmAction> {
    selected(app)
}

/// Every dialog's title, for asserting the question actually names its subject.
fn title(app: &App) -> String {
    app.confirm
        .as_ref()
        .map(|d| d.title.clone())
        .unwrap_or_default()
}

#[test]
fn switching_branches_with_dirty_editors_asks_before_saving() {
    let mut app = app();
    app.tabs.push(text_tab("a.rs", "alpha"));
    app.active = app.tabs.len() - 1;
    app.tabs[app.active].dirty = true;

    app.guard_branch_switch(karet_vcs::BranchTarget::Local("feature".to_string()));
    assert!(app.confirm.is_some());
    assert!(title(&app).contains("unsaved"), "{}", title(&app));
    assert_eq!(
        safe_answer(&app),
        Some(ConfirmAction::Cancel),
        "staying put is what Enter does"
    );
}

#[test]
fn a_clean_worktree_switches_branches_without_asking() {
    let (backend, mut app) = recording_app();
    app.guard_branch_switch(karet_vcs::BranchTarget::Local("feature".to_string()));
    assert!(app.confirm.is_none(), "nothing to warn about");
    assert!(
        !sent(&backend).is_empty(),
        "the switch ran straight through"
    );
}

#[test]
fn a_hard_reset_names_the_revision_and_defaults_to_cancel() {
    let mut app = app();
    app.confirm_action(
        "Hard-reset to 76784c8?",
        "Throws away every uncommitted change.",
        "Cancel",
        "Reset --hard to 76784c8",
        ConfirmAction::ResetHard("76784c8abc".to_string()),
    );
    assert_eq!(safe_answer(&app), Some(ConfirmAction::Cancel));
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert!(matches!(selected(&app), Some(ConfirmAction::ResetHard(rev)) if rev == "76784c8abc"));
}

#[test]
fn deleting_a_remote_branch_carries_both_the_remote_and_the_branch() {
    let mut app = app();
    app.handle_overlay_event(crate::overlay::OverlayEvent::AcceptDeleteRemoteBranch {
        remote: "origin".to_string(),
        branch: "feature".to_string(),
    });
    assert!(title(&app).contains("origin/feature"), "{}", title(&app));
    assert_eq!(safe_answer(&app), Some(ConfirmAction::Cancel));
    send_key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(
        selected(&app),
        Some(ConfirmAction::DeleteRemoteBranch {
            remote: "origin".to_string(),
            branch: "feature".to_string(),
        })
    );
}

#[test]
fn dropping_a_stash_asks_and_defaults_to_keeping_it() {
    let mut app = app();
    app.handle_overlay_event(crate::overlay::OverlayEvent::AcceptStashAction(
        crate::overlay::StashAction::Drop("stash@{0}".to_string()),
    ));
    assert!(title(&app).contains("stash@{0}"), "{}", title(&app));
    assert_eq!(safe_answer(&app), Some(ConfirmAction::Cancel));
}
