//! The helix grammar: select, then act.

use super::*;

fn helix(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Helix, cases);
}

/// The inclusive selection the engine shows, as `(start, end)` columns of
/// line 0.
fn shown(ed: &Ed) -> (u32, u32) {
    let (start, end) = ed.selected();
    let end = if ed.selection.head >= ed.selection.anchor {
        end.col
    } else {
        end.col - 1
    };
    (start.col, end)
}

#[test]
fn word_motions_select_the_word_and_its_trailing_blank() {
    let mut ed = Ed::helix("|foo bar baz");
    ed.type_keys("w");
    assert_eq!(shown(&ed), (0, 3));
    ed.type_keys("w");
    assert_eq!(shown(&ed), (4, 7));
    // From the blank, `b` selects back over the word before it.
    ed.type_keys("b");
    assert_eq!(shown(&ed), (4, 6));
    let mut ed = Ed::helix("|foo bar");
    ed.type_keys("e");
    assert_eq!(shown(&ed), (0, 2));
    ed.type_keys("e");
    assert_eq!(shown(&ed), (3, 6));
    let mut ed = Ed::helix("foo.ba|r");
    ed.type_keys("b");
    assert_eq!(shown(&ed), (4, 6));
    ed.type_keys("b");
    assert_eq!(shown(&ed), (3, 3));
}

#[test]
fn actions_apply_to_the_selection() {
    helix(&[
        ("|foo bar", "wd", "|bar"),
        ("|foo bar", "ed", "| bar"),
        ("|foo bar", "ecx<Esc>", "x| bar"),
        // A paste selects what it pasted, the caret on its last character.
        ("|foo bar", "wywP", "foo foo| bar"),
        ("|foo bar", "eyglp", "foo barfo|o"),
        ("|foo", "~", "|Foo"),
        ("|foo", "e~", "FO|O"),
        ("|ab", "%rx", "x|x"),
        ("|abc", "e`", "ab|c"),
        ("|a\nb", "J", "a| b"),
        ("|ab", "i-<Esc>", "-|ab"),
        ("|ab", "a-<Esc>", "a-|b"),
        ("  |ab", "A-<Esc>", "  ab|-"),
        ("  a|b", "I-<Esc>", "  -|ab"),
    ]);
}

#[test]
fn x_selects_lines_and_extends_by_lines() {
    helix(&[
        ("a\n|b\nc\nd", "xd", "a\n|c\nd"),
        ("a\n|b\nc\nd", "xxd", "a\n|d"),
        // The pasted line comes back selected through its line break.
        ("|a\nb", "xyp", "a\na|\nb"),
        ("|a\nb", "xyP", "a|\na\nb"),
        // The last line has no break to yank, so it pastes as text.
        ("a\n|b", "xyp", "a\nb|b"),
    ]);
}

#[test]
fn select_mode_extends_and_semicolon_collapses() {
    let mut ed = Ed::helix("|foo bar baz");
    ed.type_keys("vww");
    assert_eq!(ed.mode(), Mode::Select);
    assert_eq!(shown(&ed), (0, 7));
    ed.type_keys("<Esc>");
    assert_eq!(ed.mode(), Mode::Normal);
    assert_eq!(shown(&ed), (0, 7));
    ed.type_keys(";");
    assert_eq!(shown(&ed), (7, 7));
    let mut ed = Ed::helix("|foo bar");
    ed.type_keys("w<A-;>");
    // Flipped: the cursor is now the selection's start.
    assert_eq!(ed.selection.head, LineCol::new(0, 0));
}

#[test]
fn match_mode_selects_objects_and_jumps_to_brackets() {
    helix(&[
        ("f(a, |b)", "mi(d", "f(|)"),
        ("f(a, |b)", "ma(d", "|f"),
        ("x = \"a|b\"", "mi\"d", "x = \"|\""),
        ("[(|a)]", "mimd", "[(|)]"),
        ("|f(a, b)", "mm", "f(a, b|)"),
    ]);
}

#[test]
fn goto_find_and_select_all() {
    helix(&[
        ("  a|bc", "gh", "|  abc"),
        ("  a|bc", "gs", "  |abc"),
        ("|abc", "gl", "ab|c"),
        ("a\nb\n|c", "gg", "|a\nb\nc"),
        ("|a\nb\nc", "ge", "a\nb\n|c"),
        ("|a\nb\nc", "2G", "a\n|b\nc"),
        ("|a,b,c", "f,d", "|b,c"),
        ("|a,b,c", "t,d", "|,b,c"),
        ("ab|\ncd", "%d", "|"),
    ]);
}

#[test]
fn indent_keeps_a_line_selection() {
    let mut ed = Ed::helix("|a\nb");
    ed.type_keys("xx>");
    assert_eq!(ed.buffer.text(), "    a\n    b");
    ed.type_keys("<lt>");
    assert_eq!(ed.buffer.text(), "a\nb");
    assert_eq!(ed.selected().0, LineCol::new(0, 0));
}

#[test]
fn macros_record_with_shift_q_and_replay_with_q() {
    helix(&[
        ("|a\nb\nc", "QA;<Esc>jQq", "a;\nb;\n|c"),
        ("|a\nb\nc", "\"wQA;<Esc>jQ\"wq", "a;\nb;\n|c"),
    ]);
}

#[test]
fn dot_repeats_the_last_change() {
    // `.` repeats the delete on whatever is selected now.
    helix(&[("|aa bb cc", "wd.", "|b cc")]);
}

#[test]
fn search_selects_the_next_match() {
    let mut ed = Ed::helix("|foo x foo");
    ed.type_keys("e*n");
    assert_eq!(shown(&ed), (6, 8));
    ed.type_keys("n");
    assert_eq!(shown(&ed), (0, 2));
}
