//! The editor's pointer selection drag: word and line granularity, and the
//! autoscroll that keeps a drag alive past the edge of the viewport.

use super::harness::*;
use super::support::text_tab;
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
fn a_double_click_on_a_tabs_trailing_cell_selects_the_tab_not_the_next_word() {
    // A single click there rounds the caret past the tab, but the word
    // gestures ask what is *under* the pointer, which is still the tab.
    let mut rig = Rig::sized("|\tfoo bar", 40, 8);
    let (x, y) = rig.cell(text(0, 0));
    let trailing = At::Cell(x + 3, y);
    rig.run(&[click(trailing), click(trailing)]);
    assert_eq!(
        rig.tab()
            .editor
            .selection_range()
            .map(|range| (range.start.col, range.end.col)),
        Some((0, 1))
    );

    // A double-click drag from the same cell keeps the tab as its anchor
    // span and grows forward word by word from it.
    let mut rig = Rig::sized("|\tfoo bar", 40, 8);
    let (bar_x, _) = rig.cell(text(0, 6));
    rig.run(&double_click_drag(trailing, At::Cell(bar_x, y)));
    assert_eq!(
        rig.tab()
            .editor
            .selection_range()
            .map(|range| (range.start.col, range.end.col)),
        Some((0, 8))
    );
}

#[test]
fn a_double_click_on_the_right_half_of_a_wide_punctuation_mark_selects_it() {
    // `、` is a two-cell non-word character: rounding its right half past it
    // would select `foo` instead.
    let mut rig = Rig::sized("|、foo", 40, 8);
    let (x, y) = rig.cell(text(0, 0));
    let right_half = At::Cell(x + 1, y);
    rig.run(&[click(right_half), click(right_half)]);
    assert_eq!(
        rig.tab()
            .editor
            .selection_range()
            .map(|range| (range.start.col, range.end.col)),
        Some((0, 1))
    );
}

#[test]
fn a_double_click_near_the_right_edge_selects_the_word_under_the_pointer() {
    // The pointer stays on one cell for both presses, as a real one does. Were
    // the first press to scroll the line (as the keyboard's margin would),
    // the second would land on a different word.
    let line: String = (0..40).map(|n| format!("w{n:02} ")).collect();
    let marked = format!("|{line}");
    let mut rig = Rig::sized(&marked, 40, 8);
    let width = u32::from(rig.tab().editor.content_width());
    // A word's cell, on the word nearest the edge that ends before it, so
    // selecting the word needs no reveal either.
    let mut col = width.saturating_sub(2);
    while col % 4 == 3 || col / 4 * 4 + 3 >= width {
        col -= 1;
    }
    let (x, y) = rig.cell(text(0, col));
    rig.run(&[click(At::Cell(x, y)), click(At::Cell(x, y))]);

    assert_eq!(
        rig.tab().editor.scroll_col,
        0,
        "the click scrolled the line"
    );
    let start = col / 4 * 4;
    assert_eq!(
        rig.tab()
            .editor
            .selection_range()
            .map(|range| (range.start.col, range.end.col)),
        Some((start, start + 3))
    );
    // The view is unwrapped, so the keyboard does scroll it.
    rig.step(key(KeyCode::End));
    assert!(
        rig.tab().editor.scroll_col > 0,
        "the line does not overflow"
    );
}

#[test]
fn a_triple_click_drag_extends_by_whole_lines() {
    let mut rig = Rig::new("|alpha\nbravo\ncharlie\n");
    let mut steps = vec![click(text(0, 2)), click(text(0, 2))];
    steps.extend(drag(text(0, 2), text(1, 3)));
    rig.run(&steps);

    assert_eq!(
        rig.marked(),
        "[alpha\nbravo\n|]charlie\n",
        "a line drag covers both lines, each with its line break"
    );
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

/// Forty numbered lines with the caret at the top.
fn forty_lines() -> String {
    let body: String = (0..40).map(|n| format!("line {n}\n")).collect();
    format!("|{body}")
}

/// A rig whose drag from `(0, 4)` rests `rows` rows below the viewport.
fn drag_below(rows: u16) -> Rig {
    let mut rig = Rig::sized(&forty_lines(), 40, 12);
    let area = rig.app.editor_rect;
    let (x, _) = rig.cell(text(0, 4));
    rig.step(Step::Down(text(0, 4), KeyModifiers::NONE));
    rig.step(Step::Drag(
        At::Cell(x, area.bottom() + rows - 1),
        KeyModifiers::NONE,
    ));
    rig
}

/// The primary selection's head (caret) line, or `None` when nothing is
/// selected.
fn head_line(rig: &Rig) -> Option<u32> {
    let editor = &rig.tab().editor;
    editor.selection_range().map(|_| editor.cursor().line)
}

/// Assert the drag is over: nothing armed, and the loop no longer wakes for it
/// or scrolls on a tick.
fn assert_drag_ended(rig: &mut Rig, why: &str) {
    assert!(
        rig.app.editor_drag.is_none(),
        "{why}: the drag is still armed"
    );
    assert_eq!(rig.app.drag_autoscroll_wake(), None, "{why}: still waking");
    let before = rig.scroll_line();
    rig.step(Step::Tick);
    assert_eq!(rig.scroll_line(), before, "{why}: a tick still scrolls");
}

#[test]
fn a_release_under_an_overlay_ends_the_drag() {
    // The issue's repro: drag below the viewport, open the picker, release over
    // it, close it. The release must not be lost to the overlay's swallow.
    let mut rig = drag_below(1);
    rig.step(Step::Cmd(Command::OpenQuickOpen));
    assert!(rig.app.overlay.is_some());
    rig.step(Step::Up(At::Cell(1, 1), KeyModifiers::NONE));
    rig.app.overlay = None;
    assert_drag_ended(&mut rig, "release under the picker");
}

#[test]
fn an_overlay_opening_ends_the_drag_even_without_a_release() {
    let mut rig = drag_below(1);
    rig.step(Step::Cmd(Command::OpenQuickOpen));
    assert_eq!(
        rig.app.drag_autoscroll_wake(),
        None,
        "no wake under the picker"
    );
    rig.step(Step::Tick);
    assert!(rig.app.editor_drag.is_none());
}

#[test]
fn a_key_press_ends_the_drag() {
    let mut rig = drag_below(1);
    super::super::runtime::handle_terminal_event(
        &mut rig.app,
        Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
    );
    assert_drag_ended(&mut rig, "key press");
}

#[test]
fn losing_terminal_focus_ends_the_drag() {
    let mut rig = drag_below(1);
    super::super::runtime::handle_terminal_event(&mut rig.app, Event::FocusLost);
    assert_drag_ended(&mut rig, "focus lost");
}

#[test]
fn switching_tabs_ends_the_drag_and_leaves_the_other_tab_alone() {
    let mut rig = Rig::sized(&forty_lines(), 40, 12);
    let body: String = (0..40).map(|n| format!("other {n}\n")).collect();
    rig.app.push_tab(text_tab("u.rs", &body));
    rig.app.select_tab(0);
    rig.render();
    let area = rig.app.editor_rect;
    let (x, _) = rig.cell(text(0, 4));
    rig.step(Step::Down(text(0, 4), KeyModifiers::NONE));
    rig.step(Step::Drag(At::Cell(x, area.bottom()), KeyModifiers::NONE));

    rig.step(Step::Cmd(Command::NextTab));
    assert_eq!(rig.app.active, 1);
    assert_drag_ended(&mut rig, "tab switch");
    assert_eq!(rig.scroll_line(), 0, "the other tab never scrolled");
    assert_eq!(head_line(&rig), None, "the other tab never selected");
}

/// A press with no release before it (the release was lost) starts a fresh
/// click instead of being swallowed by the stale drag.
#[test]
fn a_press_without_a_prior_release_is_a_fresh_click() {
    check(vec![case(
        "the second press places the caret where it landed",
        "|foo bar\nbaz qux",
        vec![
            Step::Down(text(0, 1), KeyModifiers::NONE),
            Step::Drag(text(1, 2), KeyModifiers::NONE),
            Step::Down(text(1, 5), KeyModifiers::NONE),
            Step::Up(text(1, 5), KeyModifiers::NONE),
        ],
        "foo bar\nbaz q|ux",
    )]);
}

#[test]
fn autoscroll_steps_on_a_fixed_clock_not_on_every_wake() {
    let mut rig = drag_below(1);
    let start = rig.scroll_line();
    let t0 = Instant::now();
    rig.app.tick_drag_autoscroll(t0);
    assert_eq!(rig.scroll_line(), start + 1, "the first step is immediate");
    rig.app.tick_drag_autoscroll(t0 + Duration::from_millis(10));
    assert_eq!(rig.scroll_line(), start + 1, "an early wake does not step");
    rig.app.tick_drag_autoscroll(t0 + Duration::from_millis(50));
    assert_eq!(rig.scroll_line(), start + 2, "the next interval steps");
    assert!(
        rig.app
            .drag_autoscroll_wake()
            .is_some_and(|wake| wake <= Duration::from_millis(50)),
        "the loop is told when the next step is due"
    );
}

#[test]
fn autoscroll_speed_grows_with_the_distance_past_the_edge() {
    let mut rig = drag_below(3);
    let start = rig.scroll_line();
    rig.step(Step::Tick);
    assert_eq!(
        rig.scroll_line(),
        start + 3,
        "three rows out, three per step"
    );
}

#[test]
fn dragging_past_the_right_edge_scrolls_horizontally() {
    let line: String = (0..200)
        .map(|n| char::from(b'a' + (n % 26) as u8))
        .collect();
    let mut rig = Rig::sized(&format!("|{line}\n"), 40, 8);
    let area = rig.app.editor_rect;
    let (_, y) = rig.cell(text(0, 2));
    rig.step(Step::Down(text(0, 2), KeyModifiers::NONE));
    rig.step(Step::Drag(
        At::Cell(area.right() + 1, y),
        KeyModifiers::NONE,
    ));
    let width = u32::from(rig.tab().editor.content_width());
    assert!(
        rig.tab()
            .editor
            .selection_range()
            .is_some_and(|range| range.end.col < width),
        "past the edge the selection stops at the edge"
    );
    rig.step(Step::Tick);
    assert_eq!(
        rig.tab().editor.scroll_col,
        2,
        "two columns out, two per step"
    );
    assert!(
        rig.tab()
            .editor
            .selection_range()
            .is_some_and(|range| range.end.col >= width),
        "the selection follows the columns scrolling into view"
    );
}

#[test]
fn the_wheel_during_a_drag_scrolls_and_extends() {
    let mut rig = Rig::sized(&forty_lines(), 40, 12);
    let pointer = rig.cell(text(2, 1));
    rig.step(Step::Down(text(0, 1), KeyModifiers::NONE));
    rig.step(Step::Drag(
        At::Cell(pointer.0, pointer.1),
        KeyModifiers::NONE,
    ));
    rig.step(Step::WheelDown(
        At::Cell(pointer.0, pointer.1),
        KeyModifiers::NONE,
    ));
    assert_eq!(rig.scroll_line(), 3, "the notch scrolled");
    assert_eq!(
        head_line(&rig),
        Some(5),
        "the head follows the text under it"
    );
    assert!(rig.app.editor_drag.is_some(), "the drag is still held");
}

#[test]
fn shift_wheel_during_a_drag_scrolls_columns_and_extends() {
    let line: String = (0..200)
        .map(|n| char::from(b'a' + (n % 26) as u8))
        .collect();
    let mut rig = Rig::sized(&format!("|{line}\n{line}\n"), 40, 8);
    let pointer = rig.cell(text(1, 4));
    rig.step(Step::Down(text(0, 1), KeyModifiers::NONE));
    rig.step(Step::Drag(
        At::Cell(pointer.0, pointer.1),
        KeyModifiers::NONE,
    ));
    rig.step(Step::WheelDown(
        At::Cell(pointer.0, pointer.1),
        KeyModifiers::SHIFT,
    ));
    assert_eq!(rig.scroll_line(), 0, "the notch did not scroll rows");
    assert_eq!(rig.tab().editor.scroll_col, 3, "the notch scrolled columns");
    assert_eq!(
        rig.tab().editor.cursor().col,
        7,
        "the head follows the text under it"
    );
    assert!(rig.app.editor_drag.is_some(), "the drag is still held");
}

#[test]
fn buttonless_motion_ends_a_drag_whose_release_was_lost() {
    // With mouse capture on, motion under a held button arrives as a drag, so
    // a plain move means the button is up and its release never arrived.
    let mut rig = drag_below(1);
    let (column, row) = rig.cell(text(0, 2));
    rig.app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    assert_drag_ended(&mut rig, "buttonless motion");
}

#[test]
fn sticky_header_rows_count_as_above_the_text() {
    let body: String = std::iter::once("fn main() {\n".to_string())
        .chain((1..39).map(|n| format!("    line {n}\n")))
        .chain(std::iter::once("}\n".to_string()))
        .collect();
    let mut rig = Rig::sized(&format!("|{body}"), 40, 12);
    rig.app.settings.editor.sticky_scroll = true;
    let idx = rig.app.active;
    if let TabKind::Code {
        semantic_blocks, ..
    } = &mut rig.app.tabs[idx].kind
    {
        *semantic_blocks = karet_syntax::SemanticBlocks::new(vec![karet_syntax::SemanticBlock {
            header_start: 0,
            header_end: 0,
            scope_end: 39,
        }]);
    }
    rig.app.scroll_lines(10);
    rig.render();
    let area = rig.app.editor_rect;
    let viewport = rig.tab().editor.viewport();
    assert!(
        viewport.end.line - viewport.start.line < u32::from(area.height),
        "the header is pinned above the text"
    );

    let (x, _) = rig.cell(text(14, 4));
    rig.step(Step::Down(text(14, 4), KeyModifiers::NONE));
    rig.step(Step::Drag(At::Cell(x, area.y), KeyModifiers::NONE));
    let head = head_line(&rig);
    assert!(
        head.is_some_and(|line| line >= rig.scroll_line()),
        "over the header the selection stops at the first text row, got {head:?}"
    );
    let before = rig.scroll_line();
    rig.step(Step::Tick);
    assert!(rig.scroll_line() < before, "the header row creeps upward");
}

#[test]
fn a_drag_into_another_pane_stops_at_its_own_edge() {
    let line: String = (0..100)
        .map(|n| char::from(b'a' + (n % 26) as u8))
        .collect();
    let mut rig = Rig::new(&format!("|{line}\n{line}\n"));
    rig.step(Step::Cmd(Command::SplitRight));
    rig.step(click(At::Pane(0, 0, 1)));
    rig.step(PAUSE);
    rig.step(Step::Down(At::Pane(0, 0, 1), KeyModifiers::NONE));
    rig.step(Step::Drag(At::Pane(1, 1, 3), KeyModifiers::NONE));

    assert_eq!(rig.focused_index(), Some(0), "the drag keeps its pane");
    let width = u32::from(rig.tab().editor.content_width());
    let range = rig.tab().editor.selection_range();
    assert!(
        range.is_some_and(|range| range.end.line == 1 && range.end.col < width),
        "the pointer maps to this pane's edge, not a far column: {range:?}"
    );
}

#[test]
fn a_resize_mid_drag_maps_against_the_new_viewport() {
    let mut rig = Rig::sized(&forty_lines(), 40, 8);
    rig.step(Step::Down(text(0, 1), KeyModifiers::NONE));
    rig.resize(40, 16);
    let (x, y) = rig.cell(text(9, 1));
    rig.step(Step::Drag(At::Cell(x, y), KeyModifiers::NONE));
    assert_eq!(head_line(&rig), Some(9), "the row is inside the grown view");
    assert_eq!(
        rig.app.drag_autoscroll_wake(),
        None,
        "and is not past its edge"
    );
}
