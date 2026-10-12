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
    rig.app.completion = Some(crate::completion::CompletionUi {
        items: vec![karet_core::CompletionItem {
            label: label.to_owned(),
            kind: karet_core::CompletionKind::Function,
            detail: None,
            documentation: None,
            insert_text: label.to_owned(),
            edit: None,
            sort_text: None,
            deprecated: false,
        }],
        list: karet_widgets::CompletionState::default(),
        doc: DocumentId(9),
        anchor,
        last_filter: String::new(),
        mode: crate::completion::CompletionMode::Filtered,
    });
    rig.render();
}

#[test]
fn completion_accept_applies_at_every_caret() {
    let mut rig = Rig::new("fo|\nfo#\nx|");
    open_completion(&mut rig, LineCol::new(1, 0), "foobar");
    rig.step(key(KeyCode::Enter));
    assert_eq!(rig.marked(), "foobar|\nfoobar#\nxfoobar|");
    assert!(rig.app.completion.is_none());
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
                .filter(|(_, c)| matches!(c, SessionCommand::ApplyChange { .. }))
                .count()
        })
        .unwrap_or(usize::MAX);
    assert_eq!(changes, 0, "the rejected change was not sent");
}
