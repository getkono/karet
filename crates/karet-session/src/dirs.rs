//! karet's per-user directories, all on one platform root.
//!
//! Every store karet keeps for a user — configuration, swaps, dictionaries, the
//! managed language servers, logs, recent workspaces, and the caches — resolves
//! through this module, so they share the one root [`ORGANIZATION`]/[`APPLICATION`]
//! names: `~/.config/karet` and `~/.cache/karet` on Linux,
//! `~/Library/…/getkono.karet` on macOS, `%APPDATA%\getkono\karet` on Windows.
//! Clearing or backing up that root therefore covers all of karet's data.
//!
//! Earlier releases kept four caches (the seam index, queued WakaTime heartbeats,
//! jdtls workspaces and review state) under an organisation-less root, which is a
//! different directory on macOS and Windows. [`migrate_legacy_cache`] moves them
//! across once; on Linux the two roots coincide and it does nothing.

use std::io;
use std::path::Path;
use std::path::PathBuf;

use directories::ProjectDirs;

/// The reverse-domain qualifier of karet's platform directories (none).
pub const QUALIFIER: &str = "";
/// The organisation karet's platform directories are filed under.
pub const ORGANIZATION: &str = "getkono";
/// The application name karet's platform directories are filed under.
pub const APPLICATION: &str = "karet";

fn project() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

/// The user configuration directory, where `setting.jsonc` lives.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    project().map(|dirs| dirs.config_dir().to_path_buf())
}

/// The cache directory: everything under it can be lost at the cost of a rebuild.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    project().map(|dirs| dirs.cache_dir().to_path_buf())
}

/// The machine-local data directory (swaps, dictionaries, managed language servers).
#[must_use]
pub fn data_local_dir() -> Option<PathBuf> {
    project().map(|dirs| dirs.data_local_dir().to_path_buf())
}

/// The state directory (logs, recent workspaces), or the machine-local data
/// directory on platforms that have no state directory.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    project().map(|dirs| {
        dirs.state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_path_buf()
    })
}

/// The cache directory earlier releases used for the seam index, the WakaTime
/// queue, jdtls workspaces and review state.
fn legacy_cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", APPLICATION).map(|dirs| dirs.cache_dir().to_path_buf())
}

/// Move the caches earlier releases kept under the organisation-less root into
/// [`cache_dir`], returning how many entries moved.
///
/// An entry already present in the canonical directory wins and its legacy copy is
/// left in place. Where the two roots coincide (Linux) this does nothing. Meant to
/// be called once by the application at startup, before any cache is opened.
///
/// # Errors
///
/// Returns the first I/O error from reading the legacy directory, creating the
/// canonical one, or moving an entry.
pub fn migrate_legacy_cache() -> io::Result<usize> {
    match (legacy_cache_dir(), cache_dir()) {
        (Some(from), Some(to)) => migrate_entries(&from, &to),
        _ => Ok(0),
    }
}

/// Move each entry of `from` into `to` unless `to` already has one by that name,
/// then remove `from` if that left it empty.
fn migrate_entries(from: &Path, to: &Path) -> io::Result<usize> {
    if from == to {
        return Ok(0);
    }
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    std::fs::create_dir_all(to)?;
    let mut moved = 0;
    for entry in entries {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if target.symlink_metadata().is_ok() {
            continue;
        }
        std::fs::rename(entry.path(), target)?;
        moved += 1;
    }
    // Only succeeds when empty; a legacy entry the canonical root shadowed stays.
    let _ = std::fs::remove_dir(from);
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_store_shares_one_root() {
        let (Some(config), Some(cache), Some(data), Some(state)) =
            (config_dir(), cache_dir(), data_local_dir(), state_dir())
        else {
            // No home directory in this environment: nothing to compare.
            return;
        };
        let expected = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION);
        assert_eq!(
            Some(config.as_path()),
            expected.as_ref().map(ProjectDirs::config_dir)
        );
        assert_eq!(
            Some(cache.as_path()),
            expected.as_ref().map(ProjectDirs::cache_dir)
        );
        assert_eq!(
            Some(data.as_path()),
            expected.as_ref().map(ProjectDirs::data_local_dir)
        );
        assert_eq!(
            Some(state.as_path()),
            expected
                .as_ref()
                .map(|dirs| dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir()))
        );
    }

    #[test]
    fn migration_moves_entries_and_removes_the_emptied_legacy_dir() -> io::Result<()> {
        let scratch = tempfile::tempdir()?;
        let from = scratch.path().join("legacy");
        let to = scratch.path().join("canonical");
        std::fs::create_dir_all(from.join("seam"))?;
        std::fs::write(from.join("seam/0.cbor"), b"index")?;
        std::fs::write(from.join("wakatime-queue.jsonl"), b"beat\n")?;

        assert_eq!(migrate_entries(&from, &to)?, 2);
        assert_eq!(std::fs::read(to.join("seam/0.cbor"))?, b"index");
        assert_eq!(std::fs::read(to.join("wakatime-queue.jsonl"))?, b"beat\n");
        assert!(!from.exists());
        Ok(())
    }

    #[test]
    fn migration_never_overwrites_a_canonical_entry() -> io::Result<()> {
        let scratch = tempfile::tempdir()?;
        let from = scratch.path().join("legacy");
        let to = scratch.path().join("canonical");
        std::fs::create_dir_all(&from)?;
        std::fs::create_dir_all(&to)?;
        std::fs::write(from.join("review-1.json"), b"old")?;
        std::fs::write(to.join("review-1.json"), b"new")?;

        assert_eq!(migrate_entries(&from, &to)?, 0);
        assert_eq!(std::fs::read(to.join("review-1.json"))?, b"new");
        // The shadowed legacy copy is left, so the legacy dir stays too.
        assert_eq!(std::fs::read(from.join("review-1.json"))?, b"old");
        Ok(())
    }

    #[test]
    fn migration_is_a_no_op_without_a_legacy_dir_or_when_roots_coincide() -> io::Result<()> {
        let scratch = tempfile::tempdir()?;
        let to = scratch.path().join("canonical");
        assert_eq!(migrate_entries(&scratch.path().join("absent"), &to)?, 0);
        assert!(!to.exists());

        std::fs::create_dir_all(&to)?;
        std::fs::write(to.join("keep"), b"x")?;
        assert_eq!(migrate_entries(&to, &to)?, 0);
        assert_eq!(std::fs::read(to.join("keep"))?, b"x");
        Ok(())
    }
}
