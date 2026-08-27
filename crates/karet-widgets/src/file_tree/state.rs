use std::collections::BTreeMap;

use karet_core::DirEntry;

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
    /// Whether the filesystem entry itself is a symbolic link.
    pub is_symlink: bool,
    /// Whether this directory is itself a Git worktree (the explorer root excluded).
    pub is_repository: bool,
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
    /// Cursor/selection mechanics shared with every other single-line field.
    pub(super) field: crate::textfield::TextFieldState,
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
    /// Directory listings the tree has been given, keyed by directory.
    ///
    /// The tree does not read directories; it renders the ones it has been told
    /// about. That is what lets it show a workspace on another machine, and it
    /// makes the cache the single place a stale listing can live.
    listings: BTreeMap<PathBuf, Vec<DirEntry>>,
    /// Directories a rebuild wanted and did not have.
    ///
    /// Drained by the embedder, which fetches them and calls
    /// [`supply`](Self::supply); each answer triggers another rebuild, so the
    /// tree converges one level at a time.
    missing: BTreeSet<PathBuf>,
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
            listings: BTreeMap::new(),
            missing: BTreeSet::new(),
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

    /// How many rows the tree currently has, expanded state included — the
    /// vertical scroll extent to pair with [`offset`](Self::offset).
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The absolute row index of the cursor row.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    /// Scroll so `row` is the first visible row of a viewport `height` rows tall.
    ///
    /// The cursor comes along when it would otherwise fall outside the new viewport.
    /// That is not a courtesy: the render pins the offset to the cursor, so an offset
    /// written on its own would snap straight back on the next frame. Moving the
    /// cursor is also what the wheel already does in this panel
    /// (`sidebar_wheel` → `sidebar_move`), so a dragged scrollbar behaves the same way
    /// a rolled wheel does.
    pub fn scroll_to(&mut self, row: usize, height: usize) {
        let last = self.rows.len().saturating_sub(1);
        let offset = row.min(self.rows.len().saturating_sub(height));
        self.offset = offset;
        if height == 0 {
            return;
        }
        let cursor = self.cursor();
        let bottom = offset.saturating_add(height - 1).min(last);
        if cursor < offset {
            self.select_index(offset);
        } else if cursor > bottom {
            self.select_index(bottom);
        }
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
            field: crate::textfield::TextFieldState::default(),
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
                field: crate::textfield::TextFieldState::default(),
            });
            if let Some(edit) = self.editing.as_mut() {
                // Select the stem (or the full name), ready to be typed over.
                match rename_selection(&old, &edit.buffer) {
                    Some((start, end)) => {
                        edit.field.set_cursor(&edit.buffer, start, false);
                        edit.field.set_cursor(&edit.buffer, end, true);
                    },
                    None => edit
                        .field
                        .set_cursor(&edit.buffer, edit.buffer.len(), false),
                }
            }
            self.needs_rebuild = true;
        }
    }

    /// Insert a character into the inline edit buffer (no-op when not editing).
    pub fn edit_push(&mut self, c: char) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field
                .insert(&mut edit.buffer, c.encode_utf8(&mut [0; 4]));
            self.needs_rebuild = true;
        }
    }

    /// Delete backward in the inline edit buffer (no-op when not editing).
    pub fn edit_backspace(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.backspace(&mut edit.buffer, false);
            self.needs_rebuild = true;
        }
    }

    /// Delete the character after the inline edit cursor (no-op when not editing).
    pub fn edit_delete(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.delete(&mut edit.buffer, false);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor left by one character.
    pub fn edit_left(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.move_left(&edit.buffer, false);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor right by one character.
    pub fn edit_right(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.move_right(&edit.buffer, false);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor to the start of the buffer.
    pub fn edit_home(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.move_start(&edit.buffer, false, false);
            self.needs_rebuild = true;
        }
    }

    /// Move the inline edit cursor to the end of the buffer.
    pub fn edit_end(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.move_end(&edit.buffer, false, false);
            self.needs_rebuild = true;
        }
    }

    /// Select the full inline edit buffer.
    pub fn edit_select_all(&mut self) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.select_all(&edit.buffer);
            self.needs_rebuild = true;
        }
    }

    /// Insert pasted text at the inline edit cursor (no-op when not editing).
    pub fn edit_paste(&mut self, text: &str) {
        if let Some(edit) = self.editing.as_mut() {
            edit.field.insert(&mut edit.buffer, text);
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
            PendingEdit::Create { path, folder } => path.parent().map(|parent| {
                let buffer = file_label(path);
                let mut field = crate::textfield::TextFieldState::default();
                field.set_cursor(&buffer, buffer.len(), false);
                EditState {
                    kind: if *folder {
                        EditKind::NewFolder
                    } else {
                        EditKind::NewFile
                    },
                    parent: parent.to_path_buf(),
                    buffer,
                    field,
                }
            }),
            PendingEdit::Rename { from, to } => {
                let parent = from
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.root.clone());
                let buffer = file_label(to);
                let mut field = crate::textfield::TextFieldState::default();
                field.set_cursor(&buffer, buffer.len(), false);
                Some(EditState {
                    kind: EditKind::Rename(from.clone()),
                    parent,
                    buffer,
                    field,
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
                        is_symlink: false,
                        is_repository: false,
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

    /// Whether hidden (dot) entries should be listed.
    ///
    /// Read by the embedder when it asks for a directory: the tree records the
    /// preference, and the side doing the listing applies it.
    #[must_use]
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// Whether gitignored entries should be flagged as ignored.
    #[must_use]
    pub fn respect_gitignore(&self) -> bool {
        self.respect_gitignore
    }

    /// The children of `dir` as the tree knows them.
    ///
    /// A directory nobody has supplied yet is recorded as missing and renders
    /// empty until its listing arrives — the tree fills in behind itself rather
    /// than blocking a frame on I/O it cannot do.
    fn children_of(&mut self, dir: &Path) -> Vec<DirEntry> {
        match self.listings.get(dir) {
            Some(children) => children.clone(),
            None => {
                self.missing.insert(dir.to_path_buf());
                Vec::new()
            },
        }
    }

    /// Supply `children` as the listing for `dir`.
    ///
    /// Marks the tree for rebuild, so the rows appear on the next frame. A
    /// listing that replaces an earlier one simply wins: the newest answer is
    /// always the truest.
    pub fn supply(&mut self, dir: PathBuf, mut children: Vec<DirEntry>) {
        karet_core::sort_entries(&mut children);
        self.missing.remove(&dir);
        self.listings.insert(dir, children);
        self.needs_rebuild = true;
    }

    /// Take the directories the last rebuild wanted and did not have.
    ///
    /// The embedder fetches these and calls [`supply`](Self::supply). Draining
    /// rather than reading means a directory is asked for once per miss, not once
    /// per frame.
    #[must_use]
    pub fn take_missing(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.missing).into_iter().collect()
    }

    /// Record `dir` as still needed.
    ///
    /// Used when an embedder took a miss and could not act on it — the tree must
    /// go on knowing it is incomplete, or a caller waiting for the level would
    /// conclude it had arrived empty.
    pub fn mark_missing(&mut self, dir: PathBuf) {
        self.missing.insert(dir);
    }

    /// Whether the tree is still waiting on any directory.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        !self.missing.is_empty()
    }

    /// Forget the listing for `dir`, so the next rebuild asks for it again.
    ///
    /// Used when something changed underneath: a file created or deleted, a
    /// filesystem event, an explicit refresh.
    pub fn invalidate(&mut self, dir: &Path) {
        self.listings.remove(dir);
        self.needs_rebuild = true;
    }

    /// Forget every listing.
    pub fn invalidate_all(&mut self) {
        self.listings.clear();
        self.needs_rebuild = true;
    }

    /// Whether `dir`'s listing is already known.
    #[must_use]
    pub fn has_listing(&self, dir: &Path) -> bool {
        self.listings.contains_key(dir)
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
        let children = self.children_of(root);
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
        &mut self,
        children: Vec<DirEntry>,
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
                    is_symlink: entry.is_symlink,
                    is_repository: false,
                    ignored: parent_ignored || entry.ignored,
                    editing: false,
                });
            }
        }
    }

    /// Push a directory row, compacting a single-child directory chain into one
    /// `a/b/c` row, and recursing into the chain's tip when it is expanded.
    fn push_compacted_dir(
        &mut self,
        first: DirEntry,
        depth: u16,
        parent_ignored: bool,
        rows: &mut Vec<FileTreeRow>,
    ) {
        let mut label = file_label(&first.path);
        let mut tip = first.path;
        // Ignore inherits strictly: once an ancestor is ignored the whole subtree is.
        let mut ignored = parent_ignored || first.ignored;
        // Descend while the current directory's *only* entry is another directory.
        let mut is_repository = first.is_repository;
        let children = loop {
            let entries = self.children_of(&tip);
            if is_repository {
                break entries;
            }
            match entries.as_slice() {
                [child] if child.is_dir => {
                    label.push('/');
                    label.push_str(&file_label(&child.path));
                    ignored = ignored || child.ignored;
                    is_repository = child.is_repository;
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
            is_symlink: first.is_symlink,
            is_repository,
            ignored,
            editing: false,
        });
        if expanded {
            self.push_entries(children, depth + 1, ignored, rows);
        }
    }
}
