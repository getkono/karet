//! The workspace Search panel's key resolution: the list-only arrows, the
//! deliberately unbound ones in a field, and the Global chords the Search modals
//! still let through. Split out of `tests.rs` to keep that file under the
//! workspace code-line ceiling.

use super::*;

/// The arrows never cross the field/list seam. In the list they move the row
/// cursor, exactly as `j`/`k` do; in a field they resolve to nothing at all, so
/// they fall through to `search_edit` rather than lifting the caret out.
#[test]
fn search_arrows_stay_inside_the_surface_they_start_in() {
    let plain = |code| [KeyChord::from_event(key(code, KeyModifiers::NONE))];
    let list = Context::modal(Modal::SearchList, FocusTarget::Search);
    for (code, command) in [
        (KeyCode::Up, Command::SearchSelectUp),
        (KeyCode::Down, Command::SearchSelectDown),
        (KeyCode::Char('k'), Command::SearchSelectUp),
        (KeyCode::Char('j'), Command::SearchSelectDown),
    ] {
        assert_eq!(resolve(list, &plain(code)), Resolved::Command(command));
    }

    let input = Context::modal(Modal::SearchInput, FocusTarget::Search);
    for code in [KeyCode::Up, KeyCode::Down] {
        assert_eq!(
            resolve(input, &plain(code)),
            Resolved::None,
            "{code:?} belongs to the field, not the panel"
        );
    }
}

/// `Cmd`+arrow canonicalizes to `Ctrl+Home`/`Ctrl+End`, which is likewise unbound
/// in the Search modal — so it reaches `search_edit` and moves the caret.
#[test]
fn a_command_arrow_falls_through_to_the_field_editor() {
    let input = Context::modal(Modal::SearchInput, FocusTarget::Search);
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right] {
        assert_eq!(
            resolve(
                input,
                &[KeyChord::from_event(key(code, KeyModifiers::SUPER))],
            ),
            Resolved::None,
            "{code:?}"
        );
    }
}

#[test]
fn search_modal_still_resolves_global_chords() {
    // The Search modals layer their own keys over Global, so Ctrl+B still toggles
    // the sidebar while a bare 'j' navigates the results rather than typing.
    let list = Context::modal(Modal::SearchList, FocusTarget::Search);
    assert_eq!(
        resolve(
            list,
            &[KeyChord::from_event(key(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::Command(Command::ToggleSidebar)
    );
    let input = Context::modal(Modal::SearchInput, FocusTarget::Search);
    assert_eq!(
        resolve(
            input,
            &[KeyChord::from_event(key(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::Command(Command::Cut)
    );
    let commit = Context::modal(Modal::CommitInput, FocusTarget::SourceControl);
    assert_eq!(
        resolve(
            commit,
            &[KeyChord::from_event(key(
                KeyCode::Char('a'),
                KeyModifiers::SUPER
            ))]
        ),
        Resolved::Command(Command::EditorSelectAll)
    );
    // A plain overlay is exclusive: Ctrl+B does not leak through to Global.
    let overlay = Context::modal(Modal::Overlay, FocusTarget::Editor);
    assert_eq!(
        resolve(
            overlay,
            &[KeyChord::from_event(key(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::None
    );
}
