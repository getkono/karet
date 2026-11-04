//! The keyboard paths for pointer gestures a terminal may swallow. Split out
//! of `tests.rs` to keep that file under the workspace code-line ceiling.

use super::*;

/// The keyboard paths `docs/editing-behavior.md` lists for the pointer
/// gestures a terminal may swallow (its "Modifiers terminals may not deliver"
/// table) are all bound, so none of those gestures is the only way in.
#[test]
fn every_swallowable_gesture_has_a_bound_keyboard_path() {
    use KeyCode::*;
    let ctrl = KeyModifiers::CONTROL;
    let shift = KeyModifiers::SHIFT;
    let alt = KeyModifiers::ALT;
    let none = KeyModifiers::NONE;
    for (code, modifiers, command) in [
        // Ctrl/Super+click: go to definition.
        (F(12), none, Command::GoToDefinition),
        // Alt+click: add carets.
        (Up, ctrl | alt, Command::AddCursorAbove),
        (Down, ctrl | alt, Command::AddCursorBelow),
        (Char('d'), ctrl, Command::AddCursorNextOccurrence),
        // Shift+click: extend the selection.
        (Left, shift, Command::SelectLeft),
        (Right, shift, Command::SelectRight),
        (Up, shift, Command::SelectUp),
        (Down, shift, Command::SelectDown),
        // Shift+Alt+drag: box selection.
        (Up, ctrl | shift | alt, Command::ColumnSelectUp),
        (Down, ctrl | shift | alt, Command::ColumnSelectDown),
        (Left, ctrl | shift | alt, Command::ColumnSelectLeft),
        (Right, ctrl | shift | alt, Command::ColumnSelectRight),
    ] {
        assert_eq!(
            res(Focus::Editor, false, key(code, modifiers)),
            Some(command),
            "{modifiers:?}+{code:?}"
        );
    }
    // Ctrl/Super+click on a link written in the source: open it (Ctrl+K Enter).
    let ctx = Context::focus(FocusTarget::Editor);
    let ctrl_k = KeyChord::from_event(key(Char('k'), ctrl));
    let enter = KeyChord::from_event(key(Enter, none));
    assert_eq!(
        resolve(ctx, &[ctrl_k, enter]),
        Resolved::Command(Command::OpenLinkAtCaret)
    );
}
