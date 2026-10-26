//! Edge cases: a file's final line break, empty lines, counts that cross
//! lines, numbers in other bases, and text whose case changes its length.

use super::*;

fn vim(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Vim, cases);
}

fn helix(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Helix, cases);
}

#[test]
fn a_final_line_break_ends_the_last_line_rather_than_starting_one() {
    vim(&[
        ("a\n|b\n", "dd", "|a\n"),
        ("a\n|b\n", "dddd", "|\n"),
        ("a\n|b\n", "j", "a\n|b\n"),
        ("|a\nb\n", "G", "a\n|b\n"),
        ("|a\nb\n", "yyGp", "a\nb\n|a\n"),
        ("|a\nb\n", "yGGp", "a\nb\n|a\nb\n"),
        ("|a\nb\n", "yjGp", "a\nb\n|a\nb\n"),
        ("|a\n\nb\n", "dap", "|b\n"),
    ]);
    // Helix selects through the final break, as helix does.
    helix(&[("|a\nb\n", "%d", "|"), ("a\n|b\n", "xd", "a\n|")]);
}

#[test]
fn a_just_opened_empty_last_line_keeps_the_caret() {
    vim(&[
        // No final line break: `o` or `<CR>` at the end opens a real line.
        ("a\n|b", "o<Esc>", "a\nb\n|"),
        ("a\n|b", "o<Esc>dd", "a\n|b"),
        ("a\n|b", "o<Esc>ix<Esc>", "a\nb\n|x"),
        ("a\n|b", "A<CR><Esc>", "a\nb\n|"),
        ("a\n|b", "A<CR><Esc>dd", "a\n|b"),
        ("|a", "ihi<CR><Esc>", "hi\n|a"),
        // An empty new buffer.
        ("|", "o<Esc>", "\n|"),
        ("|", "o<Esc>dd", "|"),
        ("|", "ihi<CR><Esc>", "hi\n|"),
        ("|", "ihi<CR><Esc>dd", "|hi"),
        ("|", "ihi<CR><Esc>ix<Esc>", "hi\n|x"),
        // With a final line break the line `o` opens is real either way.
        ("a\n|b\n", "o<Esc>", "a\nb\n|\n"),
        ("a\n|b\n", "o<Esc>dd", "a\n|b\n"),
        // Leaving the caret off the empty last line leaves it unvisited.
        ("a\n|b", "o<Esc>kj", "a\n|b\n"),
    ]);
}

#[test]
fn a_change_with_nothing_to_delete_still_enters_insert_mode() {
    vim(&[
        ("|", "Cx<Esc>", "|x"),
        ("|", "sx<Esc>", "|x"),
        ("a\n|\nb", "Cx<Esc>", "a\n|x\nb"),
        ("a\n|\nb", "sx<Esc>", "a\n|x\nb"),
        ("a\n|\nb", "cwx<Esc>", "a\n|x\nb"),
        ("a\n|\nb", "clx<Esc>", "a\n|x\nb"),
        ("a\n|\nb", "c$x<Esc>", "a\n|x\nb"),
    ]);
}

#[test]
fn a_counted_word_delete_stops_at_a_line_end_only_after_the_last_word() {
    vim(&[
        ("|foo\nbar baz", "d2w", "|baz"),
        ("|foo\nbar", "dw", "|\nbar"),
        ("|a b\n  c d", "d2w", "|\n  c d"),
    ]);
}

#[test]
fn visual_dollar_takes_in_the_line_break() {
    vim(&[
        ("|ab\ncd", "v$d", "|cd"),
        ("|ab\ncd\nef", "v$jd", "|ef"),
        ("|ab\ncd", "v$y$p", "ab|ab\n\ncd"),
        ("|ab\ncd", "v$<Esc>", "a|b\ncd"),
    ]);
}

#[test]
fn join_separates_by_the_text_joined_so_far() {
    vim(&[
        ("|a\n\nb", "3J", "a| b"),
        ("|\n\nb", "3J", "|b"),
        ("|a \n\nb", "3J", "a |b"),
    ]);
}

#[test]
fn increment_keeps_the_base_width_and_case_of_the_number() {
    vim(&[
        ("|0x1f", "<C-a>", "0x2|0"),
        ("|0x0F", "<C-a>", "0x1|0"),
        ("|0xff", "<C-a>", "0x10|0"),
        ("0|x10", "<C-x>", "0x0|f"),
        ("|0x00", "<C-x>", "0xfffffffffffffff|f"),
        ("|0b101", "<C-a>", "0b11|0"),
        ("|007", "<C-a>", "00|8"),
        ("|099", "<C-a>", "10|0"),
        ("|-007", "<C-a>", "-00|6"),
        ("|x 0xg", "<C-a>", "x |1xg"),
    ]);
}

#[test]
fn an_edit_forgets_the_column_vertical_motions_steer_to() {
    vim(&[
        ("abc|d\nab\nabcd\nabcd", "jddj", "abcd\nabcd\n|abcd"),
        ("|abc\nabcd", "$xj", "ab\na|bcd"),
    ]);
}

#[test]
fn a_case_change_that_lengthens_text_moves_past_it() {
    vim(&[("|ßa", "~~", "SS|A"), ("ß|a", "gUU", "SS|A")]);
    helix(&[("|ßb", "e~", "SS|B")]);
}

#[test]
fn a_backward_helix_selection_ending_at_a_line_break_survives_an_edit() {
    helix(&[("|ab\ncd", "x<A-;>>d", "|cd")]);
}

#[test]
fn an_uppercase_macro_register_appends() {
    vim(&[("|a\nb\nc", "qaA;<Esc>qqAjq@a", "a;\nb;\n|c")]);
}

#[test]
fn star_never_matches_the_word_it_starts_from() {
    vim(&[
        ("|  foo foo", "*", "  foo |foo"),
        ("f|oo foo", "*", "foo |foo"),
    ]);
}

#[test]
fn outdent_with_a_tab_unit_removes_a_tab_width_of_spaces() {
    let mut ed = Ed::vim("        |x");
    ed.indent = "\t";
    ed.type_keys("<lt><lt>");
    assert_eq!(ed.marked(), "    |x");
    ed.type_keys("<lt><lt>");
    assert_eq!(ed.marked(), "|x");
}

#[test]
fn a_completion_replays_as_the_text_it_inserted_not_the_key_that_took_it() {
    let mut ed = Ed::vim("|a\nb\nc");
    ed.type_keys("qqA.x");
    // The popup takes Enter, accepting `xyz` over the typed `x`.
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    ed.engine.record(enter);
    ed.engine.observe_insert(enter);
    ed.engine.forget(enter, true);
    ed.complete((1, 0), "xyz", false, true);
    ed.type_keys("<Esc>jq");
    assert_eq!(ed.marked(), "a.xyz\n|b\nc");
    ed.type_keys("@q");
    assert_eq!(ed.marked(), "a.xyz\nb.xyz\n|c");
    // The macro's change, completion included, is what `.` repeats.
    ed.type_keys(".");
    assert_eq!(ed.marked(), "a.xyz\nb.xyz\nc.xy|z");
}

#[test]
fn a_commit_character_follows_the_completion_it_accepted() {
    let mut ed = Ed::vim("|a\nb");
    ed.type_keys("qqAf");
    // Typing `(` commits `foo` for the typed `f`, then types itself.
    let paren = KeyEvent::new(KeyCode::Char('('), KeyModifiers::NONE);
    ed.engine.record(paren);
    ed.engine.observe_insert(paren);
    ed.complete((1, 0), "foo", true, true);
    let head = ed.selection.head;
    ed.insert_text(head, "(");
    ed.type_keys("<Esc>jq");
    assert_eq!(ed.marked(), "afoo(\n|b");
    ed.type_keys("@q");
    assert_eq!(ed.marked(), "afoo(\nbfoo|(");
}

#[test]
fn a_replay_starts_from_the_caret_the_engine_settled_on() {
    // A caret past the end of a line (a click) settles on the last
    // character before `.` replays, and stays where the replay left it.
    let mut ed = Ed::vim("|ab\ncd");
    ed.type_keys("x");
    ed.selection = Selection::caret(LineCol::new(1, 2));
    ed.type_keys(".");
    assert_eq!(ed.marked(), "b\n|c");
}
