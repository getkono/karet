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

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ignore::DirEntry;
use ignore::WalkBuilder;
use notify::Config;
use notify::ErrorKind;
use notify::EventKind;
use notify::PollWatcher;
use notify::RecursiveMode;
use notify::event::ModifyKind;
use notify_debouncer_full::DebounceEventResult;
use notify_debouncer_full::Debouncer;
use notify_debouncer_full::RecommendedCache;
use notify_debouncer_full::new_debouncer;
use notify_debouncer_full::new_debouncer_opt;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{self};

/// The debounce window: filesystem event bursts within this interval coalesce. Also
/// used as the background worker's poll interval for noticing shutdown.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Polling fallback interval used only after the native backend reports watch
/// exhaustion. Kept slower than the main debounce so degraded trees remain cheap.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

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
    /// Native watch coverage was exhausted and some paths are now polled.
    WatchDegraded,
}

/// A debounced, neutral filesystem change.
#[derive(Clone, Debug)]
pub struct FsEvent {
    /// The kind of change.
    pub kind: FsEventKind,
    /// The affected paths (a rename carries both the old and new locations).
    ///
    /// For [`FsEventKind::WatchDegraded`], these are the roots that fell back to
    /// polling.
    pub paths: Vec<PathBuf>,
}

type NativeDebouncer = Debouncer<notify::RecommendedWatcher, RecommendedCache>;
type PollDebouncer = Debouncer<PollWatcher, RecommendedCache>;

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
        Self::spawn_with_paths(roots, git_dirs, &[])
    }

    /// Start watching workspace roots plus a narrow set of exact file paths.
    ///
    /// Exact paths are useful for configuration files that may live outside the
    /// workspace or under hidden directories. Their existing ancestor directories
    /// are watched non-recursively so an absent file or directory can be created
    /// later, and unrelated events from those ancestors are filtered out. This also
    /// covers editors that save by atomically renaming a temporary file over the
    /// destination.
    ///
    /// # Errors
    /// Returns [`WatchError::Backend`] if the platform watch backend cannot start.
    pub fn spawn_with_paths(
        roots: &[PathBuf],
        git_dirs: &[PathBuf],
        exact_paths: &[PathBuf],
    ) -> Result<(Self, UnboundedReceiver<FsEvent>), WatchError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<DebounceEventResult>();
        let (poll_tx, poll_rx) = std::sync::mpsc::channel::<DebounceEventResult>();

        // Cheap: starts the debouncer's own threads but registers no watches yet, so
        // a backend-start failure is still reported synchronously to the caller.
        let debouncer = new_debouncer(DEBOUNCE, None, raw_tx)
            .map_err(|e| WatchError::Backend(e.to_string()))?;
        let poll_debouncer = new_debouncer_opt::<_, PollWatcher, RecommendedCache>(
            DEBOUNCE,
            None,
            poll_tx,
            RecommendedCache::new(),
            Config::default().with_poll_interval(POLL_INTERVAL),
        )
        .map_err(|e| WatchError::Backend(e.to_string()))?;

        // Longest path first, so a nested per-worktree git dir is matched before the
        // common dir it lives under.
        let mut meta_dirs: Vec<PathBuf> = git_dirs.to_vec();
        meta_dirs.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));

        let roots = roots.to_vec();
        let exact_paths: BTreeSet<PathBuf> = exact_paths.iter().cloned().collect();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("karet-watch".to_string())
                .spawn(move || {
                    worker_main(
                        debouncer,
                        poll_debouncer,
                        WorkerMainArgs {
                            raw_rx: &raw_rx,
                            poll_rx: &poll_rx,
                            roots: &roots,
                            meta_dirs: &meta_dirs,
                            exact_paths: &exact_paths,
                            tx: &tx,
                            stop: &stop,
                        },
                    )
                })
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
struct WorkerMainArgs<'a> {
    raw_rx: &'a std::sync::mpsc::Receiver<DebounceEventResult>,
    poll_rx: &'a std::sync::mpsc::Receiver<DebounceEventResult>,
    roots: &'a [PathBuf],
    meta_dirs: &'a [PathBuf],
    exact_paths: &'a BTreeSet<PathBuf>,
    tx: &'a mpsc::UnboundedSender<FsEvent>,
    stop: &'a AtomicBool,
}

fn worker_main(mut native: NativeDebouncer, mut poll: PollDebouncer, args: WorkerMainArgs<'_>) {
    let WorkerMainArgs {
        raw_rx,
        poll_rx,
        roots,
        meta_dirs,
        exact_paths,
        tx,
        stop,
    } = args;
    let mut watched = WatchState::default();
    'roots: for root in roots {
        for dir in enumerate_dirs(root) {
            if stop.load(Ordering::Relaxed) {
                break 'roots;
            }
            watch_dir(&mut native, &mut poll, &mut watched, tx, dir);
        }
    }
    for git_dir in meta_dirs {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        watch_git_dir(&mut native, &mut poll, &mut watched, tx, git_dir);
    }
    if stop.load(Ordering::Relaxed) {
        return;
    }
    watch_explicit_ancestors(&mut native, &mut poll, &mut watched, tx, exact_paths);
    {
        let mut context = EventContext {
            native: &mut native,
            poll: &mut poll,
            watched: &mut watched,
            roots,
            meta_dirs,
            exact_paths,
            tx,
        };

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match raw_rx.recv_timeout(DEBOUNCE) {
                Ok(Ok(events)) => {
                    handle_events(events, &mut context);
                },
                // A batch of backend errors: best-effort, matches the prior behavior of
                // ignoring individual watch/backend hiccups.
                Ok(Err(_)) => {},
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
                // The debouncer's sender side is gone — nothing more will ever arrive.
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(result) = poll_rx.try_recv() {
                if let Ok(events) = result {
                    handle_events(events, &mut context);
                }
            }
        }
    }
    poll.stop();
    native.stop();
}

struct EventContext<'a> {
    native: &'a mut NativeDebouncer,
    poll: &'a mut PollDebouncer,
    watched: &'a mut WatchState,
    roots: &'a [PathBuf],
    meta_dirs: &'a [PathBuf],
    exact_paths: &'a BTreeSet<PathBuf>,
    tx: &'a mpsc::UnboundedSender<FsEvent>,
}

fn handle_events(
    events: Vec<notify_debouncer_full::DebouncedEvent>,
    context: &mut EventContext<'_>,
) {
    for event in events {
        if matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        ) {
            for path in &event.paths {
                unwatch_removed(context.native, context.poll, context.watched, path);
            }
        }
        // A newly created directory needs its own watch (the parent's watch is
        // non-recursive); re-enumerating the new subtree also covers a `mkdir -p`
        // that surfaced only the outermost directory. For rename events, notify
        // carries old and new paths; only the path that exists now enumerates.
        if matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        ) {
            for path in &event.paths {
                if path.is_dir()
                    && context.roots.iter().any(|root| path_is_within(path, root))
                    && !is_ignored(path)
                {
                    for dir in enumerate_dirs(path) {
                        watch_dir(
                            context.native,
                            context.poll,
                            context.watched,
                            context.tx,
                            dir,
                        );
                    }
                }
            }
        }
        // A previously missing exact-path directory may now exist. Register its
        // chain directly rather than enumerating unrelated siblings outside the
        // workspace.
        watch_explicit_ancestors(
            context.native,
            context.poll,
            context.watched,
            context.tx,
            context.exact_paths,
        );
        if let Some(fs) = convert(
            event.kind,
            &event.paths,
            context.roots,
            context.meta_dirs,
            context.exact_paths,
        ) {
            // The receiver going away just means the session shut down.
            let _ = context.tx.send(fs);
        }
    }
}

/// Watch the closest existing directory for each exact file path, plus its parent
/// as an anchor when the immediate directory already exists. This observes later
/// creation and removal/recreation without registering noisy watches all the way to
/// the filesystem root.
fn watch_explicit_ancestors(
    native: &mut NativeDebouncer,
    poll: &mut PollDebouncer,
    watched: &mut WatchState,
    tx: &mpsc::UnboundedSender<FsEvent>,
    exact_paths: &BTreeSet<PathBuf>,
) {
    for path in exact_paths {
        let mut ancestor = path.parent();
        let mut watched_closest = false;
        while let Some(dir) = ancestor {
            if dir.is_dir() {
                watch_dir(native, poll, watched, tx, dir.to_path_buf());
                if watched_closest {
                    break;
                }
                watched_closest = true;
            }
            ancestor = dir.parent();
        }
    }
}

#[derive(Default)]
struct WatchState {
    native: BTreeSet<PathBuf>,
    polled: BTreeSet<PathBuf>,
    degraded_sent: bool,
}

fn watch_dir(
    native: &mut NativeDebouncer,
    poll: &mut PollDebouncer,
    watched: &mut WatchState,
    tx: &mpsc::UnboundedSender<FsEvent>,
    dir: PathBuf,
) {
    if watched.native.contains(&dir) || watched.polled.contains(&dir) {
        return;
    }
    match native.watch(&dir, RecursiveMode::NonRecursive) {
        Ok(()) => {
            watched.native.insert(dir);
        },
        Err(e) if matches!(e.kind, ErrorKind::MaxFilesWatch) => {
            tracing::warn!(
                path = %dir.display(),
                error = %e,
                "native watch limit reached; falling back to polling",
            );
            watch_polled(poll, watched, tx, dir);
        },
        Err(e) => {
            tracing::warn!(path = %dir.display(), error = %e, "watch failed");
        },
    }
}

fn watch_polled(
    poll: &mut PollDebouncer,
    watched: &mut WatchState,
    tx: &mpsc::UnboundedSender<FsEvent>,
    dir: PathBuf,
) {
    match poll.watch(&dir, RecursiveMode::NonRecursive) {
        Ok(()) => {
            watched.polled.insert(dir.clone());
            if !watched.degraded_sent {
                watched.degraded_sent = true;
                let _ = tx.send(FsEvent {
                    kind: FsEventKind::WatchDegraded,
                    paths: vec![dir],
                });
            }
        },
        Err(e) => {
            tracing::warn!(path = %dir.display(), error = %e, "poll watch failed");
        },
    }
}

fn unwatch_removed(
    native: &mut NativeDebouncer,
    poll: &mut PollDebouncer,
    watched: &mut WatchState,
    removed: &Path,
) {
    let native_paths = remove_watched_under(&mut watched.native, removed);
    for path in native_paths {
        if let Err(e) = native.unwatch(&path) {
            tracing::warn!(path = %path.display(), error = %e, "unwatch failed");
        }
    }
    let polled_paths = remove_watched_under(&mut watched.polled, removed);
    for path in polled_paths {
        if let Err(e) = poll.unwatch(&path) {
            tracing::warn!(path = %path.display(), error = %e, "poll unwatch failed");
        }
    }
}

fn remove_watched_under(watched: &mut BTreeSet<PathBuf>, removed: &Path) -> Vec<PathBuf> {
    let paths: Vec<PathBuf> = watched
        .iter()
        .filter(|p| p.as_path() == removed || p.starts_with(removed))
        .cloned()
        .collect();
    for path in &paths {
        watched.remove(path);
    }
    paths
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
    native: &mut NativeDebouncer,
    poll: &mut PollDebouncer,
    watched: &mut WatchState,
    tx: &mpsc::UnboundedSender<FsEvent>,
    git_dir: &Path,
) {
    watch_dir(native, poll, watched, tx, git_dir.to_path_buf());
    let refs = git_dir.join("refs");
    if refs.is_dir() && !watched.native.contains(&refs) && !watched.polled.contains(&refs) {
        match native.watch(&refs, RecursiveMode::Recursive) {
            Ok(()) => {
                watched.native.insert(refs);
            },
            Err(e) if matches!(e.kind, ErrorKind::MaxFilesWatch) => {
                watch_polled(poll, watched, tx, refs);
            },
            Err(e) => {
                tracing::warn!(path = %refs.display(), error = %e, "watch failed");
            },
        }
    }
}

/// Convert a `notify` event into a neutral [`FsEvent`], dropping ignored paths and
/// uninteresting (access-only) events.
fn convert(
    kind: EventKind,
    paths: &[PathBuf],
    roots: &[PathBuf],
    git_dirs: &[PathBuf],
    exact_paths: &BTreeSet<PathBuf>,
) -> Option<FsEvent> {
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
        .filter_map(|path| {
            exact_paths
                .iter()
                .find(|exact| same_path(path, exact))
                .cloned()
                .or_else(|| {
                    keep_path(path, roots, git_dirs, exact_paths).then(|| {
                        path_under_caller_root(path, roots).unwrap_or_else(|| path.clone())
                    })
                })
        })
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some(FsEvent { kind, paths })
}

/// Whether `path` should be surfaced. A path inside one of `git_dirs` is kept only
/// when it names allowlisted git metadata; otherwise the usual ignore rules apply.
fn keep_path(
    path: &Path,
    roots: &[PathBuf],
    git_dirs: &[PathBuf],
    exact_paths: &BTreeSet<PathBuf>,
) -> bool {
    if exact_paths.iter().any(|exact| same_path(path, exact)) {
        return true;
    }
    for git_dir in git_dirs {
        if let Ok(rel) = path.strip_prefix(git_dir) {
            return is_interesting_git_meta(rel);
        }
    }
    roots.iter().any(|root| path_is_within(path, root)) && !is_ignored(path)
}

/// Compare paths through the nearest existing ancestor as well as lexically. On
/// macOS, for example, notify may report `/private/var/...` for a watched
/// `/var/...` path; keeping the caller's spelling in emitted exact-path events makes
/// downstream identity checks stable.
fn same_path(left: &Path, right: &Path) -> bool {
    left == right || normalize_path(left) == normalize_path(right)
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root) || normalize_path(path).starts_with(normalize_path(root))
}

fn path_under_caller_root(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    for root in roots {
        if let Ok(relative) = path.strip_prefix(root) {
            return Some(root.join(relative));
        }
        let normalized_root = normalize_path(root);
        if let Ok(relative) = normalize_path(path).strip_prefix(&normalized_root) {
            return Some(root.join(relative));
        }
    }
    None
}

fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    let mut existing = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        if let Some(name) = existing.file_name() {
            suffix.push(name.to_os_string());
        }
        let Some(parent) = existing.parent() else {
            break;
        };
        existing = parent;
    }
    let mut normalized = std::fs::canonicalize(existing).unwrap_or_else(|_| existing.to_path_buf());
    for component in suffix.into_iter().rev() {
        normalized.push(component);
    }
    normalized
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
