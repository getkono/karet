//! Explorer filesystem mutations.
//!
//! Every one of these used to run in the presentation layer. They are here now
//! because a client need not share a machine with the workspace — and because
//! having one implementation means the create-then-refresh dance is defined once.

use std::path::Path;

use crate::api::PathMutation;

/// Perform `mutation`, mapping any failure to a displayable message.
pub(super) fn run(mutation: &PathMutation) -> Result<(), String> {
    match mutation {
        PathMutation::CreateFile { path } => create_file(path),
        PathMutation::CreateDirectory { path } => {
            std::fs::create_dir_all(path).map_err(|error| error.to_string())
        },
        PathMutation::Rename { from, to } => rename(from, to),
        PathMutation::Copy { from, to } => copy(from, to),
        PathMutation::Delete { path } => delete(path),
    }
}

/// Create an empty file, failing if something is already there.
///
/// `create_new` rather than `create`: the explorer's "new file" must never
/// silently truncate a file the user forgot about.
fn create_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Rename `from` to `to`, refusing to clobber an existing destination.
///
/// `fs::rename` overwrites on Unix. The explorer's rename and drag-move must not,
/// so the destination is checked first. The check races, but it turns the common
/// mistake into an error instead of silent data loss.
fn rename(from: &Path, to: &Path) -> Result<(), String> {
    if to.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", to.display()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::rename(from, to).map_err(|error| error.to_string())
}

/// Copy a file, or a directory and everything under it.
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if to.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", to.display()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    copy_recursive(from, to)
}

/// Copy `from` to `to`, recursing through directories.
///
/// Symlinks are copied by content, not recreated as links: an explorer copy that
/// produced a link pointing outside the destination would surprise, and a link
/// into the copied tree would still point at the original.
fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Delete a path, recursively for a directory.
///
/// A symlink is removed as a link, never followed — deleting a link to a
/// directory must not delete the directory.
fn delete(path: &Path) -> Result<(), String> {
    let metadata = path.symlink_metadata().map_err(|error| error.to_string())?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path).map_err(|error| error.to_string())
    } else {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    }
}
