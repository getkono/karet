//! The explorer against a backend that holds its listings: changes made outside
//! the editor, and names the explorer cannot see.

use karet_session::api::PathMutation;

use super::support::*;
use crate::app::*;

fn explorer_app(dir: &Path) -> App {
    let mut app = app_at(dir.to_path_buf());
    app.sidebar_panel = SidebarPanel::Explorer;
    build_explorer_from_disk(&mut app);
    app
}

fn shows(app: &App, path: &Path) -> bool {
    app.explorer.rows().iter().any(|row| row.path == path)
}

/// A file created outside the editor (a checkout, a build, a shell) appears
/// once the backend reports the change, without a manual refresh.
#[test]
fn a_file_created_outside_the_editor_appears_on_the_change_event() {
    let dir = test_dir("explorer-fs-change");
    write_file(&dir, "a.txt", b"a");
    let mut app = explorer_app(&dir);
    write_file(&dir, "made-elsewhere.txt", b"new");

    app.on_backend_event(
        None,
        SessionEvent::FsChanged {
            paths: vec![dir.join("made-elsewhere.txt")],
        },
    );
    build_explorer_from_disk(&mut app);

    assert!(shows(&app, &dir.join("made-elsewhere.txt")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Refresh re-reads every directory the tree holds, including an expanded
/// subdirectory, rather than redrawing the listings it already had.
#[test]
fn refresh_rereads_every_listed_directory() {
    let dir = test_dir("explorer-refresh");
    write_file(&dir, "src/lib.rs", b"lib");
    write_file(&dir, "a.txt", b"a");
    let mut app = explorer_app(&dir);
    app.explorer.expand(&dir.join("src"));
    build_explorer_from_disk(&mut app);
    write_file(&dir, "src/new.rs", b"new");
    std::fs::remove_file(dir.join("a.txt")).ok();

    app.explorer_refresh();
    build_explorer_from_disk(&mut app);

    assert!(shows(&app, &dir.join("src/new.rs")));
    assert!(!shows(&app, &dir.join("a.txt")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Pasting into a collapsed folder that already holds the name: the explorer
/// has no row for it, the backend refuses, and the paste takes the next name.
#[test]
fn pasting_into_a_collapsed_folder_holding_the_name_makes_a_copy() {
    let dir = test_dir("explorer-paste-collapsed");
    write_file(&dir, "a.txt", b"outer");
    write_file(&dir, "sub/a.txt", b"inner");
    write_file(&dir, "sub/b.txt", b"b");
    let mut app = explorer_app(&dir);

    select_explorer_path(&mut app, &dir.join("a.txt"));
    app.dispatch(Command::Copy);
    select_explorer_path(&mut app, &dir.join("sub"));
    app.dispatch(Command::Paste);
    settle_mutations(&mut app);

    assert_eq!(
        std::fs::read(dir.join("sub/a.txt")).ok(),
        Some(b"inner".to_vec())
    );
    assert_eq!(
        std::fs::read(dir.join("sub/a copy.txt")).ok(),
        Some(b"outer".to_vec())
    );
    assert!(
        last_message(&app).is_some_and(|message| !message.contains("failed")),
        "a name the explorer could not see is not a failure"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mutation landing re-reads the directories it touched without blanking
/// them: the rows, and the cursor on them, stay until each answer replaces them.
#[test]
fn a_mutation_keeps_the_rows_and_the_cursor_until_the_listing_answers() {
    let dir = test_dir("explorer-mutation-rows");
    write_file(&dir, "a.txt", b"a");
    write_file(&dir, "b.txt", b"b");
    let mut app = explorer_app(&dir);
    select_explorer_path(&mut app, &dir.join("b.txt"));
    app.mutate_path(
        PathMutation::CreateFile {
            path: dir.join("c.txt"),
        },
        crate::app::explorer_mutate::FollowUp::Deleted,
    );
    let (id, mutation) = app
        .pending_mutations
        .iter()
        .map(|(id, pending)| (*id, pending.mutation.clone()))
        .next()
        .expect("the mutation was sent");
    std::fs::write(dir.join("c.txt"), b"").ok();

    app.on_backend_event(
        Some(id),
        SessionEvent::PathMutated {
            mutation,
            result: Ok(()),
        },
    );
    app.build_explorer();

    assert!(shows(&app, &dir.join("a.txt")) && shows(&app, &dir.join("b.txt")));
    assert_eq!(
        app.explorer.selected_path(),
        Some(dir.join("b.txt").as_path())
    );
    assert!(
        app.pending_listings.values().any(|listed| *listed == dir),
        "the parent is read again"
    );
    build_explorer_from_disk(&mut app);
    assert!(shows(&app, &dir.join("c.txt")));
    assert_eq!(
        app.explorer.selected_path(),
        Some(dir.join("b.txt").as_path())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A folder the change watcher says nothing about (an ignored `target/`, a
/// hidden one) is read again when it is expanded, so reopening it shows what is
/// there now rather than what was there when it was first listed.
#[test]
fn expanding_a_listed_folder_reads_it_again() {
    let dir = test_dir("explorer-expand-reread");
    write_file(&dir, "target/old.o", b"o");
    write_file(&dir, "a.txt", b"a");
    let mut app = explorer_app(&dir);
    let target = dir.join("target");
    app.explorer.expand(&target);
    build_explorer_from_disk(&mut app);
    app.explorer.collapse(&target);
    build_explorer_from_disk(&mut app);
    write_file(&dir, "target/new.o", b"n");

    app.explorer.expand(&target);
    build_explorer_from_disk(&mut app);

    assert!(shows(&app, &dir.join("target/new.o")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// After the backend has gone, the tree keeps asking for the listings it lacks
/// on every build; a failed send must not raise a notification each time. The
/// loss is reported once, when the backend's streams close.
#[test]
fn listings_asked_of_a_closed_backend_raise_no_notifications() {
    let dir = test_dir("explorer-closed-backend");
    write_file(&dir, "a.txt", b"a");
    let mut app = app_at(dir.clone());
    app.sidebar_panel = SidebarPanel::Explorer;
    app.backend = Some(Arc::new(ClosedBackend));
    let before = app.notifications.active().len();

    for _ in 0..5 {
        app.build_explorer();
    }

    assert_eq!(app.notifications.active().len(), before);
    assert!(
        app.explorer.is_waiting(),
        "the tree still knows it is incomplete"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A backend whose connection has closed: every send fails.
struct ClosedBackend;

impl karet_session::backend::Backend for ClosedBackend {
    fn send(
        &self,
        _id: RequestId,
        _command: SessionCommand,
    ) -> Result<(), karet_session::backend::BackendError> {
        Err(karet_session::backend::BackendError::Closed)
    }

    fn next_id(&self) -> RequestId {
        RequestId(1)
    }

    fn take_events(&self) -> Option<karet_session::EventRx> {
        None
    }
}

/// A backend refusing every name stops a paste after a bounded number of tries
/// and reports the clash, rather than looping.
#[test]
fn a_paste_refused_at_every_name_gives_up_and_says_so() {
    let dir = test_dir("explorer-paste-cap");
    write_file(&dir, "a.txt", b"a");
    let mut app = explorer_app(&dir);
    select_explorer_path(&mut app, &dir.join("a.txt"));
    app.dispatch(Command::Copy);
    app.dispatch(Command::Paste);

    let mut answered = 0;
    while let Some((id, mutation)) = app
        .pending_mutations
        .iter()
        .map(|(id, pending)| (*id, pending.mutation.clone()))
        .next()
    {
        answered += 1;
        assert!(answered <= 100, "the paste must stop retrying");
        let PathMutation::Copy { to, .. } = &mutation else {
            panic!("a paste copies");
        };
        let refusal = format!("{} already exists", to.display());
        app.on_backend_event(
            Some(id),
            SessionEvent::PathMutated {
                mutation,
                result: Err(refusal),
            },
        );
    }

    assert_eq!(answered, 65, "the first name and 64 more");
    assert!(last_message(&app).is_some_and(|message| message.contains("paste failed")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two pastes before the first lands: the second must not choose the name the
/// first is about to create.
#[test]
fn pasting_twice_before_the_first_lands_makes_two_copies() {
    let dir = test_dir("explorer-paste-twice");
    write_file(&dir, "a.txt", b"a");
    let mut app = explorer_app(&dir);

    select_explorer_path(&mut app, &dir.join("a.txt"));
    app.dispatch(Command::Copy);
    app.dispatch(Command::Paste);
    app.dispatch(Command::Paste);
    settle_mutations(&mut app);

    assert!(dir.join("a copy.txt").exists());
    assert!(dir.join("a copy 2.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
