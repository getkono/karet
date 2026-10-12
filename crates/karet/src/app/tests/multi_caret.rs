//! Multi-caret motion, merging, primary and `Esc` rows of
//! `docs/editing-behavior.md`, run through the conformance harness.

use super::harness::*;
use crate::app::*;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const ALT: KeyModifiers = KeyModifiers::ALT;

/// A plain motion collapses a selection to its edge in the motion's direction
/// before it moves; Left and Right stop at that edge.
#[test]
fn plain_motions_collapse_a_selection_first() {
    use KeyCode::*;
    check(vec![
        case(
            "Left collapses a forward selection to its start",
            "a[bcd|]e",
            vec![key(Left)],
            "a|bcde",
        ),
        case(
            "Right collapses a backward selection to its end",
            "a[|bcd]e",
            vec![key(Right)],
            "abcd|e",
        ),
        case(
            "Left after Ctrl+A lands at the start of the document",
            "ab\n|cd",
            vec![Step::Key(Char('a'), CTRL), key(Left)],
            "|ab\ncd",
        ),
        case(
            "Up moves from the start of a forward selection",
            "abc\nd[ef\ngh|]i\njkl",
            vec![key(Up)],
            "a|bc\ndef\nghi\njkl",
        ),
        case(
            "Down moves from the end of a backward selection",
            "abc\nd[|ef\ngh]i\njkl",
            vec![key(Down)],
            "abc\ndef\nghi\njk|l",
        ),
    ]);
}

/// Every caret moves on its own, and carets merge only when they land on the
/// same position.
#[test]
fn plain_motions_keep_every_caret() {
    use KeyCode::*;
    check(vec![
        case(
            "Down after Ctrl+Alt+Down moves both carets",
            "a|bc\nabc\nabc",
            vec![Step::Key(Down, CTRL | ALT), key(Down)],
            "abc\na|bc\na#bc",
        ),
        case(
            "Right moves adjacent carets without merging them",
            "a|b#cd",
            vec![key(Right)],
            "ab|c#d",
        ),
        case(
            "Ctrl+Home leaves one caret at the start of the document",
            "|abc\nabc\nabc",
            vec![
                click(text(0, 1)),
                PAUSE,
                Step::Click(text(2, 1), ALT),
                Step::Key(Home, CTRL),
            ],
            "|abc\nabc\nabc",
        ),
    ]);
}

/// Touching selections stay apart; overlapping ones merge and keep their
/// direction.
#[test]
fn selections_merge_only_when_they_overlap() {
    use KeyCode::*;
    let left = Step::Key(Left, SHIFT);
    check(vec![
        case(
            "Shift+Left until two backward selections touch",
            "abcd|efgh#ij",
            vec![left, left, left, left],
            "[|abcd][#efgh]ij",
        ),
        case(
            "an overlap merges into one backward selection",
            "abcd|efgh#ij",
            vec![left, left, left, left, left, Step::Key(Right, SHIFT)],
            "a[|bcdefgh]ij",
        ),
        case(
            "Ctrl+D keeps adjacent matches apart",
            "[aa|]aa",
            vec![Step::Key(Char('d'), CTRL)],
            "[aa|][aa#]",
        ),
    ]);
}

/// Removing a caret with Alt+click leaves the primary on the same caret.
#[test]
fn alt_click_removal_keeps_the_primary() {
    check(vec![case(
        "remove the first caret while the middle one is primary",
        "a|bc\nabc\na#bc",
        vec![
            Step::Click(text(1, 1), ALT),
            PAUSE,
            Step::Click(text(0, 1), ALT),
        ],
        "abc\na#bc\na|bc",
    )]);
}

/// `Esc` clears a lone selection, and with several carets keeps the first one
/// placed, with its selection.
#[test]
fn esc_keeps_the_original_caret_then_clears_its_selection() {
    use KeyCode::*;
    let esc = key(Esc);
    check(vec![
        case(
            "Esc clears the selection of a single caret",
            "a[bc|]d",
            vec![esc],
            "abc|d",
        ),
        case(
            "Esc keeps the original caret, not the newest",
            "a\nb|\nc",
            vec![Step::Key(Up, CTRL | ALT), esc],
            "a\nb|\nc",
        ),
        case(
            "Esc keeps the original caret's selection",
            "[ab|]\ncd",
            vec![Step::Key(Down, CTRL | ALT), esc],
            "[ab|]\ncd",
        ),
        case(
            "a second Esc clears that selection",
            "[ab|]\ncd",
            vec![Step::Key(Down, CTRL | ALT), esc, esc],
            "ab|\ncd",
        ),
    ]);
}
