use super::model::*;
use super::*;

/// One flattened, visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeRow {
    /// The absolute path of the entry. For a compacted directory chain this is the
    /// *deepest* directory (the one expansion and selection act on).
    pub path: PathBuf,
    /// The text to display: a file/directory name, or a `a/b/c` chain for a
    /// compacted directory.
    pub label: String,
    /// The nesting depth (0 for top-level entries).
    pub depth: u16,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Whether the entry is an expanded directory.
    pub expanded: bool,
    /// Whether the entry is gitignored (shown dimmed, VS Code style).
    pub ignored: bool,
    /// Whether this row is the in-progress inline name editor (a new file/folder
    /// placeholder or a rename): its [`label`](Self::label) holds the typed buffer and
    /// it renders with a text cursor rather than as a real entry.
    pub editing: bool,
}

/// What an in-progress inline edit will create or change once committed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum EditKind {
    /// Create a new file under [`EditState::parent`].
    NewFile,
    /// Create a new folder under [`EditState::parent`].
    NewFolder,
    /// Rename the entry at this path.
    Rename(PathBuf),
}

/// The in-progress inline name edit: what it will do, the directory it acts in, and
/// the name typed so far.
#[derive(Clone, Debug)]
pub(super) struct EditState {
    kind: EditKind,
    parent: PathBuf,
    pub(super) buffer: String,
    pub(super) cursor: usize,
    pub(super) selection: Option<(usize, usize)>,
}

/// A committed inline edit for the host to apply on the filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingEdit {
    /// Create a file or (when `folder`) a directory at `path`.
    Create {
        /// The absolute path to create.
        path: PathBuf,
        /// Whether to create a directory (else an empty file).
        folder: bool,
    },
    /// Rename `from` to `to`.
    Rename {
        /// The existing absolute path.
        from: PathBuf,
        /// The new absolute path.
        to: PathBuf,
    },
}

/// Persistent file-tree state: expansion, selection, and the flattened row cache.
#[derive(Clone, Debug)]
pub struct FileTreeState {
    root: PathBuf,
    expanded: BTreeSet<PathBuf>,
    pub(super) selection: ListSelection,
    pub(super) offset: usize,
    pub(super) rows: Vec<FileTreeRow>,
    show_hidden: bool,
    respect_gitignore: bool,
    needs_rebuild: bool,
    pub(super) editing: Option<EditState>,
    selected_paths: BTreeSet<PathBuf>,
    cursor_path: Option<PathBuf>,
}

impl Default for FileTreeState {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            expanded: BTreeSet::new(),
            selection: ListSelection::new(0),
            offset: 0,
            rows: Vec::new(),
            show_hidden: true,
            respect_gitignore: true,
            needs_rebuild: true,
            editing: None,
            selected_paths: BTreeSet::new(),
            cursor_path: None,
        }
    }
}

impl FileTreeState {
    /// Create a fresh state (VS Code defaults: dotfiles shown, gitignored files
    /// shown dimmed rather than hidden; only `.git` is excluded).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether to show hidden (dot) files. Note the `.git` directory is always
    /// excluded regardless.
    pub fn set_show_hidden(&mut self, show: bool) {
        self.show_hidden = show;
        self.needs_rebuild = true;
    }

    /// The currently-visible rows.
    #[must_use]
    pub fn rows(&self) -> &[FileTreeRow] {
        &self.rows
    }

    /// The row at the selection cursor, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&FileTreeRow> {
        self.rows.get(self.selection.cursor())
    }

    /// Whether the row at `index` is part of the (possibly multi-row) selection.
    #[must_use]
    pub fn is_selected(&self, index: usize) -> bool {
        self.selection.is_selected(index)
    }

    /// The first visible row (vertical scroll offset) from the last render.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The absolute row index of the cursor row.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    /// The absolute row index for a viewport row, if it currently maps to a row.
    #[must_use]
    pub fn visible_index(&self, viewport_row: usize) -> Option<usize> {
        let idx = self.offset + viewport_row;
        (idx < self.rows.len()).then_some(idx)
    }

    /// Whether the row shown at `viewport_row` is selected.
    #[must_use]
    pub fn is_visible_selected(&self, viewport_row: usize) -> bool {
        self.visible_index(viewport_row)
            .is_some_and(|idx| self.selection.is_selected(idx))
    }

    /// Select the absolute row index, collapsing any multi-selection.
    pub fn select_index(&mut self, index: usize) {
        self.selection.move_to(index);
        self.sync_selection_paths();
    }

    /// Move the cursor to the row currently shown at `viewport_row` (0 = top of the
    /// viewport), collapsing any multi-selection. A no-op when the tree is empty.
    pub fn select_visible(&mut self, viewport_row: usize) {
        self.selection.move_to(self.offset + viewport_row);
        self.sync_selection_paths();
    }

    /// Extend the range selection to the row at `viewport_row`.
    pub fn extend_visible(&mut self, viewport_row: usize) {
        self.selection.extend_to(self.offset + viewport_row);
        self.sync_selection_paths();
    }

    /// Toggle selection of the row at `viewport_row` (Ctrl-click).
    pub fn toggle_visible(&mut self, viewport_row: usize) {
        self.selection.toggle(self.offset + viewport_row);
        self.sync_selection_paths();
    }

    /// The path of the cursor row, if any.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|r| r.path.as_path())
    }

    /// The paths of every effectively-selected row, in visible row order.
    #[must_use]
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .selected_indices()
            .into_iter()
            .filter_map(|i| self.rows.get(i))
            .map(|row| row.path.clone())
            .collect()
    }

    /// Move the cursor to the next row, collapsing any multi-selection.
    pub fn select_next(&mut self) {
        self.selection.move_by(1);
        self.sync_selection_paths();
    }

    /// Move the cursor to the previous row, collapsing any multi-selection.
    pub fn select_prev(&mut self) {
        self.selection.move_by(-1);
        self.sync_selection_paths();
    }

    /// Extend the range selection by `delta` rows (Shift+Arrows).
    pub fn select_extend(&mut self, delta: i32) {
        self.selection.extend_by(delta);
        self.sync_selection_paths();
    }

    /// Toggle whether the cursor row is part of the selection (Space/`x`).
    pub fn mark_toggle(&mut self) {
        self.selection.toggle_cursor();
        self.sync_selection_paths();
    }

    /// Select every row.
    pub fn select_all(&mut self) {
        self.selection.select_all();
        self.sync_selection_paths();
    }

    /// Collapse every expanded directory (VS Code's "Collapse Folders").
    pub fn collapse_all(&mut self) {
        if !self.expanded.is_empty() {
            self.expanded.clear();
            self.needs_rebuild = true;
        }
    }

    /// Whether an inline name edit (new file/folder or rename) is in progress.
    #[must_use]
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Store the current effective selection as path identities.
    fn sync_selection_paths(&mut self) {
        self.selected_paths = self
            .selection
            .selected_indices()
            .into_iter()
            .filter_map(|i| self.rows.get(i))
            .map(|row| row.path.clone())
            .collect();
        self.cursor_path = self.selected().map(|row| row.path.clone());
    }

    /// Rebuild the index-based selection from remembered path identities.
    fn restore_selection_paths(&mut self) {
        let indices: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| self.selected_paths.contains(&row.path).then_some(i))
            .collect();
        let cursor = self
            .cursor_path
            .as_ref()
            .and_then(|path| self.rows.iter().position(|row| &row.path == path))
            .or_else(|| indices.first().copied());
        self.selection.replace_selection(indices, cursor);
        self.sync_selection_paths();
    }

    /// The directory a newly-created entry should live in: the selected directory, a
    /// selected file's parent, or the root when nothing is selected.
    fn new_entry_parent(&self) -> PathBuf {
        match self.selected() {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// Begin creating a new file (or, when `folder`, a directory) under the selection,
    /// showing an inline name editor. The parent directory is expanded so the editor
    /// is visible as its first child.
    pub fn begin_new(&mut self, folder: bool) {
        let parent = self.new_entry_parent();
        if parent != self.root {
            self.expanded.insert(parent.clone());
        }
        let kind = if folder {
            EditKind::NewFolder
        } else {
            EditKind::NewFile
        };
        self.editing = Some(EditState {
            kind,
            parent,
            buffer: String::new(),
            cursor: 0,
            selection: None,
        });
        self.needs_rebuild = true;
    }

    /// Begin renaming the selected entry, seeding the editor with its current name.
    pub fn begin_rename(&mut self) {
        if let Some(row) = self.selected() {
            let old = row.path.clone();
            let parent = old
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone());
            self.editing = Some(EditState {
                kind: EditKind::Rename(old.clone()),
                parent,
                buffer: file_label(&old),
                cursor: 0,
                selection: None,
            });
            if let Some(edit) = self.editing.as_mut() {
                edit.cursor = edit.buffer.len();
                edit.selection = rename_selection(&old, &edit.buffer);
            }
            self.needs_rebuild = true;
        }
    }

    /// Append a character to the inline edit buffer (no-op when not editing).
    pub fn edit_push(&mut self, c: char) {
        if let Some(edit) = self.editing.as_mut() {
            replace_edit_selection(edit, "");
            edit.buffer.insert(edit.cursor, c);
            edit.cursor += c.len_utf8();
            self.needs_rebuild = true;
        }
    }

    /// Delete the last character of the inline edit buffer (no-op when not editing).
    pub fn edit_backspace(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            if !replace_edit_selection(edit, "") && edit.cursor > 0 {
                let prev = prev_boundary(&edit.buffer, edit.cursor);
                edit.buffer.replace_range(prev..edit.cursor, "");
                edit.cursor = prev;
            }
            self.needs_rebuild = true;
        }
    }

    /// Delete the character after the inline edit cursor (no-op when not editing).
    pub fn edit_delete(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            if !replace_edit_selection(edit, "") && edit.cursor < edit.buffer.len() {
                let next = next_boundary(&edit.buffer, edit.cursor);
                edit.buffer.replace_range(edit.cursor..next, "");
            }
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor left by one character.
    pub fn edit_left(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.selection = None;
            edit.cursor = prev_boundary(&edit.buffer, edit.cursor);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor right by one character.
    pub fn edit_right(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.selection = None;
            edit.cursor = next_boundary(&edit.buffer, edit.cursor);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor to the start of the buffer.
    pub fn edit_home(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.selection = None;
            edit.cursor = 0;
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor to the end of the buffer.
    pub fn edit_end(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.selection = None;
            edit.cursor = edit.buffer.len();
            self.needs_rebuild = true;
        }
    }

    /// Select the full inline edit buffer.
    pub fn edit_select_all(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.selection = Some((0, edit.buffer.len()));
            edit.cursor = edit.buffer.len();
            self.needs_rebuild = true;
        }
    }

    /// Append pasted text to the inline edit buffer (no-op when not editing).
    pub fn edit_paste(&mut self, text: &str) {
        if let Some(edit) = self.editing.as_mut() {
            if !replace_edit_selection(edit, text) {
                edit.buffer.insert_str(edit.cursor, text);
                edit.cursor += text.len();
            }
            self.needs_rebuild = true;
        }
    }

    /// Cancel any in-progress inline edit.
    pub fn cancel_edit(&mut self) {
        if self.editing.take().is_some() {
            self.needs_rebuild = true;
        }
    }

    /// Finish the inline edit, returning the filesystem action to apply (or `None` if
    /// the name is blank). The editor is cleared either way.
    #[must_use]
    pub fn take_edit(&mut self) -> Option<PendingEdit> {
        let edit = self.editing.take()?;
        self.needs_rebuild = true;
        let name = edit.buffer.trim();
        if name.is_empty() {
            return None;
        }
        Some(match edit.kind {
            EditKind::NewFile => PendingEdit::Create {
                path: edit.parent.join(name),
                folder: false,
            },
            EditKind::NewFolder => PendingEdit::Create {
                path: edit.parent.join(name),
                folder: true,
            },
            EditKind::Rename(old) => {
                let to = old
                    .parent()
                    .map_or_else(|| edit.parent.join(name), |p| p.join(name));
                PendingEdit::Rename { from: old, to }
            },
        })
    }

    /// Restore a failed inline edit so the user can correct or retry it.
    ///
    /// The app calls this when the filesystem rejects a create/rename after
    /// [`take_edit`](Self::take_edit) has already consumed the editor state.
    pub fn restore_edit(&mut self, pending: &PendingEdit) {
        self.editing = match pending {
            PendingEdit::Create { path, folder } => path.parent().map(|parent| EditState {
                kind: if *folder {
                    EditKind::NewFolder
                } else {
                    EditKind::NewFile
                },
                parent: parent.to_path_buf(),
                buffer: file_label(path),
                cursor: file_label(path).len(),
                selection: None,
            }),
            PendingEdit::Rename { from, to } => {
                let parent = from
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.root.clone());
                Some(EditState {
                    kind: EditKind::Rename(from.clone()),
                    parent,
                    buffer: file_label(to),
                    cursor: file_label(to).len(),
                    selection: None,
                })
            },
        };
        self.needs_rebuild = true;
    }

    /// Overlay the in-progress inline edit onto freshly-built `rows`: a rename marks
    /// its target row as editing; a new file/folder inserts a placeholder editing row
    /// under its parent. Returns the row index the cursor should follow, if any.
    fn apply_editing(&self, rows: &mut Vec<FileTreeRow>) -> Option<usize> {
        let edit = self.editing.as_ref()?;
        match &edit.kind {
            EditKind::Rename(old) => {
                let idx = rows.iter().position(|r| &r.path == old)?;
                rows[idx].editing = true;
                rows[idx].label = edit.buffer.clone();
                Some(idx)
            },
            EditKind::NewFile | EditKind::NewFolder => {
                let is_dir = matches!(edit.kind, EditKind::NewFolder);
                let name = edit.buffer.trim();
                let path = if name.is_empty() {
                    edit.parent.clone()
                } else {
                    edit.parent.join(name)
                };
                let (at, depth) = if edit.parent == self.root {
                    (0, 0)
                } else if let Some(idx) = rows.iter().position(|r| r.path == edit.parent) {
                    (idx + 1, rows[idx].depth + 1)
                } else {
                    (0, 0)
                };
                let at = at.min(rows.len());
                rows.insert(
                    at,
                    FileTreeRow {
                        path,
                        label: edit.buffer.clone(),
                        depth,
                        is_dir,
                        expanded: false,
                        ignored: false,
                        editing: true,
                    },
                );
                Some(at)
            },
        }
    }

    /// Expand directory `path`.
    pub fn expand(&mut self, path: &Path) {
        if self.expanded.insert(path.to_path_buf()) {
            self.needs_rebuild = true;
        }
    }

    /// Collapse directory `path`.
    pub fn collapse(&mut self, path: &Path) {
        if self.expanded.remove(path) {
            self.needs_rebuild = true;
        }
    }

    /// Toggle the expansion of directory `path`.
    pub fn toggle(&mut self, path: &Path) {
        if self.expanded.contains(path) {
            self.collapse(path);
        } else {
            self.expand(path);
        }
    }

    /// Toggle the expansion of the cursor's directory (no-op on a file).
    pub fn toggle_selected(&mut self) {
        if let Some(row) = self.rows.get(self.selection.cursor())
            && row.is_dir
        {
            let path = row.path.clone();
            self.toggle(&path);
        }
    }

    /// Rebuild the visible rows for `root` if the root changed or the tree is dirty.
    pub fn ensure_built(&mut self, root: &Path) {
        if self.needs_rebuild || self.root != root {
            self.rebuild(root);
        }
    }

    /// Force a rebuild of the visible rows for `root`.
    pub fn rebuild(&mut self, root: &Path) {
        self.root = root.to_path_buf();
        let mut rows = Vec::new();
        let children = read_dir_sorted(root, self.show_hidden, self.respect_gitignore);
        self.push_entries(children, 0, false, &mut rows);
        // Overlay any in-progress inline edit, then keep its row under the cursor.
        let follow = self.apply_editing(&mut rows);
        self.rows = rows;
        self.selection.set_len(self.rows.len());
        if let Some(idx) = follow {
            self.selection.move_to(idx);
            self.sync_selection_paths();
        } else {
            self.restore_selection_paths();
        }
        self.needs_rebuild = false;
    }

    /// Append pre-read `children` (files and compacted directory chains) to `rows`.
    ///
    /// `parent_ignored` propagates gitignore state downward: git cannot re-include a
    /// path once an ancestor directory is excluded, so every descendant of an ignored
    /// directory is ignored too — even though the descendant's own name matches no
    /// pattern (a `target/` rule dims everything under `target/`, not just `target/`).
    fn push_entries(
        &self,
        children: Vec<Entry>,
        depth: u16,
        parent_ignored: bool,
        rows: &mut Vec<FileTreeRow>,
    ) {
        for entry in children {
            if entry.is_dir {
                self.push_compacted_dir(entry, depth, parent_ignored, rows);
            } else {
                rows.push(FileTreeRow {
                    label: file_label(&entry.path),
                    path: entry.path,
                    depth,
                    is_dir: false,
                    expanded: false,
                    ignored: parent_ignored || entry.ignored,
                    editing: false,
                });
            }
        }
    }

    /// Push a directory row, compacting a single-child directory chain into one
    /// `a/b/c` row, and recursing into the chain's tip when it is expanded.
    fn push_compacted_dir(
        &self,
        first: Entry,
        depth: u16,
        parent_ignored: bool,
        rows: &mut Vec<FileTreeRow>,
    ) {
        let mut label = file_label(&first.path);
        let mut tip = first.path;
        // Ignore inherits strictly: once an ancestor is ignored the whole subtree is.
        let mut ignored = parent_ignored || first.ignored;
        // Descend while the current directory's *only* entry is another directory.
        let children = loop {
            let entries = read_dir_sorted(&tip, self.show_hidden, self.respect_gitignore);
            match entries.as_slice() {
                [child] if child.is_dir => {
                    label.push('/');
                    label.push_str(&file_label(&child.path));
                    ignored = ignored || child.ignored;
                    tip = child.path.clone();
                },
                _ => break entries,
            }
        };
        let expanded = self.expanded.contains(&tip);
        rows.push(FileTreeRow {
            path: tip,
            label,
            depth,
            is_dir: true,
            expanded,
            ignored,
            editing: false,
        });
        if expanded {
            self.push_entries(children, depth + 1, ignored, rows);
        }
    }
}
