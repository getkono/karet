//! Who owns the caret and the keyboard while the find bar, an inline rename, the
//! commit console or the completion popup is open (issue #333).
//!
//! The rule under test: the caret only moves because the user moved it, and keys
//! go to whatever the user last put them in.

use std::sync::Arc;

use karet_core::LineCol;
use karet_vcs::CommitOutputLine;
use karet_vcs::OutputStream;

use super::harness::*;
use super::support::*;
use crate::app::*;

/// Type `text` key by key, as a user would.
fn typed(text: &str) -> Vec<Step> {
    text.chars().map(|c| key(KeyCode::Char(c))).collect()
}

/// Open the find bar and type `query` into it.
fn find(query: &str) -> Vec<Step> {
    let mut steps = vec![Step::Cmd(Command::OpenFind)];
    steps.extend(typed(query));
    steps
}

/// A snapshot republishing the active tab's buffer exactly as it is, as an
/// auto-save or a highlight pass does.
fn republish(app: &mut App) {
    let buffer = match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => karet_text::TextBuffer::from_text(""),
    };
    let snapshot = DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::new(karet_syntax::Highlights::default()),
        semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
        folds: Arc::new(FoldRegions::default()),
        decorations: Arc::new(Vec::new()),
        syntax_error_lines: Arc::new(Vec::new()),
        table_lines: Arc::default(),
        language: Some("Rust"),
        dirty: false,
        cursor: None,
    };
    app.on_snapshot(DocumentId(9), &snapshot);
}

fn find_state(rig: &Rig) -> (usize, usize, String) {
    let find = rig.app.active_find();
    (
        find.map_or(0, |f| f.count),
        find.map_or(0, |f| f.current),
        find.map(|f| f.query.clone()).unwrap_or_default(),
    )
}

#[test]
fn find_moves_the_caret_only_when_the_user_searches() {
    let mut esc = find("foo");
    esc.push(key(KeyCode::Esc));
    check(vec![
        case(
            "typing a query searches from the caret, not the file top",
            "foo\nbar |\nfoo\n",
            find("foo"),
            "foo\nbar \n|foo\n",
        ),
        case(
            "a query with nothing after the caret wraps to the first match",
            "foo\nfoo |\n",
            find("foo"),
            "|foo\nfoo \n",
        ),
        case(
            "opening the bar over a restored query leaves the caret alone",
            "foo\n|bar\nfoo\n",
            {
                let mut steps = find("foo");
                steps.push(key(KeyCode::Esc));
                steps.push(click(text(1, 2)));
                steps.push(Step::Cmd(Command::OpenFind));
                steps
            },
            "foo\nba|r\nfoo\n",
        ),
        case(
            "next steps from the caret",
            "foo\n|bar\nfoo foo\n",
            {
                let mut steps = find("foo");
                steps.push(key(KeyCode::Enter));
                steps
            },
            "foo\nbar\nfoo |foo\n",
        ),
        case(
            "previous steps back from the caret, wrapping",
            "|foo\nbar\nfoo\n",
            {
                let mut steps = find("foo");
                steps.push(key(KeyCode::Up));
                steps
            },
            "foo\nbar\n|foo\n",
        ),
        case(
            "closing the bar keeps the caret",
            "a |foo\n",
            esc,
            "a |foo\n",
        ),
    ]);
}

#[test]
fn a_snapshot_refreshes_the_matches_without_moving_the_caret() {
    let mut rig = Rig::new("foo\nfoo\nfoo\n");
    rig.run(&find("foo"));
    // The user goes back to the text and selects across two lines, with a
    // second caret beside it.
    rig.run(&[
        click(text(1, 1)),
        Step::Key(KeyCode::Down, KeyModifiers::SHIFT),
    ]);
    let idx = rig.app.active;
    let carets = vec![
        karet_core::Selection {
            anchor: LineCol::new(1, 1),
            head: LineCol::new(2, 1),
        },
        karet_core::Selection::caret(LineCol::new(0, 3)),
    ];
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut rig.app.tabs[idx]
    {
        editor.set_cursor_state(
            buffer,
            karet_core::CursorState {
                selections: carets,
                primary: 0,
            },
        );
    }
    let before = rig.marked();
    let before_find = find_state(&rig);

    republish(&mut rig.app);
    rig.render();

    assert_eq!(
        rig.marked(),
        before,
        "an auto-save snapshot is not a motion"
    );
    assert_eq!(
        find_state(&rig),
        (before_find.0, 1, before_find.2),
        "nor does it reset the counter: it names the match the caret is on"
    );
    assert_eq!(find_state(&rig).0, 3, "the matches are still counted");
}

#[test]
fn replace_continues_from_the_replaced_match() {
    let mut steps = find("a");
    // Tab moves to the replacement field, which takes "b".
    steps.push(key(KeyCode::Tab));
    steps.extend(typed("b"));
    let once = {
        let mut s = steps.clone();
        s.push(key(KeyCode::Enter));
        s
    };
    let twice = {
        let mut s = once.clone();
        s.push(key(KeyCode::Enter));
        s
    };
    let then_next = {
        let mut s = once.clone();
        s.push(key(KeyCode::Down));
        s
    };
    check(vec![
        case(
            "replace lands on the next match",
            "|a1 a2 a3\n",
            once,
            "b1 |a2 a3\n",
        ),
        case(
            "a second replace takes that match",
            "|a1 a2 a3\n",
            twice,
            "b1 b2 |a3\n",
        ),
        case(
            "next after a replace continues past the current match",
            "|a1 a2 a3\n",
            then_next,
            "b1 a2 |a3\n",
        ),
        case(
            "replacement text that still matches is not matched again",
            "|a1 a2\n",
            {
                let mut s = find("a");
                s.push(key(KeyCode::Tab));
                s.extend(typed("aa"));
                s.push(key(KeyCode::Enter));
                s
            },
            "aa1 |a2\n",
        ),
    ]);
}

/// Step to a match, leave the bar for the text, run `edit` there (which shifts
/// the match indices), put the caret back on `target`, and return to the
/// replacement field, ready for Enter.
fn replace_after_edit(marked: &str, steps_to_match: usize, edit: &[Step], target: At) -> Rig {
    let mut rig = Rig::new(marked);
    rig.run(&find("a"));
    rig.run(&[key(KeyCode::Tab)]);
    rig.run(&typed("b"));
    for _ in 0..steps_to_match {
        rig.run(&[key(KeyCode::Down)]);
    }
    rig.run(&[click(text(0, 0))]);
    rig.run(edit);
    rig.run(&[click(target)]);
    republish(&mut rig.app);
    rig.render();
    if let Some(field) = rig.app.find_rects.replace {
        rig.run(&[click(At::Cell(field.x + 1, field.y))]);
    }
    rig
}

#[test]
fn replace_after_an_edit_takes_the_match_at_the_caret() {
    // A match added before the current one shifts every index after it.
    let mut rig = replace_after_edit("|q a1 a2\n", 1, &typed("a"), text(0, 6));
    assert_eq!(rig.marked(), "aq a1 |a2\n");
    assert_eq!(
        find_state(&rig).1,
        2,
        "the counter names the match the caret is on"
    );
    rig.run(&[key(KeyCode::Enter)]);
    assert_eq!(
        rig.marked(),
        "|aq a1 b2\n",
        "the match at the caret is replaced, then the search wraps"
    );

    // A match removed before the current one shifts them the other way.
    let mut rig = replace_after_edit("|a1 a2 a3 a4\n", 2, &[key(KeyCode::Delete)], text(0, 5));
    assert_eq!(rig.marked(), "1 a2 |a3 a4\n");
    assert_eq!(find_state(&rig).1, 1);
    rig.run(&[key(KeyCode::Enter)]);
    assert_eq!(rig.marked(), "1 a2 b3 |a4\n");
}

#[test]
fn a_replace_with_several_carets_replaces_the_match_once() {
    let mut rig = Rig::new("|a1\na2\n");
    rig.run(&find("a"));
    rig.run(&[key(KeyCode::Tab)]);
    rig.run(&typed("b"));
    let idx = rig.app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut rig.app.tabs[idx]
    {
        editor.set_cursor_state(
            buffer,
            karet_core::CursorState {
                selections: vec![
                    karet_core::Selection::caret(LineCol::new(0, 0)),
                    karet_core::Selection::caret(LineCol::new(1, 2)),
                ],
                primary: 0,
            },
        );
    }
    rig.run(&[key(KeyCode::Enter)]);
    assert_eq!(rig.marked(), "b1\n|a2\n");
}

#[test]
fn clicking_the_text_gives_the_editor_the_keyboard_and_keeps_the_bar() {
    let mut steps = find("foo");
    steps.push(click(text(0, 5)));
    steps.extend(typed("x"));
    check(vec![case(
        "a key after the click is typed into the document",
        "|foo bar foo\n",
        steps.clone(),
        "foo bx|ar foo\n",
    )]);

    let mut rig = Rig::new("|foo bar foo\n");
    rig.run(&steps);
    assert!(rig.app.find_open, "the bar stays open");
    assert_eq!(find_state(&rig).2, "foo", "the query never saw the key");
    assert_eq!(
        rig.app.input_context().modal,
        None,
        "copy and cut now read the editor's selection, not the query's"
    );

    // Esc from the text closes the bar it left open.
    rig.run(&[key(KeyCode::Esc)]);
    assert!(!rig.app.find_open);
    assert_eq!(rig.marked(), "foo bx|ar foo\n");
}

#[test]
fn clicking_the_find_field_takes_the_keyboard_back() {
    let mut rig = Rig::new("|foo bar\n");
    rig.run(&find("fo"));
    rig.run(&[click(text(0, 5))]);
    let query = rig.app.find_rects.query;
    // Even from the commit box, which outranks the bar while it is focused.
    rig.app.commit_input.focused = true;
    rig.run(&[click(At::Cell(query.x + 2, query.y))]);
    assert_eq!(rig.app.input_context().modal, Some(Modal::Find));
    rig.run(&typed("o"));
    assert_eq!(find_state(&rig).2, "foo");
    assert_eq!(rig.marked(), "|foo bar\n", "the document never saw the key");
}

#[test]
fn completion_keys_belong_to_the_focused_editor_only() {
    let (_backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    app.completion = Some(crate::completion::CompletionUi {
        items: vec![karet_core::CompletionItem {
            label: "foobar".to_owned(),
            kind: karet_core::CompletionKind::Function,
            detail: None,
            documentation: None,
            insert_text: "foobar".to_owned(),
            edit: None,
            sort_text: None,
            deprecated: false,
        }],
        list: karet_widgets::CompletionState::default(),
        doc: DocumentId(9),
        anchor: LineCol::new(0, 0),
        last_filter: String::new(),
        mode: crate::completion::CompletionMode::Filtered,
    });
    app.focus = Focus::Sidebar;
    app.sidebar_panel = SidebarPanel::Explorer;

    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(code_tab_text(&app), "fo\n", "Enter went to the Explorer");
    assert!(
        app.completion.is_none(),
        "the popup closes once its editor has lost the keyboard"
    );
}

#[test]
fn an_outside_press_commits_an_inline_rename() {
    let dir = test_dir("rename-outside-press");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.sidebar_panel = SidebarPanel::Explorer;
    app.explorer_begin_new(false);
    for c in "made.txt".chars() {
        app.explorer.edit_push(c);
    }
    assert_eq!(app.input_context().modal, Some(Modal::ExplorerEdit));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 50,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        !app.explorer.is_editing(),
        "the rename no longer holds the keys"
    );
    assert!(
        dir.join("made.txt").exists(),
        "and it committed, not cancelled"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_commit_console_hides_on_an_outside_press_and_keeps_its_own() {
    let mut rig = Rig::sized("|alpha\nbeta\ngamma\n", 80, 24);
    rig.app.commit_console_reset();
    rig.app.on_commit_output(vec![CommitOutputLine {
        stream: OutputStream::Stderr,
        text: "running checks".to_string(),
    }]);
    rig.render();
    let console = rig.app.commit_console.rect;
    assert!(rig.app.commit_console.open && console.width > 0);

    // A press on the console is the console's: the text under it keeps its caret.
    rig.run(&[click(At::Cell(console.x + 1, console.y + 1))]);
    assert!(rig.app.commit_console.open);
    assert_eq!(rig.marked(), "|alpha\nbeta\ngamma\n");

    // A press outside hides it and lands where it was aimed.
    let (col, row) = rig.cell(text(1, 2));
    assert!(
        !rect_contains(console, (col, row)),
        "the target is clear of it"
    );
    rig.run(&[click(At::Cell(col, row))]);
    assert!(!rig.app.commit_console.open);
    assert_eq!(rig.app.input_context().modal, None);
    assert_eq!(rig.marked(), "alpha\nbe|ta\ngamma\n");
}
