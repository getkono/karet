//! The Expand Selection and Shrink Selection rows of `docs/editing-behavior.md`,
//! run through the conformance harness over a Rust buffer (`t.rs`). Which step
//! each caret takes is covered in `karet_editor`; the node chains themselves,
//! for Rust, TypeScript and Python, in `karet_treesitter`.

use super::harness::*;
use crate::app::*;

/// Shift+Alt+Right.
fn expand() -> Step {
    Step::Key(KeyCode::Right, KeyModifiers::SHIFT | KeyModifiers::ALT)
}

/// Shift+Alt+Left.
fn shrink() -> Step {
    Step::Key(KeyCode::Left, KeyModifiers::SHIFT | KeyModifiers::ALT)
}

#[test]
fn expand_selection_grows_along_the_syntax_tree() {
    check(vec![
        case(
            "a caret in a word selects the word",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand()],
            "fn f() { g(\"[hello|] world\", 2); }",
        ),
        case(
            "then the string's contents, the string, the arguments, the call",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), expand(), expand()],
            "fn f() { [g(\"hello world\", 2)|]; }",
        ),
        case(
            "the string's contents before its quotes",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand()],
            "fn f() { g(\"[hello world|]\", 2); }",
        ),
        case(
            "every caret grows on its own",
            "fn f() { g(a|b, c#d); }",
            vec![expand()],
            "fn f() { g([ab|], [cd#]); }",
        ),
        case(
            "carets that grow into one node merge",
            "fn f() { g(a|b, c#d); }",
            vec![expand(), expand()],
            "fn f() { g[(ab, cd)|]; }",
        ),
    ]);
}

#[test]
fn shrink_selection_restores_what_expand_grew_from() {
    check(vec![
        case(
            "back through every step to the caret",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), shrink(), shrink(), shrink()],
            "fn f() { g(\"hel|lo world\", 2); }",
        ),
        case(
            "one step back",
            "fn f() { g(\"hel|lo world\", 2); }",
            vec![expand(), expand(), expand(), shrink()],
            "fn f() { g(\"[hello world|]\", 2); }",
        ),
        case(
            "merged carets come back apart, with the primary",
            "fn f() { g(a|b, c#d); }",
            vec![expand(), expand(), shrink(), shrink()],
            "fn f() { g(a|b, c#d); }",
        ),
        case(
            "a backward selection keeps its direction",
            "fn f() { g([|ab], c); }",
            vec![expand(), shrink()],
            "fn f() { g([|ab], c); }",
        ),
        case(
            "with nothing expanded, shrink does nothing",
            "fn f() { g([ab|], c); }",
            vec![shrink()],
            "fn f() { g([ab|], c); }",
        ),
        case(
            "any other selection change ends the history",
            "fn f() { g(a|b, c); }",
            vec![expand(), key(KeyCode::Right), shrink()],
            "fn f() { g(ab|, c); }",
        ),
    ]);
}
