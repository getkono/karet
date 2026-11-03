//! The explorer against a backend that holds its listings: changes made outside
//! the editor, and names the explorer cannot see.

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
