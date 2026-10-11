//! The editor's pointer selection drag: word and line granularity, and the
//! autoscroll that keeps a drag alive past the edge of the viewport.

use super::harness::*;
use crate::app::*;

/// A press, a drag to `to`, and a release there, all with no modifier.
fn drag(from: At, to: At) -> Vec<Step> {
    let none = KeyModifiers::NONE;
    vec![
        Step::Down(from, none),
        Step::Drag(to, none),
        Step::Up(to, none),
    ]
}

/// A double click on `from` whose second press drags to `to`.
fn double_click_drag(from: At, to: At) -> Vec<Step> {
    let mut steps = vec![click(from)];
    steps.extend(drag(from, to));
    steps
}

/// Drag rows (`docs/editing-behavior.md`, "Drag").
#[test]
fn drag_selection_conformance() {
    check(vec![
        case(
            "a plain drag lands on the exact characters under the pointer",
            "|foo bar baz",
            drag(text(0, 5), text(0, 7)),
            "foo b[ar|] baz",
        ),
        case(
            "a plain drag backward puts the head first",
            "|foo bar baz",
            drag(text(0, 7), text(0, 5)),
            "foo b[|ar] baz",
        ),
        case(
            "a double-click drag swallows whole words, never a partial one",
            "|foo bar baz qux",
            double_click_drag(text(0, 5), text(0, 9)),
            "foo [bar baz|] qux",
        ),
        case(
            "a double-click drag backward keeps the word it started on",
            "|foo bar baz qux",
            double_click_drag(text(0, 9), text(0, 1)),
            "[|foo bar baz] qux",
        ),
    ]);
}

#[test]
fn a_triple_click_drag_extends_by_whole_lines() {
    let mut rig = Rig::new("|alpha\nbravo\ncharlie\n");
    let mut steps = vec![click(text(0, 2)), click(text(0, 2))];
    steps.extend(drag(text(0, 2), text(1, 3)));
    rig.run(&steps);

    let sel = rig.tab().editor.selection_range();
    assert_eq!(
        sel.map(|range| (range.start.line, range.end.line)),
        Some((0, 1)),
        "a line drag covers both lines end to end"
    );
    assert_eq!(sel.map(|range| range.start.col), Some(0));
}

#[test]
fn dragging_below_the_viewport_scrolls_and_keeps_extending() {
    let text_body: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let marked = format!("|{text_body}");
    let mut rig = Rig::sized(&marked, 40, 8);
    let area = rig.app.editor_rect;
    let (x, _) = rig.cell(text(0, 4));
    let below = At::Cell(x, area.bottom());
    rig.step(Step::Down(text(0, 4), KeyModifiers::NONE));

    // Park the pointer one row below the viewport and let the loop tick.
    rig.step(Step::Drag(below, KeyModifiers::NONE));
    let before = rig.scroll_line();
    rig.step(Step::Tick);
    assert!(
        rig.scroll_line() > before,
        "the viewport creeps toward the pointer"
    );
    assert!(
        rig.tab()
            .editor
            .selection_range()
            .is_some_and(|range| range.end.line > 0),
        "the selection follows the rows scrolling into view"
    );

    // The loop is told to come back for the next step.
    assert!(rig.app.drag_autoscroll_wake().is_some());

    // Releasing the button ends both the drag and the autoscroll.
    rig.step(Step::Up(below, KeyModifiers::NONE));
    assert!(rig.app.editor_drag.is_none());
    assert_eq!(rig.app.drag_autoscroll_wake(), None);
}

#[test]
fn a_drag_inside_the_viewport_never_autoscrolls() {
    let text_body: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let marked = format!("|{text_body}");
    let mut rig = Rig::sized(&marked, 40, 10);
    rig.step(Step::Down(text(0, 1), KeyModifiers::NONE));
    rig.step(Step::Drag(text(2, 1), KeyModifiers::NONE));

    assert_eq!(rig.app.drag_autoscroll_wake(), None);
    let before = rig.scroll_line();
    rig.step(Step::Tick);
    assert_eq!(rig.scroll_line(), before);
}
