//! Text objects, through vim's operators and Visual mode.

use super::*;

fn vim(cases: &[(&str, &str, &str)]) {
    check(KeymapPreset::Vim, cases);
}

#[test]
fn word_objects() {
    vim(&[
        ("foo b|ar baz", "diw", "foo | baz"),
        ("foo b|ar baz", "daw", "foo |baz"),
        // At the line's end `aw` takes the blanks before the word instead.
        ("foo b|ar", "daw", "fo|o"),
        ("foo|  bar", "diw", "foo|bar"),
        ("foo|  bar", "daw", "fo|o"),
        ("a.b|c.d e", "diW", "| e"),
        ("a b| c d", "d2aw", "a |b"),
        ("|a b c d", "d2aw", "|c d"),
        ("f(|bar)", "daw", "f(|)"),
        ("x fo|o y", "ciwbar<Esc>", "x ba|r y"),
        ("x fo|o y", "yiwP", "x fo|ofoo y"),
    ]);
}

#[test]
fn quote_objects() {
    vim(&[
        ("x = \"a|bc\";", "di\"", "x = \"|\";"),
        // No blank after the string: `a"` takes the blank before it.
        ("x = \"a|bc\";", "da\"", "x =|;"),
        ("x = \"abc\" |+ 1", "da\"", "x = \"abc\" |+ 1"),
        // Before the first string on the line, the object is that string.
        ("|x = 'abc'", "ci'z<Esc>", "x = '|z'"),
        // An escaped quote does not end the string.
        ("\"a\\\"|b\"", "di\"", "\"|\""),
        ("say `hi|`", "di`", "say `|`"),
        ("\"ab\" |\"cd\"", "da\"", "\"ab|\""),
    ]);
}

#[test]
fn bracket_objects() {
    vim(&[
        ("f(a, |b)", "di(", "f(|)"),
        ("f(a, |b)", "da(", "|f"),
        ("f(a, |b)", "dib", "f(|)"),
        ("f(a, (|b))", "di)", "f(a, (|))"),
        ("f(a, (|b))", "2di(", "f(|)"),
        ("x[|1]", "ci]9<Esc>", "x[|9]"),
        ("{ a |}", "di}", "{|}"),
        ("<a|b>", "di<lt>", "<|>"),
        // On a bracket itself, the pair it belongs to.
        ("f|(a)", "di(", "f(|)"),
        ("f(a|)", "di(", "f(|)"),
        // Not inside any pair: nothing happens.
        ("|abc", "di(", "|abc"),
    ]);
}

#[test]
fn an_inner_block_on_its_own_lines_is_those_lines() {
    vim(&[
        ("if x {\n    |a\n    b\n}", "di{", "if x {\n|}"),
        ("if x {\n    |a\n}", "yi{P", "if x {\n    |a\n    a\n}"),
    ]);
}

#[test]
fn paragraph_and_tag_objects() {
    vim(&[
        ("a\n|b\n\nc", "dip", "|\nc"),
        ("a\n|b\n\nc", "dap", "|c"),
        ("a\n\nb\n|c", "dap", "|a"),
        ("<p>he|llo</p>", "dit", "<p>|</p>"),
        ("<p>he|llo</p>", "dat", "|"),
        ("<div><b>x|y</b> z</div>", "dit", "<div><b>|</b> z</div>"),
        ("<div><b>x|y</b> z</div>", "d2it", "<div>|</div>"),
        (
            "<div a=\"1\"><br/>|x</div>",
            "cit-<Esc>",
            "<div a=\"1\">|-</div>",
        ),
    ]);
}

#[test]
fn visual_mode_selects_an_object() {
    let mut ed = Ed::vim("foo b|ar baz");
    ed.type_keys("viw");
    assert_eq!(ed.mode(), Mode::Visual);
    assert_eq!(ed.selected(), (LineCol::new(0, 4), LineCol::new(0, 6)));
    ed.type_keys("d");
    assert_eq!(ed.marked(), "foo | baz");
    let mut ed = Ed::vim("a\n|b\n\nc");
    ed.type_keys("vipd");
    assert_eq!(ed.marked(), "|\nc");
}
