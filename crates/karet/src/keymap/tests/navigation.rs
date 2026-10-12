//! The code-navigation chords beside F12. Split out of `tests.rs` to keep that
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
