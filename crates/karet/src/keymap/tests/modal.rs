//! The modal editing layers (`editor.keymap`): the Normal layers resolve
//! alone and outrank the grammar; the layers under them answer only modified
//! chords; Insert mode is the editor's keymap plus `Esc`.

use super::*;

/// The chords of `keys` (`"g d"`), each a plain character.
fn plain(keys: &str) -> Vec<KeyChord> {
    keys.split(' ')
        .filter_map(|k| k.chars().next())
        .map(|c| KeyChord::from_event(key(Char(c), KeyModifiers::NONE)))
        .collect()
}

#[test]
fn the_vim_normal_layer_resolves_alone_and_sequences_wait() {
    let ctx = Context::focus(FocusTarget::Editor).with_editing(Editing::VimNormal);
    assert_eq!(
        resolve_normal(ctx, &plain("u")),
        Resolved::Command(Command::Undo)
    );
    assert_eq!(resolve_normal(ctx, &plain("g")), Resolved::Pending);
    assert_eq!(
        resolve_normal(ctx, &plain("g d")),
        Resolved::Command(Command::GoToDefinition)
    );
    assert_eq!(resolve_normal(ctx, &plain("g r")), Resolved::Pending);
    assert_eq!(
        resolve_normal(ctx, &plain("g r r")),
        Resolved::Command(Command::FindReferences)
    );
    // `gg` is the grammar's: the layer has no such binding.
    assert_eq!(resolve_normal(ctx, &plain("g g")), Resolved::None);
    assert_eq!(resolve_normal(ctx, &plain("d")), Resolved::None);
    // Ctrl+R is redo here, not the global recent-workspace picker.
    let ctrl_r = KeyChord::from_event(key(Char('r'), KeyModifiers::CONTROL));
    assert_eq!(
        resolve_normal(ctx, &[ctrl_r]),
        Resolved::Command(Command::Redo)
    );
    // Away from a Normal mode, there is no Normal layer to consult.
    let modeless = Context::focus(FocusTarget::Editor);
    assert_eq!(resolve_normal(modeless, &plain("u")), Resolved::None);
}

#[test]
fn under_the_normal_layer_only_modified_chords_resolve() {
    let ctx = Context::focus(FocusTarget::Editor).with_editing(Editing::VimNormal);
    let chord = |code, mods| KeyChord::from_event(key(code, mods));
    // The editor's Enter, Backspace and Tab would edit: Normal mode drops them.
    for code in [Enter, Backspace, Tab, Delete, Esc] {
        assert_eq!(
            resolve_below_normal(ctx, &[chord(code, KeyModifiers::NONE)]),
            Resolved::None,
            "{code:?}"
        );
    }
    assert_eq!(
        resolve_below_normal(ctx, &[chord(Char('s'), KeyModifiers::CONTROL)]),
        Resolved::Command(Command::Save)
    );
    assert_eq!(
        resolve_below_normal(ctx, &[chord(Char('p'), KeyModifiers::CONTROL)]),
        Resolved::Command(Command::OpenQuickOpen)
    );
    assert_eq!(
        resolve_below_normal(ctx, &[chord(F(12), KeyModifiers::NONE)]),
        Resolved::Command(Command::GoToDefinition)
    );
    // A `Ctrl+K` chord still waits for its second key.
    let ctrl_k = chord(Char('k'), KeyModifiers::CONTROL);
    assert_eq!(resolve_below_normal(ctx, &[ctrl_k]), Resolved::Pending);
    assert_eq!(
        resolve_below_normal(ctx, &[ctrl_k, chord(Char('z'), KeyModifiers::NONE)]),
        Resolved::Command(Command::ToggleZenMode)
    );
}

#[test]
fn helix_and_insert_layers() {
    let helix = Context::focus(FocusTarget::Editor).with_editing(Editing::HelixNormal);
    assert_eq!(
        resolve_normal(helix, &plain("U")),
        Resolved::Command(Command::Redo)
    );
    let space_f = [
        KeyChord::from_event(key(Char(' '), KeyModifiers::NONE)),
        KeyChord::from_event(key(Char('f'), KeyModifiers::NONE)),
    ];
    assert_eq!(
        resolve_normal(helix, &space_f),
        Resolved::Command(Command::OpenQuickOpen)
    );
    assert_eq!(resolve_normal(helix, &plain("g h")), Resolved::None);
    // Insert mode is the editor's keymap plus the Esc that leaves it.
    let insert = Context::focus(FocusTarget::Editor).with_editing(Editing::Insert);
    let esc = KeyChord::from_event(key(Esc, KeyModifiers::NONE));
    assert_eq!(
        resolve(insert, &[esc]),
        Resolved::Command(Command::EnterNormalMode)
    );
    let enter = KeyChord::from_event(key(Enter, KeyModifiers::NONE));
    assert_eq!(
        resolve(insert, &[enter]),
        Resolved::Command(Command::InsertNewline)
    );
    // The modeless editor's Esc is untouched.
    let modeless = Context::focus(FocusTarget::Editor);
    assert_eq!(
        resolve(modeless, &[esc]),
        Resolved::Command(Command::CollapseCarets)
    );
}
