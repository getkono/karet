//! The tree's listing bookkeeping: what it asks for, what it holds, and what it
//! forgets. Driven by `supply` alone, the way an embedder feeds it, with no disk.

use std::path::Path;
use std::path::PathBuf;

use karet_core::DirEntry;

use super::FileTreeState;

fn file(path: &str) -> DirEntry {
    DirEntry {
        path: PathBuf::from(path),
        is_dir: false,
        is_symlink: false,
        ignored: false,
        is_repository: false,
    }
}

fn dir(path: &str) -> DirEntry {
    DirEntry {
        is_dir: true,
        ..file(path)
    }
}

fn row_paths(state: &FileTreeState) -> Vec<PathBuf> {
    state.rows().iter().map(|row| row.path.clone()).collect()
}

/// A tree built over a root nobody has listed asks for it, and waits.
#[test]
fn an_unlisted_root_is_reported_missing_and_waited_on() {
    let mut state = FileTreeState::new();

    state.rebuild(Path::new("/w"));

    assert!(state.is_waiting());
    assert!(!state.has_listing(Path::new("/w")));
    assert_eq!(state.take_missing(), vec![PathBuf::from("/w")]);
    assert!(!state.is_waiting(), "taking the misses drains them");
}

/// A miss the embedder could not act on goes back, so the tree keeps saying it
/// is incomplete.
#[test]
fn a_miss_marked_again_is_still_waited_on() {
    let mut state = FileTreeState::new();
    state.rebuild(Path::new("/w"));
    let _ = state.take_missing();

    state.mark_missing(PathBuf::from("/w"));

    assert!(state.is_waiting());
    assert_eq!(state.take_missing(), vec![PathBuf::from("/w")]);
}

/// Supplying a listing settles the miss and its rows appear on the next build.
#[test]
fn a_supplied_listing_is_held_and_rendered() {
    let mut state = FileTreeState::new();
    state.rebuild(Path::new("/w"));
    let _ = state.take_missing();

    state.supply(PathBuf::from("/w"), vec![file("/w/b.rs"), file("/w/a.rs")]);
    state.ensure_built(Path::new("/w"));

    assert!(state.has_listing(Path::new("/w")));
    assert!(!state.is_waiting());
    assert_eq!(
        row_paths(&state),
        vec![PathBuf::from("/w/a.rs"), PathBuf::from("/w/b.rs")]
    );
}

/// Invalidating one directory forgets only it, and the next build asks again.
#[test]
fn invalidating_a_directory_forgets_it_and_asks_again() {
    let mut state = FileTreeState::new();
    state.supply(PathBuf::from("/w"), vec![dir("/w/src"), file("/w/a.rs")]);
    state.supply(PathBuf::from("/w/src"), vec![file("/w/src/lib.rs")]);

    state.invalidate(Path::new("/w/src"));
    state.ensure_built(Path::new("/w"));
    state.toggle(Path::new("/w/src"));

    assert!(state.has_listing(Path::new("/w")));
    assert!(!state.has_listing(Path::new("/w/src")));
    assert_eq!(state.take_missing(), vec![PathBuf::from("/w/src")]);
}

/// Invalidating everything (a new root) forgets every listing.
#[test]
fn invalidating_everything_forgets_every_listing() {
    let mut state = FileTreeState::new();
    state.supply(PathBuf::from("/w"), vec![dir("/w/src")]);
    state.supply(PathBuf::from("/w/src"), Vec::new());

    state.invalidate_all();

    assert!(state.listed_dirs().is_empty());
    state.ensure_built(Path::new("/w"));
    assert_eq!(state.take_missing(), vec![PathBuf::from("/w")]);
}

/// A refresh re-reads every held directory without dropping what is shown.
#[test]
fn listed_dirs_names_every_held_listing_in_path_order() {
    let mut state = FileTreeState::new();
    state.supply(PathBuf::from("/w/src"), Vec::new());
    state.supply(PathBuf::from("/w"), vec![dir("/w/src")]);

    assert_eq!(
        state.listed_dirs(),
        vec![PathBuf::from("/w"), PathBuf::from("/w/src")]
    );
}

/// A new tree lists hidden entries and flags gitignored ones; the embedder
/// reads these to ask the lister for the same thing.
#[test]
fn a_new_tree_shows_hidden_entries_and_respects_gitignore() {
    let state = FileTreeState::new();

    assert!(state.show_hidden());
    assert!(state.respect_gitignore());
}
