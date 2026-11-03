//! Playing the backend's part for the workspace filesystem.
//!
//! The shell asks its backend to list directories, mutate paths and read media
//! bytes; a test whose backend records rather than answers has to supply those
//! answers itself. Each helper here performs the work against the local disk
//! the way the session's filesystem worker does and hands the answer back
//! through the same entry point a real session uses, so a test exercises the
//! whole round trip rather than a stub.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use karet_session::Event as SessionEvent;
use karet_session::RequestId;
use karet_session::api::PathMutation;

use super::RecordingBackend;
use crate::app::App;
use crate::tab::TabKind;

/// An app rooted at `root` with a recording backend attached.
///
/// A shell with no backend cannot ask for anything — every request is dropped
/// before it is made — so any test that exercises the explorer's filesystem
/// actions needs one.
pub(crate) fn app_at(root: PathBuf) -> App {
    let mut app = App::new(root, Vec::new(), Vec::new(), false);
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app
}

/// Run every filesystem mutation `app` has in flight, the way the backend does.
pub(crate) fn settle_mutations(app: &mut App) {
    for _ in 0..64 {
        let pending: Vec<(RequestId, PathMutation)> = app
            .pending_mutations
            .iter()
            .map(|(id, pending)| (*id, pending.mutation.clone()))
            .collect();
        if pending.is_empty() {
            break;
        }
        for (id, mutation) in pending {
            let result = apply_mutation(&mutation);
            app.on_backend_event(Some(id), SessionEvent::PathMutated { mutation, result });
        }
    }
}

/// Perform `mutation`, mirroring what the session's filesystem worker does.
fn apply_mutation(mutation: &PathMutation) -> Result<(), String> {
    let created = |path: &Path| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    let copy = |from: &Path, to: &Path| -> Result<(), String> {
        fn walk(from: &Path, to: &Path) -> Result<(), String> {
            if from.is_dir() {
                std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
                for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    walk(&entry.path(), &to.join(entry.file_name()))?;
                }
                return Ok(());
            }
            std::fs::copy(from, to)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        if to.symlink_metadata().is_ok() {
            return Err(format!("{} already exists", to.display()));
        }
        created(to)?;
        walk(from, to)
    };
    match mutation {
        PathMutation::CreateFile { path } => {
            created(path)?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        PathMutation::CreateDirectory { path } => {
            std::fs::create_dir_all(path).map_err(|error| error.to_string())
        },
        PathMutation::Rename { from, to } => {
            if to.symlink_metadata().is_ok() {
                return Err(format!("{} already exists", to.display()));
            }
            created(to)?;
            std::fs::rename(from, to).map_err(|error| error.to_string())
        },
        PathMutation::Copy { from, to } => copy(from, to),
        PathMutation::Delete { path } => {
            let metadata = path.symlink_metadata().map_err(|error| error.to_string())?;
            if metadata.is_dir() {
                std::fs::remove_dir_all(path).map_err(|error| error.to_string())
            } else {
                std::fs::remove_file(path).map_err(|error| error.to_string())
            }
        },
        // The enum is non-exhaustive; a mutation this helper has not learned yet
        // must fail loudly rather than quietly report success.
        other => Err(format!("unhandled mutation: {other:?}")),
    }
}

/// Build `app`'s explorer, answering every listing it asks for from the local
/// disk, including a reveal completing as levels arrive.
pub(crate) fn build_explorer_from_disk(app: &mut App) {
    app.build_explorer();
    // Bounded so a bug cannot hang the suite; one round resolves a whole level.
    for _ in 0..64 {
        let asked: Vec<(RequestId, PathBuf)> = app
            .pending_listings
            .iter()
            .map(|(id, path)| (*id, path.clone()))
            .collect();
        // A shell with no backend keeps its misses rather than sending them.
        let unsent = app.explorer.take_missing();
        if asked.is_empty() && unsent.is_empty() {
            break;
        }
        for dir in unsent {
            let children = list_dir(
                &dir,
                app.explorer.show_hidden(),
                app.explorer.respect_gitignore(),
            );
            app.explorer.supply(dir, children);
        }
        for (id, dir) in asked {
            let children = list_dir(
                &dir,
                app.explorer.show_hidden(),
                app.explorer.respect_gitignore(),
            );
            app.on_backend_event(
                Some(id),
                SessionEvent::DirectoryListed {
                    path: dir,
                    result: Ok(children),
                },
            );
        }
        app.build_explorer();
        app.finish_reveal();
    }
}

/// List `dir` the way the session's filesystem worker does.
fn list_dir(dir: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<karet_core::DirEntry> {
    let walk = |git_ignore: bool| -> Vec<(PathBuf, bool, bool)> {
        ignore::WalkBuilder::new(dir)
            .max_depth(Some(1))
            .standard_filters(git_ignore)
            .hidden(!show_hidden)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build()
            .flatten()
            .filter(|entry| entry.path() != dir)
            .map(|entry| {
                let path = entry.path().to_path_buf();
                let is_symlink = entry.path_is_symlink();
                let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
                (path, is_dir, is_symlink)
            })
            .collect()
    };
    let visible: BTreeSet<PathBuf> = if respect_gitignore {
        walk(true).into_iter().map(|(path, _, _)| path).collect()
    } else {
        BTreeSet::new()
    };
    let mut entries: Vec<karet_core::DirEntry> = walk(false)
        .into_iter()
        .map(|(path, is_dir, is_symlink)| karet_core::DirEntry {
            ignored: respect_gitignore && !visible.contains(&path),
            is_repository: is_dir && path.join(".git").exists(),
            path,
            is_dir,
            is_symlink,
        })
        .collect();
    karet_core::sort_entries(&mut entries);
    entries
}

/// Stand in for the backend's classification and byte reads on every tab
/// reserved for content the app renders from raw bytes (an image, a PDF, a hex
/// dump): read each file from disk and build the tab its bytes warrant, as the
/// answering events do.
pub(crate) fn fill_media_tabs(app: &mut App) {
    for tab in app.all_tabs_mut() {
        let path = match &tab.kind {
            TabKind::Placeholder { path, .. } => path.clone(),
            TabKind::Hex { path, bytes, .. } if bytes.is_empty() => path.clone(),
            _ => continue,
        };
        let bytes = std::fs::read(&path).unwrap_or_default();
        let len = bytes.len() as u64;
        let kind = karet_filetype::classify(&path, &bytes, len);
        let mut filled = crate::tab::open::realize(&path, kind, bytes, len);
        filled.view = tab.view;
        filled.is_preview = tab.is_preview;
        *tab = filled;
    }
}
