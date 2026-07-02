//! A lazy, gitignore-aware file-tree widget with per-file-type icons, VS Code–style
//! folder compaction, and a git-status overlay.
//!
//! [`FileTreeState`] owns the expansion set, selection, and a flattened cache of
//! the currently-visible rows. The [`FileTree`] builder supplies presentation: an
//! [`IconStyle`] (file icons resolved from the [`karet_filetype`] registry), an
//! optional theme, and a path-keyed status overlay (the application maps
//! `karet-vcs` statuses to `karet-core` [`Decoration`]s).
//!
//! **Gitignore (VS Code behavior):** gitignored files are *not* hidden — they are
//! listed and rendered dimmed (their [`ignored`](FileTreeRow::ignored) flag), so a
//! `target/` or `node_modules/` is visible but visually recedes. Dotfiles are shown
//! too; only the `.git` directory itself is always excluded.
//!
//! **Folder compaction:** a directory whose only entry is another directory is
//! merged into a single `a/b/c` row (like VS Code's "compact folders"). The row's
//! [`path`](FileTreeRow::path) is the *deepest* directory — expansion, selection,
//! and opening all act on it; toggling expands/collapses the whole chain.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::ThemeRole;
use karet_filetype::Category;
use karet_filetype::IconStyle;
use karet_filetype::category_for_path;
use karet_filetype::chevron;
use karet_filetype::directory_icon;
use karet_filetype::icon_for_path;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;

use crate::ListSelection;

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
            show_hidden: true,
            respect_gitignore: true,
            needs_rebuild: true,
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
    fn push_entries(&self, children: Vec<Entry>, depth: u16, rows: &mut Vec<FileTreeRow>) {
        for entry in children {
            if entry.is_dir {
                self.push_compacted_dir(entry, depth, rows);
            } else {
                rows.push(FileTreeRow {
                    label: file_label(&entry.path),
                    path: entry.path,
                    depth,
                    is_dir: false,
                    expanded: false,
                    ignored: entry.ignored,
                });
            }
        }
    }

    /// Push a directory row, compacting a single-child directory chain into one
    /// `a/b/c` row, and recursing into the chain's tip when it is expanded.
    fn push_compacted_dir(&self, first: Entry, depth: u16, rows: &mut Vec<FileTreeRow>) {
        let mut label = file_label(&first.path);
        let mut tip = first.path;
        let mut ignored = first.ignored;
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
        });
        if expanded {
            self.push_entries(children, depth + 1, rows);
        }
    }
}

/// One immediate directory entry, with its gitignore status.
struct Entry {
    path: PathBuf,
    is_dir: bool,
    ignored: bool,
}

/// The display label for a path: its file name, or `?` if it has none.
fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

/// Read the immediate entries of `dir`, dirs first then case-insensitive name.
///
/// Gitignored entries are listed and flagged `ignored` (VS Code dims them) rather
/// than filtered out. The `.git` directory is always excluded; dotfiles are shown
/// unless `show_hidden` is false.
fn read_dir_sorted(dir: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<Entry> {
    // The full listing (gitignore off): everything the user should see.
    let all = walk_immediate(dir, show_hidden, false);
    let mut entries: Vec<Entry> = if respect_gitignore {
        // The non-ignored subset; anything in `all` but not here is gitignored.
        let visible: BTreeSet<PathBuf> = walk_immediate(dir, show_hidden, true)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        all.into_iter()
            .map(|(path, is_dir)| {
                let ignored = !visible.contains(&path);
                Entry {
                    path,
                    is_dir,
                    ignored,
                }
            })
            .collect()
    } else {
        all.into_iter()
            .map(|(path, is_dir)| Entry {
                path,
                is_dir,
                ignored: false,
            })
            .collect()
    };
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| name_key(&a.path).cmp(&name_key(&b.path)))
    });
    entries
}

/// List the immediate children of `dir` as `(path, is_dir)`, honoring the hidden
/// and gitignore filters, but always excluding the `.git` directory.
fn walk_immediate(dir: &Path, show_hidden: bool, git_ignore: bool) -> Vec<(PathBuf, bool)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(git_ignore)
        .git_global(git_ignore)
        .git_exclude(git_ignore)
        .require_git(false)
        .parents(git_ignore);
    builder
        .build()
        .flatten()
        .filter(|e| e.depth() > 0) // skip the directory itself
        .filter(|e| e.file_name() != std::ffi::OsStr::new(".git"))
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            (e.path().to_path_buf(), is_dir)
        })
        .collect()
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
    open: &'a [PathBuf],
    active: Option<&'a Path>,
    hover: Option<usize>,
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
            open: &[],
            active: None,
            hover: None,
            icons: IconStyle::default(),
            theme: None,
        }
    }

    /// Supply the (absolute) row index the mouse is hovering, so it gets a secondary
    /// highlight distinct from the selection.
    #[must_use]
    pub fn hover(mut self, hover: Option<usize>) -> Self {
        self.hover = hover;
        self
    }

    /// Supply the paths of files currently open in editor tabs, so their rows are
    /// highlighted (the [`active`](Self::active) one most prominently).
    #[must_use]
    pub fn open(mut self, open: &'a [PathBuf]) -> Self {
        self.open = open;
        self
    }

    /// Supply the path of the active editor tab, so its row gets the strongest
    /// highlight (VS Code shows the active file emphasized in the explorer).
    #[must_use]
    pub fn active(mut self, active: Option<&'a Path>) -> Self {
        self.active = active;
        self
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

/// Map a file [`Category`] to the explorer icon-tint role: text-like types share
/// one tint, media and documents another, binaries/archives a third, and everything
/// unrecognized falls back to the neutral [`Foreground`](ThemeRole::Foreground).
fn category_role(category: Category) -> ThemeRole {
    match category {
        Category::Code | Category::Markup | Category::Data | Category::Config | Category::Shell => {
            ThemeRole::FileIconText
        },
        Category::Image | Category::Document => ThemeRole::FileIconMedia,
        Category::Archive | Category::Binary => ThemeRole::FileIconBinary,
        // Unknown — and any future Category variant — stays neutral.
        _ => ThemeRole::Foreground,
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
            },
        };
        let fg = theme.role(ThemeRole::Foreground);
        let guide = theme.role(ThemeRole::IndentGuide);
        let muted = theme.role(ThemeRole::Muted);
        let accent = theme.role(ThemeRole::LineNumberActive);

        for (i, row) in state
            .rows
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(height)
        {
            let y = area.y + u16::try_from(i - state.offset).unwrap_or(0);
            let selected = state.selection.is_selected(i);
            // The primary Selection highlight wins over the secondary hover highlight.
            let row_bg = if selected {
                Some(ThemeRole::Selection)
            } else if self.hover == Some(i) {
                Some(ThemeRole::HoverHighlight)
            } else {
                None
            };
            if let Some(role) = row_bg {
                buf.set_style(
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    Style::default().bg(theme.role(role).to_ratatui()),
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

            // Foreground precedence: the active editor's file is accented and bold;
            // other open files are accented; gitignored entries recede to a readable
            // muted grey (VS Code style); everything else is normal.
            let is_active = self.active == Some(row.path.as_path());
            let is_open = self.open.iter().any(|p| p == &row.path);
            let (row_fg, label_style) = if is_active {
                (
                    accent,
                    Style::default()
                        .fg(accent.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )
            } else if is_open {
                (accent, Style::default().fg(accent.to_ratatui()))
            } else if row.ignored {
                (muted, Style::default().fg(muted.to_ratatui()))
            } else {
                (fg, Style::default().fg(fg.to_ratatui()))
            };
            // The type icon is tinted by file Category (text / media / binary /
            // neutral); directories follow the row color, and gitignored entries
            // recede to muted so the whole row dims together.
            let icon_color = if row.ignored {
                muted
            } else if row.is_dir {
                row_fg
            } else {
                theme.role(category_role(category_for_path(&row.path)))
            };
            // Indent guides: one vertical rule per ancestor depth level. Rows are
            // flattened depth-first, so a rule at each ancestor column draws a
            // continuous line down every expanded directory's children.
            let mut spans = Vec::with_capacity(row.depth as usize + 3);
            for _ in 0..row.depth {
                spans.push(Span::styled(
                    "\u{2502} ", // "│ " — box-drawing rule + spacer, 2 cells per level
                    Style::default().fg(guide.to_ratatui()),
                ));
            }
            spans.push(Span::styled(
                format!("{chev} "),
                Style::default().fg(row_fg.to_ratatui()),
            ));
            spans.push(Span::styled(
                format!("{icon} "),
                Style::default().fg(icon_color.to_ratatui()),
            ));
            spans.push(Span::styled(row.label.clone(), label_style));
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use karet_core::LineCol;
    use karet_core::Range;

    use super::*;

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
    fn gitignored_files_are_dimmed_not_hidden() {
        let dir = temp_dir();
        write(&dir.path, ".gitignore", b"ignored.txt\n");
        write(&dir.path, "kept.txt", b"k");
        write(&dir.path, "ignored.txt", b"i");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        // VS Code behavior: nothing is hidden (dotfiles shown too); the gitignored
        // file is listed but flagged for dimming.
        assert_eq!(names(&state), vec![".gitignore", "ignored.txt", "kept.txt"]);
        let ignored: Vec<String> = state
            .rows()
            .iter()
            .filter(|r| r.ignored)
            .map(|r| name_key(&r.path))
            .collect();
        assert_eq!(ignored, vec!["ignored.txt"]);
    }

    #[test]
    fn git_directory_is_always_excluded() {
        let dir = temp_dir();
        write(&dir.path, ".git/config", b"[core]\n");
        write(&dir.path, "src/main.rs", b"fn main() {}\n");
        let mut state = FileTreeState::new();
        state.ensure_built(&dir.path);
        assert!(!names(&state).contains(&".git".to_string()));
        assert!(names(&state).contains(&"src".to_string()));
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
    fn active_file_row_is_bold() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"a");
        let mut state = FileTreeState::new();
        let theme = Theme::dark();
        let active = dir.path.join("a.txt");
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        FileTree::new(&dir.path)
            .theme(&theme)
            .active(Some(&active))
            .render(area, &mut buf, &mut state);
        // The label starts at column 4 (2 chevron + 2 icon cells) and is bold.
        assert!(buf.content()[4].modifier.contains(Modifier::BOLD));

        // Without an active path, the same row is not bold.
        let mut plain = Buffer::empty(area);
        let mut state2 = FileTreeState::new();
        FileTree::new(&dir.path)
            .theme(&theme)
            .render(area, &mut plain, &mut state2);
        assert!(!plain.content()[4].modifier.contains(Modifier::BOLD));
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

    #[test]
    fn nested_rows_draw_indent_guides() {
        let dir = temp_dir();
        write(&dir.path, "sub/b.txt", b"b");
        let mut state = FileTreeState::new();
        let theme = Theme::dark();
        state.ensure_built(&dir.path);
        state.toggle(&dir.path.join("sub"));
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        FileTree::new(&dir.path)
            .theme(&theme)
            .render(area, &mut buf, &mut state);
        // Row 0 is the expanded `sub` (depth 0, no guide, ▼ chevron); row 1 is the
        // nested `b.txt` (depth 1), whose first cell is the box-drawing indent rule.
        let width = area.width as usize;
        assert_eq!(buf.content()[0].symbol(), "\u{25bc}"); // ▼ expanded directory
        assert_eq!(buf.content()[width].symbol(), "\u{2502}"); // │ indent guide
    }

    #[test]
    fn file_icons_are_tinted_by_category() {
        let dir = temp_dir();
        write(&dir.path, "main.rs", b"fn main() {}");
        let mut state = FileTreeState::new();
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        FileTree::new(&dir.path)
            .theme(&theme)
            .render(area, &mut buf, &mut state);
        // A code file's icon (column 2, after the blank chevron cells) is tinted with
        // the text-file role, not the neutral foreground.
        assert_eq!(
            buf.content()[2].fg,
            theme.role(ThemeRole::FileIconText).to_ratatui()
        );
    }
}
