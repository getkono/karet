use karet_core::BytePos;
use karet_core::HighlightSpan;
use karet_core::Span;

use super::*;

const PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];

fn rules(highlights: Option<&Highlights>) -> AutoPairs<'_> {
    AutoPairs {
        pairs: PAIRS,
        brackets: AutoClose::LanguageDefined,
        quotes: AutoClose::LanguageDefined,
        highlights,
    }
}

/// A buffer and cursor set from `marked`, where `|` marks a caret, `«`…`»` a
/// selection whose head is its end, and `‹`…`›` one whose head is its start.
fn parse(marked: &str) -> (TextBuffer, CursorState) {
    let (mut text, mut selections) = (String::new(), Vec::new());
    let (mut line, mut col) = (0u32, 0u32);
    let mut open: Option<LineCol> = None;
    for c in marked.chars() {
        let here = LineCol::new(line, col);
        match c {
            '|' => selections.push(Selection::caret(here)),
            '«' | '‹' => open = Some(here),
            '»' => selections.push(Selection {
                anchor: open.take().unwrap_or(here),
                head: here,
            }),
            '›' => selections.push(Selection {
                anchor: here,
                head: open.take().unwrap_or(here),
            }),
            _ => {
                text.push(c);
                if c == '\n' {
                    line += 1;
                    col = 0;
                } else {
                    col += 1;
                }
            },
        }
    }
    let cursors = CursorState {
        selections,
        primary: 0,
    };
    (TextBuffer::from_text(&text), cursors)
}

/// `buffer` after `edit`, with `cursors` marked as [`parse`] reads them.
fn render(mut buffer: TextBuffer, edit: Option<&MultiEdit>, cursors: &CursorState) -> String {
    if let Some(edit) = edit {
        assert!(
            buffer.apply_simple(&edit.change).is_ok(),
            "the batch applies"
        );
    }
    let text = buffer.text_snapshot().to_string();
    let mut marks: Vec<(LineCol, &str)> = Vec::new();
    for s in &cursors.selections {
        if s.is_empty() {
            marks.push((s.head, "|"));
        } else if s.anchor < s.head {
            marks.push((s.anchor, "«"));
            marks.push((s.head, "»"));
        } else {
            marks.push((s.head, "‹"));
            marks.push((s.anchor, "›"));
        }
    }
    let mut out = String::new();
    let (mut line, mut col) = (0u32, 0u32);
    for c in text.chars().chain(std::iter::once('\u{0}')) {
        for (_, mark) in marks
            .iter()
            .filter(|(at, _)| *at == LineCol::new(line, col))
        {
            out.push_str(mark);
        }
        if c == '\u{0}' {
            break;
        }
        out.push(c);
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    out
}

/// Type `ch` into `before` (no tracked closers) and render the result, or
/// `"plain"` when the auto-pair logic leaves it to ordinary typing.
fn typed(before: &str, ch: char) -> String {
    typed_with(before, ch, &rules(None)).0
}

fn typed_with(before: &str, ch: char, rules: &AutoPairs<'_>) -> (String, Vec<LineCol>) {
    let (buffer, cursors) = parse(before);
    match type_char(&buffer, &cursors, 0, ch, rules, &[]) {
        Some(t) => (render(buffer, t.edit.as_ref(), &t.cursors), t.closers),
        None => ("plain".to_string(), Vec::new()),
    }
}

#[test]
fn an_opener_before_whitespace_or_a_closer_inserts_its_pair() {
    assert_eq!(typed("f|", '('), "f(|)");
    assert_eq!(typed("f| x", '['), "f[|] x");
    assert_eq!(typed("(|)", '{'), "({|})");
    assert_eq!(typed("a = |;", '"'), "a = \"|\";");
}

#[test]
fn an_opener_before_a_word_types_alone() {
    assert_eq!(typed("|foo", '('), "plain");
    assert_eq!(typed("|foo", '"'), "plain");
}

#[test]
fn a_quote_after_a_word_character_types_alone() {
    assert_eq!(typed("don|", '\''), "plain");
    assert_eq!(typed("x_|", '"'), "plain");
}

#[test]
fn closing_reports_the_closer_it_inserted() {
    let (out, closers) = typed_with("ab|", '(', &rules(None));
    assert_eq!(out, "ab(|)");
    assert_eq!(closers, vec![LineCol::new(0, 3)]);
}

#[test]
fn never_and_always_override_the_language_rules() {
    let never = AutoPairs {
        brackets: AutoClose::Never,
        ..rules(None)
    };
    assert_eq!(typed_with("f|", '(', &never).0, "plain");
    assert_eq!(typed_with("f |", '"', &never).0, "f \"|\"");
    let always = AutoPairs {
        quotes: AutoClose::Always,
        brackets: AutoClose::Always,
        ..rules(None)
    };
    assert_eq!(typed_with("|foo", '(', &always).0, "(|)foo");
    assert_eq!(typed_with("don|", '\'', &always).0, "don'|'");
}

#[test]
fn a_language_without_a_pair_never_closes_it() {
    let rust = AutoPairs {
        pairs: &[('(', ')'), ('"', '"')],
        ..rules(None)
    };
    assert_eq!(typed_with("fn f<|>", '\'', &rust).0, "plain");
    assert_eq!(typed_with("fn f<|>", '(', &rust).0, "fn f<(|)>");
}

/// Highlights marking each `(start, end)` byte range with `token`.
fn spans(ranges: &[(usize, usize, StandardToken)]) -> Highlights {
    Highlights::from_sorted_spans(
        ranges
            .iter()
            .map(|&(start, end, token)| HighlightSpan {
                span: Span {
                    start: BytePos(start),
                    end: BytePos(end),
                },
                token: token.id(),
            })
            .collect(),
    )
}

#[test]
fn a_quote_inside_a_string_or_comment_types_alone() {
    // `x = "ab cd"` with the string at bytes 4..11; a caret before ` cd`.
    let string = spans(&[(4, 11, StandardToken::String)]);
    assert_eq!(
        typed_with("x = \"ab| cd\"", '\'', &rules(Some(&string))).0,
        "plain"
    );
    // Just past the closing quote is outside the string again.
    assert_eq!(
        typed_with("x = \"ab cd\"|", '"', &rules(Some(&string))).0,
        "x = \"ab cd\"\"|\""
    );
    let comment = spans(&[(0, 7, StandardToken::Comment)]);
    assert_eq!(
        typed_with("// it |", '"', &rules(Some(&comment))).0,
        "plain"
    );
    // Brackets still close in a comment.
    assert_eq!(
        typed_with("// it |", '(', &rules(Some(&comment))).0,
        "// it (|)"
    );
}

#[test]
fn typing_a_tracked_closer_steps_over_it() -> Result<(), &'static str> {
    let (buffer, cursors) = parse("f(|)");
    let closers = [LineCol::new(0, 2)];
    let t = type_char(&buffer, &cursors, 0, ')', &rules(None), &closers).ok_or("typed")?;
    assert!(t.edit.is_none(), "stepping over changes no text");
    assert_eq!(render(buffer, None, &t.cursors), "f()|");
    assert!(t.closers.is_empty(), "the closer was consumed");
    Ok(())
}

#[test]
fn an_untracked_closer_is_typed_not_stepped_over() {
    assert_eq!(typed("f(|)", ')'), "plain");
}

#[test]
fn tracked_closers_move_with_the_text_typed_before_them() -> Result<(), &'static str> {
    // `(` then `[` inside: both closers stay tracked and shift right.
    let (buffer, cursors) = parse("(|)");
    let t = type_char(
        &buffer,
        &cursors,
        0,
        '[',
        &rules(None),
        &[LineCol::new(0, 1)],
    )
    .ok_or("typed")?;
    assert_eq!(render(buffer, t.edit.as_ref(), &t.cursors), "([|])");
    assert_eq!(t.closers, vec![LineCol::new(0, 2), LineCol::new(0, 3)]);
    Ok(())
}

#[test]
fn an_opener_wraps_every_selection_and_keeps_it_selected() {
    assert_eq!(typed("a «bc» d", '('), "a («bc») d");
    assert_eq!(typed("‹bc›", '"'), "\"‹bc›\"");
    assert_eq!(typed("«ab\nc»", '['), "[«ab\nc»]");
    // A character that opens nothing replaces the selection.
    assert_eq!(typed("«ab»", 'x'), "plain");
}

#[test]
fn every_caret_takes_part_and_mixed_plans_compose() {
    assert_eq!(typed("a|\nb|", '('), "a(|)\nb(|)");
    // One caret closes, the other is before a word and types plainly.
    assert_eq!(typed("a| b\n|word", '('), "a(|) b\n(|word");
}

#[test]
fn a_closer_on_a_blank_line_snaps_to_its_opener_indent() {
    assert_eq!(typed("    if x {\n        |", '}'), "    if x {\n    }|");
    assert_eq!(typed("f(\n|", ')'), "plain");
    // A line with text, or a closer with no opener, types alone.
    assert_eq!(typed("  {\n    x|", '}'), "plain");
    assert_eq!(typed("\n    |", '}'), "plain");
}

#[test]
fn backspace_inside_an_empty_pair_deletes_both_halves() -> Result<(), &'static str> {
    let (buffer, cursors) = parse("f(|) \"|\"");
    let edit = backspace(&buffer, &cursors, 0, &rules(None)).ok_or("edit")?;
    assert_eq!(render(buffer, Some(&edit), &edit.cursors), "f| |");
    Ok(())
}

#[test]
fn backspace_outside_a_pair_is_left_to_the_ordinary_path() {
    let (buffer, cursors) = parse("f(x|)");
    assert!(backspace(&buffer, &cursors, 0, &rules(None)).is_none());
    let never = AutoPairs {
        brackets: AutoClose::Never,
        ..rules(None)
    };
    let (buffer, cursors) = parse("f(|)");
    assert!(backspace(&buffer, &cursors, 0, &never).is_none());
}

#[test]
fn backspace_handles_other_carets_ordinarily() -> Result<(), &'static str> {
    let (buffer, cursors) = parse("(|)\nab|");
    let edit = backspace(&buffer, &cursors, 0, &rules(None)).ok_or("edit")?;
    assert_eq!(render(buffer, Some(&edit), &edit.cursors), "|\na|");
    Ok(())
}

#[test]
fn enter_between_brackets_opens_an_indented_line() -> Result<(), &'static str> {
    let (buffer, cursors) = parse("  f {|}\nx|");
    let edit = newline(&buffer, &cursors, 0, &rules(None), "    ").ok_or("edit")?;
    assert_eq!(
        render(buffer, Some(&edit), &edit.cursors),
        "  f {\n      |\n  }\nx\n|"
    );
    Ok(())
}

#[test]
fn enter_indented_asks_for_the_ordinary_carets_indentation() -> Result<(), &'static str> {
    let (buffer, cursors) = parse("  f {|}\nx {|\ny|");
    // The provider answers for the caret on line 1 only; line 2 copies.
    let indent = |range: Range| (range.start.line == 1).then(|| "\t".to_owned());
    let edit =
        newline_indented(&buffer, &cursors, 0, &rules(None), "    ", &indent).ok_or("edit")?;
    assert_eq!(
        render(buffer, Some(&edit), &edit.cursors),
        "  f {\n      |\n  }\nx {\n\t|\ny\n|"
    );
    Ok(())
}

#[test]
fn enter_between_quotes_or_outside_a_pair_is_ordinary() {
    let (buffer, cursors) = parse("\"|\"");
    assert!(newline(&buffer, &cursors, 0, &rules(None), "    ").is_none());
    let (buffer, cursors) = parse("f(|x)");
    assert!(newline(&buffer, &cursors, 0, &rules(None), "    ").is_none());
}
