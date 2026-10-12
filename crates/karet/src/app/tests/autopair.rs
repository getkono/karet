//! Brackets and quotes rows of `docs/editing-behavior.md`: auto-closing,
//! type-over, pair-deleting Backspace, selection wrapping, Enter between a
//! pair, and a closing bracket snapping to its opener, in a Rust buffer (whose
//! pairs leave `'` alone), plus the `editor.autoClosing*` settings.

use karet_session::config::schema::AutoClosing;

use super::harness::*;
use crate::app::*;

fn typed(c: char) -> Step {
    key(KeyCode::Char(c))
}

#[test]
fn openers_close_themselves() {
    check(vec![
        case(
            "a bracket at the end of a line",
            "f|",
            vec![typed('(')],
            "f(|)",
        ),
        case(
            "a quote before closing punctuation",
            "x = |;",
            vec![typed('"')],
            "x = \"|\";",
        ),
        case(
            "every caret, the primary kept",
            "a|\nb#",
            vec![typed('{')],
            "a{|}\nb{#}",
        ),
        case("not before a word", "|foo", vec![typed('(')], "(|foo"),
        case(
            "never a single quote in Rust, where it starts a lifetime",
            "fn f<|>",
            vec![typed('\'')],
            "fn f<'|>",
        ),
    ]);
}

#[test]
fn an_inserted_closer_is_typed_over() {
    check(vec![
        case(
            "straight after it was inserted",
            "f|",
            vec![typed('('), typed(')')],
            "f()|",
        ),
        case(
            "after typing inside the pair",
            "f|",
            vec![typed('('), typed('a'), typed(')')],
            "f(a)|",
        ),
        case(
            "nested pairs, one closer at a time",
            "f|",
            vec![typed('('), typed('['), typed(']'), typed(')')],
            "f(\\[\\])|",
        ),
        case(
            "a closer that was already there is typed",
            "f(|)",
            vec![typed(')')],
            "f()|)",
        ),
        case(
            "not once the caret has moved off it",
            "(|)",
            vec![typed('('), key(KeyCode::Right), typed(')')],
            "(())|)",
        ),
        case(
            "not once Backspace has deleted it with its pair",
            "a|)",
            vec![typed('('), key(KeyCode::Backspace), typed(')')],
            "a)|)",
        ),
        case(
            "not once Delete has deleted it",
            "a|)",
            vec![typed('('), key(KeyCode::Delete), typed(')')],
            "a()|)",
        ),
    ]);
}

#[test]
fn backspace_deletes_an_empty_pair() {
    check(vec![
        case("brackets", "f(|)", vec![key(KeyCode::Backspace)], "f|"),
        case(
            "quotes",
            "x = \"|\"",
            vec![key(KeyCode::Backspace)],
            "x = |",
        ),
        case(
            "not a pair with text between",
            "f(x|)",
            vec![key(KeyCode::Backspace)],
            "f(|)",
        ),
    ]);
}

#[test]
fn an_opener_wraps_a_selection() {
    check(vec![
        case("brackets", "a [bc|] d", vec![typed('(')], "a ([bc|]) d"),
        case("quotes, backward", "[|bc]", vec![typed('"')], "\"[|bc]\""),
    ]);
}

#[test]
fn enter_between_brackets_opens_an_indented_line() {
    check(vec![case(
        "the closer drops to the opener's indentation",
        "    f {|}",
        vec![key(KeyCode::Enter)],
        "    f {\n        |\n    }",
    )]);
}

#[test]
fn a_closer_on_a_blank_line_snaps_to_its_opener() {
    check(vec![case(
        "dedents to the opening line",
        "    if x {\n        |",
        vec![typed('}')],
        "    if x {\n    }|",
    )]);
}

#[test]
fn the_settings_turn_each_kind_off() {
    let mut rig = Rig::new("f |");
    rig.app.settings.editor.auto_closing_brackets = AutoClosing::Never;
    rig.run(&[typed('('), typed('"')]);
    assert_eq!(rig.marked(), "f (\"|\"");

    let mut rig = Rig::new("f |");
    rig.app.settings.editor.auto_closing_quotes = AutoClosing::Never;
    rig.run(&[typed('"'), key(KeyCode::Backspace), typed('(')]);
    assert_eq!(rig.marked(), "f (|)");
}

#[test]
fn always_closes_before_a_word() {
    let mut rig = Rig::new("|foo");
    rig.app.settings.editor.auto_closing_brackets = AutoClosing::Always;
    rig.run(&[typed('[')]);
    assert_eq!(rig.marked(), "\\[|\\]foo");
}
