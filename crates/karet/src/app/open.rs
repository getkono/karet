//! Opening things through the backend rather than off the local disk.
//!
//! The shell used to read a whole file, classify its bytes and build a tab, all
//! before the backend heard about it. That works only while the editor and the
//! files share a machine. Now the tab is reserved immediately and the content
//! follows from the backend. Where the workspace is remote the path is routed on
//! its *name* — a registry lookup, no I/O; a co-located workspace still routes on
//! content, with a stat and an 8 KiB head read in [`App::open_tab`], since the
//! disk is right here.
//!
//! Two answers can correct a remote guess. `Event::PathClassified` carries the
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
use karet_session::api::DocumentCommand;
use karet_session::api::DocumentId;
use karet_session::api::PathClass;
use karet_session::api::RequestId;
use karet_session::api::ViewId;
use karet_session::api::WorkspaceCommand;

use super::App;
use crate::tab::TabKind;
use crate::tab::open;

/// Read a file's bytes in pieces of this size.
///
/// Matches the backend's own chunk cap, so an image or a PDF page arrives in as
/// few round trips as the backend is willing to answer in.
const CHUNK: u64 = 1024 * 1024;

/// How many leading bytes a co-located open reads to classify a file: the same
/// sample the backend's classification reads.
const HEAD_BYTES: u64 = 8 * 1024;

/// Whether `tab` is a code tab with no document, or a conversion preview not yet
/// requested, on `path` — a tab whose content waits on the size guard.
fn size_waiting(tab: &crate::tab::Tab, path: &Path) -> bool {
    tab.size_unchecked && is_unregistered(tab, path)
}

/// Whether `tab` is on `path` and has not asked the backend for its content yet:
/// a code tab with no document, or a DOCX or notebook preview still pending.
fn is_unregistered(tab: &crate::tab::Tab, path: &Path) -> bool {
    match &tab.kind {
        TabKind::Code {
            path: p, doc: None, ..
        } => p == path,
        TabKind::MarkdownPreview {
            path: p,
            pending_since: Some(_),
            ..
        } => p == path,
        _ => false,
    }
}

/// Whether `tab` is still waiting for its content: a byte view with no bytes, a
/// placeholder, a code tab with no document, or a conversion preview not yet
/// answered. Only these are replaced when content or a verdict arrives; a tab
/// already showing the file is left alone.
fn is_reservation(tab: &crate::tab::Tab) -> bool {
    match &tab.kind {
        TabKind::Hex { bytes, .. } => bytes.is_empty(),
        TabKind::Placeholder { .. }
        | TabKind::Code { doc: None, .. }
        | TabKind::MarkdownPreview {
            pending_since: Some(_),
            ..
        } => true,
        _ => false,
    }
}

/// The leading bytes and length of a file on this machine's disk, as a
/// co-located open classifies it; empty and zero for anything that is not a
/// regular file (a file not created yet opens empty, and a FIFO is never opened).
pub(super) fn local_head(path: &Path) -> (Vec<u8>, u64) {
    use std::io::Read;

    let Ok(metadata) = std::fs::metadata(path) else {
        return (Vec::new(), 0);
    };
    if !metadata.is_file() {
        return (Vec::new(), 0);
    }
    let mut head = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(HEAD_BYTES).read_to_end(&mut head);
    }
    (head, metadata.len())
}

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
    /// Reserve the tab `path` should open in.
    ///
    /// A co-located workspace routes on the file's content, since its disk is
    /// right here: a stat and an 8 KiB read settle a name that lies (an
    /// extensionless PNG or PDF, an ELF binary, text full of NULs) and the size
    /// guard before the tab exists, as a co-located open always did. A remote
    /// workspace routes on the name and is corrected when the backend's
    /// classification answers.
    pub(super) fn open_tab(&self, path: &Path) -> crate::tab::Tab {
        if self.workspace_remote {
            return crate::tab::Tab::open(path);
        }
        let (head, len) = local_head(path);
        let mut tab = open::reserve(path, karet_filetype::classify(path, &head, len), len);
        tab.size_unchecked = false; // settled by the stat above
        tab
    }

    /// [`open_tab`](Self::open_tab) for the "open anyway" override: the size
    /// guard is waived, so a large file opens as what its content says it is.
    pub(super) fn open_tab_ignoring_size(&self, path: &Path) -> crate::tab::Tab {
        if self.workspace_remote {
            return crate::tab::Tab::open_ignoring_size(path);
        }
        let (head, len) = local_head(path);
        let mut tab = open::reserve(
            path,
            karet_filetype::classify_ignoring_size(path, &head),
            len,
        );
        tab.size_unchecked = false;
        tab
    }

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

    /// Abandon every classification and read in flight for `path`, so the next
    /// [`request_content`](Self::request_content) starts from the beginning
    /// rather than leaving a request about the file as it used to be to finish.
    /// Their answers arrive for requests this shell has forgotten and are
    /// dropped.
    pub(super) fn forget_content_requests(&mut self, path: &Path) {
        self.pending_classify.retain(|_, pending| pending != path);
        self.pending_bytes.retain(|_, pending| pending.path != path);
    }

    /// Ask the backend what `path` really is, so a reserved tab can be corrected.
    ///
    /// Cheap and always worth sending: the answer either confirms the guess (and
    /// nothing happens) or prevents a file from being rendered as something it is
    /// not.
    pub(super) fn request_classification(&mut self, path: &Path, ignore_size: bool) {
        let Some(id) = self.send(WorkspaceCommand::ClassifyPath {
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
        if expected != path {
            return;
        }
        let class = match result {
            Ok(class) => class,
            Err(error) => {
                // A tab waiting on the size guard opens anyway: the session
                // decides what an unreadable path is (a file not yet created
                // opens empty, anything else reports why), and nothing over the
                // guard can be read without a classification succeeding first.
                if self.any_tab(|tab| size_waiting(tab, path)) {
                    self.for_each_tab(|tab| {
                        if size_waiting(tab, path) {
                            tab.size_unchecked = false;
                        }
                    });
                    self.register_unregistered(path);
                }
                // A tab reserved for bytes says why it will never get them,
                // rather than staying an empty view.
                self.fail_byte_reservations(path, &error);
                return;
            },
        };
        // A tab waiting on the size guard: under it, its content is asked for;
        // over it, or not what the name said after all, the tab becomes what
        // the bytes say.
        if self.any_tab(|tab| size_waiting(tab, path)) {
            self.for_each_tab(|tab| {
                if size_waiting(tab, path) {
                    tab.size_unchecked = false;
                }
            });
            let converted = open::is_converted_kind(class.kind);
            if open::is_text_kind(class.kind)
                || (converted && open::kind_from_path(path) == class.kind)
            {
                self.register_unregistered(path);
                return;
            }
            self.replace_tab_for(path, || open::reserve(path, class.kind, class.len));
            if open::needs_bytes(class.kind) {
                self.request_bytes(path, class.kind, 0);
            } else if converted {
                self.register_unregistered(path);
            }
            return;
        }
        if open::needs_bytes(class.kind) {
            self.request_bytes(path, class.kind, 0);
            return;
        }
        // A guess that did not hold: re-reserve the tab as what the bytes say it
        // is. Only byte tabs ask to be classified, so a text verdict here is a
        // file the name hid (CBOR sniffed from its content, say): its new code
        // tab registers as a document like any other.
        if open::kind_from_path(path) == class.kind {
            // The guess held, but a placeholder reserved on the name alone (a
            // kind this build cannot render) did not know the length it shows.
            self.for_each_tab(|tab| {
                if let TabKind::Placeholder {
                    path: p, kind, len, ..
                } = &mut tab.kind
                    && p == path
                    && *kind == class.kind
                {
                    *len = class.len;
                }
            });
            return;
        }
        self.replace_tab_for(path, || open::reserve(path, class.kind, class.len));
        if open::is_text_kind(class.kind) {
            self.register_unregistered(path);
        }
    }

    /// Settle the size guard for the text tab at `idx` before its document
    /// opens; `true` when it may open now.
    ///
    /// The session loads a document's whole file, so a file over the guard must
    /// become a too-large placeholder before it is asked to. A co-located
    /// workspace answers with a stat, at once. A remote one is asked to
    /// classify the path, and the answer opens the document or replaces the tab
    /// (see [`on_path_classified`](Self::on_path_classified)).
    pub(super) fn check_size_guard(&mut self, idx: usize, path: &Path) -> bool {
        if self.workspace_remote {
            if !self
                .pending_classify
                .values()
                .any(|pending| pending == path)
            {
                self.request_classification(path, false);
            }
            return false;
        }
        let len = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
        let Some(tab) = self.tabs.get_mut(idx) else {
            return false;
        };
        tab.size_unchecked = false;
        if len <= karet_filetype::SIZE_GUARD {
            return true;
        }
        let mut placeholder = open::reserve(path, FileKind::TooLarge { len }, len);
        placeholder.view = tab.view;
        placeholder.is_preview = tab.is_preview;
        *tab = placeholder;
        false
    }

    /// Open the document of every not-yet-registered code tab on `path`, in
    /// every pane.
    fn register_unregistered(&mut self, path: &Path) {
        let waiting = |tabs: &[crate::tab::Tab]| -> Vec<usize> {
            tabs.iter()
                .enumerate()
                .filter(|(_, tab)| is_unregistered(tab, path))
                .map(|(idx, _)| idx)
                .collect()
        };
        for idx in waiting(&self.tabs) {
            self.register_doc(idx);
        }
        let panes: Vec<_> = self.stored.keys().copied().collect();
        for pane in panes {
            let Some(mut stored) = self.stored.remove(&pane) else {
                continue;
            };
            // `register_doc` reads the focused pane's tabs; lend it this pane's.
            std::mem::swap(&mut self.tabs, &mut stored.tabs);
            for idx in waiting(&self.tabs) {
                self.register_doc(idx);
            }
            std::mem::swap(&mut self.tabs, &mut stored.tabs);
            self.stored.insert(pane, stored);
        }
    }

    fn any_tab(&self, matches: impl Fn(&crate::tab::Tab) -> bool) -> bool {
        self.all_tabs().any(matches)
    }

    fn for_each_tab(&mut self, mut apply: impl FnMut(&mut crate::tab::Tab)) {
        for tab in self.all_tabs_mut() {
            apply(tab);
        }
    }

    /// Fetch `path`'s bytes for the hex view a text open fell back to, unless
    /// they are already on the way.
    pub(super) fn request_fallback_bytes(&mut self, path: &Path) {
        self.request_bytes(path, FileKind::Binary, 0);
    }

    /// Start reading `path` from `offset`, for a tab still reserved for it.
    ///
    /// One read per path at a time: a verdict arriving while a read is already
    /// streaming (a reopen, a classification answered late) leaves it to
    /// finish, rather than starting a second stream from the beginning.
    pub(super) fn request_bytes(&mut self, path: &Path, kind: FileKind, offset: u64) {
        let streaming = self
            .pending_bytes
            .values()
            .any(|pending| pending.path == path);
        if streaming || !self.awaits_bytes(path) {
            return;
        }
        let Some(id) = self.send(WorkspaceCommand::ReadFileBytes {
            path: path.to_path_buf(),
            offset,
            len: CHUNK,
        }) else {
            return;
        };
        self.pending_bytes.insert(
            id,
            PendingBytes {
                path: path.to_path_buf(),
                kind,
                bytes: Vec::new(),
            },
        );
    }

    /// Whether any tab, in any pane, is still reserved for `path`'s content.
    fn awaits_bytes(&self, path: &Path) -> bool {
        self.any_tab(|tab| {
            !tab.is_diff() && tab.path().is_some_and(|open| open == path) && is_reservation(tab)
        })
    }

    /// Mark every tab reserved for `path`'s bytes as failed with `error`, so it
    /// says why rather than staying an empty view.
    fn fail_byte_reservations(&mut self, path: &Path, error: &str) {
        self.for_each_tab(|tab| {
            let reserved = match &tab.kind {
                TabKind::Hex { path: p, bytes, .. } => p == path && bytes.is_empty(),
                TabKind::Placeholder { path: p, .. } => p == path && tab.fill_since.is_some(),
                _ => false,
            };
            if reserved {
                tab.fill_since = None;
                tab.fill_error = Some(error.to_owned());
            }
        });
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
        if pending.path != path {
            return;
        }
        let chunk = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                self.fail_byte_reservations(path, &error);
                return;
            },
        };
        // Every tab waiting for this file closed, or was replaced, while it was
        // being read: stop rather than read the rest for nobody.
        if !self.awaits_bytes(path) {
            return;
        }
        pending.bytes.extend_from_slice(&chunk.bytes);
        if chunk.is_final() {
            let total = chunk.total_len;
            let (kind, bytes) = (pending.kind, pending.bytes);
            self.replace_tab_for(path, || open::realize(path, kind, bytes.clone(), total));
            return;
        }
        let next = chunk.offset.saturating_add(chunk.bytes.len() as u64);
        let kind = pending.kind;
        let path = pending.path.clone();
        // Carry what has arrived into the next request rather than restarting.
        if let Some(id) = self.send(WorkspaceCommand::ReadFileBytes {
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

    /// Swap a tab built by `make` in wherever `path` is currently open, in every
    /// pane.
    ///
    /// The tab was reserved when the open began, so the user is already looking at
    /// its position; replacing it in place is what makes the content appear rather
    /// than a second tab open beside the first. Each replacement keeps the view
    /// it replaces: MRU order, viewport reports and every other view-keyed
    /// lookup name the tab by it, and a second pane showing the same file is
    /// filled too rather than left an empty reservation.
    fn replace_tab_for(&mut self, path: &Path, make: impl Fn() -> crate::tab::Tab) {
        for existing in self.all_tabs_mut() {
            let matches = !existing.is_diff()
                && existing.path().is_some_and(|open| open == path)
                && is_reservation(existing);
            if !matches {
                continue;
            }
            let mut tab = make();
            tab.view = existing.view;
            tab.is_preview = existing.is_preview;
            *existing = tab;
        }
    }

    /// Tell the backend what each open document's views are displaying.
    ///
    /// Highlights are resolved per rendered line, so spans outside a viewport are
    /// never read. Bounding them is what keeps a keystroke's answer to about a
    /// screenful rather than a whole file's worth of spans — the difference
    /// between a usable and an unusable connection once the two ends are not on
    /// one machine.
    ///
    /// Called after each frame, because the visible range is only known once the
    /// editor has been laid out. Sends only what changed: a viewport that has not
    /// moved is a message that says nothing. A view that closed, or now shows
    /// another document, has its old range withdrawn, so it stops widening the
    /// window of a document another view still shows.
    ///
    /// Only a remote workspace reports: a co-located session hands over the
    /// whole document's highlights at no cost, and narrowing them would only
    /// paint a long jump uncoloured for a frame.
    pub(super) fn report_viewports(&mut self) {
        if !self.workspace_remote {
            return;
        }
        let mut seen: Vec<(ViewId, DocumentId, u32, u32)> = Vec::new();
        for tab in self.all_tabs_mut() {
            let TabKind::Code { doc: Some(doc), .. } = &tab.kind else {
                continue;
            };
            let visible = tab.editor.viewport();
            seen.push((tab.view, *doc, visible.start.line, visible.end.line));
        }
        let withdrawn: Vec<(ViewId, DocumentId)> = self
            .reported_viewports
            .iter()
            .filter(|(view, (doc, ..))| {
                !seen
                    .iter()
                    .any(|(shown, showing, ..)| shown == *view && showing == doc)
            })
            .map(|(view, (doc, ..))| (*view, *doc))
            .collect();
        for (view, doc) in withdrawn {
            self.reported_viewports.remove(&view);
            self.send_command(DocumentCommand::ClearViewport { doc, view });
        }
        for (view, doc, first_line, last_line) in seen {
            if self.reported_viewports.get(&view) == Some(&(doc, first_line, last_line)) {
                continue;
            }
            self.reported_viewports
                .insert(view, (doc, first_line, last_line));
            self.send_command(DocumentCommand::SetViewport {
                doc,
                view,
                first_line,
                last_line,
            });
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
        self.root = root.clone();
        // Every listing was keyed to the old root and none of it applies.
        self.explorer.invalidate_all();
        self.build_explorer();
        // Nor does a file list walked from it.
        self.file_list_req = None;
        self.file_index.retarget(root);
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
        let wanted = self.explorer.take_missing();
        // A listed directory just expanded is read again too: the tree serves
        // listings it holds, and nothing reports a change inside a hidden or
        // ignored directory (`target/`, `node_modules/`), so looking at one is
        // the moment to bring it up to date. Its rows stay until the answer.
        let stale = self.explorer.take_stale();
        for path in stale {
            if self.pending_listings.values().any(|open| *open == path) {
                continue;
            }
            if let Some(id) = self.send_listing(&path, show_hidden, respect_gitignore) {
                self.pending_listings.insert(id, path);
            }
        }
        for path in wanted {
            if self.pending_listings.values().any(|open| *open == path) {
                continue;
            }
            let Some(id) = self.send_listing(&path, show_hidden, respect_gitignore) else {
                // No backend attached yet, or it has closed. Put the miss back
                // rather than dropping it: the tree must keep reporting itself
                // incomplete, or anything waiting on this level concludes it
                // arrived empty.
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

    /// Read every directory the tree has listed again, keeping what it shows
    /// until each fresh answer replaces it, so nothing blanks on the way.
    ///
    /// The tree holds listings rather than reading the disk on each build, so a
    /// file created or deleted outside the editor (a checkout, a build, a shell)
    /// shows only once its directory is asked about again.
    pub(super) fn refresh_listings(&mut self) {
        let dirs = self.explorer.listed_dirs();
        self.refetch_listings(dirs);
    }

    /// Read again whichever directories touched by `changed` the tree has
    /// listed: a changed path's parent gains or loses an entry, and a changed
    /// directory may have new children.
    pub(super) fn refresh_changed_listings(&mut self, changed: &[PathBuf]) {
        let mut dirs: Vec<PathBuf> = changed
            .iter()
            .flat_map(|path| [Some(path.as_path()), path.parent()])
            .flatten()
            .filter(|dir| self.explorer.has_listing(dir))
            .map(Path::to_path_buf)
            .collect();
        dirs.sort();
        dirs.dedup();
        self.refetch_listings(dirs);
    }

    /// Request `dirs` again even when a request is already in flight: that
    /// answer may have been read before the change that prompted this one, and
    /// the later answer, arriving later, wins.
    fn refetch_listings(&mut self, dirs: Vec<PathBuf>) {
        let (show_hidden, respect_gitignore) = (
            self.explorer.show_hidden(),
            self.explorer.respect_gitignore(),
        );
        for path in dirs {
            match self.send_listing(&path, show_hidden, respect_gitignore) {
                Some(id) => {
                    self.pending_listings.insert(id, path);
                },
                None => self.explorer.mark_missing(path),
            }
        }
    }

    /// Ask for one directory listing, without reporting a send that fails.
    ///
    /// The tree asks again on every build until a listing arrives, so a closed
    /// backend would otherwise raise a failure notification each frame. That
    /// failure is reported once, when the backend's streams close.
    fn send_listing(
        &self,
        path: &Path,
        show_hidden: bool,
        respect_gitignore: bool,
    ) -> Option<RequestId> {
        let backend = self.backend.as_ref()?;
        let id = backend.next_id();
        let command = WorkspaceCommand::ReadDirectory {
            path: path.to_path_buf(),
            show_hidden,
            respect_gitignore,
        };
        backend.send(id, command.into()).ok().map(|()| id)
    }

    /// Read again the listed directories a filesystem mutation touched, keeping
    /// their rows (and the cursor on them) until each answer replaces them.
    ///
    /// Each end's parent is read where the tree lists it; where it does not —
    /// a path created under folders that did not exist yet — its nearest listed
    /// ancestor is, which is where the first new folder appears.
    pub(super) fn refresh_mutated_listings(&mut self, dirs: &[PathBuf]) {
        let mut listed: Vec<PathBuf> = dirs
            .iter()
            .filter_map(|dir| {
                dir.ancestors()
                    .take_while(|ancestor| ancestor.starts_with(&self.root))
                    .find(|ancestor| self.explorer.has_listing(ancestor))
                    .map(Path::to_path_buf)
            })
            .collect();
        listed.sort();
        listed.dedup();
        self.refetch_listings(listed);
    }
}
