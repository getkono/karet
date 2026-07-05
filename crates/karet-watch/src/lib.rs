//! `karet-watch` — debounced filesystem watching for karet editors.
//!
//! Wraps the cross-platform [`notify`] backend (inotify / FSEvents /
//! ReadDirectoryChangesW) behind a neutral [`FsEvent`] stream, so the rest of the
//! toolkit never sees a platform watcher. This is the single, centralized watcher
//! the session subscribes to and fans out to the UI — there is no per-feature
//! watching.
//!
//! Events are debounced (coalescing the storms inotify/FSEvents produce and pairing
//! renames) and delivered on a Tokio channel. Directories are watched
//! **non-recursively**, one at a time, skipping hidden dotdirs, gitignored paths, and
//! common build/VCS directories (`target`, `node_modules`, and most of `.git`) — so
//! opening a huge tree (a user's home directory, say) never has to walk it eagerly. A
//! curated allowlist of git-metadata files (`index`, `HEAD`, `refs/**`, …) under the
//! caller-supplied git directories *is* surfaced, so source-control status can stay
//! fresh without drowning in `.git/objects` churn. Enumeration and watch registration
//! happen on a dedicated background thread, so [`Watcher::spawn`] returns immediately
//! and never blocks the caller on the size of the tree; directories created later are
//! picked up as they appear.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ignore::DirEntry;
use ignore::WalkBuilder;
use notify::EventKind;
use notify::RecursiveMode;
use notify::event::ModifyKind;
use notify_debouncer_full::DebounceEventResult;
use notify_debouncer_full::Debouncer;
use notify_debouncer_full::RecommendedCache;
use notify_debouncer_full::new_debouncer;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{self};

/// The debounce window: filesystem event bursts within this interval coalesce. Also
/// used as the background worker's poll interval for noticing shutdown.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Directory names whose contents are never surfaced or watched (build output / VCS
/// metadata). Git dirs are watched separately and narrowly — see [`watch_git_dir`].
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
    /// The platform watch backend failed to start.
    #[error("filesystem watch error: {0}")]
    Backend(String),
}

/// A live filesystem watcher. Keep it alive for as long as events are wanted;
/// dropping it stops the background worker and releases all watches.
pub struct Watcher {
    // Signals the background worker to wind down.
    stop: Arc<AtomicBool>,
    // Joined on drop so watches are released deterministically.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Watcher {
    /// Start watching `roots`, returning the watcher and the receiving half of its
    /// [`FsEvent`] stream.
    ///
    /// Enumeration of the directories under `roots` (skipping hidden, gitignored, and
    /// [`IGNORED_DIRS`] entries) and registration of their non-recursive watches
    /// happens on a background thread, so this call returns as soon as the platform
    /// watch backend itself starts — it does not block on the size of `roots`.
    /// Directories created later are picked up as their creation events arrive.
    ///
    /// `git_dirs` are git-metadata directories (a repository's git dir and, for a
    /// linked worktree, its common dir) whose allowlisted files (`index`, `HEAD`,
    /// `refs/**`, …) should be surfaced even though `.git` is otherwise never watched.
    ///
    /// # Errors
    /// Returns [`WatchError::Backend`] if the platform watch backend cannot start.
    pub fn spawn(
        roots: &[PathBuf],
        git_dirs: &[PathBuf],
    ) -> Result<(Self, UnboundedReceiver<FsEvent>), WatchError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<DebounceEventResult>();

        // Cheap: starts the debouncer's own threads but registers no watches yet, so
        // a backend-start failure is still reported synchronously to the caller.
        let debouncer = new_debouncer(DEBOUNCE, None, raw_tx)
            .map_err(|e| WatchError::Backend(e.to_string()))?;

        // Longest path first, so a nested per-worktree git dir is matched before the
        // common dir it lives under.
        let mut meta_dirs: Vec<PathBuf> = git_dirs.to_vec();
        meta_dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));

        let roots = roots.to_vec();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("karet-watch".to_string())
                .spawn(move || worker_main(debouncer, &raw_rx, &roots, &meta_dirs, &tx, &stop))
                .map_err(|e| WatchError::Backend(e.to_string()))?
        };

        Ok((
            Self {
                stop,
                worker: Some(worker),
            },
            rx,
        ))
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            // The worker notices `stop` within one `DEBOUNCE` tick and joins the
            // debouncer's own thread before returning, releasing all watches.
            let _ = handle.join();
        }
    }
}

/// The background worker: registers watches (off the caller's critical path), then
/// loops converting and forwarding events until told to [`Watcher::drop`]. Owns the
/// debouncer so it can register new watches (e.g. for a freshly created directory)
/// from the same thread that reads its event stream.
fn worker_main(
    mut debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    raw_rx: &std::sync::mpsc::Receiver<DebounceEventResult>,
    roots: &[PathBuf],
    meta_dirs: &[PathBuf],
    tx: &mpsc::UnboundedSender<FsEvent>,
    stop: &AtomicBool,
) {
    for root in roots {
        for dir in enumerate_dirs(root) {
            if let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
                tracing::warn!(path = %dir.display(), error = %e, "watch failed");
            }
        }
    }
    for git_dir in meta_dirs {
        watch_git_dir(&mut debouncer, git_dir);
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match raw_rx.recv_timeout(DEBOUNCE) {
            Ok(Ok(events)) => {
                for event in events {
                    // A newly created directory needs its own watch (the parent's
                    // watch is non-recursive); re-enumerating the new subtree also
                    // covers a `mkdir -p` that surfaced only the outermost directory.
                    if matches!(event.kind, EventKind::Create(_)) {
                        for path in &event.paths {
                            if path.is_dir() && !is_ignored(path) {
                                for dir in enumerate_dirs(path) {
                                    if let Err(e) =
                                        debouncer.watch(&dir, RecursiveMode::NonRecursive)
                                    {
                                        tracing::warn!(
                                            path = %dir.display(),
                                            error = %e,
                                            "watch failed",
                                        );
                                    }
                                }
                            }
                        }
                    }
                    if let Some(fs) = convert(event.kind, &event.paths, meta_dirs) {
                        // The receiver going away just means the session shut down.
                        let _ = tx.send(fs);
                    }
                }
            },
            // A batch of backend errors: best-effort, matches the prior behavior of
            // ignoring individual watch/backend hiccups.
            Ok(Err(_)) => {},
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
            // The debouncer's sender side is gone — nothing more will ever arrive.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    debouncer.stop();
}

/// Enumerate the directories under `root` worth watching: hidden dotdirs, gitignored
/// paths (via `.gitignore`/`.ignore`, even outside a repository), and [`IGNORED_DIRS`]
/// are pruned — including from descent, so their subtrees are never walked. Symlinks
/// are not followed, so a symlink cycle cannot stall the walk. `root` itself is
/// included when it is a directory.
fn enumerate_dirs(root: &Path) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .require_git(false)
        .follow_links(false);
    builder.filter_entry(|entry: &DirEntry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !IGNORED_DIRS.contains(&name))
    });
    builder
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_dir()))
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

/// Register narrow watches for a git-metadata directory: the top level (covers
/// `index`, `HEAD`, `packed-refs`, …) non-recursively, and `refs/` recursively (small,
/// and where branch/tag updates land). The high-churn `objects`/`logs` trees are never
/// watched. Best-effort: a failure is logged, not propagated.
fn watch_git_dir(
    debouncer: &mut Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    git_dir: &Path,
) {
    if let Err(e) = debouncer.watch(git_dir, RecursiveMode::NonRecursive) {
        tracing::warn!(path = %git_dir.display(), error = %e, "watch failed");
    }
    let refs = git_dir.join("refs");
    if refs.is_dir()
        && let Err(e) = debouncer.watch(&refs, RecursiveMode::Recursive)
    {
        tracing::warn!(path = %refs.display(), error = %e, "watch failed");
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
    use std::fs;
    use std::time::Instant;

    use tempfile::TempDir;

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

    #[test]
    fn enumerate_dirs_skips_ignored_hidden_and_gitignored() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = TempDir::new()?;
        let root = root.path();
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(root.join("node_modules/x"))?;
        fs::create_dir_all(root.join("target/debug"))?;
        fs::create_dir_all(root.join(".hidden"))?;
        fs::create_dir_all(root.join("build"))?;
        fs::write(root.join(".gitignore"), "build/\n")?;

        let dirs = enumerate_dirs(root);
        assert!(dirs.contains(&root.to_path_buf()));
        assert!(dirs.contains(&root.join("src")));
        assert!(
            !dirs
                .iter()
                .any(|d| d.starts_with(root.join("node_modules")))
        );
        assert!(!dirs.iter().any(|d| d.starts_with(root.join("target"))));
        assert!(!dirs.contains(&root.join(".hidden")));
        assert!(!dirs.contains(&root.join("build")));
        Ok(())
    }

    /// Poll `rx` until an event matching `pred` arrives or `deadline` elapses.
    fn wait_for_event(
        rx: &mut UnboundedReceiver<FsEvent>,
        deadline: Duration,
        pred: impl Fn(&FsEvent) -> bool,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            match rx.try_recv() {
                Ok(event) if pred(&event) => return true,
                Ok(_) => {},
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        false
    }

    #[test]
    fn watcher_surfaces_nested_source_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let deep = root.path().join("src/deep");
        fs::create_dir_all(&deep)?;

        let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
        // Let the background worker finish registering watches.
        std::thread::sleep(Duration::from_millis(300));

        let file = deep.join("new.rs");
        fs::write(&file, "fn main() {}")?;

        assert!(wait_for_event(&mut rx, Duration::from_secs(5), |e| {
            e.kind == FsEventKind::Created && e.paths.contains(&file)
        }));
        Ok(())
    }

    #[test]
    fn watcher_covers_dynamically_created_dir() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        fs::create_dir_all(root.path().join("src"))?;

        let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
        std::thread::sleep(Duration::from_millis(300));

        let sub = root.path().join("src/added/sub");
        fs::create_dir_all(&sub)?;
        // Give the worker a moment to notice the new directory and watch it before a
        // file lands inside it.
        std::thread::sleep(Duration::from_millis(300));
        let file = sub.join("f.rs");
        fs::write(&file, "fn main() {}")?;

        assert!(wait_for_event(&mut rx, Duration::from_secs(5), |e| {
            e.paths.contains(&file)
        }));
        Ok(())
    }

    #[test]
    fn watcher_filters_build_dirs() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        fs::create_dir_all(root.path().join("target"))?;
        fs::create_dir_all(root.path().join("node_modules"))?;

        let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
        std::thread::sleep(Duration::from_millis(300));

        fs::write(root.path().join("target/x"), "x")?;
        fs::write(root.path().join("node_modules/y"), "y")?;

        assert!(!wait_for_event(&mut rx, Duration::from_millis(800), |_| {
            true
        }));
        Ok(())
    }
}
