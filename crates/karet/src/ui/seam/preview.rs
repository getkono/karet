//! The source preview: the lines that define the selection, with muted context.
//!
//! The pane's job is to answer one question before the reader spends a keystroke on it —
//! is pressing Enter worth it? A name and a kind do not answer that. The declaration does:
//! the signature with its parameters, the attribute above the item, its doc comment.
//!
//! So the **declaration head is never sacrificed**. Whatever else the budget goes to —
//! context, body, the elision marker — the head is painted first and in full, because a
//! signature cut after its second parameter has told the reader less than nothing.
//!
//! The block's height depends on the terminal and on nothing else. It does not follow the
//! selection: a pane that changes height as you arrow down makes you re-find the edge list
//! on every keystroke. Context that does not exist — at the top or the bottom of a file —
//! is reserved and left blank rather than closed up, for the same reason.

use karet_core::ThemeRole;
use karet_diff::TokenSpan;
use karet_filetype::IconStyle;
use karet_session::api::SeamPreview;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::PENDING;
use super::UNRESOLVABLE;
use crate::app::seam::SeamViewState;

/// Rows of context shown on each side of the node's own lines.
const CONTEXT: usize = 3;

/// The fewest rows worth giving the block: three of context each side and three of source.
pub(super) const MIN_HEIGHT: u16 = 9;

/// The most rows worth giving it.
///
/// Past this the pane is competing with the spine, which is the primary surface, and a
/// preview is a peek rather than an editor.
const MAX_HEIGHT: u16 = 16;

/// The narrowest gutter worth drawing, in digits.
const MIN_GUTTER: usize = 3;

/// The widest gutter worth drawing, in digits.
const MAX_GUTTER: usize = 6;

/// Cells a tab stands for, since a raw tab would paint as one cell and misalign the block.
const TAB: usize = 4;

/// The block's height for a view occupying `area`.
///
/// A function of the terminal, deliberately, and never of the selection: constant while
/// the reader arrows around, generous when there are rows to be generous with.
#[must_use]
pub(super) fn height(area: Rect) -> u16 {
    (area.height / 3).clamp(MIN_HEIGHT, MAX_HEIGHT)
}

/// Draw the preview block into `area`, filling exactly `area.height` rows.
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
    let budget = usize::from(area.height);
    f.render_widget(
        Paragraph::new(rows(theme, state, area.width, budget, icons)),
        area,
    );
}

/// The rows the block paints: exactly `budget` of them, whatever has been answered.
///
/// Split from [`draw`] so the row budget — the part that must not move — is testable
/// without a terminal.
#[must_use]
fn rows<'a>(
    theme: &Theme,
    state: &SeamViewState,
    width: u16,
    budget: usize,
    icons: IconStyle,
) -> Vec<Line<'a>> {
    match (&state.preview, state.detail_since) {
        (Some(Ok(preview)), _) => source_rows(theme, preview, width, budget, icons),
        (Some(Err(message)), _) => reserved(
            budget,
            Some(Span::styled(
                format!("{UNRESOLVABLE} {message}"),
                theme.style(ThemeRole::DiagnosticWarning),
            )),
        ),
        // Past the shared reveal delay and still unanswered.
        (None, Some(pending)) if pending.visible() => reserved(
            budget,
            Some(Span::styled(
                PENDING.to_owned(),
                theme.style(ThemeRole::Muted),
            )),
        ),
        // A fast path must never flash: nothing at all until the delay elapses.
        (None, _) => reserved(budget, None),
    }
}

/// A single row carrying `message` in the block's middle, the rest blank.
///
/// The shape every unanswered state takes, so "not yet", "never" and "nothing so far"
/// cannot differ in height from each other or from a real preview.
#[must_use]
fn reserved<'a>(budget: usize, message: Option<Span<'a>>) -> Vec<Line<'a>> {
    let middle = budget / 2;
    (0..budget)
        .map(|row| match (row == middle, &message) {
            (true, Some(span)) => Line::from(span.clone()),
            _ => Line::default(),
        })
        .collect()
}

/// How the budget is divided between context, the node, and the marker.
///
/// Pulled out of the painting so the one decision that matters — *does the whole
/// signature fit* — is testable as arithmetic rather than through a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Budget {
    /// Rows reserved above the node, blank where the file has no line to put there.
    lead: usize,
    /// Rows of the node's own text to paint, head first.
    shown: usize,
    /// Whether a marker row stands for what did not fit.
    elides: bool,
}

/// Divide `budget` rows between the node and its surroundings.
///
/// The rules, in order of who gives way to whom. The head is never cut while any row
/// remains — it is the reason the pane exists. Trailing context gives way to the head
/// before leading context does, because a signature reads downward, and it also gives way
/// to a node that is one row short of fitting, since a marker reading "1 more line" has
/// spent a row to say less than the line would have. Leading context gives way last, and
/// only when the head could not otherwise fit at all.
#[must_use]
fn divide(preview: &SeamPreview, budget: usize) -> Budget {
    let head = preview.head_end.saturating_sub(preview.body_start);
    let body = preview.body_end.saturating_sub(preview.body_start);
    let after = preview.lines.len().saturating_sub(preview.body_end);

    // The head, plus a row for the marker under it, is what the block must find room for.
    let lead = CONTEXT.min(budget.saturating_sub(head.saturating_add(1)));
    let for_node = budget
        .saturating_sub(lead)
        .saturating_sub(CONTEXT.min(after))
        .max(head.saturating_add(1).min(budget.saturating_sub(lead)))
        .max(1);

    // A marker that hides one line has cost a row to say less than the row would have.
    // When the node is that close to fitting, the trailing context gives up the row.
    let for_node = if preview.dropped == 0 && body <= for_node.saturating_add(1) {
        body.min(budget.saturating_sub(lead))
    } else {
        for_node
    };

    let elides = body > for_node || preview.dropped > 0;
    let shown = if elides {
        for_node.saturating_sub(1).max(1)
    } else {
        body
    };
    Budget {
        lead,
        shown: shown.min(body),
        elides,
    }
}

/// The rows of an answered preview.
#[must_use]
fn source_rows<'a>(
    theme: &Theme,
    preview: &SeamPreview,
    width: u16,
    budget: usize,
    icons: IconStyle,
) -> Vec<Line<'a>> {
    let gutter = gutter_width(preview);
    let indent = shared_indent(&preview.lines);
    let plan = divide(preview, budget);
    let mut lines = Vec::with_capacity(budget);
    let paint =
        |row: usize, body: bool| source_row(theme, preview, row, indent, gutter, width, body);

    // Leading context, reserved from the top so a node near line zero pushes nothing.
    let have = plan.lead.min(preview.body_start);
    for _ in 0..plan.lead - have {
        lines.push(Line::default());
    }
    for row in (preview.body_start - have)..preview.body_start {
        lines.push(paint(row, false));
    }

    // The node's own lines, head first — the signature is the informative part, and a
    // closing brace is not worth a row.
    for offset in 0..plan.shown {
        lines.push(paint(preview.body_start + offset, true));
    }
    if plan.elides {
        let hidden = (preview.body_end - preview.body_start - plan.shown)
            .saturating_add(preview.dropped as usize);
        lines.push(elision(theme, hidden, gutter, icons));
    }

    // Trailing context, from after the node ends rather than from its own continuation:
    // muting lines that belong to the node would misdescribe them.
    for row in preview.body_end..preview.lines.len() {
        if lines.len() >= budget {
            break;
        }
        lines.push(paint(row, false));
    }
    lines.resize(budget, Line::default());
    lines.truncate(budget);
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
    // Context recedes on weight as well as hue, since a single grey step is not much of a
    // signal in a pane where most rows are grey.
    let (base, gutter_style) = if body {
        (
            theme.style(ThemeRole::Foreground),
            theme.style(ThemeRole::LineNumber),
        )
    } else {
        let muted = theme.style(ThemeRole::Muted).add_modifier(Modifier::DIM);
        (muted, muted)
    };
    let mut spans = vec![Span::styled(format!("{number:>gutter$} "), gutter_style)];
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
    let digits = preview.last_line().saturating_add(1).to_string().len();
    digits.clamp(MIN_GUTTER, MAX_GUTTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preview whose node runs `body` lines from `body_start`, its head `head` of them.
    fn built(
        first_line: u32,
        body_start: usize,
        head: usize,
        body: usize,
        count: usize,
    ) -> SeamPreview {
        SeamPreview {
            file: std::path::PathBuf::from("src/lib.rs"),
            lines: (0..count).map(|n| format!("    line {n}")).collect(),
            numbers: (first_line..).take(count).collect(),
            body_start,
            head_end: body_start + head,
            body_end: body_start + body,
            dropped: 0,
            context: 3,
            tokens: Vec::new(),
        }
    }

    fn preview(first_line: u32, body_start: usize, body_end: usize, count: usize) -> SeamPreview {
        built(
            first_line,
            body_start,
            1.min(body_end - body_start),
            body_end - body_start,
            count,
        )
    }

    /// The text of every painted row, for whole-block assertions.
    fn painted(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_block_fills_its_budget_whatever_was_answered() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending(std::path::PathBuf::new());
        // Nothing answered, an error, a node mid-file, and a node at line zero all have
        // to occupy the same rows, or the pane below shifts as the selection moves.
        let answers = [
            None,
            Some(Err("gone".to_owned())),
            Some(Ok(preview(20, 3, 5, 9))),
            Some(Ok(preview(0, 0, 2, 6))),
            Some(Ok(built(20, 3, 6, 40, 46))),
        ];
        for budget in [usize::from(MIN_HEIGHT), 12, usize::from(MAX_HEIGHT)] {
            for answer in answers.clone() {
                state.preview = answer;
                let rows = rows(&theme, &state, 60, budget, IconStyle::Ascii);
                assert_eq!(rows.len(), budget, "budget {budget}");
            }
        }
    }

    #[test]
    fn the_pane_grows_with_the_terminal_and_stops() {
        assert_eq!(height(Rect::new(0, 0, 100, 24)), MIN_HEIGHT);
        assert_eq!(height(Rect::new(0, 0, 100, 40)), 13);
        assert_eq!(height(Rect::new(0, 0, 100, 200)), MAX_HEIGHT);
        // Never below the floor, however little there is.
        assert_eq!(height(Rect::new(0, 0, 100, 1)), MIN_HEIGHT);
    }

    #[test]
    fn a_wrapped_signature_is_painted_whole() {
        // The bug this pane had: a four-line signature showed two lines and an ellipsis.
        let theme = Theme::dark();
        let mut state = SeamViewState::pending(std::path::PathBuf::new());
        state.preview = Some(Ok(built(20, 3, 4, 40, 46)));
        let rows = painted(&rows(&theme, &state, 60, 13, IconStyle::Ascii));
        for offset in 0..4 {
            assert!(
                rows[3 + offset].contains(&format!("line {}", 3 + offset)),
                "head row {offset} missing from {rows:?}"
            );
        }
        let marker = rows.iter().position(|row| row.contains("more lines"));
        assert!(marker.is_some_and(|at| at >= 7), "{rows:?}");
    }

    #[test]
    fn a_head_taller_than_the_context_takes_the_context_s_rows() {
        // Nine rows cannot hold three of context, a six-line signature and a marker. The
        // signature wins: it is the only thing in the block a reader cannot reconstruct.
        let preview = built(20, 3, 6, 40, 46);
        let plan = divide(&preview, usize::from(MIN_HEIGHT));
        assert_eq!(plan.shown, 6);
        assert!(plan.elides);
        assert!(plan.lead < CONTEXT);
    }

    #[test]
    fn an_ordinary_node_keeps_its_three_rows_of_leading_context() {
        // The common case must not move: a one-line head leaves the lead untouched, so
        // the definition lands on the same row for every selection.
        for budget in [usize::from(MIN_HEIGHT), 13, usize::from(MAX_HEIGHT)] {
            let plan = divide(&built(20, 3, 1, 40, 46), budget);
            assert_eq!(plan.lead, CONTEXT, "budget {budget}");
        }
    }

    #[test]
    fn a_node_that_fits_is_painted_whole_with_no_marker() {
        let preview = built(20, 3, 1, 4, 10);
        let plan = divide(&preview, 13);
        assert_eq!(plan.shown, 4);
        assert!(!plan.elides);
    }

    #[test]
    fn a_bigger_budget_spends_the_extra_rows_on_the_definition() {
        let preview = built(20, 3, 1, 40, 46);
        let small = divide(&preview, usize::from(MIN_HEIGHT));
        let large = divide(&preview, usize::from(MAX_HEIGHT));
        assert!(large.shown > small.shown, "{small:?} vs {large:?}");
    }

    #[test]
    fn a_node_whose_lines_were_dropped_still_says_so() {
        // The fetch cap cut the middle out; the marker has to account for both the rows
        // the pane could not show and the lines the worker never sent.
        let theme = Theme::dark();
        let mut state = SeamViewState::pending(std::path::PathBuf::new());
        let mut preview = built(0, 3, 1, 200, 206);
        preview.dropped = 300;
        state.preview = Some(Ok(preview));
        let rows = painted(&rows(&theme, &state, 60, 13, IconStyle::Ascii));
        let marker = rows.iter().find(|row| row.contains("more lines"));
        assert_eq!(
            marker.map(String::as_str).map(str::trim),
            Some("... 494 more lines")
        );
    }

    #[test]
    fn context_the_file_does_not_have_is_reserved_blank() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending(std::path::PathBuf::new());
        state.preview = Some(Ok(preview(0, 0, 2, 6)));
        let rows = rows(
            &theme,
            &state,
            60,
            usize::from(MIN_HEIGHT),
            IconStyle::Ascii,
        );
        // Blank, and numberless: an absent line number is the honest rendering of an
        // absent line.
        for row in &rows[..CONTEXT] {
            assert_eq!(row.spans.len(), 0, "{row:?}");
        }
    }

    #[test]
    fn trailing_context_comes_from_after_the_node() {
        let theme = Theme::dark();
        let mut state = SeamViewState::pending(std::path::PathBuf::new());
        state.preview = Some(Ok(built(0, 3, 1, 4, 10)));
        let rows = painted(&rows(
            &theme,
            &state,
            60,
            usize::from(MIN_HEIGHT),
            IconStyle::Ascii,
        ));
        // Four body lines fit in the floor budget once the marker row is not spent on
        // hiding one of them, leaving two rows for what follows the node.
        assert!(rows[7].contains("line 7"), "{rows:?}");
        assert!(rows[8].contains("line 8"), "{rows:?}");
        assert!(
            !rows.iter().any(|row| row.contains("more lines")),
            "{rows:?}"
        );
    }

    #[test]
    fn a_marker_is_never_spent_to_hide_a_single_line() {
        // The floor budget leaves the node three rows. A four-line node showing three and
        // a marker has spent that marker to hide one line; showing all four says more for
        // the same rows.
        let plan = divide(&built(20, 3, 1, 4, 12), usize::from(MIN_HEIGHT));
        assert_eq!(plan.shown, 4);
        assert!(!plan.elides);
        // Two short is a genuine elision, and stays one.
        let plan = divide(&built(20, 3, 1, 5, 14), usize::from(MIN_HEIGHT));
        assert!(plan.elides);
        assert_eq!(plan.shown, 2);
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
