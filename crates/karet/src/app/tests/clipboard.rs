//! Clipboard rows of `docs/editing-behavior.md`: copy and cut take every
//! caret's selection, or every caret's whole line when nothing is selected, and
//! paste puts a line copy above the line and gives each caret its own piece when
//! the counts match.

use super::harness::*;
use crate::app::*;

/// A Ctrl+`c` press.
fn ctrl(c: char) -> Step {
    Step::Key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Run `steps` from `start` and return the rig's marked text and clipboard.
fn run(start: &str, steps: &[Step]) -> (String, String) {
    let mut rig = Rig::new(start);
    rig.run(steps);
    let clipboard = rig.app.clipboard.get().unwrap_or_default();
    (rig.marked(), clipboard)
}

/// Copy takes every selection, or every caret's line; the text never changes.
#[test]
fn copy_takes_every_caret() {
    let rows = [
        ("one selection", "a[bc|]d", "a[bc|]d", "bc"),
        (
            "several selections, joined in document order",
            "[ab#] x\n[cd|]",
            "[ab#] x\n[cd|]",
            "ab\ncd",
        ),
        (
            "empty carets beside a selection add nothing",
            "a|b\nc[de#]",
            "a|b\nc[de#]",
            "de",
        ),
        ("no selection copies the line", "a|b\ncd", "a|b\ncd", "ab\n"),
        (
            "no selection copies every caret's line once",
            "a|b#\ncd\ne|f",
            "a|b#\ncd\ne|f",
            "ab\nef\n",
        ),
    ];
    for (name, start, expect, clipboard) in rows {
        let (got, copied) = run(start, &[ctrl('c')]);
        assert_eq!(got, expect, "{name}");
        assert_eq!(copied, clipboard, "{name}");
    }
}

/// Cut deletes exactly what its copy took, and never an unselected character.
#[test]
fn cut_deletes_what_it_copied() {
    let rows = [
        (
            "an empty caret beside a selection keeps its text (#332)",
            "foo |bar\nfoo [bar#]",
            "foo |bar\nfoo #",
            "bar",
        ),
        ("every selection is cut", "[ab|] x [cd#]", "| x #", "ab\ncd"),
        (
            "no selection cuts the line, the caret keeps its column",
            "ab\nc|d\nef",
            "ab\ne|f",
            "cd\n",
        ),
        (
            "the last line takes the break before it",
            "ab\nc|d",
            "a|b",
            "cd\n",
        ),
        ("the only line empties the buffer", "a|b", "|", "ab\n"),
        (
            "carets on adjacent lines cut both lines",
            "a|\nb#\nc",
            "c|",
            "a\nb\n",
        ),
        (
            "carets on separate lines each cut their own",
            "a|\nb\nc#\nd",
            "b|\nd#",
            "a\nc\n",
        ),
    ];
    for (name, start, expect, clipboard) in rows {
        let (got, copied) = run(start, &[ctrl('x')]);
        assert_eq!(got, expect, "{name}");
        assert_eq!(copied, clipboard, "{name}");
    }
}

/// Paste inserts at every caret, a line copy above the line, and one piece per
/// caret when the counts match.
#[test]
fn paste_places_what_was_copied() {
    use KeyCode::*;
    check(vec![
        case(
            "a line copy pastes above the caret's line",
            "a|b\ncd",
            vec![ctrl('c'), key(Down), ctrl('v')],
            "ab\nab\nc|d",
        ),
        case(
            "a line cut pastes back above the line it moved up",
            "ab\nc|d\nef",
            vec![ctrl('x'), ctrl('v')],
            "ab\ncd\ne|f",
        ),
        case(
            "a line copy replaces a selection like any text",
            "a|b\ncd",
            vec![
                ctrl('c'),
                key(Down),
                Step::Key(Home, KeyModifiers::SHIFT),
                ctrl('v'),
            ],
            "ab\nab\n|d",
        ),
        case(
            "each caret pastes the piece it copied",
            "[x|]\n[y#]",
            vec![ctrl('c'), key(End), ctrl('v')],
            "xx|\nyy#",
        ),
        case(
            "each caret pastes the line it copied above its own line",
            "a|\nb#",
            vec![ctrl('c'), ctrl('v')],
            "a\na|\nb\nb#",
        ),
    ]);
}

/// Text from outside karet: one line per caret is distributed, anything else
/// goes in whole at every caret.
#[test]
fn paste_of_outside_text() -> Result<(), crate::clipboard::ClipboardError> {
    let rows = [
        ("one line per caret", "a|\nb#", "1\n2", "a1|\nb2#"),
        (
            "a trailing newline still counts lines",
            "a|\nb#",
            "1\n2\n",
            "a1|\nb2#",
        ),
        ("counts that differ paste whole", "a|\nb#", "z", "az|\nbz#"),
        ("one caret pastes whole", "a|", "1\n2", "a1\n2|"),
    ];
    for (name, start, clipboard, expect) in rows {
        let mut rig = Rig::new(start);
        rig.app.clipboard.set(clipboard)?;
        rig.run(&[ctrl('v')]);
        assert_eq!(rig.marked(), expect, "{name}");
    }
    Ok(())
}

/// The line-copy marker belongs to the text karet wrote: the same text from
/// elsewhere pastes at the caret.
#[test]
fn outside_text_equal_to_a_line_has_no_marker() -> Result<(), crate::clipboard::ClipboardError> {
    let mut rig = Rig::new("a|b");
    rig.run(&[ctrl('c')]);
    rig.app.clipboard.set("ab\n")?;
    rig.run(&[ctrl('v')]);
    assert_eq!(rig.marked(), "aab\n|b");
    Ok(())
}
