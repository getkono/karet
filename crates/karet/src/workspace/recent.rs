//! Recently opened workspaces and files, kept in the user state directory.
//!
//! One small JSON file (`recent.json`) holds the workspaces newest first and, for
//! each, the files opened in it newest first:
//!
//! ```json
//! { "version": 1, "workspaces": [ { "root": "/src/a", "files": ["/src/a/x.rs"] } ] }
//! ```
//!
//! Several karet windows share the file, so a write never saves a snapshot of
//! this window's view: each touch is replayed on a writer thread against what is
//! on disk at that moment, and written back atomically (temporary file, then
//! rename) under an exclusive lock on a sibling `recent.json.lock`, so two
//! windows writing at the same moment cannot lose either's entry. The UI thread
//! never blocks on the disk after the first load.
//!
//! Every path is stored absolute and lexically normalized: a workspace opened as
//! `.` or `src` is remembered by where it is, not by how it was typed, so two
//! projects never share one entry and a reopen never resolves it twice.

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;

use serde_json::Value;
use serde_json::json;

/// How many workspaces are remembered.
const MAX_WORKSPACES: usize = 20;
/// How many files are remembered per workspace.
const MAX_FILES: usize = 50;
/// The store's file name inside the state directory.
const FILE_NAME: &str = "recent.json";

/// `path` made absolute against the current directory, with `.` and `..`
/// resolved lexically (symlinks are kept, as the window title keeps them). A
/// current directory that cannot be read leaves a relative path as it is.
#[must_use]
pub(crate) fn absolute(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    crate::lexically_normalize(&path)
}

/// One remembered change, replayed in memory and against the file on disk.
#[derive(Clone, Debug)]
enum Touch {
    /// A workspace was opened.
    Workspace(PathBuf),
    /// A file was opened in a workspace (which counts as using the workspace).
    File { root: PathBuf, file: PathBuf },
}

/// The remembered lists: workspaces newest first, each with its files newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Recent {
    workspaces: Vec<(PathBuf, Vec<PathBuf>)>,
}

impl Recent {
    /// Read the stored form; anything unreadable is simply not remembered, and
    /// neither is a relative path, which names no place on its own.
    fn parse(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let paths = |value: &Value| -> Vec<PathBuf> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .collect()
        };
        let workspaces = value
            .get("workspaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let root = PathBuf::from(entry.get("root")?.as_str()?);
                let files = entry.get("files").map(paths).unwrap_or_default();
                root.is_absolute().then_some((root, files))
            })
            .take(MAX_WORKSPACES)
            .collect();
        Self { workspaces }
    }

    /// The stored form. A path that is not UTF-8 cannot be written as JSON
    /// text and is left out.
    fn render(&self) -> String {
        let workspaces: Vec<Value> = self
            .workspaces
            .iter()
            .filter_map(|(root, files)| {
                let files: Vec<&str> = files.iter().filter_map(|f| f.to_str()).collect();
                Some(json!({ "root": root.to_str()?, "files": files }))
            })
            .collect();
        let value = json!({ "version": 1, "workspaces": workspaces });
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }

    fn apply(&mut self, touch: &Touch) {
        let (root, file) = match touch {
            Touch::Workspace(root) => (root, None),
            Touch::File { root, file } => (root, Some(file)),
        };
        let at = self.workspaces.iter().position(|(r, _)| r == root);
        let mut entry = at.map_or_else(
            || (root.clone(), Vec::new()),
            |at| self.workspaces.remove(at),
        );
        if let Some(file) = file {
            entry.1.retain(|f| f != file);
            entry.1.insert(0, file.clone());
            entry.1.truncate(MAX_FILES);
        }
        self.workspaces.insert(0, entry);
        self.workspaces.truncate(MAX_WORKSPACES);
    }
}

/// The recently opened workspaces and files, persisted when opened on a path.
pub(crate) struct RecentStore {
    recent: Recent,
    /// Where touches are written; `None` keeps everything in memory (tests,
    /// `--capture`), so nothing but the live editor writes the user's state.
    writer: Option<Writer>,
}

impl RecentStore {
    /// A store that remembers for this process only.
    #[must_use]
    pub(crate) fn in_memory() -> Self {
        Self {
            recent: Recent::default(),
            writer: None,
        }
    }

    /// Where the live editor keeps its store: the platform state directory
    /// (the local data directory where there is none, as for the logs).
    #[must_use]
    pub(crate) fn default_path() -> Option<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "getkono", "karet")?;
        let state = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
        Some(state.join(FILE_NAME))
    }

    /// Load the store at `path` and write every later touch back to it.
    #[must_use]
    pub(crate) fn open(path: PathBuf) -> Self {
        let recent = std::fs::read_to_string(&path)
            .map(|text| Recent::parse(&text))
            .unwrap_or_default();
        Self {
            recent,
            writer: Writer::spawn(path),
        }
    }

    /// The remembered workspaces, newest first.
    pub(crate) fn workspaces(&self) -> impl Iterator<Item = &Path> {
        self.recent
            .workspaces
            .iter()
            .map(|(root, _)| root.as_path())
    }

    /// The files remembered for `root` (however it is written), newest first,
    /// each absolute.
    #[must_use]
    pub(crate) fn files(&self, root: &Path) -> &[PathBuf] {
        let root = absolute(root);
        self.recent
            .workspaces
            .iter()
            .find(|(r, _)| *r == root)
            .map_or(&[], |(_, files)| files.as_slice())
    }

    /// Remember that `root` was opened as a workspace.
    pub(crate) fn touch_workspace(&mut self, root: &Path) {
        self.touch(Touch::Workspace(absolute(root)));
    }

    /// Remember that `file` was opened in the workspace `root`. A relative
    /// `file` is taken relative to `root`; one outside `root` is not remembered.
    pub(crate) fn touch_file(&mut self, root: &Path, file: &Path) {
        let root = absolute(root);
        let file = absolute(&root.join(file));
        if file.starts_with(&root) {
            self.touch(Touch::File { root, file });
        }
    }

    fn touch(&mut self, touch: Touch) {
        self.recent.apply(&touch);
        if let Some(writer) = &self.writer {
            writer.send(touch);
        }
    }
}

/// The thread that replays touches against the file on disk. Dropping it
/// finishes the writes already queued, so quitting right after opening a file
/// still remembers it.
struct Writer {
    tx: Option<mpsc::Sender<Touch>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Writer {
    fn spawn(path: PathBuf) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Touch>();
        let thread = std::thread::Builder::new()
            .name("karet-recent".to_string())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    // Coalesce a burst into one read-modify-write.
                    let touches: Vec<Touch> = std::iter::once(first).chain(rx.try_iter()).collect();
                    if let Err(error) = write_through(&path, &touches) {
                        tracing::warn!("recent: cannot write {}: {error}", path.display());
                    }
                }
            })
            .map_err(|error| tracing::warn!("recent: cannot start the writer: {error}"))
            .ok()?;
        Some(Self {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    fn send(&self, touch: Touch) {
        if let Some(tx) = &self.tx {
            // The thread only stops once this sender is dropped.
            let _ = tx.send(touch);
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Replay `touches` against the store at `path` and write it back atomically.
///
/// The read, the replay and the rename happen under an exclusive lock on a
/// sibling lock file (the store itself is replaced by the rename, so it cannot
/// carry the lock): another window's write waits for this one and then replays
/// against what it left, rather than both reading the same old file and the
/// later rename discarding the earlier entry.
fn write_through(path: &Path, touches: &[Touch]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"))?;
    // Released when `lock` is dropped, after the rename.
    lock.lock()?;
    let mut recent = std::fs::read_to_string(path)
        .map(|text| Recent::parse(&text))
        .unwrap_or_default();
    for touch in touches {
        recent.apply(touch);
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, recent.render())?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn touches_order_newest_first_without_duplicates() {
        let mut store = RecentStore::in_memory();
        store.touch_workspace(&p("/a"));
        store.touch_workspace(&p("/b"));
        store.touch_file(&p("/a"), &p("/a/x.rs"));
        store.touch_file(&p("/a"), &p("/a/y.rs"));
        store.touch_file(&p("/a"), &p("/a/x.rs"));
        let workspaces: Vec<&Path> = store.workspaces().collect();
        assert_eq!(
            workspaces,
            [p("/a"), p("/b")],
            "opening a file uses its workspace"
        );
        assert_eq!(store.files(&p("/a")), [p("/a/x.rs"), p("/a/y.rs")]);
        assert!(store.files(&p("/b")).is_empty());
        assert!(store.files(&p("/never")).is_empty());
    }

    #[test]
    fn relative_roots_and_files_are_stored_absolute_and_normalized() {
        let cwd = std::env::current_dir().expect("current dir");
        let mut store = RecentStore::in_memory();
        store.touch_workspace(Path::new("."));
        store.touch_file(Path::new("."), Path::new("./src/../a.rs"));
        store.touch_file(Path::new("src"), Path::new("b.rs"));
        store.touch_file(Path::new("src"), Path::new("../outside.rs"));
        let workspaces: Vec<&Path> = store.workspaces().collect();
        assert_eq!(workspaces, [cwd.join("src"), cwd.clone()]);
        assert_eq!(store.files(Path::new(".")), [cwd.join("a.rs")]);
        assert_eq!(store.files(&cwd), [cwd.join("a.rs")], "one entry per place");
        assert_eq!(
            store.files(&cwd.join("src")),
            [cwd.join("src/b.rs")],
            "a file outside its workspace is not remembered"
        );
    }

    #[test]
    fn relative_paths_on_disk_are_not_remembered() {
        assert_eq!(
            Recent::parse(
                r#"{"workspaces":[{"root":".","files":[]},{"root":"/c","files":["./x","/c/y"]}]}"#
            ),
            Recent {
                workspaces: vec![(p("/c"), vec![p("/c/y")])],
            },
        );
    }

    #[test]
    fn concurrent_writers_never_lose_an_entry() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(FILE_NAME);
        let writers: Vec<_> = (0..MAX_WORKSPACES)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    write_through(&path, &[Touch::Workspace(p(&format!("/w{n}")))])
                })
            })
            .collect();
        for writer in writers {
            assert!(writer.join().is_ok_and(|written| written.is_ok()));
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(Recent::parse(&text).workspaces.len(), MAX_WORKSPACES);
    }

    #[test]
    fn lists_are_capped() {
        let mut recent = Recent::default();
        for n in 0..MAX_WORKSPACES + 5 {
            recent.apply(&Touch::Workspace(p(&format!("/w{n}"))));
        }
        for n in 0..MAX_FILES + 5 {
            recent.apply(&Touch::File {
                root: p("/w0"),
                file: p(&format!("/w0/f{n}")),
            });
        }
        assert_eq!(recent.workspaces.len(), MAX_WORKSPACES);
        assert_eq!(recent.workspaces[0].0, p("/w0"));
        assert_eq!(recent.workspaces[0].1.len(), MAX_FILES);
        assert_eq!(
            recent.workspaces[0].1[0],
            p(&format!("/w0/f{}", MAX_FILES + 4))
        );
    }

    #[test]
    fn the_stored_form_round_trips_and_garbage_reads_as_empty() {
        let mut recent = Recent::default();
        recent.apply(&Touch::File {
            root: p("/a"),
            file: p("/a/x.rs"),
        });
        recent.apply(&Touch::Workspace(p("/b")));
        assert_eq!(Recent::parse(&recent.render()), recent);
        assert_eq!(Recent::parse("not json"), Recent::default());
        assert_eq!(
            Recent::parse(r#"{"workspaces":[{"files":[]},{"root":"/c","files":[1,"/c/z"]}]}"#),
            Recent {
                workspaces: vec![(p("/c"), vec![p("/c/z")])],
            },
        );
    }

    #[test]
    fn a_persisted_store_merges_with_other_windows_and_survives_a_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("state").join(FILE_NAME);
        {
            let mut one = RecentStore::open(path.clone());
            let mut two = RecentStore::open(path.clone());
            one.touch_workspace(&p("/one"));
            drop(one);
            // `two` loaded before `/one` was written; its write must keep it.
            two.touch_file(&p("/two"), &p("/two/main.rs"));
        }
        let reopened = RecentStore::open(path);
        let workspaces: Vec<&Path> = reopened.workspaces().collect();
        assert_eq!(workspaces, [p("/two"), p("/one")]);
        assert_eq!(reopened.files(&p("/two")), [p("/two/main.rs")]);
    }
}
