//! Carets across changes a view did not make itself (#331): an edit in another
//! split, an undo, a formatter or a reload reaches a view as a snapshot, and
//! every view carries its carets through it. An undo or redo restores its
//! whole selection set, and only in the view that asked for it.

use std::sync::Arc;

use karet_core::Change;
use karet_core::CursorState;
use karet_core::Range;
use karet_core::TextEdit;

use super::harness::*;
use super::support::*;
use crate::app::*;

/// Line 3 is fifty characters long, so a caret can sit at column 40.
fn body() -> String {
    format!("a\nb\nc\n{}0123456789\nend\n", "x".repeat(40))
}

/// A rig split in two over one document: pane 0 is `A`, pane 1 is `B`, and
/// `A` is focused. Each pane's carets are given as marked text.
fn two_splits(a: &str, b: &str) -> Rig {
    let (_, a_cursors) = parse_marked(a);
    let (_, b_cursors) = parse_marked(b);
    let mut rig = Rig::sized(a, 140, 20);
    rig.step(Step::Cmd(Command::SplitRight));
    place(&mut rig, b_cursors);
    rig.step(Step::Cmd(Command::FocusPrevPane));
    place(&mut rig, a_cursors);
    assert_eq!(rig.focused_index(), Some(0), "A is focused");
    rig
}

/// Put the focused view's carets at `cursors`.
fn place(rig: &mut Rig, cursors: CursorState) {
    let idx = rig.app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut rig.app.tabs[idx]
    {
        editor.set_cursor_state(buffer, cursors);
    }
    rig.render();
}

/// The session's echo of the focused view's buffer, after `edits` (when any)
/// were applied on top of it, carrying `cursor`.
fn snapshot(rig: &Rig, edits: Vec<TextEdit>, cursor: Option<CursorState>) -> DocSnapshot {
    let mut buffer = match &rig.tab().kind {
        TabKind::Code { buffer, .. } => (**buffer).clone(),
        _ => karet_text::TextBuffer::from_text(""),
    };
    if !edits.is_empty() {
        let applied = buffer.apply_simple(&Change::new(buffer.version(), edits));
        assert!(applied.is_ok(), "the echoed change applies");
    }
    DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::new(karet_syntax::Highlights::default()),
        semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
        folds: Arc::new(FoldRegions::default()),
        syntax_error_lines: Arc::new(Vec::new()),
        table_lines: Arc::default(),
        language: Some("Rust"),
        dirty: true,
        cursor,
    }
}

fn replace(line: u32, from: u32, to: u32, text: &str) -> TextEdit {
    TextEdit {
        range: Range {
            start: LineCol::new(line, from),
            end: LineCol::new(line, to),
        },
        new_text: text.to_owned(),
    }
}

/// `text` with `mark` inserted at `(line, col)`.
fn marked_at(text: &str, line: usize, col: usize, mark: &str) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if let Some(target) = lines.get_mut(line) {
        let byte = target
            .char_indices()
            .nth(col)
            .map_or(target.len(), |(i, _)| i);
        target.insert_str(byte, mark);
    }
    lines.join("\n")
}

/// The issue's repro: a caret at (3,40) in B, and A deletes 40 characters of
/// line 3. B's caret must follow the text, stay visible, and type where it is
/// drawn.
#[test]
fn an_edit_in_one_split_carries_the_other_splits_caret() {
    let text = body();
    let mut rig = two_splits(&format!("|{text}"), &marked_at(&text, 3, 40, "|"));
    // In A: select the first 40 characters of line 3 and delete them.
    let selected = marked_at(&marked_at(&text, 3, 40, "|]"), 3, 0, "[");
    place(&mut rig, parse_marked(&selected).1);
    rig.step(key(KeyCode::Backspace));
    let echo = snapshot(&rig, Vec::new(), None);
    rig.app.on_snapshot(DocumentId(9), &echo);
    rig.render();

    let after = "a\nb\nc\n0123456789\nend\n";
    assert_eq!(rig.marked_in(0), marked_at(after, 3, 0, "|"), "A");
    assert_eq!(
        rig.marked_in(1),
        marked_at(after, 3, 0, "|"),
        "B's caret moved with the text after it, not left at column 40"
    );

    rig.step(Step::Cmd(Command::FocusNextPane));
    rig.step(key(KeyCode::Char('x')));
    assert_eq!(
        rig.marked(),
        marked_at("a\nb\nc\nx0123456789\nend\n", 3, 1, "|"),
        "typing lands where B's caret is drawn"
    );
}

/// A caret on a line another split deleted is pulled back onto the text, so
/// the next edit from it is in bounds.
#[test]
fn a_caret_on_a_deleted_line_is_clamped_onto_the_text() {
    let mut rig = two_splits("|one\ntwo\nthree", "one\ntwo\nthr|ee");
    // A deletes everything after `one`.
    let edits = vec![TextEdit {
        range: Range {
            start: LineCol::new(0, 3),
            end: LineCol::new(2, 5),
        },
        new_text: String::new(),
    }];
    let echo = snapshot(&rig, edits, None);
    rig.app.on_snapshot(DocumentId(9), &echo);
    rig.render();
    assert_eq!(rig.marked_in(1), "one|", "B sits at the end of the text");

    rig.step(Step::Cmd(Command::FocusNextPane));
    rig.step(key(KeyCode::Char('!')));
    assert_eq!(rig.marked(), "one!|", "B's edit applies instead of failing");
}

/// A formatter-like rewrite with changes on several lines keeps a caret
/// between them on its own line rather than collapsing it to the first one.
#[test]
fn a_caret_between_two_changes_keeps_its_line() {
    let mut rig = two_splits("|a  \nmiddle\nz  ", "a  \nmid|dle\nz  ");
    let edits = vec![replace(0, 1, 3, ""), replace(2, 1, 3, "")];
    let echo = snapshot(&rig, edits, None);
    rig.app.on_snapshot(DocumentId(9), &echo);
    assert_eq!(rig.marked_in(1), "a\nmid|dle\nz");
}

/// A republish of the version already on screen (a highlight pass) moves no
/// caret and keeps every selection.
#[test]
fn a_republished_version_moves_no_caret() {
    let mut rig = two_splits("ab|c", "a[b|]c");
    let echo = snapshot(&rig, Vec::new(), None);
    rig.app.on_snapshot(DocumentId(9), &echo);
    assert_eq!(rig.marked_in(0), "ab|c");
    assert_eq!(rig.marked_in(1), "a[b|]c");
}

/// The selection set on either side of an edit is reported around it, so the
/// session can restore it on undo and redo.
#[test]
fn an_edit_is_wrapped_in_its_before_and_after_selections() {
    let mut rig = two_splits("a[lp|]ha\nbeta#\n", "|alpha\nbeta\n");
    let backend = Arc::new(RecordingBackend::new());
    rig.app.backend = Some(backend.clone());
    rig.step(key(KeyCode::Char('!')));
    assert_eq!(rig.marked(), "a!|ha\nbeta!#\n");

    let view = rig.tab().view;
    let sent: Vec<SessionCommand> = backend
        .sent
        .lock()
        .map(|sent| sent.iter().map(|(_, command)| command.clone()).collect())
        .unwrap_or_default();
    let reported = |command: &SessionCommand| match command {
        SessionCommand::SetCursor {
            doc: DocumentId(9),
            view: from,
            cursors,
        } if *from == view => Some(cursors.clone()),
        _ => None,
    };
    let [before, SessionCommand::ApplyChange { .. }, after, ..] = sent.as_slice() else {
        panic!("expected SetCursor, ApplyChange, SetCursor; got {sent:?}");
    };
    assert_eq!(reported(before), Some(parse_marked("a[lp|]ha\nbeta#\n").1));
    assert_eq!(reported(after), Some(parse_marked("a!|ha\nbeta!#\n").1));
}

/// An undo's selection set lands in the view that sent the undo, even after
/// focus moved on, and never in another split of the same document.
#[test]
fn undo_restores_the_selection_set_only_in_the_issuing_view() {
    let mut rig = two_splits("a[lp|]ha\nbeta#\n", "al|pha\nbeta\n");
    rig.step(key(KeyCode::Char('!')));
    let echo = snapshot(&rig, Vec::new(), None);
    rig.app.on_snapshot(DocumentId(9), &echo);
    assert_eq!(
        rig.marked_in(1),
        "a!|ha\nbeta!\n",
        "B's caret inside the replaced text keeps its column"
    );

    rig.step(Step::Cmd(Command::Undo));
    // Focus moves to B before the session answers.
    rig.step(Step::Cmd(Command::FocusNextPane));
    // The undo puts `lp` back over the `!` on line 0 and drops the one on
    // line 1, restoring A's selections from before the edit.
    let undo = snapshot(
        &rig,
        vec![replace(0, 1, 2, "lp"), replace(1, 4, 5, "")],
        Some(parse_marked("a[lp|]ha\nbeta#\n").1),
    );
    rig.app.on_snapshot(DocumentId(9), &undo);
    assert_eq!(
        rig.marked_in(0),
        "a[lp|]ha\nbeta#\n",
        "A gets its selections back"
    );
    assert_eq!(
        rig.marked_in(1),
        "al|pha\nbeta\n",
        "B only follows the text"
    );
}
