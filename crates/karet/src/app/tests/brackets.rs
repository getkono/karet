//! The Go to Bracket and Select to Bracket rows of `docs/editing-behavior.md`,
//! run through the conformance harness. The matching rules themselves (strings
//! and comments, bracket kinds, enclosing pairs) are covered in `karet_editor`.

use super::harness::*;
use crate::app::*;

/// Ctrl+Shift+\ as a terminal reporting alternate keys sends it (`|`), and as
/// one that does not (`\` with Shift).
fn go_to_bracket(alternate: bool) -> Step {
    let code = KeyCode::Char(if alternate { '|' } else { '\\' });
    Step::Key(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
}

/// Ctrl+Shift+Alt+\, in the alternate-key encoding.
fn select_to_bracket() -> Step {
    Step::Key(
        KeyCode::Char('|'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT | KeyModifiers::ALT,
    )
}

#[test]
fn go_to_bracket_jumps_between_the_ends_of_a_pair() {
    check(vec![
        case(
            "from the opening bracket to the closing one",
            "f|(a, b) x",
            vec![go_to_bracket(true)],
            "f(a, b|) x",
        ),
        case(
            "pressed again, back to the opening bracket",
            "f(a, b|) x",
            vec![go_to_bracket(false)],
            "f|(a, b) x",
        ),
        case(
            "from just after the closing bracket",
            "f(a, b)| x",
            vec![go_to_bracket(true)],
            "f|(a, b) x",
        ),
        case(
            "from inside a pair, to its closing bracket",
            "f(a|, b) x",
            vec![go_to_bracket(true)],
            "f(a, b|) x",
        ),
        case(
            "outside any pair, the caret stays",
            "f(a, b) |x",
            vec![go_to_bracket(true)],
            "f(a, b) |x",
        ),
    ]);
}

#[test]
fn select_to_bracket_selects_the_pair_then_widens() {
    check(vec![
        case(
            "the enclosing pair, brackets included",
            "{a(b|c)d}",
            vec![select_to_bracket()],
            "{a[(bc)|]d}",
        ),
        case(
            "pressed again, the pair around it",
            "{a(b|c)d}",
            vec![select_to_bracket(), select_to_bracket()],
            "[{a(bc)d}|]",
        ),
    ]);
}
