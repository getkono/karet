//! A lazy, gitignore-aware file-tree widget with a git-status overlay.
//!
//! [`FileTreeState`] owns the expansion set, selection, and a flattened cache of
//! the currently-visible rows; it reads only the root and each expanded directory
//! (never recursing into collapsed ones). The [`FileTree`] builder supplies
//! presentation: an [`IconSet`], an optional theme, and a path-keyed status
//! overlay (the application maps `karet-vcs` statuses to `karet-core`
//! [`Decoration`]s).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use karet_core::{Decoration, DecorationKind, ThemeRole};
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::StatefulWidget;

/// The glyph set used to draw the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconSet {
    /// ASCII-only (`+`/`-`) — maximally portable.
    Ascii,
    /// Unicode triangles (`▸`/`▾`) — the default for modern terminals.
    #[default]
    Unicode,
    /// Nerd Font folder/file glyphs.
    NerdFont,
}

impl IconSet {
    /// The glyph for a collapsed directory.
    fn dir_closed(self) -> char {
        match self {
            Self::Ascii => '+',
            Self::Unicode => '▸',
            Self::NerdFont => '\u{f07b}',
        }
    }

    /// The glyph for an expanded directory.
    fn dir_open(self) -> char {
        match self {
            Self::Ascii => '-',
            Self::Unicode => '▾',
            Self::NerdFont => '\u{f07c}',
        }
    }

    /// The glyph for a file (a leading space keeps files aligned under dirs).
    fn file(self) -> char {
        match self {
            Self::Ascii | Self::Unicode => ' ',
            Self::NerdFont => '\u{f15b}',
        }
    }
}

/// One flattened, visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTreeRow {
    /// The absolute path of the entry.
    pub path: PathBuf,
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
    selected: usize,
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
            selected: 0,
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

    /// The selected row, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&FileTreeRow> {
        self.rows.get(self.selected)
    }

    /// The first visible row (vertical scroll offset) from the last render.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Select the row currently shown at `viewport_row` (0 = top of the viewport),
    /// clamped to the last row. A no-op when the tree is empty.
    pub fn select_visible(&mut self, viewport_row: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.offset + viewport_row).min(self.rows.len() - 1);
    }

    /// The path of the selected row, if any.
    #[must_use]
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|r| r.path.as_path())
    }

    /// Move the selection to the next row.
    pub fn select_next(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1).min(self.rows.len() - 1);
        }
    }

    /// Move the selection to the previous row.
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
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

    /// Toggle the currently-selected directory (no-op on a file).
    pub fn toggle_selected(&mut self) {
        if let Some(row) = self.rows.get(self.selected)
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
        self.push_dir(root, 0, &mut rows);
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.needs_rebuild = false;
    }

    /// Append `dir`'s entries (and expanded descendants) to `rows`.
    fn push_dir(&self, dir: &Path, depth: u16, rows: &mut Vec<FileTreeRow>) {
        for (path, is_dir) in read_dir_sorted(dir, self.show_hidden, self.respect_gitignore) {
            let expanded = is_dir && self.expanded.contains(&path);
            rows.push(FileTreeRow {
                path: path.clone(),
                depth,
                is_dir,
                expanded,
            });
            if expanded {
                self.push_dir(&path, depth + 1, rows);
            }
        }
    }
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

/// A gitignore-aware file tree with a git-status overlay.
pub struct FileTree<'a> {
    root: &'a Path,
    status: &'a [(PathBuf, Decoration)],
    icons: IconSet,
    theme: Option<&'a Theme>,
}

impl<'a> FileTree<'a> {
    /// Build a file tree rooted at `root`.
    #[must_use]
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            status: &[],
            icons: IconSet::default(),
            theme: None,
        }
    }

    /// Supply a path-keyed status overlay (e.g. from `karet-vcs`).
    #[must_use]
    pub fn status(mut self, status: &'a [(PathBuf, Decoration)]) -> Self {
        self.status = status;
        self
    }

    /// Choose the glyph set.
    #[must_use]
    pub fn icons(mut self, icons: IconSet) -> Self {
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

        // Keep the selection within the viewport.
        if state.selected < state.offset {
            state.offset = state.selected;
        } else if state.selected >= state.offset + height {
            state.offset = state.selected + 1 - height;
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
            let selected = i == state.selected;
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

            let glyph = if row.is_dir {
                if row.expanded {
                    self.icons.dir_open()
                } else {
                    self.icons.dir_closed()
                }
            } else {
                self.icons.file()
            };
            let name = row.path.file_name().and_then(|n| n.to_str()).unwrap_or("?");

            let mut spans = vec![
                Span::styled(
                    "  ".repeat(row.depth as usize),
                    Style::default().fg(guide.to_ratatui()),
                ),
                Span::styled(format!("{glyph} "), Style::default().fg(guide.to_ratatui())),
                Span::styled(name.to_string(), Style::default().fg(fg.to_ratatui())),
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

    #[test]
    fn rebuild_lists_top_level_dirs_first() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "sub/b.txt", b"b");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        // "sub" (dir) before "a.txt" (file); the subdir's child is hidden.
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
    fn select_visible_maps_viewport_rows_via_offset() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        write(&dir.path, "b.txt", b"b");
        write(&dir.path, "c.txt", b"c");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        assert_eq!(state.offset(), 0);
        state.select_visible(2);
        assert_eq!(state.selected, 2);
        state.select_visible(99); // clamps to the last row
        assert_eq!(state.selected, state.rows().len() - 1);
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
