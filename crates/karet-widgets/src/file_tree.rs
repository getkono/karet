//! A lazy, gitignore-aware file-tree widget with per-file-type icons, VS Code–style
//! folder compaction, and a git-status overlay.
//!
//! [`FileTreeState`] owns the expansion set, selection, and a flattened cache of
//! the currently-visible rows. The [`FileTree`] builder supplies presentation: an
//! [`IconStyle`] (file icons resolved from the [`karet_filetype`] registry), an
//! optional theme, and a path-keyed status overlay (the application maps
//! `karet-vcs` statuses to `karet-core` [`Decoration`]s).
//!
//! **Folder compaction:** a directory whose only entry is another directory is
//! merged into a single `a/b/c` row (like VS Code's "compact folders"). The row's
//! [`path`](FileTreeRow::path) is the *deepest* directory — expansion, selection,
//! and opening all act on it; toggling expands/collapses the whole chain.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use karet_core::{Decoration, DecorationKind, ThemeRole};
use karet_filetype::{IconStyle, chevron, directory_icon, icon_for_path};
use karet_theme::Theme;

use crate::ListSelection;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::StatefulWidget;

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
}

/// Persistent file-tree state: expansion, selection, and the flattened row cache.
#[derive(Clone, Debug)]
pub struct FileTreeState {
    root: PathBuf,
    expanded: BTreeSet<PathBuf>,
    selection: ListSelection,
    offset: usize,
    rows: Vec<FileTreeRow>,
    show_hidden: bool,
    respect_gitignore: bool,
    needs_rebuild: bool,
}

impl Default for FileTreeState {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            expanded: BTreeSet::new(),
            selection: ListSelection::new(0),
            offset: 0,
            rows: Vec::new(),
            show_hidden: false,
            respect_gitignore: true,
            needs_rebuild: true,
        }
    }
}

impl FileTreeState {
    /// Create a fresh state (gitignore respected, hidden files hidden).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether to show hidden (dot) files.
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

    /// Move the cursor to the row currently shown at `viewport_row` (0 = top of the
    /// viewport), collapsing any multi-selection. A no-op when the tree is empty.
    pub fn select_visible(&mut self, viewport_row: usize) {
        self.selection.move_to(self.offset + viewport_row);
    }

    /// Extend the range selection to the row at `viewport_row`.
    pub fn extend_visible(&mut self, viewport_row: usize) {
        self.selection.extend_to(self.offset + viewport_row);
    }

    /// Toggle selection of the row at `viewport_row` (Ctrl-click).
    pub fn toggle_visible(&mut self, viewport_row: usize) {
        self.selection.toggle(self.offset + viewport_row);
    }

    /// The path of the cursor row, if any.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|r| r.path.as_path())
    }

    /// Move the cursor to the next row, collapsing any multi-selection.
    pub fn select_next(&mut self) {
        self.selection.move_by(1);
    }

    /// Move the cursor to the previous row, collapsing any multi-selection.
    pub fn select_prev(&mut self) {
        self.selection.move_by(-1);
    }

    /// Extend the range selection by `delta` rows (Shift+Arrows).
    pub fn select_extend(&mut self, delta: i32) {
        self.selection.extend_by(delta);
    }

    /// Toggle whether the cursor row is part of the selection (Space/`x`).
    pub fn mark_toggle(&mut self) {
        self.selection.toggle_cursor();
    }

    /// Select every row.
    pub fn select_all(&mut self) {
        self.selection.select_all();
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
        self.push_entries(children, 0, &mut rows);
        self.rows = rows;
        self.selection.set_len(self.rows.len());
        self.needs_rebuild = false;
    }

    /// Append pre-read `children` (files and compacted directory chains) to `rows`.
    fn push_entries(
        &self,
        children: Vec<(PathBuf, bool)>,
        depth: u16,
        rows: &mut Vec<FileTreeRow>,
    ) {
        for (path, is_dir) in children {
            if is_dir {
                self.push_compacted_dir(path, depth, rows);
            } else {
                rows.push(FileTreeRow {
                    label: file_label(&path),
                    path,
                    depth,
                    is_dir: false,
                    expanded: false,
                });
            }
        }
    }

    /// Push a directory row, compacting a single-child directory chain into one
    /// `a/b/c` row, and recursing into the chain's tip when it is expanded.
    fn push_compacted_dir(&self, first: PathBuf, depth: u16, rows: &mut Vec<FileTreeRow>) {
        let mut label = file_label(&first);
        let mut tip = first;
        // Descend while the current directory's *only* entry is another directory.
        let children = loop {
            let entries = read_dir_sorted(&tip, self.show_hidden, self.respect_gitignore);
            match entries.as_slice() {
                [(child, true)] => {
                    let child = child.clone();
                    label.push('/');
                    label.push_str(&file_label(&child));
                    tip = child;
                }
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
        });
        if expanded {
            self.push_entries(children, depth + 1, rows);
        }
    }
}

/// The display label for a path: its file name, or `?` if it has none.
fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

/// Read the immediate entries of `dir`, dirs first then case-insensitive name.
fn read_dir_sorted(dir: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<(PathBuf, bool)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .require_git(false)
        .parents(respect_gitignore);

    let mut entries: Vec<(PathBuf, bool)> = builder
        .build()
        .flatten()
        .filter(|e| e.depth() > 0) // skip the directory itself
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            (e.path().to_path_buf(), is_dir)
        })
        .collect();
    entries.sort_by(|(pa, da), (pb, db)| db.cmp(da).then_with(|| name_key(pa).cmp(&name_key(pb))));
    entries
}

/// A case-insensitive sort key from a path's file name.
fn name_key(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// A gitignore-aware file tree with per-file-type icons and a git-status overlay.
pub struct FileTree<'a> {
    root: &'a Path,
    status: &'a [(PathBuf, Decoration)],
    icons: IconStyle,
    theme: Option<&'a Theme>,
}

impl<'a> FileTree<'a> {
    /// Build a file tree rooted at `root`.
    #[must_use]
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            status: &[],
            icons: IconStyle::default(),
            theme: None,
        }
    }

    /// Supply a path-keyed status overlay (e.g. from `karet-vcs`).
    #[must_use]
    pub fn status(mut self, status: &'a [(PathBuf, Decoration)]) -> Self {
        self.status = status;
        self
    }

    /// Choose the icon style (Nerd Font / Unicode / ASCII).
    #[must_use]
    pub fn icons(mut self, icons: IconStyle) -> Self {
        self.icons = icons;
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The status decoration for `path`, if any.
    fn status_for(&self, path: &Path) -> Option<&Decoration> {
        self.status.iter().find(|(p, _)| p == path).map(|(_, d)| d)
    }
}

impl StatefulWidget for FileTree<'_> {
    type State = FileTreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut FileTreeState) {
        state.ensure_built(self.root);
        let height = area.height as usize;
        if area.width == 0 || height == 0 {
            return;
        }

        // Keep the cursor within the viewport.
        let cursor = state.selection.cursor();
        if cursor < state.offset {
            state.offset = cursor;
        } else if cursor >= state.offset + height {
            state.offset = cursor + 1 - height;
        }

        let fallback;
        let theme = match self.theme {
            Some(theme) => theme,
            None => {
                fallback = Theme::dark();
                &fallback
            }
        };
        let fg = theme.role(ThemeRole::Foreground);
        let guide = theme.role(ThemeRole::IndentGuide);

        for (i, row) in state
            .rows
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(height)
        {
            let y = area.y + u16::try_from(i - state.offset).unwrap_or(0);
            let selected = state.selection.is_selected(i);
            if selected {
                buf.set_style(
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()),
                );
            }

            // Layout: indent, an expand chevron (directories only), then the
            // type icon (folder / per-file-type), then the label. Files leave the
            // chevron column blank so names stay aligned under directories.
            let chev = if row.is_dir {
                chevron(row.expanded, self.icons)
            } else {
                ' '
            };
            let icon = if row.is_dir {
                directory_icon(row.expanded, self.icons).unwrap_or(' ')
            } else {
                icon_for_path(&row.path, self.icons)
            };

            let mut spans = vec![
                Span::styled(
                    "  ".repeat(row.depth as usize),
                    Style::default().fg(guide.to_ratatui()),
                ),
                Span::styled(format!("{chev} "), Style::default().fg(guide.to_ratatui())),
                Span::styled(format!("{icon} "), Style::default().fg(fg.to_ratatui())),
                Span::styled(row.label.clone(), Style::default().fg(fg.to_ratatui())),
            ];
            if let Some(dec) = self.status_for(&row.path)
                && let DecorationKind::GutterMarker { glyph } = &dec.kind
            {
                let color = dec.role.map_or(fg, |r| theme.role(r));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    glyph.to_string(),
                    Style::default().fg(color.to_ratatui()),
                ));
            }

            buf.set_line(area.x, y, &Line::from(spans), area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use karet_core::{LineCol, Range};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_dir() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("karet-widgets-{}-{}", std::process::id(), n));
        let _ = std::fs::create_dir_all(&path);
        TempDir { path }
    }

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
    }

    fn names(state: &FileTreeState) -> Vec<String> {
        state.rows().iter().map(|r| name_key(&r.path)).collect()
    }

    fn labels(state: &FileTreeState) -> Vec<String> {
        state.rows().iter().map(|r| r.label.clone()).collect()
    }

    #[test]
    fn rebuild_lists_top_level_dirs_first() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "sub/b.txt", b"b");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        // "sub" (dir, single *file* child → not compacted) before "a.txt" (file).
        assert_eq!(names(&state), vec!["sub", "a.txt"]);
    }

    #[test]
    fn toggle_reveals_children() {
        let dir = temp_dir();
        write(&dir.path, "sub/b.txt", b"b");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        state.toggle(&dir.path.join("sub"));
        state.ensure_built(&dir.path);
        assert_eq!(names(&state), vec!["sub", "b.txt"]);
    }

    #[test]
    fn compacts_single_child_directory_chains() {
        let dir = temp_dir();
        // a → b → c, with the leaf file under c: the chain a/b/c collapses to one row.
        write(&dir.path, "a/b/c/leaf.txt", b"x");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        assert_eq!(labels(&state), vec!["a/b/c"]);
        // The row's path is the *deepest* directory.
        assert_eq!(state.rows()[0].path, dir.path.join("a/b/c"));
        // Toggling the chain expands the tip and reveals its child.
        state.toggle_selected();
        state.ensure_built(&dir.path);
        assert_eq!(labels(&state), vec!["a/b/c", "leaf.txt"]);
    }

    #[test]
    fn does_not_compact_when_directory_has_a_file_sibling() {
        let dir = temp_dir();
        write(&dir.path, "a/b/c.txt", b"x");
        write(&dir.path, "a/note.txt", b"y");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        // "a" has two entries (dir b + file note.txt) → not compacted.
        assert_eq!(labels(&state), vec!["a"]);
    }

    #[test]
    fn respects_gitignore() {
        let dir = temp_dir();
        write(&dir.path, ".gitignore", b"ignored.txt\n");
        write(&dir.path, "kept.txt", b"k");
        write(&dir.path, "ignored.txt", b"i");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        // .gitignore is hidden (dotfile); ignored.txt is filtered; kept.txt remains.
        assert_eq!(names(&state), vec!["kept.txt"]);
    }

    #[test]
    fn selection_moves_and_clamps() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "b.txt", b"b");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        assert_eq!(
            state.selected_path(),
            Some(dir.path.join("a.txt").as_path())
        );
        state.select_next();
        assert_eq!(
            state.selected_path(),
            Some(dir.path.join("b.txt").as_path())
        );
        state.select_next(); // clamps at the last row
        assert_eq!(
            state.selected_path(),
            Some(dir.path.join("b.txt").as_path())
        );
        state.select_prev();
        state.select_prev(); // clamps at 0
        assert_eq!(
            state.selected_path(),
            Some(dir.path.join("a.txt").as_path())
        );
    }

    #[test]
    fn multi_select_extends_toggles_and_selects_all() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "b.txt", b"b");
        write(&dir.path, "c.txt", b"c");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);

        // Range: cursor 0, extend down one → rows 0 and 1 selected, 2 not.
        state.select_extend(1);
        assert!(state.is_selected(0));
        assert!(state.is_selected(1));
        assert!(!state.is_selected(2));

        // A plain move collapses the range back to a single row.
        state.select_next();
        assert!(!state.is_selected(0));
        assert!(state.is_selected(2));

        // Toggle keeps the cursor row and adds another; select_all covers everything.
        state.select_prev(); // cursor 1
        state.mark_toggle(); // {1}
        state.select_all();
        assert!((0..3).all(|i| state.is_selected(i)));
    }

    #[test]
    fn select_visible_maps_viewport_rows_via_offset() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "b.txt", b"b");
        write(&dir.path, "c.txt", b"c");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        assert_eq!(state.offset(), 0);
        state.select_visible(2);
        assert_eq!(
            state.selected_path(),
            Some(dir.path.join("c.txt").as_path())
        );
        state.select_visible(99); // clamps to the last row
        assert_eq!(
            state.selected_path(),
            state.rows().last().map(|r| r.path.as_path())
        );
    }

    #[test]
    fn render_draws_status_glyph() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        let mut state = FileTreeState::new();
        let theme = Theme::dark();
        let status = vec![(
            dir.path.join("a.txt"),
            Decoration {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                kind: DecorationKind::GutterMarker { glyph: 'M' },
                role: Some(ThemeRole::DiffModified),
            },
        )];
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        FileTree::new(&dir.path)
            .theme(&theme)
            .status(&status)
            .render(area, &mut buf, &mut state);
        let rendered: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("a.txt"));
        assert!(rendered.contains('M'));
    }
}
