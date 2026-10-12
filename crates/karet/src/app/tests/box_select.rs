//! The box-selection and middle-click rows of `docs/editing-behavior.md`, run
//! through the conformance harness.

use super::harness::*;
use super::support::last_message;
use crate::app::*;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const ALT: KeyModifiers = KeyModifiers::ALT;

/// Ctrl+Shift+Alt+`code`: one keyboard step of the box.
fn boxed(code: KeyCode) -> Step {
    Step::Key(code, CTRL | SHIFT | ALT)
}

/// Shift+Alt+click and Shift+Alt+drag box-select from the caret (or the box in
/// progress) to the pointer, one selection per line.
#[test]
fn shift_alt_pointer_gestures_select_a_box() {
    check(vec![
        case(
            "Shift+Alt+click boxes from the caret to the click",
            "a|bcdef\nabcdef\nabcdef",
            vec![Step::Click(text(2, 4), SHIFT | ALT)],
            "a[bcd|]ef\na[bcd|]ef\na[bcd#]ef",
        ),
        case(
            "Shift+Alt+drag follows the pointer, up and back as well as out",
            "a|bcdef\nabcdef\nabcdef",
            vec![
                Step::Down(text(2, 2), SHIFT | ALT),
                Step::Drag(text(2, 5), SHIFT | ALT),
                Step::Drag(text(1, 5), SHIFT | ALT),
                Step::Up(text(1, 5), SHIFT | ALT),
            ],
            "a[bcde|]f\na[bcde#]f\nabcdef",
        ),
        case(
            "A second Shift+Alt+click grows the box from its first corner",
            "a|bcdef\nabcdef\nabcdef",
            vec![
                Step::Click(text(1, 3), SHIFT | ALT),
                Step::Click(text(2, 4), SHIFT | ALT),
            ],
            "a[bcd|]ef\na[bcd|]ef\na[bcd#]ef",
        ),
        case(
            "A box drawn up and left selects backward, primary on the pointer's line",
            "abcdef\nabcdef\nabc|def",
            vec![Step::Click(text(0, 1), SHIFT | ALT)],
            "a[#bc]def\na[|bc]def\na[|bc]def",
        ),
        case(
            "A line ending left of the box gets no caret",
            "ab|cdef\nx\nabcdef",
            vec![Step::Click(text(2, 5), SHIFT | ALT)],
            "ab[cde|]f\nx\nab[cde#]f",
        ),
        case(
            "A line ending inside the box is selected to its end",
            "a|bcdef\nab\nabcdef",
            vec![Step::Click(text(2, 4), SHIFT | ALT)],
            "a[bcd|]ef\na[b|]\na[bcd#]ef",
        ),
    ]);
}

/// Ctrl+Shift+Alt+arrows are the keyboard path to the same box, for terminals
/// and window managers that never deliver Shift+Alt+drag.
#[test]
fn ctrl_shift_alt_arrows_grow_a_box() {
    use KeyCode::*;
    check(vec![
        case(
            "Right and Down grow the box from the caret",
            "a|bcdef\nabcdef\nabcdef",
            vec![boxed(Right), boxed(Right), boxed(Down), boxed(Down)],
            "a[bc|]def\na[bc|]def\na[bc#]def",
        ),
        case(
            "Left and Up grow it the other way",
            "abcdef\nabcdef\nabc|def",
            vec![boxed(Left), boxed(Left), boxed(Up)],
            "abcdef\na[#bc]def\na[|bc]def",
        ),
        case(
            "Up shrinks a box drawn down",
            "a|bc\nabc\nabc",
            vec![boxed(Right), boxed(Down), boxed(Down), boxed(Up)],
            "a[b|]c\na[b#]c\nabc",
        ),
        case(
            "Right stops at the longest line the box spans",
            "a|b\nabc",
            vec![boxed(Right), boxed(Right), boxed(Right)],
            "a[b|]\nabc",
        ),
        case(
            "A box selection starts from an existing selection",
            "a[bc|]d\nabcd",
            vec![boxed(Down)],
            "a[bc|]d\na[bc#]d",
        ),
        case(
            "Columns are display cells: a tab spans to its stop",
            "|\tx\nabcdefgh",
            vec![
                boxed(Right),
                boxed(Right),
                boxed(Right),
                boxed(Right),
                boxed(Down),
            ],
            "[\t|]x\n[abcd#]efgh",
        ),
        case(
            "Another selection change ends the box; the next step starts afresh",
            "a|bc\nabc\nabc",
            vec![boxed(Down), PAUSE, click(text(0, 2)), boxed(Down)],
            "ab|c\nab#c\nabc",
        ),
    ]);
}

/// The pointer's cell is kept past a line's end, so the box stays as wide as
/// the pointer left it when it next reaches a longer line.
#[test]
fn a_box_remembers_a_column_past_the_line_end() {
    use KeyCode::*;
    check(vec![case(
        "Shift+Alt+click past the end, then Ctrl+Shift+Alt+Down",
        "a|bc\nabcdef",
        vec![
            Step::Click(At::Offset(0, 3, 2, 0), SHIFT | ALT),
            boxed(Down),
        ],
        "a[bc|]\na[bcde#]f",
    )]);
}

/// A rig whose middle click pastes `primary`, whatever the platform's default.
fn middle_click_rig(marked: &str, primary: Option<&str>, enabled: bool) -> Rig {
    let mut rig = Rig::new(marked);
    rig.app.settings.editor.middle_click_paste = Some(enabled);
    rig.app.clipboard.primary = primary.map(str::to_string);
    rig
}

#[test]
fn a_middle_click_pastes_the_primary_selection_at_the_click() {
    let mut rig = middle_click_rig("ab|cd\nef", Some("XY"), true);
    rig.step(Step::MiddleClick(text(1, 1), KeyModifiers::NONE));
    assert_eq!(rig.marked(), "abcd\neXY|f");
}

#[test]
fn a_middle_click_with_several_carets_pastes_at_the_click_only() {
    let mut rig = middle_click_rig("|ab\n#cd", Some("-"), true);
    rig.step(Step::MiddleClick(text(0, 1), KeyModifiers::NONE));
    assert_eq!(rig.marked(), "a-|b\ncd");
}

#[test]
fn a_middle_click_does_nothing_with_the_setting_off() {
    let mut rig = middle_click_rig("ab|cd", Some("XY"), false);
    rig.step(Step::MiddleClick(text(0, 0), KeyModifiers::NONE));
    assert_eq!(rig.marked(), "ab|cd");
}

#[test]
fn a_middle_click_without_a_primary_selection_says_so_and_edits_nothing() {
    let mut rig = middle_click_rig("ab|cd", None, true);
    rig.step(Step::MiddleClick(text(0, 0), KeyModifiers::NONE));
    assert_eq!(rig.marked(), "ab|cd");
    assert_eq!(
        last_message(&rig.app).as_deref(),
        Some("paste: no primary selection available")
    );
}

#[test]
fn a_middle_click_on_a_tab_still_closes_it_rather_than_pasting() {
    let mut rig = middle_click_rig("ab|cd", Some("XY"), true);
    let tab = rig
        .app
        .pane_frames
        .iter()
        .find_map(|frame| Some((frame.tab_hits.first()?.start, frame.tabstrip_rect.y)));
    let Some((col, row)) = tab else {
        panic!("the tab strip was not drawn");
    };
    rig.step(Step::MiddleClick(At::Cell(col, row), KeyModifiers::NONE));
    // The sole tab gives way to the Welcome tab.
    assert!(
        rig.app
            .tabs
            .iter()
            .all(|tab| !matches!(tab.kind, TabKind::Code { .. })),
        "the code tab closed"
    );
}
