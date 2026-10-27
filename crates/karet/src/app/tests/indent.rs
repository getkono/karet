//! Indentation rows of `docs/editing-behavior.md`: Enter indents from the
//! language's indent query, Tab indents and Shift+Tab
//! outdents at every caret, keeping the selections, while an open completion
//! popup and an inline macro take Tab first and focus cycling moves to
//! Ctrl+Shift+E.

use super::harness::*;
use super::support::*;
use crate::app::*;

fn shift_tab() -> Step {
    Step::Key(KeyCode::Tab, KeyModifiers::SHIFT)
}

#[test]
fn tab_indents() {
    check(vec![
        case(
            "Tab pads a bare caret to the next indent stop",
            "ab|c",
            vec![key(KeyCode::Tab)],
            "ab  |c",
        ),
        case(
            "Tab on an indent stop inserts a whole level",
            "    |x",
            vec![key(KeyCode::Tab)],
            "        |x",
        ),
        case(
            "Tab indents every line a selection touches and keeps the selection",
            "o[ne\ntw|]o",
            vec![key(KeyCode::Tab)],
            "    o[ne\n    tw|]o",
        ),
        case(
            "Tab keeps a backward whole-line selection whole",
            "[|one\ntwo\n]three",
            vec![key(KeyCode::Tab)],
            "[|    one\n    two\n]three",
        ),
        case(
            "Tab indents a single-line selection instead of replacing it",
            "let [x|]",
            vec![key(KeyCode::Tab)],
            "    let [x|]",
        ),
        case(
            "Tab works at every caret",
            "|a\nb#",
            vec![key(KeyCode::Tab)],
            "    |a\nb   #",
        ),
    ]);
}

#[test]
fn shift_tab_outdents() {
    check(vec![
        case(
            "Shift+Tab removes one level and keeps the selection",
            "        [on|]e",
            vec![shift_tab()],
            "    [on|]e",
        ),
        case(
            "Shift+Tab snaps off-grid indentation to the previous stop",
            "      x|",
            vec![shift_tab()],
            "    x|",
        ),
        case(
            "Shift+Tab outdents every caret's line once",
            "    a| b#\n  |c",
            vec![shift_tab()],
            "a| b#\n|c",
        ),
        case(
            "a terminal's BackTab is Shift+Tab",
            "\t[x\n\ty|]",
            vec![key(KeyCode::BackTab)],
            "[x\ny|]",
        ),
        case(
            "Shift+Tab on an unindented line changes nothing",
            "a|b",
            vec![shift_tab()],
            "a|b",
        ),
    ]);
}

#[test]
fn tab_then_shift_tab_round_trips() {
    check(vec![case(
        "indenting and outdenting a selection restores text and selection",
        "[fn a() {}\n    let |]x",
        vec![key(KeyCode::Tab), shift_tab()],
        "[fn a() {}\n    let |]x",
    )]);
}

#[test]
fn an_inline_macro_takes_tab_first() {
    check(vec![case(
        "Tab after `fn` expands the Rust function macro rather than indenting",
        "fn|",
        vec![key(KeyCode::Tab)],
        "fn |() {\n    \n}",
    )]);
}

#[test]
fn an_open_completion_takes_tab_first() {
    let mut rig = Rig::new("fo|");
    rig.app.completion.popup = Some(crate::completion::CompletionUi::new(
        DocumentId(9),
        LineCol::new(0, 0),
        LineCol::new(0, 0),
        vec![karet_core::CompletionItem::new(
            "foobar",
            karet_core::CompletionKind::Function,
        )],
        crate::completion::CompletionMode::Filtered,
    ));
    rig.render();
    rig.step(key(KeyCode::Tab));
    assert_eq!(rig.marked(), "foobar|");
    assert!(rig.app.completion.popup.is_none());
}

#[test]
fn tab_in_the_editor_keeps_focus_and_ctrl_shift_e_toggles_it() {
    let mut rig = Rig::new("a|");
    rig.step(key(KeyCode::Tab));
    assert_eq!(rig.app.focus, Focus::Editor);
    rig.app.sidebar_visible = true;
    rig.step(Step::Key(
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ));
    assert_eq!(rig.app.focus, Focus::Sidebar);
    // From the sidebar, Tab still returns to the editor.
    rig.step(key(KeyCode::Tab));
    assert_eq!(rig.app.focus, Focus::Editor);
}

#[test]
fn enter_indents_from_the_rust_indent_query() {
    check(vec![
        case(
            "after an opener with no closer yet, one level deeper",
            "fn f() {|",
            vec![key(KeyCode::Enter)],
            "fn f() {\n    |",
        ),
        case(
            "inside a block, at the body indentation the block already has",
            "fn f() {\n  x();|\n}",
            vec![key(KeyCode::Enter)],
            "fn f() {\n  x();\n  |\n}",
        ),
        case(
            "after an opener in a tab-indented file, one tab deeper",
            "fn g() {\n\ty();\n}\nfn f() {|",
            vec![key(KeyCode::Enter)],
            "fn g() {\n\ty();\n}\nfn f() {\n\t|",
        ),
        case(
            "before a closer, at the opener's level",
            "    fn f() {\n        x();|}",
            vec![key(KeyCode::Enter)],
            "    fn f() {\n        x();\n    |}",
        ),
        case(
            "past a closed block, a copy of the current line",
            "    fn f() {}|",
            vec![key(KeyCode::Enter)],
            "    fn f() {}\n    |",
        ),
    ]);
}

#[test]
fn tab_without_editable_text_still_toggles_focus() {
    let mut app = app();
    app.focus = Focus::Editor;
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Sidebar);
}

#[test]
fn the_palettes_indent_line_without_editable_text_does_nothing() {
    let mut app = app();
    app.focus = Focus::Editor;
    app.handle_overlay_event(crate::overlay::OverlayEvent::AcceptCommand(Command::Indent));
    assert_eq!(app.focus, Focus::Editor);
}

#[test]
fn the_palettes_indent_line_indents_the_active_code_tab() {
    let mut rig = Rig::new("ab|c");
    rig.app
        .handle_overlay_event(crate::overlay::OverlayEvent::AcceptCommand(Command::Indent));
    assert_eq!(rig.app.focus, Focus::Editor);
    assert_eq!(rig.marked(), "ab  |c");
}
