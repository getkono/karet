//! The workspace Search panel's key resolution: the focus-ring arrows, the
//! list-only `j`/`k`, and the Global chords the Search modals still let through.
//! Split out of `tests.rs` to keep that file under the workspace code-line
//! ceiling.

use super::*;

/// The arrows walk the panel's one focus ring, in both modals; `j`/`k` stay
/// list-only so a vim-style browse never drops into a text field.
#[test]
fn search_arrows_walk_the_focus_ring_in_both_modals() {
    let plain = |code| [KeyChord::from_event(key(code, KeyModifiers::NONE))];
    for modal in [Modal::SearchList, Modal::SearchInput] {
        let ctx = Context::modal(modal, FocusTarget::Search);
        assert_eq!(
            resolve(ctx, &plain(KeyCode::Up)),
            Resolved::Command(Command::SearchFocusUp),
            "{modal:?}"
        );
        assert_eq!(
            resolve(ctx, &plain(KeyCode::Down)),
            Resolved::Command(Command::SearchFocusDown),
            "{modal:?}"
        );
    }
    let list = Context::modal(Modal::SearchList, FocusTarget::Search);
    assert_eq!(
        resolve(list, &plain(KeyCode::Char('k'))),
        Resolved::Command(Command::SearchSelectUp)
    );
    assert_eq!(
        resolve(list, &plain(KeyCode::Char('j'))),
        Resolved::Command(Command::SearchSelectDown)
    );
}

/// `Cmd`+arrow canonicalizes to `Ctrl+Home`/`Ctrl+End`, so the new plain-arrow
/// bindings cannot swallow the caret motions `search_edit` handles.
#[test]
fn a_command_arrow_does_not_resolve_to_a_search_focus_step() {
    let input = Context::modal(Modal::SearchInput, FocusTarget::Search);
    for code in [KeyCode::Up, KeyCode::Down] {
        let resolved = resolve(
            input,
            &[KeyChord::from_event(key(code, KeyModifiers::SUPER))],
        );
        assert_ne!(resolved, Resolved::Command(Command::SearchFocusUp));
        assert_ne!(resolved, Resolved::Command(Command::SearchFocusDown));
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
