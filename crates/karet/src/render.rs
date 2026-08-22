//! Presenting a backend-prepared diff ([`PreparedChange`]) as ratatui lines.
//!
//! Diffing, syntax highlighting, and intra-line pairing all happen in the session
//! backend (off the UI thread); this module only maps the theme onto `karet-diff`'s
//! `view`-feature painters and caches the unified rows per theme snapshot.

use std::cell::RefCell;

use karet_core::TokenId;
use karet_diff::DiffPalette;
pub use karet_diff::pad_diff_lines;
use karet_session::PreparedChange;
use karet_theme::Rgba;
use karet_theme::Theme;
use ratatui::text::Line;

/// Which Source-Control group a changed file belongs to, mirroring VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree (and untracked files): the working-tree changes.
    Working,
}

/// A changed file ready for display: the backend-prepared diff plus a per-theme
/// cache of the assembled unified rows. (The Source-Control group lives on the
/// tab — [`crate::tab::TabKind::Diff`] — not the file.)
pub struct FileView {
    /// The prepared change delivered by the backend (identity, language, diff).
    pub change: PreparedChange,
    /// Unified rows cached per theme snapshot, so scrolling does not re-assemble
    /// spans every frame. Live theme reloads rebuild correctly.
    unified_cache: RefCell<Option<(Theme, Vec<Line<'static>>)>>,
    /// Whether the code-review store marks this file reviewed (see
    /// `crate::app::review`).
    pub reviewed: bool,
}

impl FileView {
    /// Wrap a backend-prepared change for display.
    pub fn new(change: PreparedChange) -> Self {
        Self {
            change,
            unified_cache: RefCell::new(None),
            reviewed: false,
        }
    }

    /// The display language name (e.g. `"Rust"`).
    #[must_use]
    pub fn language(&self) -> &str {
        &self.change.language
    }

    /// The 1-based first changed line in the file's new text (see
    /// [`karet_diff::PreparedDiff::first_changed_line`]).
    #[must_use]
    pub fn first_changed_line(&self) -> Option<u32> {
        self.change.diff.first_changed_line()
    }

    /// The `(added, removed)` line counts for the per-file `+N −M` summary.
    #[must_use]
    pub fn line_stats(&self) -> (usize, usize) {
        self.change.diff.line_stats()
    }
}

/// Build a test [`FileView`] by diffing two texts locally (plaintext, no tokens).
#[cfg(test)]
pub(crate) fn test_file_view(path: &str, old: &str, new: &str) -> FileView {
    let diff = karet_diff::diff_text(
        old,
        new,
        &karet_diff::DiffOptions {
            path_hint: Some(path.to_string()),
            ..Default::default()
        },
    );
    FileView::new(PreparedChange {
        path: std::path::PathBuf::from(path),
        old_path: None,
        status: karet_vcs::StatusKind::Modified,
        language: "plaintext".to_string(),
        diff: karet_diff::PreparedDiff::new(diff, Vec::new(), Vec::new()),
    })
}

/// Map `theme` onto the diff painter's color slots and run `paint` with it.
fn with_palette<R>(theme: &Theme, paint: impl FnOnce(&DiffPalette<'_>) -> R) -> R {
    let token_fg = |token: TokenId| theme.color(token).to_ratatui();
    let role = |role: karet_core::ThemeRole| theme.role(role).to_ratatui();
    let palette = DiffPalette {
        foreground: role(karet_core::ThemeRole::Foreground),
        added_bg: role(karet_core::ThemeRole::DiffAdded),
        added_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::DiffAdded)).to_ratatui(),
        removed_bg: role(karet_core::ThemeRole::DiffRemoved),
        removed_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::DiffRemoved)).to_ratatui(),
        plain_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::Selection)).to_ratatui(),
        add_marker: role(karet_core::ThemeRole::DiagnosticHint),
        remove_marker: role(karet_core::ThemeRole::DiagnosticError),
        gutter: role(karet_core::ThemeRole::LineNumber),
        header: role(karet_core::ThemeRole::DiagnosticInfo),
        dim: role(karet_core::ThemeRole::LineNumberActive),
        token_fg: &token_fg,
    };
    paint(&palette)
}

/// The brighter variant of a base color used for intra-line change emphasis.
fn brighten(c: Rgba) -> Rgba {
    Rgba {
        r: c.r.saturating_add(0x24),
        g: c.g.saturating_add(0x24),
        b: c.b.saturating_add(0x2c),
        a: c.a,
    }
}

/// Build (or fetch from the theme-keyed cache) the unified-view lines for `file`.
pub fn unified_lines(file: &FileView, theme: &Theme) -> Vec<Line<'static>> {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |(_, lines)| lines.clone())
}

/// Return the number of rows in the unified rendering without cloning those rows.
pub fn unified_line_count(file: &FileView, theme: &Theme) -> usize {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or(0, |(_, lines)| lines.len())
}

/// Return the widest unified diff row in terminal columns without cloning rows.
pub fn unified_max_width(file: &FileView, theme: &Theme) -> usize {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or(0, |(_, lines)| {
            lines.iter().map(crate::ui::line_width).max().unwrap_or(0)
        })
}

/// Clone only a requested window of the unified rendering.
pub fn unified_lines_window(
    file: &FileView,
    theme: &Theme,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |(_, lines)| {
            lines.iter().skip(start).take(count).cloned().collect()
        })
}

/// Build the side-by-side lines for `file` as aligned `(old, new)` columns.
pub fn side_by_side_lines(
    file: &FileView,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    with_palette(theme, |palette| {
        karet_diff::side_by_side_lines(&file.change.diff, palette)
    })
}

fn ensure_unified(file: &FileView, theme: &Theme) {
    if file
        .unified_cache
        .borrow()
        .as_ref()
        .is_some_and(|(cached_theme, _)| cached_theme == theme)
    {
        return;
    }
    let lines = with_palette(theme, |palette| {
        karet_diff::unified_lines(&file.change.diff, palette)
    });
    *file.unified_cache.borrow_mut() = Some((theme.clone(), lines));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn unified_lines_render_both_sides() {
        let fv = test_file_view("notes.txt", "before\n", "after\n");
        let text = rendered_text(&unified_lines(&fv, &Theme::dark()));
        assert!(text.contains("before") && text.contains("after"));
    }

    #[test]
    fn unified_max_width_matches_the_rendered_rows() {
        let fv = test_file_view("notes.txt", "short\n", "a much longer replacement\n");
        let theme = Theme::dark();
        let expected = unified_lines(&fv, &theme)
            .iter()
            .map(crate::ui::line_width)
            .max()
            .unwrap_or_default();
        assert_eq!(unified_max_width(&fv, &theme), expected);
    }

    #[test]
    fn unified_cache_returns_stable_rows() {
        let fv = test_file_view("notes.txt", "a\n", "b\n");
        let theme = Theme::dark();
        let first = unified_lines(&fv, &theme);
        assert_eq!(unified_line_count(&fv, &theme), first.len());
        assert_eq!(unified_lines(&fv, &theme), first);
    }

    #[test]
    fn side_by_side_columns_stay_aligned() {
        let fv = test_file_view("x.rs", "a\nb\nc\n", "a\nB\nc\n");
        let (left, right) = side_by_side_lines(&fv, &Theme::dark());
        assert_eq!(left.len(), right.len());
        assert!(!left.is_empty());
    }

    #[test]
    fn first_changed_line_points_at_the_first_addition() {
        let fv = test_file_view("notes.txt", "a\nb\nc\nd\n", "a\nb\nx\nd\n");
        assert_eq!(fv.first_changed_line(), Some(3));
    }
}
