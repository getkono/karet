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
//! directories (`.git`, `target`, `node_modules`) are filtered out to keep the
//! stream (and the OS watch budget) focused on source files.

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
    /// # Errors
    /// Returns [`WatchError::Backend`] if the platform watcher cannot start or a
    /// root cannot be watched.
    pub fn spawn(roots: &[PathBuf]) -> Result<(Self, UnboundedReceiver<FsEvent>), WatchError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };
            for event in events {
                if let Some(fs) = convert(event.kind, &event.paths) {
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
fn convert(kind: EventKind, paths: &[PathBuf]) -> Option<FsEvent> {
    let kind = match kind {
        EventKind::Create(_) => FsEventKind::Created,
        EventKind::Remove(_) => FsEventKind::Removed,
        EventKind::Modify(ModifyKind::Name(_)) => FsEventKind::Renamed,
        EventKind::Modify(_) => FsEventKind::Modified,
        // Access events and catch-alls are not changes worth surfacing.
        _ => return None,
    };
    let paths: Vec<PathBuf> = paths.iter().filter(|p| !is_ignored(p)).cloned().collect();
    if paths.is_empty() {
        return None;
    }
    Some(FsEvent { kind, paths })
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
        let got = convert(EventKind::Create(CreateKind::File), &src);
        assert!(got.is_some());
        if let Some(got) = got {
            assert_eq!(got.kind, FsEventKind::Created);
            assert_eq!(got.paths, src);
        }

        // An event only touching an ignored path is dropped entirely.
        let ignored = vec![PathBuf::from("/r/target/a")];
        assert!(convert(EventKind::Create(CreateKind::File), &ignored).is_none());

        // Access events are not changes.
        assert!(convert(EventKind::Access(notify::event::AccessKind::Read), &src).is_none());
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            WatchError::Backend("nope".to_string()).to_string(),
            "filesystem watch error: nope"
        );
    }
}
