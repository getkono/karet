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
//! rename). The UI thread never blocks on the disk after the first load.

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
    /// Read the stored form; anything unreadable is simply not remembered.
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
                .collect()
        };
        let workspaces = value
            .get("workspaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let root = entry.get("root")?.as_str()?;
                let files = entry.get("files").map(paths).unwrap_or_default();
                Some((PathBuf::from(root), files))
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

    /// The files remembered for `root`, newest first.
    #[must_use]
    pub(crate) fn files(&self, root: &Path) -> &[PathBuf] {
        self.recent
            .workspaces
            .iter()
            .find(|(r, _)| r == root)
            .map_or(&[], |(_, files)| files.as_slice())
    }

    /// Remember that `root` was opened as a workspace.
    pub(crate) fn touch_workspace(&mut self, root: &Path) {
        self.touch(Touch::Workspace(root.to_path_buf()));
    }

    /// Remember that `file` was opened in the workspace `root`.
    pub(crate) fn touch_file(&mut self, root: &Path, file: &Path) {
        self.touch(Touch::File {
            root: root.to_path_buf(),
            file: file.to_path_buf(),
        });
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
fn write_through(path: &Path, touches: &[Touch]) -> std::io::Result<()> {
    let mut recent = std::fs::read_to_string(path)
        .map(|text| Recent::parse(&text))
        .unwrap_or_default();
    for touch in touches {
        recent.apply(touch);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
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
