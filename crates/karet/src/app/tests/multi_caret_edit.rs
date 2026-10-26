//! Multi-caret editing rows of `docs/editing-behavior.md`: every caret edits, a
//! caret whose edit is a no-op stays, overlapping edits merge, and nothing moves
//! when the buffer rejects a change.

use super::harness::*;
use super::support::*;
use crate::app::*;

/// A no-op edit keeps its caret, and the primary stays on the same caret.
#[test]
fn no_op_carets_are_kept() {
    use KeyCode::*;
    check(vec![
        case(
            "Backspace keeps a caret at the start of the buffer",
            "|ab\nabc#d",
            vec![key(Backspace)],
            "|ab\nab#d",
        ),
        case(
            "Backspace keeps the primary on the caret at the start",
            "#ab\nabc|d",
            vec![key(Backspace)],
            "#ab\nab|d",
        ),
        case(
            "Delete keeps a caret at the end of the buffer",
            "a|bc\nab#",
            vec![key(Delete)],
            "a|c\nab#",
        ),
        case(
            "a no-op caret after an edit on its line moves with the text",
            "ab|\nab#",
            vec![key(Delete)],
            "ab|ab#",
        ),
    ]);
}

/// Overlapping per-caret edits merge instead of failing the batch.
#[test]
fn overlapping_edits_merge() {
    check(vec![
        case(
            "Ctrl+Backspace from two carets in one word deletes back to its start",
            "abc|de#fg",
            vec![Step::Cmd(Command::DeleteWordBackward)],
            "|fg",
        ),
        case(
            "Ctrl+Delete from two carets in one word deletes to its end",
            "a#bc|defg h",
            vec![Step::Cmd(Command::DeleteWordForward)],
            "a| h",
        ),
    ]);
}

/// Open the completion popup over `items`, anchored at `anchor`.
fn open_completion(rig: &mut Rig, anchor: LineCol, label: &str) {
    rig.app.completion.popup = Some(crate::completion::CompletionUi::new(
        DocumentId(9),
        anchor,
        anchor,
        vec![karet_core::CompletionItem::new(
            label,
            karet_core::CompletionKind::Function,
        )],
        crate::completion::CompletionMode::Filtered,
    ));
    rig.render();
}

#[test]
fn completion_accept_applies_at_every_caret() {
    let mut rig = Rig::new("fo|\nfo#\nx|");
    open_completion(&mut rig, LineCol::new(1, 0), "foobar");
    rig.step(key(KeyCode::Enter));
    assert_eq!(rig.marked(), "foobar|\nfoobar#\nxfoobar|");
    assert!(rig.app.completion.popup.is_none());
}

#[test]
fn completion_accept_replaces_a_secondary_carets_selection() {
    // The second caret's text does not end in the typed `fo`, so the completion
    // replaces its selection `xy` instead.
    let mut rig = Rig::new("fo#\n[xy|]");
    open_completion(&mut rig, LineCol::new(0, 0), "foobar");
    rig.step(key(KeyCode::Enter));
    assert_eq!(rig.marked(), "foobar#\nfoobar|");
}

#[test]
fn completion_accept_with_one_caret_replaces_the_typed_prefix() {
    let mut rig = Rig::new("let fo|");
    open_completion(&mut rig, LineCol::new(0, 4), "foobar");
    rig.step(key(KeyCode::Enter));
    assert_eq!(rig.marked(), "let foobar|");
}

/// Open the find bar over `query`, with `replace` as the replacement. Searching
/// jumps to the first match, as in VS Code.
fn find(rig: &mut Rig, query: &str, replace: &str) {
    let idx = rig.app.active;
    rig.app.tabs[idx].find = Some(crate::tab::FindState {
        query: query.to_string(),
        replace: replace.to_string(),
        replace_visible: true,
        ..Default::default()
    });
    rig.app.run_find();
}

/// Put the carets back to `marked` (whose text must be the buffer's).
fn place(rig: &mut Rig, marked: &str) {
    let (_, cursors) = parse_marked(marked);
    let idx = rig.app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut rig.app.tabs[idx]
    {
        editor.set_cursor_state(buffer, cursors);
    }
}

#[test]
fn replace_all_keeps_every_caret_on_its_text() {
    let mut rig = Rig::new("ab ab ab");
    find(&mut rig, "ab", "xyz");
    place(&mut rig, "ab |ab ab#");
    rig.step(Step::Cmd(Command::FindReplaceAll));
    assert_eq!(rig.marked(), "xyz |xyz xyz#");
}

#[test]
fn replace_with_several_carets_replaces_one_match() {
    let mut rig = Rig::new("|ab ab a#b");
    find(&mut rig, "ab", "X");
    rig.app.find_replace_current();
    let idx = rig.app.active;
    let Tab {
        kind: TabKind::Code { text, .. },
        ..
    } = &rig.app.tabs[idx]
    else {
        panic!("a code tab");
    };
    assert_eq!(text.matches('X').count(), 1, "one replacement in {text:?}");
    assert_eq!(
        text.matches("ab").count(),
        2,
        "two matches left in {text:?}"
    );
}

/// Open the find bar through its key, as a user does, and type `query`.
fn open_find_typing(rig: &mut Rig, query: &str) {
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    for c in query.chars() {
        rig.step(key(KeyCode::Char(c)));
    }
}

#[test]
fn alt_enter_in_the_query_turns_every_match_into_a_caret() {
    let mut rig = Rig::new("|ab x ab x ab");
    open_find_typing(&mut rig, "ab");
    rig.step(key(KeyCode::Down));
    rig.step(Step::Key(KeyCode::Enter, KeyModifiers::ALT));
    // The match the counter named stays primary.
    assert_eq!(rig.marked(), "[ab|] x [ab#] x [ab|]");
    // The keys now edit the text at every caret, with the bar left open.
    assert!(rig.app.find_open);
    assert_eq!(rig.app.active_find().map(|f| f.focused), Some(false));
    rig.step(key(KeyCode::Char('Z')));
    assert_eq!(rig.marked(), "Z| x Z# x Z|");
}

#[test]
fn alt_enter_selects_only_the_matches_in_the_selection_scope() {
    // The scope is captured before typing: the search moves the caret, and
    // with it the selection.
    let mut rig = Rig::new("ab [ab ab|] ab");
    open_find_typing(&mut rig, "");
    rig.step(Step::Key(KeyCode::Char('l'), KeyModifiers::ALT));
    for c in "ab".chars() {
        rig.step(key(KeyCode::Char(c)));
    }
    rig.step(Step::Key(KeyCode::Enter, KeyModifiers::ALT));
    assert_eq!(rig.marked(), "ab [ab#] [ab|] ab");
}

#[test]
fn alt_enter_with_no_match_leaves_the_carets_and_the_bar_alone() {
    let mut rig = Rig::new("ab |cd");
    open_find_typing(&mut rig, "zz");
    rig.step(Step::Key(KeyCode::Enter, KeyModifiers::ALT));
    assert_eq!(rig.marked(), "ab |cd");
    assert_eq!(rig.app.active_find().map(|f| f.focused), Some(true));
}

#[test]
fn alt_enter_in_the_replace_field_still_replaces_every_match() {
    let mut rig = Rig::new("ab |ab ab");
    find(&mut rig, "ab", "xyz");
    if let Some(find) = rig.app.active_find_mut() {
        find.field = crate::tab::SearchField::Replace;
        find.focused = true;
    }
    rig.app.find_open = true;
    rig.step(Step::Key(KeyCode::Enter, KeyModifiers::ALT));
    assert_eq!(text_of(&rig), "xyz xyz xyz");
}

#[test]
fn ctrl_alt_enter_replaces_every_match_from_the_query_field() {
    let mut rig = Rig::new("ab |ab ab");
    open_find_typing(&mut rig, "ab");
    if let Some(find) = rig.app.active_find_mut() {
        find.replace = "c".to_string();
    }
    rig.step(Step::Key(
        KeyCode::Enter,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ));
    assert_eq!(text_of(&rig), "c c c");
}

/// The active tab's text.
fn text_of(rig: &Rig) -> String {
    match &rig.tab().kind {
        TabKind::Code { text, .. } => text.to_string(),
        _ => String::new(),
    }
}

/// A change the buffer rejects leaves the text and the carets alone and never
/// reaches the session.
#[test]
fn a_rejected_change_moves_nothing_and_is_not_sent() {
    let (backend, mut app) = completion_app("abcdefg", LineCol::new(0, 3));
    let idx = app.active;
    let base = match &app.tabs[idx].kind {
        TabKind::Code { next_version, .. } => *next_version,
        _ => 0,
    };
    let delete = |start: u32, end: u32| TextEdit {
        range: Range {
            start: LineCol::new(0, start),
            end: LineCol::new(0, end),
        },
        new_text: String::new(),
    };
    app.submit_multi_edit(
        EditCause::Delete,
        editing::MultiEdit {
            change: Change::new(base, vec![delete(0, 3), delete(0, 5)]),
            cursors: karet_core::CursorState::single(karet_core::Selection::caret(LineCol::new(
                0, 0,
            ))),
        },
    );
    let TabKind::Code { text, .. } = &app.tabs[idx].kind else {
        panic!("a code tab");
    };
    assert_eq!(text, "abcdefg");
    assert_eq!(app.tabs[idx].editor.cursor(), LineCol::new(0, 3));
    let changes = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, c)| {
                    matches!(
                        c,
                        SessionCommand::Document(DocumentCommand::ApplyChange { .. })
                    )
                })
                .count()
        })
        .unwrap_or(usize::MAX);
    assert_eq!(changes, 0, "the rejected change was not sent");
}
