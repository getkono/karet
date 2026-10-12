//! Hint rendering (`hint_for`, `hints_for`, `completions_for`) and multi-chord
//! sequence resolution through a pending prefix. Split out of `tests.rs` to keep
//! that file under the workspace code-line ceiling.

use super::*;

#[test]
fn hint_for_formats_chords() {
    assert_eq!(
        hint_for(Command::CloseTab, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+W")
    );
    assert_eq!(
        hint_for(Command::OpenCommandPalette, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+Shift+P")
    );
    assert_eq!(
        hint_for(Command::ToggleFocus, ChordStyle::Verbose).as_deref(),
        Some("Tab")
    );
    // The same binding renders compactly for the status bar.
    assert_eq!(
        hint_for(Command::CloseTab, ChordStyle::Caret).as_deref(),
        Some("^W")
    );
}

#[test]
fn hint_renders_multi_key_sequence() {
    assert_eq!(
        hint_for(Command::CloseAllTabs, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+K Ctrl+W")
    );
}

#[test]
fn hints_for_is_context_aware_and_deduped() {
    let editor = hints_for(Context::focus(FocusTarget::Editor), ChordStyle::Caret);
    let cmds: Vec<Command> = editor.iter().map(|h| h.command).collect();
    // Editor-context commands are advertised…
    assert!(cmds.contains(&Command::Save));
    assert!(cmds.contains(&Command::Undo));
    assert!(cmds.contains(&Command::Cut));
    assert!(cmds.contains(&Command::Copy));
    assert!(cmds.contains(&Command::OpenFind));
    // …while self-evident motion and text-editing keys are omitted.
    assert!(!cmds.contains(&Command::CaretDown));
    assert!(!cmds.contains(&Command::PageDown));
    assert!(!cmds.contains(&Command::DeleteBackward));
    // Every command appears at most once (dedup across layers).
    for &cmd in &cmds {
        assert_eq!(cmds.iter().filter(|c| **c == cmd).count(), 1);
    }
    // The most-specific layer wins ordering: an Editor-layer verb (Save) precedes
    // a Global one (Quit).
    let save = cmds.iter().position(|c| *c == Command::Save);
    let quit = cmds.iter().position(|c| *c == Command::Quit);
    assert!(save < quit);
}

#[test]
fn hints_for_reflects_the_active_modal() {
    let find = hints_for(
        Context::modal(Modal::Find, FocusTarget::Editor),
        ChordStyle::Caret,
    );
    let cmds: Vec<Command> = find.iter().map(|h| h.command).collect();
    assert!(cmds.contains(&Command::FindNext));
    assert!(cmds.contains(&Command::FindPrev));
    assert!(cmds.contains(&Command::FindCancel));
    // The Find modal is exclusive, so editor commands don't leak into the bar.
    assert!(!cmds.contains(&Command::Save));
}

#[test]
fn completions_for_renders_only_the_remaining_chord() {
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let comps = completions_for(
        Context::focus(FocusTarget::Editor),
        &[ctrl_k],
        ChordStyle::Caret,
    );
    let by_cmd = |cmd| comps.iter().find(|h| h.command == cmd);
    // Only the chord AFTER the pending `^K` prefix is rendered.
    assert_eq!(
        by_cmd(Command::CloseAllTabs).map(|h| h.chord.as_str()),
        Some("^W")
    );
    assert_eq!(
        by_cmd(Command::CloseOtherTabs).map(|h| h.chord.as_str()),
        Some("W")
    );
    // Each completion carries a verb so the pending-chord bar is self-explanatory.
    assert!(by_cmd(Command::CloseAllTabs).is_some_and(|h| !h.verb.is_empty()));
}

#[test]
fn chord_sequences_resolve_through_a_pending_prefix() {
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let ctrl_w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
    let w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::NONE));
    let esc = KeyChord::from_event(key(KeyCode::Esc, KeyModifiers::NONE));
    let ed = Context::focus(FocusTarget::Editor);
    // Ctrl+K alone is a prefix of a longer binding — the resolver waits.
    assert_eq!(resolve(ed, &[ctrl_k]), Resolved::Pending);
    // Completing the sequence fires the command; the second chord disambiguates.
    assert_eq!(
        resolve(ed, &[ctrl_k, ctrl_w]),
        Resolved::Command(Command::CloseAllTabs)
    );
    assert_eq!(
        resolve(ed, &[ctrl_k, w]),
        Resolved::Command(Command::CloseOtherTabs)
    );
    // A key that continues nothing breaks the sequence.
    assert_eq!(resolve(ed, &[ctrl_k, esc]), Resolved::None);
}

#[test]
fn no_terminal_binding_is_a_prefix_of_another() {
    // The resolver commits on the first full match, so a binding must never also
    // be a strict prefix of a longer one (which it would shadow). This invariant
    // is what lets resolution stay deterministic without a timeout.
    for a in BINDINGS {
        for b in BINDINGS {
            if a.seq_len() >= b.seq_len() {
                continue;
            }
            let a_prefixes_b = (0..a.seq_len()).all(|i| a.chord_at(i) == b.chord_at(i));
            assert!(
                !a_prefixes_b,
                "{:?} is a prefix of {:?}",
                a.command, b.command
            );
        }
    }
}
