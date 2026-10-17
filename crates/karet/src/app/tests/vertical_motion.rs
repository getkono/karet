//! Vertical and line motion rows of `docs/editing-behavior.md` — the goal
//! column, the first and last lines, PageUp / PageDown, smart Home, collapsed
//! folds and word wrap — run through the conformance harness.

use karet_core::FoldRegion;
use karet_core::FoldRegions;

use super::harness::*;
use crate::app::*;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const ALT: KeyModifiers = KeyModifiers::ALT;

#[test]
fn up_and_down_keep_the_goal_column() {
    use KeyCode::*;
    check(vec![
        case(
            "Down twice crosses a short line back to the goal column",
            "abcde|f\nab\nabcdef",
            vec![key(Down), key(Down)],
            "abcdef\nab\nabcde|f",
        ),
        case(
            "Up twice does too",
            "abcdef\nab\nabcde|f",
            vec![key(Up), key(Up)],
            "abcde|f\nab\nabcdef",
        ),
        case(
            "the goal column is in display cells: a tab is four",
            "\tf|oo\nabcdefgh",
            vec![key(Down)],
            "\tfoo\nabcde|fgh",
        ),
        case(
            "a wide character is two",
            "日本|語\nabcdef",
            vec![key(Down)],
            "日本語\nabcd|ef",
        ),
        case(
            "a horizontal motion resets the goal column",
            "abcde|f\nab\nabcdef",
            vec![key(Down), key(Left), key(Down)],
            "abcdef\nab\na|bcdef",
        ),
        case(
            "typing resets the goal column",
            "abcde|f\nab\nabcdef",
            vec![key(Down), key(Char('x')), key(Down)],
            "abcdef\nabx\nabc|def",
        ),
        case(
            "every caret keeps its own goal column",
            "abcde|f\nab\nabc#def\nab",
            vec![key(Down), key(Down)],
            "abcdef\nab\nabcde|f\nab#",
        ),
        case(
            "Ctrl+Alt+Down adds carets at the goal column",
            "abcde|f\nab\nabcdef",
            vec![Step::Key(Down, CTRL | ALT), Step::Key(Down, CTRL | ALT)],
            "abcde|f\nab|\nabcde#f",
        ),
    ]);
}

#[test]
fn up_on_the_first_line_and_down_on_the_last_go_to_its_ends() {
    use KeyCode::*;
    check(vec![
        case(
            "Up on the first line goes to its start",
            "ab|c\ndef",
            vec![key(Up)],
            "|abc\ndef",
        ),
        case(
            "Down on the last line goes to its end",
            "abc\nd|ef",
            vec![key(Down)],
            "abc\ndef|",
        ),
        case(
            "Shift+Up on the first line selects to its start",
            "ab|c\ndef",
            vec![Step::Key(Up, SHIFT)],
            "[|ab]c\ndef",
        ),
        case(
            "Shift+Down on the last line selects to its end",
            "abc\nd|ef",
            vec![Step::Key(Down, SHIFT)],
            "abc\nd[ef|]",
        ),
        case(
            "the goal column outlives the trip to the start",
            "ab|c\ndef",
            vec![key(Up), key(Down)],
            "abc\nde|f",
        ),
    ]);
}

#[test]
fn home_toggles_between_the_first_non_blank_and_column_zero() {
    use KeyCode::*;
    check(vec![
        case(
            "Home goes to the first non-blank character",
            "    let| x",
            vec![key(Home)],
            "    |let x",
        ),
        case(
            "Home there goes to column 0",
            "    let| x",
            vec![key(Home), key(Home)],
            "|    let x",
        ),
        case(
            "and Home again back to the first non-blank",
            "    let| x",
            vec![key(Home), key(Home), key(Home)],
            "    |let x",
        ),
        case(
            "Home inside the indentation goes to the first non-blank",
            "  |  let x",
            vec![key(Home)],
            "    |let x",
        ),
        case(
            "Home on a blank line goes to column 0",
            "x\n   |",
            vec![key(Home)],
            "x\n|   ",
        ),
        case(
            "Shift+Home selects to the first non-blank",
            "    let| x",
            vec![Step::Key(Home, SHIFT)],
            "    [|let] x",
        ),
    ]);
}

/// Sixty numbered lines with the caret at `line 0`'s third column.
fn sixty_lines() -> String {
    let body: String = (1..60).map(|n| format!("\nline {n}")).collect();
    format!("li|ne 0{body}")
}

/// The rows the focused pane showed in the last frame.
fn page(rig: &Rig) -> u32 {
    let viewport = rig.tab().editor.viewport();
    viewport.end.line - viewport.start.line
}

#[test]
fn page_down_and_up_move_the_caret_a_page_and_scroll_with_it() {
    use KeyCode::*;
    let mut rig = Rig::new(&sixty_lines());
    let rows = page(&rig);
    // The middle row, clear of the scroll margin at either edge.
    let mid = rows / 2;
    rig.run(&vec![key(Down); mid as usize]);
    assert!(rows > 2, "a page is the pane's text rows");

    rig.run(&[key(PageDown)]);
    assert_eq!(rig.tab().editor.cursor(), LineCol::new(mid + rows, 2));
    assert_eq!(rig.scroll_line(), rows, "the caret keeps its screen row");

    rig.run(&[key(PageUp)]);
    assert_eq!(rig.tab().editor.cursor(), LineCol::new(mid, 2));
    assert_eq!(rig.scroll_line(), 0);

    rig.run(&[key(PageUp)]);
    assert_eq!(
        rig.tab().editor.cursor(),
        LineCol::new(0, 0),
        "less than a page above: the start of the document"
    );

    rig.run(&[Step::Key(PageDown, SHIFT)]);
    let selection = rig.tab().editor.cursors().primary();
    assert_eq!(selection.anchor, LineCol::new(0, 0));
    assert_eq!(
        selection.head,
        LineCol::new(rows, 2),
        "the goal column outlived the trip to the start"
    );

    rig.run(&[Step::Key(End, CTRL), key(PageDown)]);
    assert_eq!(
        rig.tab().editor.cursor(),
        LineCol::new(59, 7),
        "PageDown on the last page goes to the document's end"
    );
}

/// A rig over `marked` with one collapsed fold from `start` through `end`.
fn folded_rig(marked: &str, start: u32, end: u32) -> Rig {
    let mut rig = Rig::new(marked);
    let active = rig.app.active;
    if let TabKind::Code { folds, folded, .. } = &mut rig.app.tabs[active].kind {
        *folds = FoldRegions::from_sorted_regions(vec![FoldRegion::new(start, end)]);
        folded.insert(start);
    }
    rig.render();
    rig
}

fn is_folded(rig: &Rig, start: u32) -> bool {
    matches!(&rig.tab().kind, TabKind::Code { folded, .. } if folded.contains(&start))
}

#[test]
fn motion_steps_over_a_collapsed_fold() {
    use KeyCode::*;
    let mut rig = folded_rig("t|op\nfn a() {\n    x\n}\ntail", 1, 3);
    rig.run(&[key(Down), key(Down)]);
    assert_eq!(rig.marked(), "top\nfn a() {\n    x\n}\nt|ail");
    rig.run(&[key(Up)]);
    assert_eq!(rig.marked(), "top\nf|n a() {\n    x\n}\ntail");
    rig.run(&[key(End), key(Right)]);
    assert_eq!(rig.marked(), "top\nfn a() {\n    x\n}\n|tail");
    rig.run(&[key(Left)]);
    assert_eq!(rig.marked(), "top\nfn a() {|\n    x\n}\ntail");
    rig.run(&[Step::Key(Down, SHIFT)]);
    assert_eq!(rig.marked(), "top\nfn a() {[\n    x\n}\ntail|]");
    assert!(is_folded(&rig, 1), "no motion opened the fold");
}

#[test]
fn a_jump_into_a_collapsed_fold_unfolds_it() {
    let mut rig = folded_rig("top\nfn a() |{\n    x\n}\ntail", 1, 3);
    rig.run(&[Step::Cmd(Command::GoToBracket)]);
    assert_eq!(rig.marked(), "top\nfn a() {\n    x\n|}\ntail");
    assert!(!is_folded(&rig, 1), "the caret is never left hidden");
}

#[test]
fn collapsing_a_fold_lifts_every_caret_inside_onto_its_header() {
    let mut rig = Rig::new("fn #a() {\n  |  x\n|}\ntail");
    let active = rig.app.active;
    if let TabKind::Code { folds, .. } = &mut rig.app.tabs[active].kind {
        *folds = FoldRegions::from_sorted_regions(vec![FoldRegion::new(0, 2)]);
    }
    rig.render();
    rig.run(&[Step::Cmd(Command::ToggleFold)]);
    assert!(is_folded(&rig, 0));
    assert_eq!(rig.marked(), "|fn| #a() {\n    x\n}\ntail");
}

/// A rig over `text` in a 30-cell terminal with word wrap on, and the cells
/// a row of its text holds.
fn wrapped_rig(text: &str) -> (Rig, u32) {
    let mut rig = Rig::sized(text, 30, 12);
    let editor = serde_json::from_str(r#"{ "wordWrap": true }"#);
    rig.app.settings.editor = editor.unwrap_or_default();
    rig.render();
    let width = u32::from(rig.tab().editor.content_width());
    (rig, width)
}

#[test]
fn with_word_wrap_motion_acts_on_visual_rows() {
    use KeyCode::*;
    // One unbroken word five cells longer than a row: rows `0..width` and
    // `width..width + 5`.
    let long = "x".repeat(40);
    let (mut rig, width) = wrapped_rig(&format!("xx|{long}\nshort"));
    assert!(width < 42, "the line wraps");
    let at = |line, col| LineCol::new(line, col);
    let mut path = Vec::new();
    for _ in 0..4 {
        let motion = if path.len() < 2 { Down } else { Up };
        rig.run(&[key(motion)]);
        path.push(rig.tab().editor.cursor());
    }
    assert_eq!(
        path,
        vec![at(0, width + 2), at(1, 2), at(0, width + 2), at(0, 2)]
    );

    rig.run(&[key(End)]);
    assert_eq!(rig.tab().editor.cursor(), at(0, width), "the row's end");
    rig.run(&[key(End)]);
    assert_eq!(rig.tab().editor.cursor(), at(0, 42), "then the line's end");
    rig.run(&[key(Home)]);
    assert_eq!(
        rig.tab().editor.cursor(),
        at(0, width),
        "Home on a continuation row goes to its start"
    );
}
