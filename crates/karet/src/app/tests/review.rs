//! Code-review marks: the per-workspace store and the flags stamped onto a
//! commit's file cards.

use super::support::*;
use crate::app::*;

/// An app rooted in a throwaway directory, so the review store never touches
/// a real workspace's cache entry.
fn review_app(name: &str) -> (App, PathBuf) {
    let root = test_dir(name);
    let mut app = responsive_commit_app();
    app.root = root.clone();
    (app, root)
}

/// The reviewed flags of the active commit tab's files.
fn flags(app: &App) -> Vec<bool> {
    match &app.tabs[app.active].kind {
        TabKind::Commit { files, .. } => files.files.iter().map(|f| f.reviewed).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn toggling_marks_the_current_file_and_reports_progress() {
    let (mut app, _root) = review_app("review-toggle");
    assert_eq!(flags(&app), vec![false, false, false]);

    app.commit_toggle_reviewed();

    assert_eq!(flags(&app), vec![true, false, false]);
    let status = app.status.clone().unwrap_or_default();
    assert!(status.contains("reviewed"), "{status}");
    assert!(status.contains("(1/3 reviewed)"), "{status}");

    // The mark is its own inverse.
    app.commit_toggle_reviewed();
    assert_eq!(flags(&app), vec![false, false, false]);
    let status = app.status.clone().unwrap_or_default();
    assert!(status.contains("unreviewed"), "{status}");
}

#[test]
fn a_mark_survives_being_reapplied_from_the_store() {
    let (mut app, _root) = review_app("review-restore");
    app.commit_toggle_reviewed();
    let hash = match &app.tabs[app.active].kind {
        TabKind::Commit { detail, .. } => detail.hash.clone(),
        _ => String::new(),
    };

    // Clear the in-memory flags as a fresh `CommitFiles::ready` would.
    let idx = app.active;
    if let TabKind::Commit { files, .. } = &mut app.tabs[idx].kind {
        for file in &mut files.files {
            file.reviewed = false;
        }
    }
    app.apply_review_flags(&hash);

    assert_eq!(
        flags(&app),
        vec![true, false, false],
        "the store restores what was marked"
    );
}

#[test]
fn a_commit_view_in_a_background_pane_gets_its_marks_too() {
    // The flags are stamped from a backend event, which can answer for a view
    // in any pane — not just the focused one.
    let (mut app, _root) = review_app("review-split");
    app.commit_toggle_reviewed();
    let hash = match &app.tabs[app.active].kind {
        TabKind::Commit { detail, .. } => detail.hash.clone(),
        _ => String::new(),
    };

    // Push the commit view into a background pane and clear its flags.
    app.split_focused(SplitDir::Right);
    for tab in app.all_tabs_mut() {
        if let TabKind::Commit { files, .. } = &mut tab.kind {
            for file in &mut files.files {
                file.reviewed = false;
            }
        }
    }
    app.apply_review_flags(&hash);

    let stored_marked = app
        .stored
        .values()
        .flat_map(|pane| pane.tabs.iter())
        .filter_map(|tab| match &tab.kind {
            TabKind::Commit { files, .. } => Some(files.files.iter().any(|f| f.reviewed)),
            _ => None,
        })
        .any(|marked| marked);
    assert!(
        stored_marked,
        "a commit view in a background pane must get its review marks"
    );
}
