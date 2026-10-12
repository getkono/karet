//! The code-navigation chords beside F12, and the formatting chords. Split out of `tests.rs` to keep that
//! file under the workspace code-line ceiling.

use super::*;

/// VS Code's chords: Shift+F12 lists references and Ctrl+F12 jumps to the
/// implementations — and, like F12, only from the editor, so neither moves
/// the caret out from under a user working in the sidebar.
#[test]
fn shift_and_ctrl_f12_navigate_from_the_editor_only() {
    for (mods, command) in [
        (KeyModifiers::SHIFT, Command::FindReferences),
        (KeyModifiers::CONTROL, Command::GoToImplementation),
    ] {
        let chord = key(KeyCode::F(12), mods);
        assert_eq!(res(Focus::Editor, false, chord), Some(command));
        assert_ne!(res(Focus::Sidebar, false, chord), Some(command));
    }
}

/// Ctrl+G is Go to Line from anywhere, except where a text field already
/// owns it: the find bar steps to the next match with it.
#[test]
fn ctrl_g_goes_to_a_line_but_the_find_bar_keeps_find_next() {
    let ctrl_g = key(KeyCode::Char('g'), KeyModifiers::CONTROL);
    assert_eq!(res(Focus::Editor, false, ctrl_g), Some(Command::GoToLine));
    assert_eq!(res(Focus::Sidebar, false, ctrl_g), Some(Command::GoToLine));
    let chord = KeyChord::from_event(ctrl_g);
    assert_eq!(
        resolve(Context::modal(Modal::Find, FocusTarget::Editor), &[chord]),
        Resolved::Command(Command::FindNext)
    );
    assert_eq!(
        hint_for(Command::GoToLine, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+G")
    );
}

/// VS Code's formatting chords, from the editor only: Shift+Alt+F formats the
/// document and `Ctrl+K Ctrl+F` the selections.
#[test]
fn formatting_chords_bind_in_the_editor() {
    let shift_alt_f = key(KeyCode::Char('F'), KeyModifiers::SHIFT | KeyModifiers::ALT);
    assert_eq!(
        res(Focus::Editor, false, shift_alt_f),
        Some(Command::FormatDocument)
    );
    assert_ne!(
        res(Focus::Sidebar, false, shift_alt_f),
        Some(Command::FormatDocument)
    );
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let ctrl_f = KeyChord::from_event(key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert_eq!(
        resolve(Context::focus(FocusTarget::Editor), &[ctrl_k, ctrl_f]),
        Resolved::Command(Command::FormatSelection)
    );
    assert_eq!(
        hint_for(Command::FormatSelection, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+K Ctrl+F")
    );
}
