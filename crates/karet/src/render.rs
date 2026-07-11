//! Turning a `karet-vcs` [`FileChange`] into renderable diff lines.
//!
//! Each file is diffed with `karet-diff` and (optionally) syntax-highlighted with
//! `karet-treesitter` + `karet-syntax`; the per-line tokens are merged with the diff
//! background and intra-line change emphasis into ratatui [`Line`]s at draw time.
//! When no grammar is available the file renders as plaintext — no token colors, but
//! add/remove and intra-line emphasis still apply.

use karet_core::BytePos;
use karet_core::Span as ByteSpan;
use karet_core::TokenId;
use karet_diff::Cell;
use karet_diff::DiffLine;
use karet_diff::DiffOptions;
use karet_diff::FileDiff;
use karet_diff::HighlightedPair;
use karet_diff::LineKind;
use karet_diff::Segment;
use karet_diff::align_hunk;
use karet_diff::compute_highlights;
use karet_diff::diff_text;
use karet_syntax::LayeredHighlighter;
use karet_theme::Rgba;
use karet_theme::Theme;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredParser;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;
use karet_vcs::FileChange;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

/// Which Source-Control group a changed file belongs to, mirroring VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree (and untracked files): the working-tree changes.
    Working,
}

/// A syntax token run within a single line: a byte range and its color class.
struct LineToken {
    start: usize,
    end: usize,
    token: TokenId,
}

/// A changed file prepared for display: its diff plus per-line syntax tokens.
pub struct FileView {
    /// The originating VCS change (path, status, binary flag).
    pub change: FileChange,
    /// The Source-Control group this file belongs to.
    pub section: Section,
    /// The display language name (e.g. `"Rust"`, or `"plaintext"`).
    pub language: &'static str,
    diff: FileDiff,
    old_tokens: Vec<Vec<LineToken>>,
    new_tokens: Vec<Vec<LineToken>>,
}

impl FileView {
    /// Diff and (optionally) highlight `change`, tagging it with its `section`.
    pub fn new(change: FileChange, section: Section, syntax: bool) -> Self {
        let language = language_name_from_path(&change.path).unwrap_or("plaintext");
        let diff = diff_text(
            &change.old,
            &change.new,
            &DiffOptions {
                path_hint: Some(change.path.to_string_lossy().into_owned()),
                ..Default::default()
            },
        );
        let lang = if syntax {
            language_id_from_path(&change.path)
        } else {
            None
        };
        let old_tokens = line_tokens(&change.old, lang);
        let new_tokens = line_tokens(&change.new, lang);
        Self {
            change,
            section,
            language,
            diff,
            old_tokens,
            new_tokens,
        }
    }

    /// The 1-based line, in the file's *new* (current) text, of the first change
    /// in this diff: the first added line's position, or — for a pure removal —
    /// the new-side line the removal collapsed onto. `None` when the diff has no
    /// changed lines (e.g. a binary or unchanged file). Used to land the caret on
    /// the first change when opening the underlying file from a diff view.
    #[must_use]
    pub fn first_changed_line(&self) -> Option<u32> {
        for hunk in &self.diff.hunks {
            // Track the new-side line the walk sits at, so a removal (which has
            // no new-side number of its own) can report where it happened.
            let mut new_line = hunk.new_start;
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Add => return Some(line.new_lineno.unwrap_or(new_line).max(1)),
                    LineKind::Remove => return Some(new_line.max(1)),
                    LineKind::Context => {
                        new_line = line.new_lineno.map_or(new_line + 1, |n| n + 1);
                    },
                }
            }
        }
        None
    }

    /// The count of `(added, removed)` lines across this file's diff, for the commit
    /// view's per-file `+N −M` summary.
    #[must_use]
    pub fn line_stats(&self) -> (usize, usize) {
        let mut added = 0;
        let mut removed = 0;
        for hunk in &self.diff.hunks {
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Add => added += 1,
                    LineKind::Remove => removed += 1,
                    LineKind::Context => {},
                }
            }
        }
        (added, removed)
    }

    fn tokens_for(
        &self,
        kind: LineKind,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
    ) -> &[LineToken] {
        let (lineno, table) = match kind {
            LineKind::Add => (new_lineno, &self.new_tokens),
            _ => (old_lineno, &self.old_tokens),
        };
        lineno
            .and_then(|n| (n as usize).checked_sub(1))
            .and_then(|i| table.get(i))
            .map_or(&[][..], Vec::as_slice)
    }
}

/// Parse and highlight `content`, returning the syntax token runs for each line.
/// Returns an empty table (plaintext) when there is no grammar or parsing fails.
fn line_tokens(content: &str, lang: Option<LanguageId>) -> Vec<Vec<LineToken>> {
    let Some(lang) = lang.filter(|_| !content.is_empty()) else {
        return Vec::new();
    };
    // Layered, so a diff of a markdown file still colours its code fences.
    let highlights = (|| {
        let mut parser = LayeredParser::new();
        let tree = parser.parse(lang, content).ok()?;
        Some(LayeredHighlighter::new().highlight(&tree, content))
    })();
    let Some(highlights) = highlights else {
        return Vec::new();
    };

    let mut table = Vec::new();
    let mut line_start = 0usize;
    for line in content.split_inclusive('\n') {
        let line_end = line_start + line.len();
        let spans = highlights.spans_in(ByteSpan {
            start: BytePos(line_start),
            end: BytePos(line_end),
        });
        let toks = spans
            .iter()
            .filter_map(|s| {
                let start = s.span.start.0.max(line_start) - line_start;
                let end = s.span.end.0.min(line_end) - line_start;
                (end > start).then_some(LineToken {
                    start,
                    end,
                    token: s.token,
                })
            })
            .collect();
        table.push(toks);
        line_start = line_end;
    }
    table
}

/// Build the unified-view lines for `file`.
pub fn unified_lines(file: &FileView, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if file.change.is_binary {
        lines.push(Line::from(Span::styled(
            "  (binary file changed)",
            dim_style(theme),
        )));
        return lines;
    }
    for hunk in &file.diff.hunks {
        if let Some(scope) = &hunk.scope {
            lines.push(scope_line(scope, theme));
        }
        lines.push(header_line(&hunk.header, theme));

        let hl = &hunk.lines;
        let mut i = 0;
        while i < hl.len() {
            if hl[i].kind == LineKind::Context {
                lines.push(diff_line(file, theme, &hl[i], None));
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
            let paired = removes.len().min(adds.len());
            let pairs: Vec<HighlightedPair> = (0..paired)
                .map(|k| compute_highlights(&removes[k].content, &adds[k].content))
                .collect();
            for (k, dl) in removes.iter().enumerate() {
                let seg = pairs.get(k).map(|p| p.old_segments.as_slice());
                lines.push(diff_line(file, theme, dl, seg));
            }
            for (k, dl) in adds.iter().enumerate() {
                let seg = pairs.get(k).map(|p| p.new_segments.as_slice());
                lines.push(diff_line(file, theme, dl, seg));
            }
        }
    }
    lines
}

/// Build the side-by-side lines for `file` as aligned `(old, new)` columns.
pub fn side_by_side_lines(
    file: &FileView,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    if file.change.is_binary {
        left.push(Line::from(Span::styled(
            "  (binary file changed)",
            dim_style(theme),
        )));
        right.push(Line::default());
        return (left, right);
    }
    for hunk in &file.diff.hunks {
        if let Some(scope) = &hunk.scope {
            left.push(scope_line(scope, theme));
            right.push(scope_line(hunk.right_scope().unwrap_or(scope), theme));
        }
        left.push(header_line(&hunk.header, theme));
        right.push(header_line(&hunk.right_header(), theme));

        for row in align_hunk(&hunk.lines) {
            let pair = match (&row.left, &row.right) {
                (Some(l), Some(r)) if l.kind == LineKind::Remove && r.kind == LineKind::Add => {
                    Some(compute_highlights(&l.content, &r.content))
                },
                _ => None,
            };
            left.push(cell_line(
                file,
                theme,
                row.left.as_ref(),
                pair.as_ref().map(|p| p.old_segments.as_slice()),
            ));
            right.push(cell_line(
                file,
                theme,
                row.right.as_ref(),
                pair.as_ref().map(|p| p.new_segments.as_slice()),
            ));
        }
    }
    (left, right)
}

fn diff_line(
    file: &FileView,
    theme: &Theme,
    dl: &DiffLine,
    segments: Option<&[Segment]>,
) -> Line<'static> {
    let marker = match dl.kind {
        LineKind::Add => '+',
        LineKind::Remove => '-',
        LineKind::Context => ' ',
    };
    let lineno = match dl.kind {
        LineKind::Add => dl.new_lineno,
        _ => dl.old_lineno,
    };
    let tokens = file.tokens_for(dl.kind, dl.old_lineno, dl.new_lineno);
    let mut spans = vec![
        gutter_span(lineno, theme),
        Span::styled(
            marker.to_string(),
            color(marker_glyph_color(dl.kind, theme)),
        ),
    ];
    spans.extend(merge_line_spans(
        &dl.content,
        tokens,
        theme,
        base_bg(dl.kind, theme),
        segments,
    ));
    Line::from(spans)
}

fn cell_line(
    file: &FileView,
    theme: &Theme,
    cell: Option<&Cell>,
    segments: Option<&[Segment]>,
) -> Line<'static> {
    let Some(cell) = cell else {
        return Line::default();
    };
    let (old_lineno, new_lineno) = match cell.kind {
        LineKind::Add => (None, Some(cell.lineno)),
        _ => (Some(cell.lineno), None),
    };
    let tokens = file.tokens_for(cell.kind, old_lineno, new_lineno);
    let mut spans = vec![gutter_span(Some(cell.lineno), theme)];
    spans.extend(merge_line_spans(
        &cell.content,
        tokens,
        theme,
        base_bg(cell.kind, theme),
        segments,
    ));
    Line::from(spans)
}

/// Merge syntax foreground + diff background + intra-line emphasis for one line.
fn merge_line_spans(
    content: &str,
    tokens: &[LineToken],
    theme: &Theme,
    base: Option<Rgba>,
    segments: Option<&[Segment]>,
) -> Vec<Span<'static>> {
    let n = content.len();
    if n == 0 {
        return Vec::new();
    }
    let default_fg = theme.role(karet_core::ThemeRole::Foreground);

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
            .map_or(default_fg, |t| theme.color(t.token));
        let changed = segments.is_some_and(|segs| byte_changed(segs, a));
        let bg = match (base, changed) {
            (Some(bg), true) => Some(brighten(bg)),
            (Some(bg), false) => Some(bg),
            (None, true) => Some(brighten(theme.role(karet_core::ThemeRole::Selection))),
            (None, false) => None,
        };
        let mut style = color(fg);
        if let Some(bg) = bg {
            style = style.bg(bg.to_ratatui());
        }
        out.push(Span::styled(
            content.get(a..b).unwrap_or("").to_string(),
            style,
        ));
    }
    out
}

/// Overlay the selection background `bg` on the selected character columns of
/// `lines[start.0..=end.0]`. `start.1` is the first selected column on the first
/// line and `end.1` the exclusive end column on the last line; the lines between
/// are fully covered. Columns are character (not byte) offsets and clamp per line.
pub fn apply_selection(
    lines: &mut [Line<'static>],
    start: (usize, usize),
    end: (usize, usize),
    bg: Rgba,
) {
    let bg = bg.to_ratatui();
    for line_idx in start.0..=end.0 {
        let Some(line) = lines.get_mut(line_idx) else {
            break;
        };
        let from = if line_idx == start.0 { start.1 } else { 0 };
        let to = if line_idx == end.0 { end.1 } else { usize::MAX };
        if from < to {
            highlight_columns(line, from, to, bg);
        }
    }
}

/// Re-span `line` so the characters in `[from, to)` carry background `bg`, leaving
/// every other style untouched.
fn highlight_columns(line: &mut Line<'static>, from: usize, to: usize, bg: ratatui::style::Color) {
    let mut out: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    let mut col = 0usize;
    for span in line.spans.drain(..) {
        let style = span.style;
        let chars: Vec<char> = span.content.chars().collect();
        let (span_start, span_end) = (col, col + chars.len());
        col = span_end;

        let sel_start = from.max(span_start);
        let sel_end = to.min(span_end);
        if sel_start >= sel_end {
            out.push(Span::styled(span.content.into_owned(), style));
            continue;
        }
        let (lo, hi) = (sel_start - span_start, sel_end - span_start);
        if lo > 0 {
            out.push(Span::styled(chars[..lo].iter().collect::<String>(), style));
        }
        out.push(Span::styled(
            chars[lo..hi].iter().collect::<String>(),
            style.bg(bg),
        ));
        if hi < chars.len() {
            out.push(Span::styled(chars[hi..].iter().collect::<String>(), style));
        }
    }
    line.spans = out;
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

fn base_bg(kind: LineKind, theme: &Theme) -> Option<Rgba> {
    match kind {
        LineKind::Add => Some(theme.role(karet_core::ThemeRole::DiffAdded)),
        LineKind::Remove => Some(theme.role(karet_core::ThemeRole::DiffRemoved)),
        LineKind::Context => None,
    }
}

fn marker_glyph_color(kind: LineKind, theme: &Theme) -> Rgba {
    match kind {
        LineKind::Add => theme.role(karet_core::ThemeRole::DiagnosticHint),
        LineKind::Remove => theme.role(karet_core::ThemeRole::DiagnosticError),
        LineKind::Context => theme.role(karet_core::ThemeRole::LineNumber),
    }
}

fn brighten(c: Rgba) -> Rgba {
    Rgba {
        r: c.r.saturating_add(0x24),
        g: c.g.saturating_add(0x24),
        b: c.b.saturating_add(0x2c),
        a: c.a,
    }
}

fn gutter_span(lineno: Option<u32>, theme: &Theme) -> Span<'static> {
    let text = lineno.map_or_else(|| "    ".to_string(), |n| format!("{n:>4}"));
    Span::styled(
        format!("{text} "),
        color(theme.role(karet_core::ThemeRole::LineNumber)),
    )
}

fn header_line(header: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        header.to_string(),
        color(theme.role(karet_core::ThemeRole::DiagnosticInfo)),
    ))
}

fn scope_line(scope: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {scope}"),
        dim_style(theme).add_modifier(Modifier::ITALIC),
    ))
}

fn dim_style(theme: &Theme) -> Style {
    color(theme.role(karet_core::ThemeRole::LineNumberActive))
}

fn color(c: Rgba) -> Style {
    Style::default().fg(c.to_ratatui())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_vcs::StatusKind;

    use super::*;

    fn change(path: &str, old: &str, new: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: old.to_string(),
            new: new.to_string(),
        }
    }

    fn rendered_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn rust_file_is_highlighted_and_rendered() {
        let fv = FileView::new(
            change("src/main.rs", "fn a() {}\n", "fn b() {}\n"),
            Section::Working,
            true,
        );
        assert_eq!(fv.language, "Rust");
        let lines = unified_lines(&fv, &Theme::dark());
        let text = rendered_text(&lines);
        assert!(text.contains("fn a() {}"));
        assert!(text.contains("fn b() {}"));
        // The Rust grammar is compiled in, so syntax tokens were produced.
        assert!(fv.old_tokens.iter().any(|line| !line.is_empty()));
    }

    #[test]
    fn first_changed_line_points_at_the_first_addition() {
        // Lines 1-2 are context; line 3 changes ("c" → "x").
        let fv = FileView::new(
            change("notes.txt", "a\nb\nc\nd\n", "a\nb\nx\nd\n"),
            Section::Working,
            false,
        );
        assert_eq!(fv.first_changed_line(), Some(3));
    }

    #[test]
    fn first_changed_line_for_a_pure_removal_lands_where_it_collapsed() {
        // "b" (old line 2) is removed with nothing added: the new side collapses
        // onto line 2 ("c"), which is where the caret should land.
        let fv = FileView::new(
            change("notes.txt", "a\nb\nc\n", "a\nc\n"),
            Section::Working,
            false,
        );
        assert_eq!(fv.first_changed_line(), Some(2));
    }

    #[test]
    fn first_changed_line_is_none_when_nothing_changed() {
        let fv = FileView::new(
            change("notes.txt", "same\n", "same\n"),
            Section::Working,
            false,
        );
        assert_eq!(fv.first_changed_line(), None);
    }

    #[test]
    fn first_changed_line_clamps_an_emptied_file_to_line_one() {
        // Deleting every line leaves the new side empty (new_start 0): the caret
        // target still clamps to a valid 1-based line.
        let fv = FileView::new(change("notes.txt", "a\nb\n", ""), Section::Working, false);
        assert_eq!(fv.first_changed_line(), Some(1));
    }

    #[test]
    fn unknown_extension_falls_back_to_plaintext() {
        let fv = FileView::new(
            change("notes.unknownext", "alpha\n", "beta\n"),
            Section::Working,
            true,
        );
        assert_eq!(fv.language, "plaintext");
        assert!(fv.old_tokens.is_empty() && fv.new_tokens.is_empty());
        let text = rendered_text(&unified_lines(&fv, &Theme::dark()));
        assert!(text.contains("alpha") && text.contains("beta"));
    }

    #[test]
    fn syntax_disabled_produces_no_tokens() {
        let fv = FileView::new(
            change("src/main.rs", "fn a() {}\n", "fn b() {}\n"),
            Section::Working,
            false,
        );
        assert_eq!(fv.language, "Rust"); // label still shown
        assert!(fv.old_tokens.is_empty() && fv.new_tokens.is_empty());
    }

    #[test]
    fn side_by_side_columns_stay_aligned() {
        let fv = FileView::new(
            change("x.rs", "a\nb\nc\n", "a\nB\nc\n"),
            Section::Working,
            true,
        );
        let (left, right) = side_by_side_lines(&fv, &Theme::dark());
        assert_eq!(left.len(), right.len());
        assert!(!left.is_empty());
    }

    #[test]
    fn apply_selection_sets_background_on_selected_columns() {
        let theme = Theme::dark();
        let bg = theme.role(karet_core::ThemeRole::Selection);
        // Two spans so the selection straddles a span boundary.
        let mut lines = vec![Line::from(vec![Span::raw("abc"), Span::raw("def")])];
        apply_selection(&mut lines, (0, 1), (0, 4), bg);

        let want = bg.to_ratatui();
        let selected: String = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(want))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(selected, "bcd");
        // The untouched text is still all there.
        assert_eq!(rendered_text(&lines), "abcdef");
    }

    #[test]
    fn binary_change_shows_placeholder() {
        let mut c = change("img.png", "", "");
        c.is_binary = true;
        let text = rendered_text(&unified_lines(
            &FileView::new(c, Section::Working, true),
            &Theme::dark(),
        ));
        assert!(text.contains("binary"));
    }
}
