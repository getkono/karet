//! Registers: unnamed, named (and appending), yank `0`, numbered deletes,
//! the black hole and the clipboard.

use super::*;

fn vim(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Vim, cases);
}

#[test]
fn paste_puts_text_after_or_before_and_lines_below_or_above() {
    vim(&[
        ("|ab", "ylp", "a|ab"),
        ("|ab", "ylP", "|aab"),
        ("|ab", "yl3p", "aaa|ab"),
        ("|a\nb", "yyjp", "a\nb\n|a"),
        ("|a\nb", "yy2P", "|a\na\na\nb"),
        ("  |a\nb", "yyjp", "  a\nb\n  |a"),
        // A multi-line piece of text pastes with the caret at its start.
        ("|ab\ncd", "vjyGp", "ab\nc|ab\ncd"),
        // Nothing yanked yet: nothing to paste.
        ("|ab", "p", "|ab"),
        ("|", "p", "|"),
    ]);
}

#[test]
fn named_registers_hold_text_and_uppercase_appends() {
    vim(&[
        ("|a\nb", "\"ayyj\"byy\"ap", "a\nb\n|a"),
        ("|a\nb", "\"ayyj\"Ayy\"aP", "a\n|a\nb\nb"),
        ("|ab", "\"ayl\"Ayl\"ap", "aa|ab"),
        // A named register also fills the unnamed one.
        ("|ab", "\"aylp", "a|ab"),
    ]);
}

#[test]
fn deletes_rotate_through_the_numbered_registers() {
    vim(&[
        // `0` keeps the last yank through later deletes.
        ("|a\nb\nc", "yyjdd\"0P", "a\n|a\nc"),
        ("|a\nb\nc", "dddd\"2P", "|a\nc"),
        ("|a\nb\nc", "dddd\"1P", "|b\nc"),
        // A small delete goes to `-`, not `1`.
        ("|ab\ncd", "ddx\"-P", "|cd"),
        ("|ab\ncd", "xdd\"-P", "|acd"),
    ]);
}

#[test]
fn the_black_hole_register_keeps_the_unnamed_register() {
    vim(&[("|a\nb", "yyj\"_ddp", "a\n|a")]);
}

#[test]
fn the_plus_register_is_the_system_clipboard() {
    let mut ed = Ed::vim("|foo bar");
    ed.type_keys("\"+yiw");
    assert_eq!(ed.clipboard(), Some("foo"));
    ed.clip.0 = Some("X\n".to_string());
    // Clipboard text ending in a line break pastes as a line.
    ed.type_keys("\"+p");
    assert_eq!(ed.marked(), "foo bar\n|X");
    ed.clip.0 = Some("Y".to_string());
    ed.type_keys("\"*P");
    assert_eq!(ed.marked(), "foo bar\n|YX");
}

#[test]
fn visual_put_replaces_the_selection() {
    vim(&[
        ("|foo bar", "yiwwviwp", "foo fo|o"),
        // `p` leaves the replaced text in the unnamed register; `P` does not.
        ("|foo bar", "yiwwviwp0viwp", "ba|r foo"),
        ("|foo bar", "yiwwviwP0viwP", "fo|o foo"),
        ("|a\nb", "yyjVp", "a\n|a"),
    ]);
}
