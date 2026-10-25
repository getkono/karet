//! The saved workspace layout (`workbench.restoreLayout`): what a clean exit
//! writes, and what the next start on the same root reopens from it.

use super::support::*;
use crate::app::*;

/// A workspace root holding `a.rs`, `b.rs`, and `c.rs`, each ten lines long,
/// and a separate directory for the saved layout.
fn workspace(name: &str) -> (PathBuf, PathBuf) {
    let root = test_dir(name);
    let body = "line\n".repeat(10);
    for file in ["a.rs", "b.rs", "c.rs"] {
        write_file(&root, file, body.as_bytes());
    }
    let state = test_dir(&format!("{name}-state"));
    (root, state)
}

fn fresh(root: &Path) -> App {
    let mut app = App::new(root.to_path_buf(), Vec::new(), Vec::new(), false);
    app.main_rect = Rect::new(0, 0, 160, 40);
    app
}

/// `a.rs` and `b.rs` (in front, with two carets) in the left pane, and `c.rs`
/// in a focused right pane.
fn two_pane_session(root: &Path) -> App {
    let mut app = fresh(root);
    app.open_initial(&root.join("a.rs"));
    app.open_initial(&root.join("b.rs"));
    let tab = &mut app.tabs[app.active];
    tab.editor
        .set_carets(&[LineCol::new(2, 1), LineCol::new(5, 3)]);
    tab.editor.scroll_line = 1;
    app.open_startup_split(&root.join("c.rs"));
    app
}

fn tab_paths(tabs: &[Tab]) -> Vec<PathBuf> {
    tabs.iter()
        .filter_map(Tab::path)
        .map(Path::to_path_buf)
        .collect()
}

fn cleanup(dirs: &[&Path]) {
    for dir in dirs {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_clean_exit_layout_reopens_its_panes_tabs_and_cursors() {
    let (root, state) = workspace("restore-round-trip");
    let before = two_pane_session(&root);
    let saved_carets = before.stored.values().next().map(|pane| {
        let tab = &pane.tabs[pane.active];
        (tab.editor.cursors().clone(), tab.editor.scroll_line)
    });
    assert!(before.save_workspace_layout(&state).is_ok());

    let mut after = fresh(&root);
    after.restore_workspace_layout(&state);

    assert_eq!(
        after.layout.tree(),
        before.layout.tree(),
        "same splits, same ratios"
    );
    assert_eq!(
        after.layout.focus(),
        before.layout.focus(),
        "the right pane has focus"
    );
    assert_eq!(tab_paths(&after.tabs), vec![root.join("c.rs")]);
    assert!(
        matches!(after.focus, Focus::Editor),
        "the keyboard is in the restored editor"
    );
    let left: Vec<&StoredPane> = after.stored.values().collect();
    assert_eq!(left.len(), 1);
    assert_eq!(
        tab_paths(&left[0].tabs),
        vec![root.join("a.rs"), root.join("b.rs")]
    );
    assert_eq!(left[0].active, 1, "b.rs is still in front");
    let front = &left[0].tabs[1];
    assert_eq!(
        Some((front.editor.cursors().clone(), front.editor.scroll_line)),
        saved_carets,
        "both carets and the scroll come back"
    );
    // Restored views get fresh ids, so none collides with a later open.
    let views: HashSet<ViewId> = after.all_tabs().map(|tab| tab.view).collect();
    assert_eq!(views.len(), 3);
    cleanup(&[&root, &state]);
}

#[test]
fn a_layout_is_keyed_by_the_absolute_root_and_reopens_in_its_spelling() {
    let (root, state) = workspace("restore-spelling");
    let other = test_dir("restore-spelling-other");
    assert!(
        two_pane_session(&root)
            .save_workspace_layout(&state)
            .is_ok()
    );

    // Another directory has no saved layout, even with the same file names.
    for file in ["a.rs", "b.rs", "c.rs"] {
        write_file(&other, file, b"x\n");
    }
    let mut elsewhere = fresh(&other);
    elsewhere.restore_workspace_layout(&state);
    assert_eq!(elsewhere.layout.pane_count(), 1);

    // The same root spelled another way finds it, and its tabs take that
    // spelling, as the tabs it opens itself would.
    let _ = std::fs::create_dir_all(root.join("sub"));
    let spelled = root.join("sub").join("..");
    let mut app = fresh(&spelled);
    app.restore_workspace_layout(&state);
    assert_eq!(app.layout.pane_count(), 2);
    assert_eq!(tab_paths(&app.tabs), vec![spelled.join("c.rs")]);
    cleanup(&[&root, &state, &other]);
}

#[test]
fn attaching_registers_the_tabs_of_every_pane() {
    let (root, state) = workspace("restore-register");
    let mut app = two_pane_session(&root);
    let backend = Arc::new(RecordingBackend::new());
    app.backend = Some(backend.clone());
    app.register_open_tabs();

    let mut opened: Vec<PathBuf> = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::OpenDocument { path, .. } => Some(path.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    opened.sort();
    assert_eq!(
        opened,
        vec![root.join("a.rs"), root.join("b.rs"), root.join("c.rs")],
        "the unfocused pane's tabs are opened too, not only the focused pane's"
    );
    assert_eq!(
        app.stored.len(),
        1,
        "registration leaves the panes in place"
    );
    assert_eq!(tab_paths(&app.tabs), vec![root.join("c.rs")]);
    cleanup(&[&root, &state]);
}

#[test]
fn a_file_opened_at_startup_keeps_the_saved_layout_out() {
    let (root, state) = workspace("restore-explicit");
    assert!(
        two_pane_session(&root)
            .save_workspace_layout(&state)
            .is_ok()
    );

    let mut app = fresh(&root);
    app.open_initial(&root.join("a.rs"));
    app.restore_workspace_layout(&state);
    assert_eq!(app.layout.pane_count(), 1);
    assert_eq!(tab_paths(&app.tabs), vec![root.join("a.rs")]);

    // A preview of anything but the README is a choice too.
    let mut previewed = fresh(&root);
    previewed.open_initial_preview(&root.join("b.rs"));
    previewed.restore_workspace_layout(&state);
    assert_eq!(previewed.layout.pane_count(), 1);
    cleanup(&[&root, &state]);
}

#[test]
fn the_readme_preview_gives_way_to_the_saved_layout() {
    let (root, state) = workspace("restore-readme");
    write_file(&root, "README.md", b"# hi\n");
    assert!(
        two_pane_session(&root)
            .save_workspace_layout(&state)
            .is_ok()
    );

    let mut app = fresh(&root);
    app.open_initial_preview(&root.join("README.md"));
    app.restore_workspace_layout(&state);
    assert_eq!(app.layout.pane_count(), 2);
    assert!(
        !app.all_tabs()
            .any(|tab| tab.path() == Some(root.join("README.md").as_path())),
        "the README preview is replaced, not kept beside the layout"
    );
    cleanup(&[&root, &state]);
}

#[test]
fn deleted_files_are_skipped_and_their_empty_pane_dropped() {
    let (root, state) = workspace("restore-missing");
    assert!(
        two_pane_session(&root)
            .save_workspace_layout(&state)
            .is_ok()
    );
    let _ = std::fs::remove_file(root.join("c.rs"));
    let _ = std::fs::remove_file(root.join("b.rs"));

    let mut app = fresh(&root);
    app.restore_workspace_layout(&state);
    assert_eq!(
        app.layout.pane_count(),
        1,
        "the pane that held only c.rs is gone"
    );
    assert!(app.stored.is_empty());
    assert_eq!(tab_paths(&app.tabs), vec![root.join("a.rs")]);
    assert_eq!(
        app.active, 0,
        "the front tab falls back to the one before it"
    );

    // Nothing left at all: the startup surface stays.
    let _ = std::fs::remove_file(root.join("a.rs"));
    let mut empty = fresh(&root);
    empty.restore_workspace_layout(&state);
    assert!(matches!(
        empty.tabs[..],
        [Tab {
            kind: TabKind::Welcome,
            ..
        }]
    ));
    cleanup(&[&root, &state]);
}

#[test]
fn only_file_tabs_are_saved_and_none_forgets_the_layout() {
    let (root, state) = workspace("restore-forget");
    let mut app = fresh(&root);
    app.open_initial(&root.join("a.rs"));
    app.push_tab(Tab::welcome());
    assert!(app.save_workspace_layout(&state).is_ok());
    let saved = karet_session::backup::read_layout(&state, &root).map(|layout| layout.root);
    let pane = match saved {
        Some(karet_session::backup::LayoutNode::Pane(pane)) => Some((pane.tabs.len(), pane.active)),
        _ => None,
    };
    // The Welcome tab is not saved, and the front one yields to the file
    // before it.
    assert_eq!(pane, Some((1, 0)));

    // Quitting with no file open leaves nothing to restore next time.
    let welcome = fresh(&root);
    assert!(welcome.save_workspace_layout(&state).is_ok());
    assert_eq!(karet_session::backup::read_layout(&state, &root), None);
    cleanup(&[&root, &state]);
}

#[test]
fn switching_restore_off_restores_nothing_and_forgets_the_layout() {
    let (root, state) = workspace("restore-off");
    assert!(
        two_pane_session(&root)
            .save_workspace_layout(&state)
            .is_ok()
    );

    let mut app = fresh(&root);
    app.settings.workbench.restore_layout = false;
    app.restore_workspace_layout(&state);
    assert_eq!(app.layout.pane_count(), 1, "nothing is restored");

    let mut session = two_pane_session(&root);
    session.settings.workbench.restore_layout = false;
    assert!(session.save_workspace_layout(&state).is_ok());
    assert_eq!(
        karet_session::backup::read_layout(&state, &root),
        None,
        "the exit deletes the layout an earlier session saved"
    );
    cleanup(&[&root, &state]);
}
