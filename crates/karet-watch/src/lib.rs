//! `karet-watch` — debounced filesystem watching for karet editors.
//!
//! Wraps the cross-platform [`notify`] backend (inotify / FSEvents /
//! ReadDirectoryChangesW) behind a neutral [`FsEvent`] stream, so the rest of the
//! toolkit never sees a platform watcher. This is the single, centralized watcher
//! the session subscribes to and fans out to the UI — there is no per-feature
//! watching.
//!
//! Events are debounced (coalescing the storms inotify/FSEvents produce and pairing
//! renames) and delivered on a Tokio channel. Paths under common build/VCS
//! directories (`target`, `node_modules`, and most of `.git`) are filtered out to
//! keep the stream (and the OS watch budget) focused on source files. A curated
//! allowlist of git-metadata files (`index`, `HEAD`, `refs/**`, …) under the
//! caller-supplied git directories *is* surfaced, so source-control status can stay
//! fresh without drowning in `.git/objects` churn.

use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// The debounce window: filesystem event bursts within this interval coalesce.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Directory names whose contents are never surfaced (build output / VCS metadata).
const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// What kind of change a [`FsEvent`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FsEventKind {
    /// A path was created.
    Created,
    /// A path's contents changed.
    Modified,
    /// A path was removed.
    Removed,
    /// A path was renamed (the paths carry the affected locations).
    Renamed,
}

/// A debounced, neutral filesystem change.
#[derive(Clone, Debug)]
pub struct FsEvent {
    /// The kind of change.
    pub kind: FsEventKind,
    /// The affected paths (a rename carries both the old and new locations).
    pub paths: Vec<PathBuf>,
}

/// Errors produced when starting a [`Watcher`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchError {
    /// The platform watch backend failed to start or register a path.
    #[error("filesystem watch error: {0}")]
    Backend(String),
}

/// A live filesystem watcher. Keep it alive for as long as events are wanted;
/// dropping it stops watching.
pub struct Watcher {
    // Held only to keep the debouncer thread (and its watches) alive.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

impl Watcher {
    /// Start watching `roots` recursively, returning the watcher and the receiving
    /// half of its [`FsEvent`] stream.
    ///
    /// `git_dirs` are git-metadata directories (a repository's git dir and, for a
    /// linked worktree, its common dir) whose allowlisted files (`index`, `HEAD`,
    /// `refs/**`, …) should be surfaced even though they sit under `.git`. A git dir
    /// already inside one of `roots` is observed through that root's recursive watch;
    /// one outside (e.g. a linked worktree's git dir) gets its own watch.
    ///
    /// # Errors
    /// Returns [`WatchError::Backend`] if the platform watcher cannot start or a
    /// path cannot be watched.
    pub fn spawn(
        roots: &[PathBuf],
        git_dirs: &[PathBuf],
    ) -> Result<(Self, UnboundedReceiver<FsEvent>), WatchError> {
        let (tx, rx) = mpsc::unbounded_channel();
        // Longest path first, so a nested per-worktree git dir is matched before the
        // common dir it lives under.
        let mut meta_dirs: Vec<PathBuf> = git_dirs.to_vec();
        meta_dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
        let classifier_dirs = meta_dirs.clone();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };
            for event in events {
                if let Some(fs) = convert(event.kind, &event.paths, &classifier_dirs) {
                    // The receiver going away just means the session shut down.
                    let _ = tx.send(fs);
                }
            }
        })
        .map_err(|e| WatchError::Backend(e.to_string()))?;

        for root in roots {
            debouncer
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| WatchError::Backend(e.to_string()))?;
        }
        // Watch any git dir not already covered by a root (a normal `<root>/.git`
        // is; a linked worktree's git dir, living outside the worktree, is not).
        for git_dir in &meta_dirs {
            if !roots.iter().any(|root| git_dir.starts_with(root)) {
                debouncer
                    .watch(git_dir, RecursiveMode::Recursive)
                    .map_err(|e| WatchError::Backend(e.to_string()))?;
            }
        }
        Ok((
            Self {
                _debouncer: debouncer,
            },
            rx,
        ))
    }
}

/// Convert a `notify` event into a neutral [`FsEvent`], dropping ignored paths and
/// uninteresting (access-only) events.
fn convert(kind: EventKind, paths: &[PathBuf], git_dirs: &[PathBuf]) -> Option<FsEvent> {
    let kind = match kind {
        EventKind::Create(_) => FsEventKind::Created,
        EventKind::Remove(_) => FsEventKind::Removed,
        EventKind::Modify(ModifyKind::Name(_)) => FsEventKind::Renamed,
        EventKind::Modify(_) => FsEventKind::Modified,
        // Access events and catch-alls are not changes worth surfacing.
        _ => return None,
    };
    let paths: Vec<PathBuf> = paths
        .iter()
        .filter(|p| keep_path(p, git_dirs))
        .cloned()
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some(FsEvent { kind, paths })
}

/// Whether `path` should be surfaced. A path inside one of `git_dirs` is kept only
/// when it names allowlisted git metadata; otherwise the usual ignore rules apply.
fn keep_path(path: &Path, git_dirs: &[PathBuf]) -> bool {
    for git_dir in git_dirs {
        if let Ok(rel) = path.strip_prefix(git_dir) {
            return is_interesting_git_meta(rel);
        }
    }
    !is_ignored(path)
}

/// Whether `rel` (a path relative to a git dir) is a git-metadata file worth
/// surfacing: the index, `HEAD` family, and refs — but never the high-churn
/// `objects`/`logs` trees, lock files, or fetch/commit-message scratch files.
fn is_interesting_git_meta(rel: &Path) -> bool {
    let first = rel.components().next().and_then(|c| c.as_os_str().to_str());
    if matches!(first, Some("objects" | "logs")) {
        return false;
    }
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".lock") || matches!(name, "FETCH_HEAD" | "COMMIT_EDITMSG") {
        return false;
    }
    if first == Some("refs") {
        return true;
    }
    matches!(
        name,
        "index"
            | "HEAD"
            | "packed-refs"
            | "MERGE_HEAD"
            | "ORIG_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "MERGE_MSG"
    )
}

/// Whether `path` lies under a build-output or VCS-metadata directory.
fn is_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|name| IGNORED_DIRS.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_build_and_vcs_dirs() {
        assert!(is_ignored(Path::new("/repo/target/debug/x")));
        assert!(is_ignored(Path::new("/repo/.git/HEAD")));
        assert!(is_ignored(Path::new("/repo/node_modules/p/i.js")));
        assert!(!is_ignored(Path::new("/repo/src/main.rs")));
    }

    #[test]
    fn convert_maps_kinds_and_filters_ignored() {
        use notify::event::CreateKind;
        let src = vec![PathBuf::from("/r/src/a.rs")];
        let got = convert(EventKind::Create(CreateKind::File), &src, &[]);
        assert!(got.is_some());
        if let Some(got) = got {
            assert_eq!(got.kind, FsEventKind::Created);
            assert_eq!(got.paths, src);
        }

        // An event only touching an ignored path is dropped entirely.
        let ignored = vec![PathBuf::from("/r/target/a")];
        assert!(convert(EventKind::Create(CreateKind::File), &ignored, &[]).is_none());

        // Access events are not changes.
        assert!(
            convert(
                EventKind::Access(notify::event::AccessKind::Read),
                &src,
                &[]
            )
            .is_none()
        );
    }

    #[test]
    fn git_meta_allowlist() {
        // Kept: the index, HEAD family, and refs.
        assert!(is_interesting_git_meta(Path::new("index")));
        assert!(is_interesting_git_meta(Path::new("HEAD")));
        assert!(is_interesting_git_meta(Path::new("packed-refs")));
        assert!(is_interesting_git_meta(Path::new("MERGE_HEAD")));
        assert!(is_interesting_git_meta(Path::new("refs/heads/main")));
        // Dropped: lock files, the object/log trees, and scratch files.
        assert!(!is_interesting_git_meta(Path::new("index.lock")));
        assert!(!is_interesting_git_meta(Path::new("objects/ab/cdef")));
        assert!(!is_interesting_git_meta(Path::new("logs/HEAD")));
        assert!(!is_interesting_git_meta(Path::new("FETCH_HEAD")));
        assert!(!is_interesting_git_meta(Path::new("COMMIT_EDITMSG")));
    }

    #[test]
    fn keep_path_applies_git_allowlist_under_git_dir() {
        let git_dirs = vec![PathBuf::from("/repo/.git")];
        // Inside the git dir: only allowlisted metadata survives.
        assert!(keep_path(Path::new("/repo/.git/index"), &git_dirs));
        assert!(keep_path(
            Path::new("/repo/.git/refs/heads/main"),
            &git_dirs
        ));
        assert!(!keep_path(Path::new("/repo/.git/objects/ab/cd"), &git_dirs));
        assert!(!keep_path(Path::new("/repo/.git/index.lock"), &git_dirs));
        // Outside the git dir: the usual ignore rules apply.
        assert!(keep_path(Path::new("/repo/src/main.rs"), &git_dirs));
        assert!(!keep_path(Path::new("/repo/target/x"), &git_dirs));
        // With no git dirs tracked, `.git` is dropped wholesale as before.
        assert!(!keep_path(Path::new("/repo/.git/index"), &[]));
    }

    #[test]
    fn keep_path_handles_worktree_git_dirs() {
        // The per-worktree git dir (longest) is listed before the common dir.
        let git_dirs = vec![
            PathBuf::from("/main/.git/worktrees/wt"),
            PathBuf::from("/main/.git"),
        ];
        assert!(keep_path(
            Path::new("/main/.git/worktrees/wt/index"),
            &git_dirs
        ));
        assert!(keep_path(
            Path::new("/main/.git/worktrees/wt/HEAD"),
            &git_dirs
        ));
        assert!(keep_path(
            Path::new("/main/.git/refs/heads/main"),
            &git_dirs
        ));
        assert!(!keep_path(
            Path::new("/main/.git/worktrees/wt/ORIG_HEAD.lock"),
            &git_dirs
        ));
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            WatchError::Backend("nope".to_string()).to_string(),
            "filesystem watch error: nope"
        );
    }
}
