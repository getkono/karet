//! Painters for the changed-file index both commit layouts show: the summary
//! line, and one row per [`ChangedFileRow`].
//!
//! Split from `responsive.rs` so the layouts there keep their room under the
//! per-file code-line ceiling. The rows are the same in either layout — the wide
//! one pins them in a rail beside the diff, the stacked one lists them above it —
//! so both go through [`tree_line`] and cannot drift apart.

use karet_filetype::IconStyle;
use karet_filetype::chevron;

use super::*;
use crate::tab::ChangedFileRow;

/// Columns a row spends before its label: a leading space, the status glyph or
/// chevron, and the space after it.
const ROW_PREFIX: usize = 3;

/// Columns one level of nesting indents by.
const INDENT: usize = 2;

/// The rail's heading: how many files changed, and the totals across them.
pub(in crate::ui) fn file_summary_line(theme: &Theme, files: &[render::FileView]) -> Line<'static> {
    let label = theme.style(ThemeRole::LineNumberActive);
    let add = theme.style(ThemeRole::DiagnosticHint);
    let remove = theme.style(ThemeRole::DiagnosticError);
    let (added, removed) = files.iter().fold((0usize, 0usize), |(a, r), file| {
        let (next_a, next_r) = file.line_stats();
        (a + next_a, r + next_r)
    });
    Line::from(vec![
        Span::styled(
            format!(
                " {} file{} changed",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            label,
        ),
        Span::raw("   "),
        Span::styled(format!("+{added}"), add),
        Span::raw(" "),
        Span::styled(format!("\u{2212}{removed}"), remove),
    ])
}

/// Paint one tree row at `width`, whichever kind it is.
pub(in crate::ui) fn tree_line(
    theme: &Theme,
    files: &[render::FileView],
    row: &ChangedFileRow,
    width: u16,
    icons: IconStyle,
    selected: bool,
) -> Line<'static> {
    match row {
        ChangedFileRow::Dir {
            label,
            depth,
            added,
            removed,
            expanded,
            ..
        } => dir_index_line(
            theme,
            label,
            *depth,
            (*added, *removed),
            *expanded,
            icons,
            width,
        ),
        ChangedFileRow::File { file, label, depth } => files
            .get(*file)
            .map(|view| file_index_line(theme, view, label, *depth, width, selected))
            .unwrap_or_default(),
    }
}

/// A directory row: a chevron, the compacted `a/b/c` name, and the subtree's stats.
fn dir_index_line(
    theme: &Theme,
    label: &str,
    depth: u16,
    stats: (usize, usize),
    expanded: bool,
    icons: IconStyle,
    width: u16,
) -> Line<'static> {
    let fg = theme.style(ThemeRole::Foreground);
    let muted = theme.style(ThemeRole::Muted);
    let add = theme.style(ThemeRole::DiagnosticHint);
    let remove = theme.style(ThemeRole::DiagnosticError);
    let (added, removed) = stats;
    let indent = INDENT * usize::from(depth);
    let stats = format!("+{added} \u{2212}{removed}");
    let stats_width = UnicodeWidthStr::width(stats.as_str());
    let show_stats = usize::from(width) >= ROW_PREFIX + indent + 1 + stats_width + 4;
    let label_width = usize::from(width)
        .saturating_sub(ROW_PREFIX + indent + if show_stats { 1 + stats_width } else { 0 })
        .max(1);
    // Directory names truncate from the start like paths do: a compacted chain's
    // tail is the part that distinguishes it from its siblings.
    let label = truncate_start(label, label_width);
    let padding = if show_stats {
        usize::from(width).saturating_sub(
            ROW_PREFIX + indent + UnicodeWidthStr::width(label.as_str()) + stats_width,
        )
    } else {
        0
    };
    let mut spans = vec![
        Span::raw(format!(" {}", " ".repeat(indent))),
        Span::styled(format!("{} ", chevron(expanded, icons)), muted),
        Span::styled(label, fg),
    ];
    if show_stats {
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(format!("+{added}"), add));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("\u{2212}{removed}"), remove));
    }
    Line::from(spans)
}

/// A file row: its status glyph, its name, why it is folded by default, and its stats.
fn file_index_line(
    theme: &Theme,
    file: &render::FileView,
    label: &str,
    depth: u16,
    width: u16,
    selected: bool,
) -> Line<'static> {
    let fg = theme.style(ThemeRole::Foreground);
    let add = theme.style(ThemeRole::DiagnosticHint);
    let remove = theme.style(ThemeRole::DiagnosticError);
    let (glyph, role) = status_glyph(file.change.status);
    let (added, removed) = file.line_stats();
    let indent = INDENT * usize::from(depth);
    let stats = format!("+{added} \u{2212}{removed}");
    let stats_width = UnicodeWidthStr::width(stats.as_str());
    let show_stats = usize::from(width) >= ROW_PREFIX + indent + 1 + stats_width + 4;
    let fixed = ROW_PREFIX + indent + usize::from(show_stats);

    // Same precedence as the card header: the generated reason yields to the name.
    let reason = auto_collapse_label(file).map(|label| format!(" {label}"));
    let reason_width = reason.as_deref().map_or(0, UnicodeWidthStr::width);
    let show_reason =
        reason_width > 0 && usize::from(width) >= fixed + reason_width + REASON_MIN_PATH;
    let reason_width = if show_reason { reason_width } else { 0 };

    let name_width = usize::from(width)
        .saturating_sub(fixed + reason_width + if show_stats { stats_width } else { 0 })
        .max(1);
    let name = truncate_start(label, name_width);
    let padding = if show_stats {
        usize::from(width).saturating_sub(
            ROW_PREFIX
                + indent
                + UnicodeWidthStr::width(name.as_str())
                + reason_width
                + stats_width,
        )
    } else {
        0
    };
    let mut spans = vec![
        Span::raw(format!(" {}", " ".repeat(indent))),
        Span::styled(format!("{glyph} "), theme.style(role)),
        Span::styled(
            name,
            if selected {
                fg.add_modifier(Modifier::BOLD)
            } else {
                fg
            },
        ),
    ];
    if let Some(reason) = reason.filter(|_| show_reason) {
        spans.push(Span::styled(reason, theme.style(ThemeRole::Muted)));
    }
    if show_stats {
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(format!("+{added}"), add));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("\u{2212}{removed}"), remove));
    }
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()));
    }
    line
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::render::test_file_view;
    use crate::tab::changed_file_rows;

    fn text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// The rail's width in the wide layout is 31..=40, so that is what the rows
    /// have to stay inside.
    const RAIL: u16 = 31;

    #[test]
    fn a_directory_row_indents_and_carries_its_subtree_stats() {
        let theme = Theme::default();
        let files = [test_file_view("a/b/one.rs", "x\n", "y\n")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        let line = tree_line(&theme, &files, &rows[0], RAIL, IconStyle::Ascii, false);
        let painted = text(&line);
        assert!(painted.starts_with(" v a/b"), "{painted:?}");
        assert!(painted.ends_with("+1 \u{2212}1"), "{painted:?}");
        assert_eq!(UnicodeWidthStr::width(painted.as_str()), usize::from(RAIL));
    }

    /// A collapsed directory says so with its chevron, and still reports what it hides.
    #[test]
    fn a_collapsed_directory_row_turns_its_chevron() {
        let theme = Theme::default();
        let files = [test_file_view("a/one.rs", "x\n", "y\n")];
        let rows = changed_file_rows(&files, &BTreeSet::from([PathBuf::from("a")]));
        let painted = text(&tree_line(
            &theme,
            &files,
            &rows[0],
            RAIL,
            IconStyle::Ascii,
            false,
        ));
        assert!(painted.starts_with(" > a"), "{painted:?}");
    }

    /// A file row prints its name, not the path the tree already stated.
    #[test]
    fn a_file_row_prints_its_name_indented_under_its_directory() {
        let theme = Theme::default();
        let files = [test_file_view(
            "crates/karet/src/ui/commit.rs",
            "x\n",
            "y\n",
        )];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        let painted = text(&tree_line(
            &theme,
            &files,
            &rows[1],
            RAIL,
            IconStyle::Ascii,
            false,
        ));
        assert!(painted.starts_with("   M commit.rs"), "{painted:?}");
        assert!(!painted.contains("crates/"), "{painted:?}");
    }

    /// Every row fits the rail exactly, so no row can push the divider over.
    #[test]
    fn rows_never_exceed_the_width_they_are_given() {
        let theme = Theme::default();
        let files = [
            test_file_view("crates/karet/src/ui/commit/responsive.rs", "x\n", "y\n"),
            test_file_view("Cargo.lock", "x\n", "y\n"),
        ];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        for width in [13, RAIL, 40] {
            for row in &rows {
                let painted = text(&tree_line(
                    &theme,
                    &files,
                    row,
                    width,
                    IconStyle::Ascii,
                    false,
                ));
                assert!(
                    UnicodeWidthStr::width(painted.as_str()) <= usize::from(width),
                    "{painted:?} at width {width}"
                );
            }
        }
    }

    /// A machine-maintained file names why its card starts folded, when it fits.
    #[test]
    fn a_generated_file_row_names_its_reason() {
        let theme = Theme::default();
        let files = [test_file_view("Cargo.lock", "x\n", "y\n")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        let painted = text(&tree_line(
            &theme,
            &files,
            &rows[0],
            40,
            IconStyle::Ascii,
            false,
        ));
        assert!(painted.contains("(lockfile)"), "{painted:?}");
    }

    #[test]
    fn the_summary_counts_files_and_totals_their_lines() {
        let theme = Theme::default();
        let files = [
            test_file_view("a.rs", "x\n", "y\n"),
            test_file_view("b.rs", "x\n", "y\n"),
        ];
        let painted = text(&file_summary_line(&theme, &files));
        assert!(painted.contains("2 files changed"), "{painted:?}");
        assert!(painted.ends_with("+2 \u{2212}2"), "{painted:?}");
    }
}
