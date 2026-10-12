//! Multi-click counting and the modifier-versus-count rules
//! (`docs/editing-behavior.md`, "Multi-click").

use super::harness::*;
use super::support::*;
use crate::app::editor::ClickSurface;
use crate::app::*;

/// Multi-click rows: what a double, triple and fourth click select, and that a
/// held modifier outranks the click count.
#[test]
fn multi_click_conformance() {
    let alt = KeyModifiers::ALT;
    let shift = KeyModifiers::SHIFT;
    check(vec![
        case(
            "a double click on punctuation selects the punctuation run",
            "|foo.bar",
            vec![click(text(0, 3)), click(text(0, 3))],
            "foo[.|]bar",
        ),
        case(
            "a double click on a run of punctuation selects all of it",
            "|a::b",
            vec![click(text(0, 2)), click(text(0, 2))],
            "a[::|]b",
        ),
        case(
            "a double click on whitespace selects the whitespace run",
            "|foo  bar",
            vec![click(text(0, 3)), click(text(0, 3))],
            "foo[  |]bar",
        ),
        case(
            "a double click past the line end selects the last word",
            "|foo bar\nbaz",
            vec![click(At::Offset(0, 7, 3, 0)), click(At::Offset(0, 7, 3, 0))],
            "foo [bar|]\nbaz",
        ),
        case(
            "a triple click selects the line with its line break",
            "|alpha\nbravo",
            vec![click(text(0, 2)), click(text(0, 2)), click(text(0, 2))],
            "[alpha\n|]bravo",
        ),
        case(
            "a triple click on the last line selects to its end",
            "|alpha\nbravo",
            vec![click(text(1, 2)), click(text(1, 2)), click(text(1, 2))],
            "alpha\n[bravo|]",
        ),
        case(
            "a fourth quick click keeps the line selected",
            "|alpha\nbravo",
            vec![
                click(text(0, 2)),
                click(text(0, 2)),
                click(text(0, 2)),
                click(text(0, 2)),
            ],
            "[alpha\n|]bravo",
        ),
        case(
            "a second press one cell over still double-clicks",
            "|foo bar baz",
            vec![click(text(0, 5)), click(text(0, 6))],
            "foo [bar|] baz",
        ),
        case(
            "a second press two cells over is a new single click",
            "|foo bar baz",
            vec![click(text(0, 4)), click(text(0, 6))],
            "foo ba|r baz",
        ),
        case(
            "two quick Alt+clicks add a caret and then remove it",
            "|foo bar baz",
            vec![Step::Click(text(0, 5), alt), Step::Click(text(0, 5), alt)],
            "|foo bar baz",
        ),
        case(
            "two quick Shift+clicks extend twice and select no word",
            "|foo bar baz",
            vec![
                Step::Click(text(0, 5), shift),
                Step::Click(text(0, 5), shift),
            ],
            "[foo b|]ar baz",
        ),
        case(
            "a quick Shift+click after a click extends rather than selecting a word",
            "|foo bar baz",
            vec![click(text(0, 5)), Step::Click(text(0, 6), shift)],
            "foo b[a|]r baz",
        ),
        case(
            "a quick plain click after an Alt+click starts a new streak",
            "|foo bar baz",
            vec![Step::Click(text(0, 5), alt), click(text(0, 5))],
            "foo b|ar baz",
        ),
    ]);
}

#[test]
fn a_double_click_drag_extends_through_a_punctuation_run_whole() {
    let mut rig = Rig::new("|foo bar::baz qux");
    rig.run(&[
        click(text(0, 5)),
        Step::Down(text(0, 5), KeyModifiers::NONE),
        Step::Drag(text(0, 7), KeyModifiers::NONE),
        Step::Up(text(0, 7), KeyModifiers::NONE),
    ]);
    assert_eq!(rig.marked(), "foo [bar::|]baz qux");
}

#[test]
fn the_streak_counts_only_on_one_surface_with_the_same_modifiers() {
    let mut app = app();
    let pane = app.focus_pane();
    let editor = ClickSurface::Editor(pane);
    let none = KeyModifiers::NONE;

    assert_eq!(app.click_streak_on(5, 5, editor, none), 1);
    assert_eq!(app.click_streak_on(5, 5, editor, none), 2);
    // The same cell on another surface starts over.
    assert_eq!(app.click_streak(5, 5), 1);
    assert_eq!(app.click_streak_on(5, 5, editor, none), 1);
    // So does a change of modifier.
    assert_eq!(app.click_streak_on(5, 5, editor, KeyModifiers::ALT), 1);
    assert_eq!(app.click_streak_on(5, 5, editor, none), 1);
    // A list row is its own target: a neighbouring row is a new click there.
    assert_eq!(app.click_streak(5, 5), 1);
    assert_eq!(app.click_streak(5, 6), 1);
    assert_eq!(app.click_streak(5, 6), 2);
    assert_eq!(app.click_streak(5, 6), 3);
    assert_eq!(app.click_streak(5, 6), 1, "a list row keeps its cycle");
    // In text the count stops at three.
    for expected in [1, 2, 3, 3, 3] {
        assert_eq!(app.click_streak_on(9, 9, editor, none), expected);
    }
    // A drift of one cell, across or down, continues the streak there.
    assert_eq!(app.click_streak_on(20, 2, editor, none), 1);
    assert_eq!(app.click_streak_on(21, 3, editor, none), 2);
}
