//! The multi-cursor chords beyond Ctrl+D. Split out of `tests.rs` to keep that
//! file under the workspace code-line ceiling.

use super::*;

/// Ctrl+Shift+L, Shift+Alt+I and Ctrl+U are the editor's (VS Code parity),
/// under either way a terminal reports a shifted letter, and only the
/// editor's: elsewhere they do not reach a caret command.
#[test]
fn multi_cursor_single_chords_bind_in_the_editor_only() {
    let ctrl_shift = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    let shift_alt = KeyModifiers::SHIFT | KeyModifiers::ALT;
    for (code, mods, command) in [
        (
            KeyCode::Char('l'),
            ctrl_shift,
            Command::SelectAllOccurrences,
        ),
        (
            KeyCode::Char('L'),
            ctrl_shift,
            Command::SelectAllOccurrences,
        ),
        (KeyCode::Char('i'), shift_alt, Command::AddCursorsToLineEnds),
        (KeyCode::Char('I'), shift_alt, Command::AddCursorsToLineEnds),
        (
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
            Command::UndoCaretOperation,
        ),
    ] {
        let chord = key(code, mods);
        assert_eq!(
            res(Focus::Editor, false, chord),
            Some(command),
            "{mods:?}+{code:?}"
        );
        assert_ne!(res(Focus::Sidebar, false, chord), Some(command));
    }
}

/// Ctrl+K Ctrl+D moves the last occurrence on, and its hint names the chord.
#[test]
fn ctrl_k_ctrl_d_skips_an_occurrence() {
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let ctrl_d = KeyChord::from_event(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_eq!(
        resolve(Context::focus(FocusTarget::Editor), &[ctrl_k, ctrl_d]),
        Resolved::Command(Command::SkipOccurrence)
    );
    // Ctrl+D alone still adds the next occurrence.
    assert_eq!(
        resolve(Context::focus(FocusTarget::Editor), &[ctrl_d]),
        Resolved::Command(Command::AddCursorNextOccurrence)
    );
    assert_eq!(
        hint_for(Command::SkipOccurrence, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+K Ctrl+D")
    );
}

/// Every new caret command is offered in the palette under its own title.
#[test]
fn multi_cursor_commands_are_in_the_palette() {
    for (command, title) in [
        (Command::SelectAllOccurrences, "Select All Occurrences"),
        (
            Command::SkipOccurrence,
            "Move Last Selection to Next Occurrence",
        ),
        (Command::UndoCaretOperation, "Undo Last Cursor Operation"),
        (Command::AddCursorsToLineEnds, "Add Cursors to Line Ends"),
    ] {
        assert!(crate::command::palette().contains(&command), "{command:?}");
        assert_eq!(crate::command::resolve_named(title), Ok(command));
    }
}
