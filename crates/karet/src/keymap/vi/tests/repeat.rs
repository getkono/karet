//! `.` repeat and macros.

use super::*;

fn vim(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Vim, cases);
}

#[test]
fn dot_repeats_the_last_change() {
    vim(&[
        ("|a b c d", "dw.", "|c d"),
        ("|a b c d", "dw2.", "|d"),
        ("|foo bar", "ciwx<Esc>w.", "x |x"),
        ("|a\nb", "A;<Esc>j.", "a;\nb|;"),
        ("|a\nb\nc", "dd.", "|c"),
        ("|abc", "x.", "|c"),
        ("|ab", "rX.", "|Xb"),
        ("|a\nb", ">>j.", "    a\n    |b"),
        ("|a\nb", "ox<Esc>.", "a\nx\n|x\nb"),
        // A motion is not a change: `.` still repeats the edit before it.
        ("|ab cd", "xw.", "b |d"),
        // Nothing changed yet: `.` does nothing.
        ("|ab", ".", "|ab"),
    ]);
}

#[test]
fn a_visual_change_is_not_what_dot_repeats() {
    vim(&[("|abcd", "xvld.", "|")]);
}

#[test]
fn macros_record_and_replay_keys() {
    vim(&[
        ("|a\nb\nc", "qaA;<Esc>jq@a", "a;\nb;\n|c"),
        ("|a\nb\nc", "qaA;<Esc>jq2@a", "a;\nb;\nc|;"),
        ("|a\nb\nc", "qaA;<Esc>jq@a@@", "a;\nb;\nc|;"),
        // An empty register replays nothing.
        ("|ab", "@zx", "|b"),
    ]);
}

#[test]
fn the_key_that_stops_a_recording_is_not_recorded() {
    let mut ed = Ed::vim("|abc");
    ed.type_keys("qbx");
    assert_eq!(ed.engine.recording(), Some('b'));
    ed.type_keys("q");
    assert_eq!(ed.engine.recording(), None);
    ed.type_keys("@b");
    assert_eq!(ed.marked(), "|c");
    // A recorded macro replays a change, and `.` then repeats that change.
    ed.type_keys(".");
    assert_eq!(ed.marked(), "|");
}

#[test]
fn a_macro_holds_insert_mode_typing() {
    vim(&[("|x\nx", "qqcwyes<Esc>jq@q", "yes\nye|s")]);
}
