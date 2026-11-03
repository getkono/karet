//! Vim motions, operators, Visual mode and the Normal/Insert switch.

use super::*;

fn vim(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Vim, cases);
}

#[test]
fn word_motions_follow_vims_word_classes() {
    vim(&[
        ("|foo.bar baz", "w", "foo|.bar baz"),
        ("|foo.bar baz", "W", "foo.bar |baz"),
        ("|foo.bar baz", "ww", "foo.|bar baz"),
        ("|foo bar", "e", "fo|o bar"),
        ("fo|o bar", "e", "foo ba|r"),
        ("foo ba|r", "b", "foo |bar"),
        ("foo |bar", "b", "|foo bar"),
        ("foo.ba|r", "B", "|foo.bar"),
        ("foo ba|r", "ge", "fo|o bar"),
        // Across lines, and an empty line is a word of its own.
        ("foo|\nbar", "w", "foo\n|bar"),
        ("|foo\n\nbar", "w", "foo\n|\nbar"),
        ("foo\n\n|bar", "b", "foo\n|\nbar"),
        // A count repeats; the end of the document stops the motion.
        ("|a b c d", "3w", "a b c |d"),
        ("a b |c", "5w", "a b |c"),
    ]);
}

#[test]
fn line_motions_and_vertical_motions_keep_their_column() {
    vim(&[
        ("  fo|o", "0", "|  foo"),
        ("  fo|o", "^", "  |foo"),
        ("|  foo", "$", "  fo|o"),
        ("a\n|b\nc", "G", "a\nb\n|c"),
        ("a\nb\n|c", "gg", "|a\nb\nc"),
        ("|a\nb\nc", "2G", "a\n|b\nc"),
        ("|a\n  b\nc", "<CR>", "a\n  |b\nc"),
        // `j` steers back to the column it left from after a short line.
        ("abc|d\nx\nabcd", "jj", "abcd\nx\nabc|d"),
        ("abcd|\nx", "$j", "abcd\n|x"),
        ("|abc\nabc", "j", "abc\n|abc"),
        // Normal mode never rests past the last character.
        ("ab|c", "l", "ab|c"),
        ("|abc", "10l", "ab|c"),
    ]);
}

#[test]
fn find_char_motions_repeat_with_semicolon_and_comma() {
    vim(&[
        ("|a,b,c,d", "f,", "a|,b,c,d"),
        ("|a,b,c,d", "2f,", "a,b|,c,d"),
        ("|a,b,c,d", "f,;;", "a,b,c|,d"),
        ("|a,b,c,d", "f,;,", "a|,b,c,d"),
        ("|a,b,c,d", "t,", "|a,b,c,d"),
        // A repeated `t` steps past the match it stopped before.
        ("|ab,c,d", "t,;", "ab,|c,d"),
        ("a,b,c|", "F,", "a,b|,c"),
        ("a,b,c|", "T,", "a,b,|c"),
        // No match: nothing moves.
        ("|abc", "fz", "|abc"),
    ]);
}

#[test]
fn bracket_and_paragraph_motions() {
    vim(&[
        ("|f(a, (b))", "%", "f(a, (b)|)"),
        ("f(a, (b)|)", "%", "f|(a, (b))"),
        ("|{\n  x\n}", "%", "{\n  x\n|}"),
        // `%` looks for a bracket only on the cursor's line.
        ("{\n  |x\n}", "%", "{\n  |x\n}"),
        ("|a\nb\n\nc\nd", "}", "a\nb\n|\nc\nd"),
        ("a\nb\n\nc\n|d", "{", "a\nb\n|\nc\nd"),
        ("a\nb\n\nc|", "}", "a\nb\n\n|c"),
    ]);
}

#[test]
fn delete_operator_over_motions() {
    vim(&[
        ("|foo bar", "dw", "|bar"),
        ("foo |bar", "dw", "foo| "),
        // `dw` on a line's last word stops at the line's end.
        ("foo |bar\nbaz", "dw", "foo| \nbaz"),
        ("|foo bar baz", "d2w", "|baz"),
        ("|a b c d e f g", "2d3w", "|g"),
        ("foo |bar", "d0", "|bar"),
        ("f|oo bar", "d$", "|f"),
        ("|foo bar", "de", "| bar"),
        ("a,|b,c", "dt,", "a,|,c"),
        ("a,|b,c", "df,", "a,|c"),
        ("a\n|b\nc\nd", "dj", "a\n|d"),
        ("a\nb\n|c", "dk", "|a"),
        ("|a\nb\nc", "dG", "|"),
        ("x|(a b)y", "d%", "x|y"),
        // Off a bracket, `%` uses the first one after the cursor.
        ("x(|a b)y", "d%", "x| b)y"),
    ]);
}

#[test]
fn doubled_operators_act_on_whole_lines() {
    vim(&[
        ("a\n|b\nc", "dd", "a\n|c"),
        ("a\nb\n|c", "dd", "a\n|b"),
        ("|a\nb\nc", "2dd", "|c"),
        ("|a\nb", "5dd", "|"),
        ("  |a\n  b", "dd", "  |b"),
        ("|a\nb", "yyp", "a\n|a\nb"),
        ("a\n|b", "yyP", "a\n|b\nb"),
        ("|a\nb", "yjP", "|a\nb\na\nb"),
        ("|a", ">>", "    |a"),
        ("    |a", "<lt><lt>", "|a"),
        ("|a\nb", "2>>", "    |a\n    b"),
        ("|abc def", "gUU", "|ABC DEF"),
        ("|abc", "g~~", "|ABC"),
    ]);
}

#[test]
fn change_operator_enters_insert_mode() {
    vim(&[
        // `cw` on a word changes to its end, keeping the blank after it.
        ("|foo bar", "cwx<Esc>", "|x bar"),
        ("|foo bar", "c2wx<Esc>", "|x"),
        ("foo |bar", "C<Esc>", "foo| "),
        ("  |foo\nbar", "ccx<Esc>", "  |x\nbar"),
        ("  |foo", "Sx<Esc>", "  |x"),
        ("|abc", "sX<Esc>", "|Xbc"),
        ("a|bc", "clX<Esc>", "a|Xc"),
    ]);
}

#[test]
fn abbreviated_commands() {
    vim(&[
        ("a|bc", "x", "a|c"),
        ("ab|c", "x", "a|b"),
        ("|abc", "3x", "|"),
        ("ab|c", "X", "a|c"),
        ("a|bcd", "D", "|a"),
        ("|", "x", "|"),
        ("a|bc", "<Del>", "a|c"),
        ("|abc", "Y$p", "abcab|c"),
    ]);
}

#[test]
fn insert_entries_place_the_caret_and_esc_steps_back() {
    vim(&[
        ("a|bc", "iX<Esc>", "a|Xbc"),
        ("a|bc", "aX<Esc>", "ab|Xc"),
        ("  a|bc", "IX<Esc>", "  |Xabc"),
        ("a|bc", "AX<Esc>", "abc|X"),
        ("  |a\nb", "ox<Esc>", "  a\n  |x\nb"),
        ("a\n  |b", "Ox<Esc>", "a\n  |x\n  b"),
        ("|a", "Ox<Esc>", "|x\na"),
        // Esc at the start of a line has nowhere to step back to.
        ("|abc", "i<Esc>", "|abc"),
        ("ab|c", "a<Esc>", "ab|c"),
    ]);
}

#[test]
fn single_key_edits() {
    vim(&[
        ("|abc", "rX", "|Xbc"),
        ("|abc", "3rX", "XX|X"),
        // Not enough characters: nothing changes.
        ("|ab", "3rX", "|ab"),
        ("|aBc", "~", "A|Bc"),
        ("|aBc", "3~", "Ab|C"),
        ("|a\n  b", "J", "a| b"),
        ("|a\n  b\nc", "3J", "a b| c"),
        ("|a\n  b", "gJ", "a|  b"),
        ("|a\n)", "J", "a|)"),
        ("x |9 y", "<C-a>", "x 1|0 y"),
        ("|x 5", "3<C-x>", "x |2"),
        ("|x -1", "<C-a>", "x |0"),
    ]);
}

#[test]
fn visual_mode_selects_inclusively_and_operates() {
    vim(&[
        ("|abcd", "vld", "|cd"),
        ("ab|cd", "vhd", "a|d"),
        ("|abc def", "vey", "|abc def"),
        ("|abc", "v$d", "|"),
        ("a\n|b\nc", "Vd", "a\n|c"),
        ("|a\nb\nc", "Vjd", "|c"),
        ("|ab cd", "veU", "|AB cd"),
        ("|AB cd", "veu", "|ab cd"),
        ("|ab", "vl~", "|AB"),
        ("|abc", "vlrX", "|XXc"),
        ("|a\nb", "VJ", "a| b"),
        ("|a\nb", "V>", "    |a\nb"),
        ("|ab", "vlcX<Esc>", "|X"),
        // `o` swaps the ends, so the motion moves the other one.
        ("a|bcd", "vlohd", "|d"),
        // Esc, and `v` again, leave Visual mode where the cursor is.
        ("|abc", "vl<Esc>x", "a|c"),
        ("|abc", "vlvx", "a|c"),
    ]);
}

#[test]
fn visual_mode_shows_its_selection_with_the_caret_on_the_last_character() {
    let mut ed = Ed::vim("|abcd");
    ed.type_keys("vll");
    assert_eq!(ed.mode(), Mode::Visual);
    // The editor's selection ends at the cursor; its block caret is the last
    // selected character.
    assert_eq!(ed.selected(), (LineCol::new(0, 0), LineCol::new(0, 2)));
    let mut ed = Ed::vim("ab|cd");
    ed.type_keys("vh");
    assert_eq!(ed.selected(), (LineCol::new(0, 1), LineCol::new(0, 3)));
    let mut ed = Ed::vim("a\n|b\nc");
    ed.type_keys("Vj");
    assert_eq!(ed.mode(), Mode::VisualLine);
    assert_eq!(ed.selected(), (LineCol::new(1, 0), LineCol::new(2, 1)));
}

#[test]
fn a_selection_made_elsewhere_becomes_visual_mode() {
    // A drag or a palette command selected `bc` (exclusive end at 3).
    let mut ed = Ed::vim("abcd");
    ed.selection = Selection {
        anchor: LineCol::new(0, 1),
        head: LineCol::new(0, 3),
    };
    ed.type_keys("d");
    assert_eq!(ed.marked(), "a|d");
    assert_eq!(ed.mode(), Mode::Normal);
}

#[test]
fn a_caret_moved_elsewhere_is_adopted_and_clamped() {
    let mut ed = Ed::vim("|abc\ndef");
    ed.selection = Selection::caret(LineCol::new(1, 3));
    ed.type_keys("x");
    assert_eq!(ed.marked(), "abc\nd|e");
}

#[test]
fn search_motions_use_the_word_or_the_find_query() {
    vim(&[
        ("|foo x foo y foo", "*", "foo x |foo y foo"),
        ("|foo x foo y foo", "*n", "foo x foo y |foo"),
        ("|foo x foo y foo", "*nn", "|foo x foo y foo"),
        ("|foo x foo y foo", "*N", "|foo x foo y foo"),
        ("foo x foo y |foo", "#", "foo x |foo y foo"),
        // `*` matches whole words only.
        ("|foo foobar foo", "*", "foo foobar |foo"),
        ("|foo\nbar foo", "d*", "|foo"),
    ]);
    let mut ed = Ed::vim("|a Foo b foo");
    ed.engine.search_for("foo");
    ed.type_keys("n");
    // A lower-case query matches either case.
    assert_eq!(ed.marked(), "a |Foo b foo");
}

#[test]
fn scroll_commands_scroll_without_editing() {
    let mut ed = Ed::vim("|a\nb\nc");
    ed.type_keys("zz<C-e>3<C-y>zt");
    assert_eq!(
        ed.scrolls,
        vec![
            Scroll::Center,
            Scroll::Lines(1),
            Scroll::Lines(-3),
            Scroll::Top
        ]
    );
    assert_eq!(ed.marked(), "|a\nb\nc");
    let mut ed = Ed::vim(&"x\n".repeat(40));
    ed.type_keys("<C-d>");
    assert_eq!(ed.selection.head, LineCol::new(10, 0));
    assert_eq!(ed.scrolls, vec![Scroll::Lines(10)]);
}

#[test]
fn keys_the_grammar_does_not_know_are_left_to_the_layers() {
    // Ctrl+P (quick open), Ctrl+F (find), Ctrl+B (sidebar) and Ctrl+S stay
    // the keymap's: the grammar claims only the Ctrl keys vim cannot do
    // without (`Ctrl+D`/`U`/`E`/`Y`/`A`/`X`).
    let mut ed = Ed::vim("|a\nb");
    ed.type_keys("<C-p><C-f><C-b><C-s>Q");
    assert_eq!(ed.outcomes, vec![Outcome::Unhandled; 5]);
    assert_eq!(ed.marked(), "|a\nb");
    // A pending command is cancelled by Esc, which is then handled.
    let mut ed = Ed::vim("|abc");
    ed.type_keys("d<Esc>x");
    assert_eq!(ed.marked(), "|bc");
    // A bare Esc is not the grammar's: the editor's own Esc still runs.
    let mut ed = Ed::vim("|abc");
    ed.type_keys("<Esc>");
    assert_eq!(ed.outcomes, vec![Outcome::Unhandled]);
}

#[test]
fn counts_are_capped_so_a_huge_count_cannot_stall() {
    let mut ed = Ed::vim("|a\nb");
    ed.type_keys("99999999999dd");
    assert_eq!(ed.marked(), "|");
    assert_eq!(ed.engine.showcmd(), "");
}

#[test]
fn showcmd_lists_the_command_being_typed() {
    let mut ed = Ed::vim("|abc");
    ed.type_keys("2d");
    assert_eq!(ed.engine.showcmd(), "2d");
    assert!(ed.engine.busy());
    ed.type_keys("<Esc>");
    assert_eq!(ed.engine.showcmd(), "");
    assert!(!ed.engine.busy());
    // A bare count is not "busy": the layers may still run a command with it.
    ed.type_keys("3");
    assert!(!ed.engine.busy());
    assert_eq!(ed.engine.take_count(), 3);
}
