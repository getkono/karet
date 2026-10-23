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

/// Ctrl+T is Go to Symbol in Workspace from anywhere; Ctrl+Shift+T still
/// reopens the last closed tab, and Ctrl+K Ctrl+T still toggles inlay hints.
#[test]
fn ctrl_t_goes_to_a_workspace_symbol_beside_its_neighbours() {
    let ctrl_t = key(KeyCode::Char('t'), KeyModifiers::CONTROL);
    for focus in [Focus::Editor, Focus::Sidebar] {
        assert_eq!(
            res(focus, false, ctrl_t),
            Some(Command::GoToSymbolInWorkspace)
        );
    }
    let ctrl_shift_t = key(
        KeyCode::Char('T'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(
        res(Focus::Editor, false, ctrl_shift_t),
        Some(Command::ReopenClosedTab)
    );
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(
        resolve(
            Context::focus(FocusTarget::Editor),
            &[ctrl_k, KeyChord::from_event(ctrl_t)]
        ),
        Resolved::Command(Command::ToggleInlayHints)
    );
    assert_eq!(
        hint_for(Command::GoToSymbolInWorkspace, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+T")
    );
}

/// VS Code parity: Ctrl+Shift+F2 starts linked editing, beside F2's rename.
#[test]
fn ctrl_shift_f2_starts_linked_editing_in_the_editor() {
    let chord = key(KeyCode::F(2), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    assert_eq!(
        res(Focus::Editor, false, chord),
        Some(Command::StartLinkedEditing)
    );
    assert_eq!(
        res(Focus::Editor, false, key(KeyCode::F(2), KeyModifiers::NONE)),
        Some(Command::RenameSymbol)
    );
}

/// VS Code's Show Call Hierarchy chord, Shift+Alt+H, opens the incoming
/// calls from the editor only; the other directions are palette commands.
#[test]
fn shift_alt_h_shows_incoming_calls_from_the_editor() {
    let shift_alt_h = key(KeyCode::Char('H'), KeyModifiers::SHIFT | KeyModifiers::ALT);
    assert_eq!(
        res(Focus::Editor, false, shift_alt_h),
        Some(Command::ShowIncomingCalls)
    );
    assert_ne!(
        res(Focus::Sidebar, false, shift_alt_h),
        Some(Command::ShowIncomingCalls)
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
