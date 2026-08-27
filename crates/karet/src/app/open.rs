//! Opening things through the backend rather than off the local disk.
//!
//! The shell used to read a file, classify its bytes and build a tab, all before
//! the backend heard about it. That works only while the editor and the files
//! share a machine. Now the path is routed on its *name* — a registry lookup, no
//! I/O — the tab is reserved immediately, and the content follows.
//!
//! Two answers can correct the guess. `Event::PathClassified` carries the
//! authoritative kind, decided from the real leading bytes and the real length, so
//! a `.txt` full of PNG data and a file too large to load inline both land where
//! they should. `Event::FileBytes` then delivers the content for the kinds the
//! client renders itself.
//!
//! Text never appears here: it arrives as a document snapshot, already decoded and
//! highlighted by the machine that holds it.

use std::path::Path;
use std::path::PathBuf;

use karet_fileview::viewer::FileKind;
use karet_session::api::Command as SessionCommand;
use karet_session::api::PathClass;
use karet_session::api::RequestId;

use super::App;
use crate::workspace;

/// Read a file's bytes in pieces of this size.
///
/// Matches the backend's own chunk cap, so an image or a PDF page arrives in as
/// few round trips as the backend is willing to answer in.
const CHUNK: u64 = 1024 * 1024;

/// A file being fetched a chunk at a time.
pub(super) struct PendingBytes {
    /// The file being read.
    pub(super) path: PathBuf,
    /// The renderer its content is destined for.
    pub(super) kind: FileKind,
    /// What has arrived so far.
    pub(super) bytes: Vec<u8>,
}

impl App {
    /// Fetch the content of a tab reserved for raw bytes.
    ///
    /// Idempotent: attaching a backend re-walks every open tab, and a user can
    /// reopen a file that is already loading, so a request already in flight for
    /// this path is left to finish rather than started again.
    pub(super) fn request_content(&mut self, path: &Path) {
        let in_flight = self
            .pending_classify
            .values()
            .any(|pending| pending == path)
            || self
                .pending_bytes
                .values()
                .any(|pending| pending.path == path);
        if in_flight {
            return;
        }
        self.request_classification(path, false);
    }

    /// Ask the backend what `path` really is, so a reserved tab can be corrected.
    ///
    /// Cheap and always worth sending: the answer either confirms the guess (and
    /// nothing happens) or prevents a file from being rendered as something it is
    /// not.
    pub(super) fn request_classification(&mut self, path: &Path, ignore_size: bool) {
        let Some(id) = self.send(SessionCommand::ClassifyPath {
            path: path.to_path_buf(),
            ignore_size,
        }) else {
            return;
        };
        self.pending_classify.insert(id, path.to_path_buf());
    }

    /// Apply the backend's verdict on a path.
    pub(super) fn on_path_classified(
        &mut self,
        id: RequestId,
        path: &Path,
        result: Result<PathClass, String>,
    ) {
        let Some(expected) = self.pending_classify.remove(&id) else {
            return; // an answer to a request this shell has forgotten
        };
        let Ok(class) = result else {
            // Unreadable: the reserved tab already renders as a placeholder, which
            // is the honest thing to show for a file that cannot be read.
            return;
        };
        if expected != path {
            return;
        }
        if workspace::needs_bytes(class.kind) {
            self.request_bytes(path, class.kind, 0);
            return;
        }
        // A guess that did not hold: re-reserve the tab as what the bytes say it
        // is. Text kinds are left alone — their content is already on its way as a
        // document, and replacing the tab would abandon it.
        let guessed = workspace::kind_from_path(path, false);
        if guessed != class.kind && !is_text(class.kind) {
            self.replace_tab_for(path, workspace::reserve(path, class.kind));
        }
    }

    /// Request the next chunk of `path`.
    pub(super) fn request_bytes(&mut self, path: &Path, kind: FileKind, offset: u64) {
        let Some(id) = self.send(SessionCommand::ReadFileBytes {
            path: path.to_path_buf(),
            offset,
            len: CHUNK,
        }) else {
            return;
        };
        let bytes = self
            .pending_bytes
            .values()
            .find(|pending| pending.path == path)
            .map(|pending| pending.bytes.clone())
            .unwrap_or_default();
        self.pending_bytes.insert(
            id,
            PendingBytes {
                path: path.to_path_buf(),
                kind,
                bytes,
            },
        );
    }

    /// Accumulate a chunk, and build the tab once the file is complete.
    pub(super) fn on_file_bytes(
        &mut self,
        id: RequestId,
        path: &Path,
        result: Result<karet_session::api::FileChunk, String>,
    ) {
        let Some(mut pending) = self.pending_bytes.remove(&id) else {
            return;
        };
        let Ok(chunk) = result else {
            // The reserved placeholder is already the right thing to show.
            return;
        };
        if pending.path != path {
            return;
        }
        pending.bytes.extend_from_slice(&chunk.bytes);
        if chunk.is_final() {
            let total = chunk.total_len;
            let tab = workspace::realize(path, pending.kind, pending.bytes, total);
            self.replace_tab_for(path, tab);
            return;
        }
        let next = chunk.offset.saturating_add(chunk.bytes.len() as u64);
        let kind = pending.kind;
        let path = pending.path.clone();
        // Carry what has arrived into the next request rather than restarting.
        if let Some(id) = self.send(SessionCommand::ReadFileBytes {
            path: path.clone(),
            offset: next,
            len: CHUNK,
        }) {
            self.pending_bytes.insert(
                id,
                PendingBytes {
                    path,
                    kind,
                    bytes: pending.bytes,
                },
            );
        }
    }

    /// Swap in `tab` wherever `path` is currently open, across every pane.
    ///
    /// The tab was reserved when the open began, so the user is already looking at
    /// its position; replacing it in place is what makes the content appear rather
    /// than a second tab open beside the first.
    fn replace_tab_for(&mut self, path: &Path, tab: crate::tab::Tab) {
        let mut replacement = Some(tab);
        for existing in self.all_tabs_mut() {
            let matches = !existing.is_diff() && existing.path().is_some_and(|open| open == path);
            if !matches {
                continue;
            }
            let Some(tab) = replacement.take() else {
                break;
            };
            let preview = existing.is_preview;
            let mut tab = tab;
            tab.is_preview = preview;
            *existing = tab;
            break;
        }
    }

    /// Adopt the workspace the backend is rooted at.
    ///
    /// A client's own working directory says nothing about which workspace it is
    /// rendering — the files may be on another machine — so the backend names it.
    /// Everything path-shaped downstream (the explorer, relative opens, the window
    /// title) resolves against this.
    pub(super) fn on_workspace_roots(&mut self, roots: Vec<PathBuf>) {
        let Some(root) = roots.into_iter().next() else {
            return;
        };
        if root == self.root {
            return;
        }
        self.root = root;
        // Every listing was keyed to the old root and none of it applies.
        self.explorer.invalidate_all();
        self.build_explorer();
    }

    /// Ask the backend for the workspace's files, to fill the quick-open picker.
    pub(super) fn request_file_list(&mut self) {
        let Some(id) = self.send(SessionCommand::ListFiles {
            limit: QUICK_OPEN_LIMIT,
        }) else {
            return;
        };
        self.file_list_req = Some(id);
    }

    /// Fill the open quick-open picker with the workspace's files.
    pub(super) fn on_files_listed(&mut self, id: RequestId, files: Vec<PathBuf>) {
        if self.file_list_req != Some(id) {
            return; // a stale answer; the overlay has moved on
        }
        self.file_list_req = None;
        let root = self.root.clone();
        let items: Vec<(String, crate::overlay::OverlayEvent)> = files
            .into_iter()
            .map(|path| {
                (
                    display_path(&root, &path),
                    crate::overlay::OverlayEvent::AcceptFile(path),
                )
            })
            .collect();
        if let Some(crate::overlay::Overlay::Picker(picker)) = self.overlay.as_mut() {
            picker.set_items(items);
        }
    }
}

impl App {
    /// Build the explorer's rows and fetch any listing it turned out to need.
    ///
    /// The tree renders listings it has been given, so every build can discover a
    /// directory nobody has fetched yet — the root on the first frame, a
    /// subdirectory the user just expanded. Each answer marks the tree dirty and
    /// the next build reveals the level below it, so the tree fills in downward as
    /// fast as the backend answers.
    pub(super) fn build_explorer(&mut self) {
        self.explorer.ensure_built(&self.root);
        self.fetch_missing_listings();
    }

    /// Force a rebuild, then fetch anything it needs.
    pub(super) fn rebuild_explorer(&mut self) {
        self.explorer.rebuild(&self.root);
        self.fetch_missing_listings();
    }

    /// Ask the backend for every directory the tree is waiting on.
    fn fetch_missing_listings(&mut self) {
        let (show_hidden, respect_gitignore) = (
            self.explorer.show_hidden(),
            self.explorer.respect_gitignore(),
        );
        for path in self.explorer.take_missing() {
            if self.pending_listings.values().any(|open| *open == path) {
                continue;
            }
            let Some(id) = self.send(SessionCommand::ReadDirectory {
                path: path.clone(),
                show_hidden,
                respect_gitignore,
            }) else {
                // No backend attached yet. Put the miss back rather than dropping
                // it: the tree must keep reporting itself incomplete, or anything
                // waiting on this level concludes it arrived empty.
                self.explorer.mark_missing(path);
                continue;
            };
            self.pending_listings.insert(id, path);
        }
    }

    /// Hand a directory listing to the tree.
    pub(super) fn on_directory_listed(
        &mut self,
        id: RequestId,
        path: &Path,
        result: Result<Vec<karet_core::DirEntry>, String>,
    ) {
        let Some(expected) = self.pending_listings.remove(&id) else {
            return;
        };
        if expected != path {
            return;
        }
        // An unreadable directory is supplied as empty rather than left missing:
        // a permission error should render as a directory with nothing in it, not
        // as one the tree asks about on every single frame.
        self.explorer
            .supply(path.to_path_buf(), result.unwrap_or_default());
        self.build_explorer();
        // A reveal waiting on this level can now advance, or finish.
        self.finish_reveal();
    }

    /// Forget the listings for `dirs`, so the tree fetches them again.
    pub(super) fn invalidate_listings(&mut self, dirs: &[PathBuf]) {
        for dir in dirs {
            self.explorer.invalidate(dir);
        }
        self.build_explorer();
    }
}

/// How many paths quick-open offers before the list is reported truncated.
const QUICK_OPEN_LIMIT: usize = 2000;

/// A path as quick-open shows it: relative to the workspace root when it is
/// under one, so the rows read like the repository rather than the filesystem.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Whether `kind` renders from a document rather than from raw bytes.
fn is_text(kind: FileKind) -> bool {
    matches!(kind, FileKind::Text | FileKind::Markdown | FileKind::Cbor)
}
