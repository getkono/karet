//! The source preview: the lines that define the selection, with muted context.
//!
//! The pane's job is to answer one question before the reader spends a keystroke on it —
//! is pressing Enter worth it? A name and a kind do not answer that. The attribute above
//! the item, its doc comment, or the thing it sits beside usually does.
//!
//! The block is a fixed nine rows whatever the node is. Context that does not exist — at
//! the top or the bottom of a file — is reserved and left blank rather than closed up,
//! because a pane that changes height as the selection moves makes the reader re-find the
//! edge list on every arrow key.

use karet_core::ThemeRole;
use karet_diff::TokenSpan;
use karet_filetype::IconStyle;
use karet_session::api::SeamPreview;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::PENDING;
use super::UNRESOLVABLE;
use crate::app::seam::SeamViewState;

/// Rows of context shown on each side of the node's own lines.
const CONTEXT: usize = 3;

/// Rows given to the node's own lines.
///
/// Fixed: a taller node is elided rather than allowed to change the pane's height as the
/// selection moves.
const BODY: usize = 3;

/// The block's height. Constant by construction — that is the property.
pub(super) const HEIGHT: u16 = (CONTEXT * 2 + BODY) as u16;

/// The narrowest gutter worth drawing, in digits.
const MIN_GUTTER: usize = 3;

/// The widest gutter worth drawing, in digits.
const MAX_GUTTER: usize = 6;

/// Cells a tab stands for, since a raw tab would paint as one cell and misalign the block.
const TAB: usize = 4;

/// Draw the preview block into `area`, which is always [`HEIGHT`] rows tall.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &SeamViewState,
    icons: IconStyle,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(Paragraph::new(rows(theme, state, area.width, icons)), area);
}

/// The rows the block paints: exactly [`HEIGHT`] of them, whatever has been answered.
///
/// Split from [`draw`] so the row budget — the part that must not move — is testable
/// without a terminal.
#[must_use]
fn rows<'a>(theme: &Theme, state: &SeamViewState, width: u16, icons: IconStyle) -> Vec<Line<'a>> {
    match (&state.preview, state.detail_since) {
        (Some(Ok(preview)), _) => source_rows(theme, preview, width, icons),
        (Some(Err(message)), _) => reserved(Some(Span::styled(
            format!("{UNRESOLVABLE} {message}"),
            theme.style(ThemeRole::DiagnosticWarning),
        ))),
        // Past the shared reveal delay and still unanswered.
        (None, Some(pending)) if pending.visible() => reserved(Some(Span::styled(
            PENDING.to_owned(),
            theme.style(ThemeRole::Muted),
        ))),
        // A fast path must never flash: nothing at all until the delay elapses.
        (None, _) => reserved(None),
    }
}

/// A single row carrying `message` in the block's middle, the rest blank.
///
/// The shape every unanswered state takes, so "not yet", "never" and "nothing so far"
/// cannot differ in height from each other or from a real preview.
#[must_use]
fn reserved<'a>(message: Option<Span<'a>>) -> Vec<Line<'a>> {
    let middle = usize::from(HEIGHT) / 2;
    (0..usize::from(HEIGHT))
        .map(|row| match (row == middle, &message) {
            (true, Some(span)) => Line::from(span.clone()),
            _ => Line::default(),
        })
        .collect()
}

/// The nine rows of an answered preview.
#[must_use]
fn source_rows<'a>(
    theme: &Theme,
    preview: &SeamPreview,
    width: u16,
    icons: IconStyle,
) -> Vec<Line<'a>> {
    let gutter = gutter_width(preview);
    let indent = shared_indent(&preview.lines);
    let head = preview.body_start.saturating_sub(CONTEXT);
    let body_rows = (preview.body_end - preview.body_start).min(BODY);
    let elided = preview.dropped > 0 || preview.body_end - preview.body_start > BODY;
    let tail_start = preview.body_end;

    let mut lines = Vec::with_capacity(usize::from(HEIGHT));

    // Leading context, reserved from the top so a node near line zero pushes nothing.
    let missing = CONTEXT.saturating_sub(preview.body_start);
    for _ in 0..missing {
        lines.push(Line::default());
    }
    for row in head..preview.body_start {
        lines.push(source_row(
            theme, preview, row, indent, gutter, width, false,
        ));
    }

    // The node's own lines: its head, since the signature is the informative part and a
    // closing brace is not worth a row.
    let shown = if elided {
        body_rows.min(BODY - 1)
    } else {
        body_rows
    };
    for offset in 0..shown {
        let row = preview.body_start + offset;
        lines.push(source_row(theme, preview, row, indent, gutter, width, true));
    }
    if elided {
        let hidden = (preview.body_end - preview.body_start - shown) + preview.dropped as usize;
        lines.push(elision(theme, hidden, gutter, icons));
    }
    for _ in lines.len()..CONTEXT + BODY {
        lines.push(Line::default());
    }

    // Trailing context, from after the node ends rather than from its own continuation:
    // muting lines that belong to the node would misdescribe them.
    for row in tail_start..(tail_start + CONTEXT).min(preview.lines.len()) {
        lines.push(source_row(
            theme, preview, row, indent, gutter, width, false,
        ));
    }
    lines.resize(usize::from(HEIGHT), Line::default());
    lines
}

/// The row standing in for the node's lines that did not fit.
#[must_use]
fn elision<'a>(theme: &Theme, hidden: usize, gutter: usize, icons: IconStyle) -> Line<'a> {
    let marker = match icons {
        IconStyle::Ascii => "...",
        IconStyle::NerdFont | IconStyle::Unicode => "\u{22ef}",
    };
    Line::from(vec![
        // Blank where a number would go: no single line was skipped, a run was.
        Span::raw(" ".repeat(gutter + 1)),
        Span::styled(
            format!("{marker} {hidden} more lines"),
            theme.style(ThemeRole::DiagnosticInfo),
        ),
    ])
}

/// One source row: a right-aligned line number, then the line itself.
#[must_use]
fn source_row<'a>(
    theme: &Theme,
    preview: &SeamPreview,
    row: usize,
    indent: usize,
    gutter: usize,
    width: u16,
    body: bool,
) -> Line<'a> {
    let Some(text) = preview.lines.get(row) else {
        return Line::default();
    };
    let number = preview.line_number(row).saturating_add(1);
    // The definition reads as the definition even in a file with no grammar: an
    // unhighlighted body line must never fall back to the colour its context wears.
    let base = if body {
        theme.style(ThemeRole::Foreground)
    } else {
        theme.style(ThemeRole::Muted)
    };
    let mut spans = vec![Span::styled(
        format!("{number:>gutter$} "),
        theme.style(ThemeRole::LineNumber),
    )];
    let room = usize::from(width).saturating_sub(gutter + 1);
    let cut = text.get(indent.min(text.len())..).unwrap_or("");
    let tokens = if body { preview.tokens_for(row) } else { &[] };
    spans.extend(code_spans(theme, cut, tokens, indent, base, room));
    Line::from(spans)
}

/// The spans for one line: one per token boundary, coloured from the theme's palette.
///
/// Context rows are handed no runs at all — muted context that competes with the
/// definition for attention is not context — and fall back to `base`, as does any line
/// the index could not highlight.
#[must_use]
fn code_spans<'a>(
    theme: &Theme,
    text: &str,
    tokens: &[TokenSpan],
    indent: usize,
    base: Style,
    room: usize,
) -> Vec<Span<'a>> {
    let plain = karet_widgets::text::fit_end(&expand_tabs(text), room);
    if tokens.is_empty() {
        return vec![Span::styled(plain, base)];
    }
    let mut spans = Vec::new();
    let mut used = 0usize;
    let mut cursor = 0usize;
    // Token offsets are bytes into the *undedented* line, so the shift is applied here
    // and tabs are expanded only once a run's own text has been sliced out.
    let push = |spans: &mut Vec<Span<'a>>, piece: &str, style: Style, used: &mut usize| {
        if *used >= room || piece.is_empty() {
            return;
        }
        let fitted = karet_widgets::text::fit_end(&expand_tabs(piece), room - *used);
        *used += karet_widgets::text::width(&fitted);
        spans.push(Span::styled(fitted, style));
    };
    for token in tokens {
        let start = token.start.saturating_sub(indent);
        let end = token.end.saturating_sub(indent);
        if end <= cursor || start >= text.len() {
            continue;
        }
        if let Some(gap) = text.get(cursor..start.min(text.len())) {
            push(
                &mut spans,
                gap,
                theme.style(ThemeRole::Foreground),
                &mut used,
            );
        }
        if let Some(run) = text.get(start.max(cursor)..end.min(text.len())) {
            push(&mut spans, run, theme.token_style(token.token), &mut used);
        }
        cursor = end.min(text.len());
    }
    if let Some(rest) = text.get(cursor..) {
        push(&mut spans, rest, base, &mut used);
    }
    if spans.is_empty() {
        spans.push(Span::styled(plain, base));
    }
    spans
}

/// Replace tabs with spaces, so a block indented with them still lines up.
#[must_use]
fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    text.replace('\t', &" ".repeat(TAB))
}

/// The longest whitespace prefix every non-blank line in the block shares, in bytes.
///
/// A method inside an `impl` starts several columns in, and in a pane this narrow that is
/// a sixth of the budget spent on nothing. Byte-wise, so the token-offset shift is exact:
/// every stripped byte is one byte, and whitespace carries no token.
#[must_use]
fn shared_indent(lines: &[String]) -> usize {
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0)
}

/// Digits the gutter needs for the largest line number in the block.
#[must_use]
fn gutter_width(preview: &SeamPreview) -> usize {
    let last = preview
        .line_number(preview.lines.len().saturating_sub(1))
        .saturating_add(1);
    let digits = last.to_string().len();
    digits.clamp(MIN_GUTTER, MAX_GUTTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(first_line: u32, body_start: usize, body_end: usize, count: usize) -> SeamPreview {
        SeamPreview {
            file: std::path::PathBuf::from("src/lib.rs"),
            first_line,
            lines: (0..count).map(|n| format!("    line {n}")).collect(),
            body_start,
            body_end,
            dropped: 0,
            context: 3,
            tokens: Vec::new(),
        }
    }

    #[test]
    fn the_block_is_always_the_same_height() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending();
        // Nothing answered, an error, a node mid-file, and a node at line zero all have
        // to occupy the same rows, or the pane below shifts as the selection moves.
        let heights = [
            None,
            Some(Err("gone".to_owned())),
            Some(Ok(preview(20, 3, 5, 9))),
            Some(Ok(preview(0, 0, 2, 6))),
        ];
        for answer in heights {
            state.preview = answer;
            let painted = rows(&theme, &state, 60, IconStyle::Ascii);
            assert_eq!(painted.len(), usize::from(HEIGHT));
        }
    }

    #[test]
    fn context_the_file_does_not_have_is_reserved_blank() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending();
        state.preview = Some(Ok(preview(0, 0, 2, 6)));
        let painted = rows(&theme, &state, 60, IconStyle::Ascii);
        // Blank, and numberless: an absent line number is the honest rendering of an
        // absent line.
        for row in &painted[..CONTEXT] {
            assert_eq!(row.spans.len(), 0, "{row:?}");
        }
    }

    #[test]
    fn a_node_longer_than_the_budget_says_how_many_lines_it_hid() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending();
        state.preview = Some(Ok(preview(0, 0, 40, 50)));
        let painted = rows(&theme, &state, 60, IconStyle::Ascii);
        let text: String = painted
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("more lines"), "{text}");
    }

    #[test]
    fn a_shared_indent_is_stripped_from_the_whole_block() {
        assert_eq!(
            shared_indent(&["    a".to_owned(), "      b".to_owned()]),
            4
        );
        // A blank line has no indent to contribute and must not collapse the block's.
        assert_eq!(
            shared_indent(&["    a".to_owned(), String::new(), "    b".to_owned()]),
            4
        );
        assert_eq!(shared_indent(&[]), 0);
    }

    #[test]
    fn the_gutter_never_shrinks_below_three_digits_or_grows_past_six() {
        assert_eq!(gutter_width(&preview(0, 0, 1, 2)), MIN_GUTTER);
        assert_eq!(gutter_width(&preview(9_999_999, 0, 1, 2)), MAX_GUTTER);
    }

    #[test]
    fn tabs_become_spaces_so_the_block_still_lines_up() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("plain"), "plain");
    }
}
