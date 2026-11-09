//! Modal editing (`editor.keymap`) through the app's real input path: the
//! Normal layer, the grammar, the layers under them, and the Insert layer.

use karet_session::config::schema::KeymapPreset;

use super::harness::*;
use super::support::screen;
use crate::app::*;
use crate::keymap::vi::Mode;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// A rig over `marked` with `preset` on.
fn rig(preset: KeymapPreset, marked: &str) -> Rig {
    let mut rig = Rig::new(marked);
    rig.app.settings.editor.keymap = preset;
    rig
}

/// Type `text` as plain keys.
fn typed(rig: &mut Rig, text: &str) {
    for c in text.chars() {
        rig.step(key(KeyCode::Char(c)));
    }
}

#[test]
fn normal_mode_never_types_and_its_editing_keys_only_move() {
    let mut rig = rig(KeymapPreset::Vim, "a|bc\ndef");
    typed(&mut rig, "Z!");
    rig.step(key(KeyCode::Enter));
    rig.step(key(KeyCode::Tab));
    rig.step(key(KeyCode::Backspace));
    assert_eq!(
        rig.marked(),
        "ab|c\ndef",
        "Enter moved down, Backspace back onto the line above"
    );
    assert_eq!(rig.app.vi.engine.mode(), Mode::Normal);
}

#[test]
fn insert_mode_types_and_esc_returns_to_normal() {
    let mut rig = rig(KeymapPreset::Vim, "a|bc");
    typed(&mut rig, "ixy");
    assert_eq!(rig.marked(), "axy|bc");
    assert_eq!(rig.app.vi.engine.mode(), Mode::Insert);
    rig.step(key(KeyCode::Esc));
    assert_eq!(rig.app.vi.engine.mode(), Mode::Normal);
    assert_eq!(
        rig.marked(),
        "ax|ybc",
        "Esc steps back onto the last typed character"
    );
}

#[test]
fn operators_edit_the_document_through_the_session() {
    let mut rig = rig(KeymapPreset::Vim, "|one\ntwo\nthree");
    typed(&mut rig, "ddp");
    assert_eq!(rig.marked(), "two\n|one\nthree");
    typed(&mut rig, "ciwnew");
    rig.step(key(KeyCode::Esc));
    assert_eq!(rig.marked(), "two\nne|w\nthree");
}

#[test]
fn macros_and_dot_replay_through_the_input_path() {
    let mut rig = rig(KeymapPreset::Vim, "|a\nb\nc");
    typed(&mut rig, "qqA;");
    rig.step(key(KeyCode::Esc));
    typed(&mut rig, "jq");
    assert_eq!(rig.app.vi.engine.recording(), None);
    typed(&mut rig, "@q");
    assert_eq!(rig.marked(), "a;\nb;\n|c");
    // The change the macro made is the one `.` repeats.
    typed(&mut rig, ".");
    assert_eq!(rig.marked(), "a;\nb;\nc|;");
}

#[test]
fn dot_replays_a_completion_as_its_text() {
    let mut rig = rig(KeymapPreset::Vim, "|a\nb");
    typed(&mut rig, "A.x");
    // What accepting `xyz` for the typed `x` does to the change.
    rig.app.vi_type(1, 0, "xyz");
    assert_eq!(rig.marked(), "a.xyz|\nb");
    rig.step(key(KeyCode::Esc));
    typed(&mut rig, "j.");
    assert_eq!(rig.marked(), "a.xyz\nb.xy|z");
}

#[test]
fn a_key_a_popup_took_is_not_replayed() {
    let mut rig = rig(KeymapPreset::Vim, "|a\nb");
    typed(&mut rig, "A.");
    rig.step(key(KeyCode::Enter));
    // As though the completion popup had taken that Enter to accept.
    rig.app
        .vi_popup_took(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    rig.app.vi_type(0, 0, "x");
    rig.step(key(KeyCode::Esc));
    typed(&mut rig, "G.");
    assert!(rig.marked().ends_with("b.|x"), "{}", rig.marked());
}

#[test]
fn a_macro_that_runs_itself_stops() {
    let mut rig = rig(KeymapPreset::Vim, "|abcdefghijklmnopqrstuvwxyz");
    typed(&mut rig, "qwx@wq");
    // Replaying `x@w` recurses until the depth limit cuts it off; the
    // document survives with some characters deleted and the app responsive.
    typed(&mut rig, "@w");
    let text = rig.marked();
    assert!(text.len() < 26, "{text}");
    typed(&mut rig, "ix");
    assert_eq!(rig.app.vi.engine.mode(), Mode::Insert);
}

#[test]
fn the_normal_layer_runs_commands_with_a_count() {
    let mut rig = rig(KeymapPreset::Vim, "|abc");
    typed(&mut rig, ":");
    assert!(
        rig.app.keyboard.overlay().is_some(),
        "`:` opens the command palette"
    );
    rig.step(key(KeyCode::Esc));
    typed(&mut rig, "/");
    assert!(rig.app.find_open, "`/` opens the find bar");
}

#[test]
fn modified_chords_still_reach_the_global_layer() {
    let mut rig = rig(KeymapPreset::Vim, "|abc");
    rig.step(Step::Key(KeyCode::Char('p'), CTRL));
    assert!(
        rig.app.keyboard.overlay().is_some(),
        "Ctrl+P opens quick open from Normal mode"
    );
}

#[test]
fn a_chord_from_the_layers_below_takes_its_second_key_from_normal_mode() {
    let mut rig = rig(KeymapPreset::Vim, "|abc");
    let outline = rig.app.outline.visible;
    // `Ctrl+K O` is the global layer's: its plain second key goes to the
    // chord, not to the grammar (where `o` would open a line).
    rig.step(Step::Key(KeyCode::Char('k'), CTRL));
    typed(&mut rig, "o");
    assert_eq!(
        rig.app.outline.visible, !outline,
        "Ctrl+K O toggled the outline"
    );
    assert_eq!(rig.marked(), "|abc");
    assert_eq!(rig.app.vi.engine.mode(), Mode::Normal);
    // A second key no chord names ends the chord without reaching the
    // grammar either.
    rig.step(Step::Key(KeyCode::Char('k'), CTRL));
    typed(&mut rig, "x");
    assert_eq!(rig.marked(), "|abc");
    assert!(rig.app.pending.is_empty());
    // The next key is the grammar's again.
    typed(&mut rig, "x");
    assert_eq!(rig.marked(), "|bc");
}

#[test]
fn a_find_query_and_enter_hand_the_keys_back_for_n() {
    let mut rig = rig(KeymapPreset::Vim, "|x foo y foo z");
    typed(&mut rig, "/foo");
    rig.step(key(KeyCode::Enter));
    assert_eq!(
        rig.app.input_context().modal,
        None,
        "Enter returns the keys to the editor"
    );
    let first = rig.tab().editor.cursor();
    typed(&mut rig, "n");
    assert_eq!(first.col, 2, "Enter jumped to the first match");
    assert_eq!(
        rig.tab().editor.cursor().col,
        8,
        "`n` goes on to the next match"
    );
}

#[test]
fn the_status_bar_names_the_mode() {
    let mut rig = rig(KeymapPreset::Vim, "|abc");
    let shown = |rig: &mut Rig| screen(&mut rig.app, 80, 24).join("\n");
    assert!(shown(&mut rig).contains("NORMAL"));
    typed(&mut rig, "qa");
    assert!(shown(&mut rig).contains("recording @a"));
    typed(&mut rig, "q2d");
    assert!(shown(&mut rig).contains("2d"));
    rig.step(key(KeyCode::Esc));
    typed(&mut rig, "i");
    assert!(shown(&mut rig).contains("INSERT"));
}

#[test]
fn helix_selects_then_acts() {
    let mut rig = rig(KeymapPreset::Helix, "|foo bar\nbaz");
    typed(&mut rig, "wd");
    assert_eq!(rig.marked(), "|bar\nbaz");
    // The pasted line comes back selected; the caret is its last character
    // (here, its line break).
    typed(&mut rig, "xyp");
    assert_eq!(rig.marked(), "bar\n[bar|]\nbaz");
}

#[test]
fn the_default_keymap_is_untouched() {
    let mut rig = rig(KeymapPreset::Default, "|abc");
    typed(&mut rig, "dd");
    assert_eq!(rig.marked(), "dd|abc");
    assert_eq!(rig.app.vi_status(), None);
}

#[test]
fn modal_editing_only_has_the_keys_while_the_editor_does() {
    let mut rig = rig(KeymapPreset::Vim, "|abc");
    rig.app.keyboard.focus_sidebar();
    assert_eq!(rig.app.vi_editing(), crate::keymap::Editing::Modeless);
    rig.app.keyboard.focus_editor();
    assert_eq!(rig.app.vi_editing(), crate::keymap::Editing::VimNormal);
    typed(&mut rig, "i");
    assert_eq!(rig.app.vi_editing(), crate::keymap::Editing::Insert);
    // A popup's own keys do not keep Esc from leaving Insert mode, and a
    // notification on screen does not swallow it either.
    rig.app
        .notify(Report::Outcome, NotificationKind::System, "hello");
    rig.step(key(KeyCode::Esc));
    assert_eq!(rig.app.vi.engine.mode(), Mode::Normal);
}
