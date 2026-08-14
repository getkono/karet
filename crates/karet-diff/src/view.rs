//! Optional ratatui painters for a [`PreparedDiff`] (feature `view`).
//!
//! Turns prepared diff data into styled [`Line`]s — unified or side-by-side —
//! merging the syntax token foreground, the add/remove background, and the
//! intra-line change emphasis. Colors come from a caller-supplied
//! [`DiffPalette`], so this module carries no theme dependency: the consumer
//! maps its theme (or any fixed palette) onto the named slots.

use karet_core::TokenId;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::HighlightedPair;
use crate::align_hunk;
use crate::compute_highlights;
use crate::intraline::Segment;
use crate::model::DiffLine;
use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::model::LineKind;
use crate::prepared::PreparedDiff;
use crate::prepared::TokenSpan;

/// The color slots a diff painter draws from, plus the syntax token lookup.
///
/// The `*_emphasis_bg` slots are the brighter variants used for intra-line
/// changed runs; the caller derives them from its base colors however it likes.
pub struct DiffPalette<'a> {
    /// Default text foreground (used where no token run applies).
    pub foreground: Color,
    /// Background for added lines.
    pub added_bg: Color,
    /// Brighter background for the changed runs within an added line.
    pub added_emphasis_bg: Color,
    /// Background for removed lines.
    pub removed_bg: Color,
    /// Brighter background for the changed runs within a removed line.
    pub removed_emphasis_bg: Color,
    /// Emphasis background for changed runs on lines with no add/remove base.
    pub plain_emphasis_bg: Color,
    /// The `+` marker glyph color.
    pub add_marker: Color,
    /// The `-` marker glyph color.
    pub remove_marker: Color,
    /// The context marker / line-number gutter color.
    pub gutter: Color,
    /// The `@@ … @@` hunk header color.
    pub header: Color,
    /// Muted color for hunk scopes and the binary placeholder.
    pub dim: Color,
    /// Resolves a syntax token run to its foreground color.
    pub token_fg: &'a dyn Fn(TokenId) -> Color,
}

/// Build the unified-view lines for `prepared`.
#[must_use]
pub fn unified_lines(prepared: &PreparedDiff, palette: &DiffPalette<'_>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if prepared.is_binary() {
        lines.push(binary_placeholder(palette));
        lines.push(Line::default());
        return lines;
    }
    if prepared.diff.hunks.is_empty() {
        lines.push(placeholder(&empty_reason(&prepared.diff), palette));
        lines.push(Line::default());
        return lines;
    }
    let empty = Vec::new();
    for (hunk_index, hunk) in prepared.diff.hunks.iter().enumerate() {
        let intraline = prepared.intraline.get(hunk_index).unwrap_or(&empty);
        if let Some(scope) = &hunk.scope {
            lines.push(scope_line(scope, palette));
        }
        lines.push(header_line(&hunk.header, palette));

        let hl = &hunk.lines;
        let mut i = 0;
        while i < hl.len() {
            if hl[i].kind == LineKind::Context {
                lines.push(diff_line(prepared, palette, &hl[i], None));
                i += 1;
                continue;
            }
            // A run of removes followed by a run of adds; pair them for intra-line diff.
            let r_start = i;
            while i < hl.len() && hl[i].kind == LineKind::Remove {
                i += 1;
            }
            let r_end = i;
            while i < hl.len() && hl[i].kind == LineKind::Add {
                i += 1;
            }
            let removes = &hl[r_start..r_end];
            let adds = &hl[r_end..i];
            let pair_at = |row: usize| intraline.get(row).and_then(Option::as_ref);
            for (k, dl) in removes.iter().enumerate() {
                let seg = pair_at(r_start + k).map(|pair| pair.old_segments.as_slice());
                lines.push(diff_line(prepared, palette, dl, seg));
            }
            for (k, dl) in adds.iter().enumerate() {
                let seg = pair_at(r_end + k).map(|pair| pair.new_segments.as_slice());
                lines.push(diff_line(prepared, palette, dl, seg));
            }
        }
    }
    lines.push(Line::default());
    lines
}

/// Which hunk of `prepared` the unified view's display `row` falls in.
///
/// Mirrors [`unified_lines`]'s layout exactly (optional scope line, header,
/// then the hunk body). Rows past the last hunk (the trailing blank line) map
/// to the last hunk, so "the hunk at the viewport top" stays well-defined at
/// the end of the document; a binary or hunkless diff has no answer.
#[must_use]
pub fn unified_hunk_at_row(prepared: &PreparedDiff, row: usize) -> Option<usize> {
    hunk_at_row(prepared, row, |hunk| hunk.lines.len())
}

/// Which hunk of `prepared` the side-by-side view's display `row` falls in
/// (both panes share row geometry). Same conventions as
/// [`unified_hunk_at_row`].
#[must_use]
pub fn side_by_side_hunk_at_row(prepared: &PreparedDiff, row: usize) -> Option<usize> {
    hunk_at_row(prepared, row, |hunk| crate::align_hunk(&hunk.lines).len())
}

fn hunk_at_row(
    prepared: &PreparedDiff,
    row: usize,
    body_rows: impl Fn(&crate::Hunk) -> usize,
) -> Option<usize> {
    if prepared.is_binary() || prepared.diff.hunks.is_empty() {
        return None;
    }
    let mut next_row = 0usize;
    let mut found = 0;
    for (index, hunk) in prepared.diff.hunks.iter().enumerate() {
        if row < next_row {
            break;
        }
        found = index;
        next_row += usize::from(hunk.scope.is_some()) + 1 + body_rows(hunk);
    }
    Some(found)
}

/// Build the side-by-side lines for `prepared` as aligned `(old, new)` columns.
#[must_use]
pub fn side_by_side_lines(
    prepared: &PreparedDiff,
    palette: &DiffPalette<'_>,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    if prepared.is_binary() {
        left.push(binary_placeholder(palette));
        right.push(Line::default());
        left.push(Line::default());
        right.push(Line::default());
        return (left, right);
    }
    if prepared.diff.hunks.is_empty() {
        left.push(placeholder(&empty_reason(&prepared.diff), palette));
        right.push(Line::default());
        left.push(Line::default());
        right.push(Line::default());
        return (left, right);
    }
    for hunk in &prepared.diff.hunks {
        if let Some(scope) = &hunk.scope {
            left.push(scope_line(scope, palette));
            right.push(scope_line(hunk.right_scope().unwrap_or(scope), palette));
        }
        left.push(header_line(&hunk.header, palette));
        right.push(header_line(&hunk.right_header(), palette));

        for row in align_hunk(&hunk.lines) {
            let pair: Option<HighlightedPair> = match (&row.left, &row.right) {
                (Some(l), Some(r)) if l.kind == LineKind::Remove && r.kind == LineKind::Add => {
                    Some(compute_highlights(&l.content, &r.content))
                },
                _ => None,
            };
            left.push(cell_line(
                prepared,
                palette,
                row.left.as_ref(),
                pair.as_ref().map(|p| p.old_segments.as_slice()),
            ));
            right.push(cell_line(
                prepared,
                palette,
                row.right.as_ref(),
                pair.as_ref().map(|p| p.new_segments.as_slice()),
            ));
        }
    }
    left.push(Line::default());
    right.push(Line::default());
    (left, right)
}

/// Pad changed rows through `width` terminal cells with their add/remove background.
/// Context/header rows are left untouched.
pub fn pad_diff_lines(lines: &mut [Line<'static>], width: u16) {
    let width = usize::from(width);
    for line in lines {
        let Some(background) = line.style.bg else {
            continue;
        };
        let used = line
            .spans
            .iter()
            .map(|span| span.content.width())
            .sum::<usize>();
        if used < width {
            line.spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(background),
            ));
        }
    }
}

fn diff_line(
    prepared: &PreparedDiff,
    palette: &DiffPalette<'_>,
    dl: &DiffLine,
    segments: Option<&[Segment]>,
) -> Line<'static> {
    let (marker, marker_color) = match dl.kind {
        LineKind::Add => ('+', palette.add_marker),
        LineKind::Remove => ('-', palette.remove_marker),
        LineKind::Context => (' ', palette.gutter),
    };
    let tokens = prepared.tokens_for(dl.kind, dl.old_lineno, dl.new_lineno);
    let base = base_bg(dl.kind, palette);
    let mut spans = vec![
        gutter_span(dl.old_lineno, palette),
        gutter_span(dl.new_lineno, palette),
        Span::styled(marker.to_string(), Style::default().fg(marker_color)),
    ];
    spans.extend(merge_line_spans(
        &dl.content,
        tokens,
        palette,
        base,
        segments,
    ));
    diff_background_line(spans, base.map(|(bg, _)| bg))
}

fn cell_line(
    prepared: &PreparedDiff,
    palette: &DiffPalette<'_>,
    cell: Option<&crate::Cell>,
    segments: Option<&[Segment]>,
) -> Line<'static> {
    let Some(cell) = cell else {
        return Line::default();
    };
    let (old_lineno, new_lineno) = match cell.kind {
        LineKind::Add => (None, Some(cell.lineno)),
        _ => (Some(cell.lineno), None),
    };
    let tokens = prepared.tokens_for(cell.kind, old_lineno, new_lineno);
    let base = base_bg(cell.kind, palette);
    let mut spans = vec![gutter_span(Some(cell.lineno), palette)];
    spans.extend(merge_line_spans(
        &cell.content,
        tokens,
        palette,
        base,
        segments,
    ));
    diff_background_line(spans, base.map(|(bg, _)| bg))
}

/// Put the add/remove color on the line itself so render boundaries can extend it
/// through their viewport. More specific span backgrounds still win.
fn diff_background_line(spans: Vec<Span<'static>>, base: Option<Color>) -> Line<'static> {
    let mut line = Line::from(spans);
    if let Some(base) = base {
        line.style = Style::default().bg(base);
    }
    line
}

/// Merge syntax foreground + diff background + intra-line emphasis for one line.
/// `base` carries the line background and its brighter emphasis variant.
fn merge_line_spans(
    content: &str,
    tokens: &[TokenSpan],
    palette: &DiffPalette<'_>,
    base: Option<(Color, Color)>,
    segments: Option<&[Segment]>,
) -> Vec<Span<'static>> {
    let n = content.len();
    if n == 0 {
        return Vec::new();
    }

    // Cut at every token boundary and segment boundary.
    let mut bounds = vec![0usize, n];
    for t in tokens {
        bounds.push(t.start.min(n));
        bounds.push(t.end.min(n));
    }
    if let Some(segs) = segments {
        let mut b = 0usize;
        for s in segs {
            b = (b + s.text.len()).min(n);
            bounds.push(b);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut out = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b {
            continue;
        }
        let fg = tokens
            .iter()
            .find(|t| t.start <= a && a < t.end)
            .map_or(palette.foreground, |t| (palette.token_fg)(t.token));
        let changed = segments.is_some_and(|segs| byte_changed(segs, a));
        let bg = match (base, changed) {
            (Some((_, emphasis)), true) => Some(emphasis),
            (Some((bg, _)), false) => Some(bg),
            (None, true) => Some(palette.plain_emphasis_bg),
            (None, false) => None,
        };
        let mut style = Style::default().fg(fg);
        if let Some(bg) = bg {
            style = style.bg(bg);
        }
        out.push(Span::styled(
            content.get(a..b).unwrap_or("").to_string(),
            style,
        ));
    }
    out
}

/// Whether byte `pos` falls inside a changed [`Segment`].
fn byte_changed(segments: &[Segment], pos: usize) -> bool {
    let mut start = 0usize;
    for s in segments {
        let end = start + s.text.len();
        if pos < end {
            return s.changed;
        }
        start = end;
    }
    false
}

/// The `(background, emphasis background)` pair for a line kind, if any.
fn base_bg(kind: LineKind, palette: &DiffPalette<'_>) -> Option<(Color, Color)> {
    match kind {
        LineKind::Add => Some((palette.added_bg, palette.added_emphasis_bg)),
        LineKind::Remove => Some((palette.removed_bg, palette.removed_emphasis_bg)),
        LineKind::Context => None,
    }
}

fn gutter_span(lineno: Option<u32>, palette: &DiffPalette<'_>) -> Span<'static> {
    let text = lineno.map_or_else(|| "    ".to_string(), |n| format!("{n:>4}"));
    Span::styled(format!("{text} "), Style::default().fg(palette.gutter))
}

fn header_line(header: &str, palette: &DiffPalette<'_>) -> Line<'static> {
    Line::from(Span::styled(
        header.to_string(),
        Style::default().fg(palette.header),
    ))
}

fn scope_line(scope: &str, palette: &DiffPalette<'_>) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {scope}"),
        Style::default()
            .fg(palette.dim)
            .add_modifier(Modifier::ITALIC),
    ))
}

fn binary_placeholder(palette: &DiffPalette<'_>) -> Line<'static> {
    placeholder("binary file changed", palette)
}

fn placeholder(text: &str, palette: &DiffPalette<'_>) -> Line<'static> {
    Line::from(Span::styled(
        format!("  ({text})"),
        Style::default().fg(palette.dim),
    ))
}

/// Why a file has no hunks to paint.
///
/// A mode-only change, a pure rename or copy, an empty new file, and an unmerged
/// path all reach the painters with nothing to show. Saying so beats painting a
/// blank pane, which reads as a failure rather than as "there is nothing here".
fn empty_reason(file: &FileDiff) -> String {
    let mut notes: Vec<String> = Vec::new();
    match file.status {
        FileStatus::Renamed { .. } => notes.push("renamed, contents identical".to_string()),
        FileStatus::Copied { .. } => notes.push("copied, contents identical".to_string()),
        FileStatus::Unmerged => notes.push("unresolved merge conflict".to_string()),
        FileStatus::Added => notes.push("empty file added".to_string()),
        FileStatus::Removed => notes.push("empty file removed".to_string()),
        _ => {},
    }
    if file.mode_changed() {
        let old = file.old_mode.unwrap_or_default();
        let new = file.new_mode.unwrap_or_default();
        notes.push(format!("file mode {old:06o} → {new:06o}"));
    }
    if notes.is_empty() {
        return "no content changes".to_string();
    }
    notes.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffOptions;
    use crate::diff_text;

    fn palette(token_fg: &dyn Fn(TokenId) -> Color) -> DiffPalette<'_> {
        DiffPalette {
            foreground: Color::White,
            added_bg: Color::Green,
            added_emphasis_bg: Color::LightGreen,
            removed_bg: Color::Red,
            removed_emphasis_bg: Color::LightRed,
            plain_emphasis_bg: Color::Blue,
            add_marker: Color::Green,
            remove_marker: Color::Red,
            gutter: Color::DarkGray,
            header: Color::Cyan,
            dim: Color::Gray,
            token_fg,
        }
    }

    fn prepared(old: &str, new: &str) -> PreparedDiff {
        PreparedDiff::new(
            diff_text(old, new, &DiffOptions::default()),
            Vec::new(),
            Vec::new(),
        )
    }

    fn rendered_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn hunk_at_row_mirrors_the_painted_layout() {
        // Two hunks: change line 1 and line 9 of a 9-line file (context split).
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\n";
        let new = "A\nb\nc\nd\ne\nf\ng\nh\nI\n";
        let prep = prepared(old, new);
        assert_eq!(prep.diff.hunks.len(), 2, "context split produces two hunks");
        // Row 0 is the first hunk header; its body follows.
        assert_eq!(unified_hunk_at_row(&prep, 0), Some(0));
        let token_fg = |_: TokenId| Color::White;
        let total = unified_lines(&prep, &palette(&token_fg)).len();
        // The final row (trailing blank) still answers with the last hunk.
        assert_eq!(unified_hunk_at_row(&prep, total - 1), Some(1));
        assert_eq!(side_by_side_hunk_at_row(&prep, 0), Some(0));
        assert_eq!(side_by_side_hunk_at_row(&prep, usize::MAX), Some(1));
        // A binary diff has no hunks to answer with.
        let mut binary_diff = diff_text("a\n", "b\n", &DiffOptions::default());
        binary_diff.is_binary = true;
        binary_diff.hunks.clear();
        let binary = PreparedDiff::new(binary_diff, Vec::new(), Vec::new());
        assert_eq!(unified_hunk_at_row(&binary, 0), None);
    }

    #[test]
    fn unified_lines_render_both_sides_and_end_with_one_empty_line() {
        let token_fg = |_: TokenId| Color::White;
        let lines = unified_lines(&prepared("before\n", "after\n"), &palette(&token_fg));
        let text = rendered_text(&lines);
        assert!(text.contains("before") && text.contains("after"));
        assert!(lines.last().is_some_and(|line| line.spans.is_empty()));
        assert!(
            lines
                .get(lines.len().saturating_sub(2))
                .is_some_and(|line| !line.spans.is_empty())
        );
    }

    #[test]
    fn changed_lines_carry_their_base_background_on_the_line_style() {
        let token_fg = |_: TokenId| Color::White;
        let lines = unified_lines(&prepared("a\n", "b\n"), &palette(&token_fg));
        assert!(lines.iter().any(|l| l.style.bg == Some(Color::Red)));
        assert!(lines.iter().any(|l| l.style.bg == Some(Color::Green)));
    }

    #[test]
    fn token_runs_color_the_foreground() {
        let token_fg = |_: TokenId| Color::Magenta;
        let diff = diff_text("old\n", "new\n", &DiffOptions::default());
        let p = PreparedDiff::new(
            diff,
            vec![vec![TokenSpan {
                start: 0,
                end: 3,
                token: TokenId(1),
            }]],
            Vec::new(),
        );
        let lines = unified_lines(&p, &palette(&token_fg));
        let magenta = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.fg == Some(Color::Magenta) && s.content.contains("old"));
        assert!(magenta);
    }

    #[test]
    fn side_by_side_columns_stay_aligned() {
        let token_fg = |_: TokenId| Color::White;
        let (left, right) =
            side_by_side_lines(&prepared("a\nb\nc\n", "a\nB\nc\n"), &palette(&token_fg));
        assert_eq!(left.len(), right.len());
        assert!(!left.is_empty());
    }

    #[test]
    fn binary_diff_shows_placeholder() {
        let token_fg = |_: TokenId| Color::White;
        let mut p = prepared("", "");
        p.diff.is_binary = true;
        let text = rendered_text(&unified_lines(&p, &palette(&token_fg)));
        assert!(text.contains("binary"));
    }

    #[test]
    fn hunkless_diffs_say_why_instead_of_painting_nothing() {
        let token_fg = |_: TokenId| Color::White;
        let palette = palette(&token_fg);

        // A pure rename: identical contents, so there is nothing to diff.
        let mut renamed = prepared("", "");
        renamed.diff.status = FileStatus::Renamed { similarity: 100 };
        let text = rendered_text(&unified_lines(&renamed, &palette));
        assert!(text.contains("renamed, contents identical"), "{text:?}");

        // A chmod: no hunks at all, but the mode is the story.
        let mut chmod = prepared("", "");
        chmod.diff.old_mode = Some(0o100644);
        chmod.diff.new_mode = Some(0o100755);
        let text = rendered_text(&unified_lines(&chmod, &palette));
        assert!(text.contains("file mode 100644 → 100755"), "{text:?}");

        // Both at once are reported together.
        let mut both = prepared("", "");
        both.diff.status = FileStatus::Copied { similarity: 100 };
        both.diff.old_mode = Some(0o100644);
        both.diff.new_mode = Some(0o100755);
        let text = rendered_text(&unified_lines(&both, &palette));
        assert!(text.contains("copied, contents identical"), "{text:?}");
        assert!(text.contains("file mode"), "{text:?}");

        // Side-by-side gets the same treatment, on the left column.
        let (left, _) = side_by_side_lines(&renamed, &palette);
        assert!(rendered_text(&left).contains("renamed"));
    }

    #[test]
    fn an_unmerged_file_is_labelled_not_blank() {
        let token_fg = |_: TokenId| Color::White;
        let mut p = prepared("", "");
        p.diff.status = FileStatus::Unmerged;
        let text = rendered_text(&unified_lines(&p, &palette(&token_fg)));
        assert!(text.contains("unresolved merge conflict"), "{text:?}");
    }

    #[test]
    fn pad_diff_lines_extends_only_background_rows() {
        let token_fg = |_: TokenId| Color::White;
        let mut lines = unified_lines(&prepared("a\n", "b\n"), &palette(&token_fg));
        pad_diff_lines(&mut lines, 40);
        for line in &lines {
            let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
            if line.style.bg.is_some() {
                assert!(used >= 40);
            }
        }
    }
}
