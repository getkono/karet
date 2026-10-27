//! Go to Line: quick-open's `:` mode, and its `file:line[:col]` file mode.
//!
//! Both read their target with the `--goto` flag's parser
//! ([`crate::cli::split_goto_suffix`], [`crate::cli::parse_line_col`]), so a
//! position means the same thing typed in the picker as on the command line:
//! 1-based, `0` read as 1, column optional.
//!
//! Neither list is a fuzzy match of the whole query: the go-to-line row is
//! computed from what was typed, and a `main.rs:42` query ranks files by
//! `main.rs` alone. So these pickers are rebuilt on every edit and seeded with
//! the whole query as their [prefix](super::Picker::with_prefix), which the
//! picker never matches against.

use std::path::Path;
use std::path::PathBuf;

use karet_core::LineCol;
use karet_fuzzy::Matcher;

use super::Overlay;
use super::OverlayEvent;
use super::Picker;
use super::PickerKind;
use crate::cli;

/// The sigil that turns quick-open into Go to Line.
pub const GO_TO_LINE_PREFIX: &str = ":";

/// The active editor a go-to-line moves in.
#[derive(Clone, Copy, Debug)]
pub struct LineTarget<'a> {
    /// The open file.
    pub path: &'a Path,
    /// How many lines it holds (at least 1).
    pub lines: usize,
    /// The primary caret (0-based).
    pub caret: LineCol,
}

impl Overlay {
    /// Build Go to Line for `query` (which starts with the `:` sigil) over the
    /// active editor `target`, or over nothing when no text editor is active.
    ///
    /// The one row is computed from the query: with no number yet it says where
    /// the caret is and what range is valid, and accepting it does nothing; with
    /// `LINE[:COL]` it jumps there, a line past the end landing on the last one.
    #[must_use]
    pub fn go_to_line(query: &str, target: Option<LineTarget<'_>>) -> Self {
        let body = query.strip_prefix(GO_TO_LINE_PREFIX).unwrap_or(query);
        let row = line_row(body, target);
        Self::picker(
            PickerKind::GoToLine,
            Picker::new("Go to Line", vec![row]).with_prefix(query),
        )
    }

    /// Build quick-open over `(display, path)` pairs, in rank order, for
    /// `query`. A query ending in `:LINE[:COL]` lists the files matching the
    /// rest, each jumping to that position; any other query is an ordinary
    /// fuzzy file search.
    #[must_use]
    pub fn quick_open_for(files: Vec<(String, PathBuf)>, query: &str) -> Self {
        let (name, Some((line, col))) = cli::split_goto_suffix(query) else {
            let mut overlay = Self::quick_open(files);
            overlay.push_str(query);
            return overlay;
        };
        let position = LineCol::new(line - 1, col - 1);
        let labels: Vec<&str> = files.iter().map(|(label, _)| label.as_str()).collect();
        let order = Matcher::new().rank_indices(name, &labels);
        let mut files: Vec<Option<(String, PathBuf)>> = files.into_iter().map(Some).collect();
        let items = order
            .into_iter()
            .filter_map(|index| files.get_mut(index).and_then(Option::take))
            .map(|(label, path)| (label, OverlayEvent::AcceptLocation { path, position }))
            .collect();
        Self::picker(
            PickerKind::FileAtLine,
            Picker::new("Go to File at Line", items).with_prefix(query),
        )
    }
}

/// The file a quick-open row opens, whether or not it names a line.
pub(super) fn opened_file(event: &OverlayEvent) -> Option<&Path> {
    match event {
        OverlayEvent::AcceptFile(path) | OverlayEvent::AcceptLocation { path, .. } => Some(path),
        _ => None,
    }
}

/// Go to Line's single row for the text after the sigil.
fn line_row(body: &str, target: Option<LineTarget<'_>>) -> (String, OverlayEvent) {
    let Some(target) = target else {
        return (
            "Open a file to go to a line in it".to_string(),
            OverlayEvent::Close,
        );
    };
    let lines = target.lines.max(1);
    if body.trim().is_empty() {
        let label = format!(
            "Current line {}, column {} · type a line number between 1 and {lines}",
            target.caret.line.saturating_add(1),
            target.caret.col.saturating_add(1),
        );
        return (label, OverlayEvent::Close);
    }
    let Some((line, col)) = cli::parse_line_col(body) else {
        let label = format!("Not a line number · type LINE or LINE:COLUMN (1 to {lines})");
        return (label, OverlayEvent::Close);
    };
    let last = u32::try_from(lines).unwrap_or(u32::MAX);
    let line = line.min(last);
    let label = if col > 1 {
        format!("Go to line {line}, column {col}")
    } else {
        format!("Go to line {line}")
    };
    let accept = OverlayEvent::AcceptLocation {
        path: target.path.to_path_buf(),
        position: LineCol::new(line - 1, col - 1),
    };
    (label, accept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::QuickOpenMode;

    fn target(path: &Path) -> LineTarget<'_> {
        LineTarget {
            path,
            lines: 120,
            caret: LineCol::new(9, 2),
        }
    }

    fn jump(overlay: &Overlay) -> Option<(PathBuf, LineCol)> {
        match overlay.accept() {
            OverlayEvent::AcceptLocation { path, position } => Some((path, position)),
            _ => None,
        }
    }

    #[test]
    fn the_bare_sigil_says_where_the_caret_is_and_goes_nowhere() {
        let path = PathBuf::from("/w/a.rs");
        let overlay = Overlay::go_to_line(":", Some(target(&path)));
        assert_eq!(overlay.quick_open_mode(), Some(QuickOpenMode::Line));
        assert_eq!(overlay.query(), ":");
        assert_eq!(
            overlay.rows(),
            vec!["Current line 10, column 3 · type a line number between 1 and 120"]
        );
        assert!(matches!(overlay.accept(), OverlayEvent::Close));
    }

    #[test]
    fn a_line_and_column_jump_zero_based() {
        let path = PathBuf::from("/w/a.rs");
        let overlay = Overlay::go_to_line(":42", Some(target(&path)));
        assert_eq!(overlay.rows(), vec!["Go to line 42"]);
        assert_eq!(jump(&overlay), Some((path.clone(), LineCol::new(41, 0))));

        let overlay = Overlay::go_to_line(":42:7", Some(target(&path)));
        assert_eq!(overlay.rows(), vec!["Go to line 42, column 7"]);
        assert_eq!(jump(&overlay), Some((path, LineCol::new(41, 6))));
    }

    #[test]
    fn a_line_past_the_end_lands_on_the_last_and_says_so() {
        let path = PathBuf::from("/w/a.rs");
        let overlay = Overlay::go_to_line(":9000", Some(target(&path)));
        assert_eq!(overlay.rows(), vec!["Go to line 120"]);
        assert_eq!(jump(&overlay), Some((path, LineCol::new(119, 0))));
    }

    #[test]
    fn junk_or_no_editor_goes_nowhere() {
        let path = PathBuf::from("/w/a.rs");
        for query in [":x", ":-4", ":4:"] {
            let overlay = Overlay::go_to_line(query, Some(target(&path)));
            assert_eq!(
                overlay.rows(),
                vec!["Not a line number · type LINE or LINE:COLUMN (1 to 120)"],
                "{query:?}"
            );
            assert!(matches!(overlay.accept(), OverlayEvent::Close));
        }
        let overlay = Overlay::go_to_line(":12", None);
        assert_eq!(overlay.rows(), vec!["Open a file to go to a line in it"]);
        assert!(matches!(overlay.accept(), OverlayEvent::Close));
    }

    fn files() -> Vec<(String, PathBuf)> {
        ["src/main.rs", "src/lib.rs", "README.md"]
            .into_iter()
            .map(|name| (name.to_string(), PathBuf::from("/w").join(name)))
            .collect()
    }

    #[test]
    fn a_file_query_with_a_line_ranks_by_the_name_and_jumps_there() {
        let overlay = Overlay::quick_open_for(files(), "main:42:5");
        assert_eq!(overlay.quick_open_mode(), Some(QuickOpenMode::FileAtLine));
        assert_eq!(overlay.query(), "main:42:5");
        assert_eq!(overlay.rows(), vec!["src/main.rs"]);
        assert_eq!(
            jump(&overlay),
            Some((PathBuf::from("/w/src/main.rs"), LineCol::new(41, 4)))
        );
        assert_eq!(overlay.selected_file(), Some(Path::new("/w/src/main.rs")));
    }

    #[test]
    fn a_file_query_without_a_line_is_an_ordinary_search() {
        let overlay = Overlay::quick_open_for(files(), "lib");
        assert_eq!(overlay.quick_open_mode(), Some(QuickOpenMode::Files));
        assert_eq!(overlay.rows(), vec!["src/lib.rs"]);
        assert!(matches!(overlay.accept(), OverlayEvent::AcceptFile(_)));
        let overlay = Overlay::quick_open_for(files(), "");
        assert_eq!(overlay.rows().len(), 3);
    }

    #[test]
    fn the_name_still_filters_and_a_located_row_can_be_reselected() {
        let overlay = Overlay::quick_open_for(files(), "x:3");
        assert!(overlay.rows().is_empty(), "the name still filters");
        let mut overlay = Overlay::quick_open_for(files(), "README:3");
        overlay.select_file(Path::new("/w/README.md"));
        assert_eq!(overlay.selected_file(), Some(Path::new("/w/README.md")));
    }
}
