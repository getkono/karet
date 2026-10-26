//! Bracket matching: the pair touching or enclosing a caret, brackets in strings
//! and comments ignored, the Go to / Select to Bracket commands, and the
//! highlight decorations painted into the buffer.

use karet_core::BytePos;
use karet_core::Highlights;
use karet_core::Span;
use karet_core::StandardToken;

use crate::*;

fn at(line: u32, col: u32) -> LineCol {
    LineCol::new(line, col)
}

fn pair(open: LineCol, close: LineCol) -> Option<BracketPair> {
    Some(BracketPair { open, close })
}

/// Highlights marking each `(start, end)` byte range with `token`.
fn spans(token: StandardToken, ranges: &[(usize, usize)]) -> Highlights {
    Highlights::from_sorted_spans(
        ranges
            .iter()
            .map(|&(start, end)| HighlightSpan {
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
fn the_bracket_after_the_caret_wins_then_the_one_before() {
    let buffer = TextBuffer::from_text("f(a)(b)\n");
    // On `(`: its own pair.
    assert_eq!(
        matching_pair(&buffer, None, at(0, 1)),
        pair(at(0, 1), at(0, 3))
    );
    // Just after `)`, which is also just before `(`: the following bracket wins.
    assert_eq!(
        matching_pair(&buffer, None, at(0, 4)),
        pair(at(0, 4), at(0, 6))
    );
    // After the last `)`: the bracket before the caret.
    assert_eq!(
        matching_pair(&buffer, None, at(0, 7)),
        pair(at(0, 4), at(0, 6))
    );
    // Next to no bracket at all.
    assert_eq!(matching_pair(&buffer, None, at(0, 0)), None);
}

#[test]
fn nested_and_multi_line_pairs_match_their_own_partner() {
    let buffer = TextBuffer::from_text("fn f() {\n    if x { y(); }\n}\n");
    assert_eq!(
        matching_pair(&buffer, None, at(0, 7)),
        pair(at(0, 7), at(2, 0))
    );
    assert_eq!(
        matching_pair(&buffer, None, at(2, 0)),
        pair(at(0, 7), at(2, 0))
    );
    assert_eq!(
        matching_pair(&buffer, None, at(1, 9)),
        pair(at(1, 9), at(1, 16))
    );
}

#[test]
fn only_brackets_of_the_same_kind_are_counted() {
    // A stray `]` inside the parens does not hide their pairing.
    let buffer = TextBuffer::from_text("(a ] b)\n");
    assert_eq!(
        matching_pair(&buffer, None, at(0, 0)),
        pair(at(0, 0), at(0, 6))
    );
    // An unclosed bracket has no pair.
    let unclosed = TextBuffer::from_text("(a\n");
    assert_eq!(matching_pair(&unclosed, None, at(0, 0)), None);
}

#[test]
fn brackets_in_strings_and_comments_are_text() {
    // `f(")") // )` — the string's `)` and the comment's `)` are not code.
    let buffer = TextBuffer::from_text("f(\")\") // )\n");
    let strings = spans(StandardToken::String, &[(2, 5)]);
    assert_eq!(
        matching_pair(&buffer, Some(&strings), at(0, 1)),
        pair(at(0, 1), at(0, 5))
    );
    // Without highlights the string's `)` closes the pair.
    assert_eq!(
        matching_pair(&buffer, None, at(0, 1)),
        pair(at(0, 1), at(0, 3))
    );
    // A caret on a commented bracket matches nothing.
    let comments = spans(StandardToken::Comment, &[(7, 11)]);
    assert_eq!(matching_pair(&buffer, Some(&comments), at(0, 10)), None);
}

#[test]
fn brackets_under_a_code_token_still_match() {
    // Only string and comment tokens make brackets text: highlighted
    // punctuation is code and pairs as usual.
    let buffer = TextBuffer::from_text("f(a)\n");
    let punctuation = spans(StandardToken::Punctuation, &[(1, 2), (3, 4)]);
    assert_eq!(
        matching_pair(&buffer, Some(&punctuation), at(0, 1)),
        pair(at(0, 1), at(0, 3))
    );
}

#[test]
fn the_enclosing_pair_skips_balanced_pairs_before_the_caret() {
    let buffer = TextBuffer::from_text("{ a(b) [c] d }\n");
    assert_eq!(
        enclosing_pair(&buffer, None, at(0, 12)),
        pair(at(0, 0), at(0, 13))
    );
    assert_eq!(
        enclosing_pair(&buffer, None, at(0, 4)),
        pair(at(0, 3), at(0, 5))
    );
    assert_eq!(
        enclosing_pair(&TextBuffer::from_text("a b\n"), None, at(0, 1)),
        None
    );
    // Touching takes precedence over enclosing.
    assert_eq!(
        bracket_pair_at(&buffer, None, at(0, 7)),
        pair(at(0, 7), at(0, 9))
    );
}

#[test]
fn the_enclosing_pair_skips_an_unclosed_bracket_of_another_kind() {
    // `f(a[b|c)`: the stray `[` is never closed, so the parens enclose the caret.
    let buffer = TextBuffer::from_text("f(a[bc)\n");
    assert_eq!(
        enclosing_pair(&buffer, None, at(0, 5)),
        pair(at(0, 1), at(0, 6))
    );
    // Several unclosed brackets are skipped on the way out.
    let several = TextBuffer::from_text("{ a ( b [ c }\n");
    assert_eq!(
        enclosing_pair(&several, None, at(0, 10)),
        pair(at(0, 0), at(0, 12))
    );
    // With nothing closed after the caret, there is no enclosing pair.
    let open = TextBuffer::from_text("{ x { y }\n");
    assert_eq!(enclosing_pair(&open, None, at(0, 3)), None);
}

#[test]
fn a_balanced_pair_of_the_same_kind_before_the_caret_is_skipped() {
    // `(a(b)c|)`: the inner parens close before the caret, the outer ones enclose it.
    let buffer = TextBuffer::from_text("(a(b)c)\n");
    assert_eq!(
        enclosing_pair(&buffer, None, at(0, 6)),
        pair(at(0, 0), at(0, 6))
    );
}

#[test]
fn a_typed_closer_pairs_with_the_opener_no_closer_answers() {
    // `(a(b)c|`: the inner `(` is answered by its `)`, so a `)` typed at the
    // caret answers the outer one.
    let buffer = TextBuffer::from_text("(a(b)c\n");
    assert_eq!(
        crate::bracket::unclosed_opener(&buffer, None, at(0, 6), ')'),
        Some(at(0, 0))
    );
    assert_eq!(
        crate::bracket::unclosed_opener(&buffer, None, at(0, 6), ']'),
        None
    );
}

#[test]
fn a_match_scans_at_most_scan_lines_past_its_start() {
    let limit = crate::bracket::SCAN_LINES;
    // `(` on line 0, its `)` on line `gap`, blank lines between.
    let text = |gap: usize| format!("({})\n", "\n".repeat(gap));
    let at_bound = TextBuffer::from_text(&text(limit));
    let bound = u32::try_from(limit).unwrap_or(u32::MAX);
    assert_eq!(
        matching_pair(&at_bound, None, at(0, 0)),
        pair(at(0, 0), at(bound, 0))
    );
    assert_eq!(
        matching_pair(&at_bound, None, at(bound, 0)),
        pair(at(0, 0), at(bound, 0))
    );
    let past_bound = TextBuffer::from_text(&text(limit + 1));
    assert_eq!(matching_pair(&past_bound, None, at(0, 0)), None);
    assert_eq!(matching_pair(&past_bound, None, at(bound + 1, 0)), None);
}

#[test]
fn go_to_bracket_toggles_between_the_ends_and_leaves_pairless_carets() {
    let buffer = TextBuffer::from_text("x(a, b) y\n");
    let mut state = EditorState::new();
    state.place_caret(at(0, 1));
    state.go_to_bracket(&buffer, None);
    assert_eq!(state.cursor(), at(0, 6));
    state.go_to_bracket(&buffer, None);
    assert_eq!(state.cursor(), at(0, 1));
    // From just after the closing bracket, back to the opening one.
    state.place_caret(at(0, 7));
    state.go_to_bracket(&buffer, None);
    assert_eq!(state.cursor(), at(0, 1));
    // Inside the pair: to its closing bracket.
    state.place_caret(at(0, 3));
    state.go_to_bracket(&buffer, None);
    assert_eq!(state.cursor(), at(0, 6));
    // Outside any pair: unchanged.
    state.place_caret(at(0, 9));
    state.go_to_bracket(&buffer, None);
    assert_eq!(state.cursor(), at(0, 9));
}

#[test]
fn select_to_bracket_selects_the_pair_then_widens_outward() {
    let buffer = TextBuffer::from_text("[a(b)c]\n");
    let mut state = EditorState::new();
    state.place_caret(at(0, 3));
    state.select_to_bracket(&buffer, None);
    assert_eq!(state.selection_range(), Range::new(at(0, 2), at(0, 5)).ok());
    assert_eq!(state.cursor(), at(0, 5));
    state.select_to_bracket(&buffer, None);
    assert_eq!(state.selection_range(), Range::new(at(0, 0), at(0, 7)).ok());
    // Nothing encloses the outermost pair: the selection stays.
    state.select_to_bracket(&buffer, None);
    assert_eq!(state.selection_range(), Range::new(at(0, 0), at(0, 7)).ok());
}

#[test]
fn bracket_commands_move_every_caret() {
    let buffer = TextBuffer::from_text("(a)\n(b)\n");
    let mut state = EditorState::new();
    state.set_carets(&[at(0, 0), at(1, 0)]);
    state.go_to_bracket(&buffer, None);
    let heads: Vec<_> = state.cursors().selections.iter().map(|s| s.head).collect();
    assert_eq!(heads, [at(0, 2), at(1, 2)]);
}

#[test]
fn decorations_cover_both_brackets_of_each_carets_pair() {
    let buffer = TextBuffer::from_text("(a) [b]\n");
    let cursors = CursorState {
        selections: vec![Selection::caret(at(0, 0)), Selection::caret(at(0, 7))],
        primary: 0,
    };
    let decos = bracket_decorations(&buffer, None, &cursors);
    let starts: Vec<_> = decos.iter().map(|d| d.range.start).collect();
    assert_eq!(starts, [at(0, 0), at(0, 2), at(0, 4), at(0, 6)]);
    assert!(
        decos
            .iter()
            .all(|d| d.kind == DecorationKind::TextBackground
                && d.role == Some(ThemeRole::MatchingBracket)
                && d.range.end.col == d.range.start.col + 1)
    );
    // A caret inside a pair but touching no bracket highlights nothing.
    let inside = CursorState::single(Selection::caret(at(0, 2)));
    assert!(bracket_decorations(&TextBuffer::from_text("(abc)\n"), None, &inside).is_empty());
}

#[test]
fn the_matched_pair_paints_in_the_matching_bracket_color() {
    let buffer = TextBuffer::from_text("f(x)\n");
    let theme = Theme::dark();
    let mut state = EditorState::new();
    state.place_caret(at(0, 1));
    let decos = bracket_decorations(&buffer, None, state.cursors());
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .theme(&theme)
        .decorations(&decos)
        .focused(false)
        .render(area, &mut buf, &mut state);
    let gutter = 3;
    let bracket = theme.role(ThemeRole::MatchingBracket).to_ratatui();
    assert_eq!(buf[(gutter + 1, 0)].bg, bracket);
    assert_eq!(buf[(gutter + 3, 0)].bg, bracket);
    assert_ne!(buf[(gutter, 0)].bg, bracket);
    assert_ne!(buf[(gutter + 2, 0)].bg, bracket);
}
