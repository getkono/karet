//! The editor gutter's lanes (`docs/editing-behavior.md`, "Click"): line
//! numbers select whole lines, the marker lane toggles breakpoints, and the
//! fold chevron and a collapsed fold's `⋯` toggle the fold.

use karet_core::FoldRegion;
use karet_core::FoldRegions;

use super::harness::*;
use super::support::*;
use crate::app::*;

/// The line-number cell of `line`, in a gutter of under ten lines: the
/// marker, one digit, then the fold lane right before the text.
fn number(line: u32) -> At {
    At::Offset(line, 0, -2, 0)
}

/// The marker (breakpoint) lane of `line`'s row, `rows` rows further down.
fn marker(line: u32, rows: i16) -> At {
    At::Offset(line, 0, -3, rows)
}

/// The fold lane of `line`'s row.
fn fold_lane(line: u32) -> At {
    At::Offset(line, 0, -1, 0)
}

#[test]
fn line_number_conformance() {
    let shift = KeyModifiers::SHIFT;
    let none = KeyModifiers::NONE;
    check(vec![
        case(
            "a click on a line number selects the line with its line break",
            "al|pha\nbravo\ncharlie",
            vec![click(number(1))],
            "alpha\n[bravo\n|]charlie",
        ),
        case(
            "a click on the last line's number selects to its end",
            "|alpha\nbravo\ncharlie",
            vec![click(number(2))],
            "alpha\nbravo\n[charlie|]",
        ),
        case(
            "a drag down the line numbers selects whole lines",
            "|alpha\nbravo\ncharlie",
            vec![
                Step::Down(number(0), none),
                Step::Drag(number(1), none),
                Step::Up(number(1), none),
            ],
            "[alpha\nbravo\n|]charlie",
        ),
        case(
            "a drag up the line numbers keeps the first line whole",
            "|alpha\nbravo\ncharlie",
            vec![
                Step::Down(number(2), none),
                Step::Drag(number(0), none),
                Step::Up(number(0), none),
            ],
            "[|alpha\nbravo\ncharlie]",
        ),
        case(
            "a drag from a line number into the text still extends by lines",
            "|alpha\nbravo\ncharlie",
            vec![
                Step::Down(number(0), none),
                Step::Drag(text(1, 2), none),
                Step::Up(text(1, 2), none),
            ],
            "[alpha\nbravo\n|]charlie",
        ),
        case(
            "Shift+click on a line number extends from the caret's line by lines",
            "al|pha\nbravo\ncharlie",
            vec![Step::Click(number(1), shift)],
            "[alpha\nbravo\n|]charlie",
        ),
        case(
            "Shift+click above a line selection keeps its first line whole",
            "|alpha\nbravo\ncharlie",
            vec![click(number(2)), PAUSE, Step::Click(number(0), shift)],
            "[|alpha\nbravo\ncharlie]",
        ),
        case(
            "Shift+click turns an upward line selection back down from its line",
            "|alpha\nbravo\ncharlie\ndelta",
            vec![
                click(number(1)),
                PAUSE,
                Step::Click(number(0), shift),
                PAUSE,
                Step::Click(number(2), shift),
            ],
            "alpha\n[bravo\ncharlie\n|]delta",
        ),
        case(
            "a click in the gutter below the last line is a click below the text",
            "|alpha\nbravo",
            vec![click(At::Offset(1, 0, -2, 2))],
            "alpha\nbravo|",
        ),
        case(
            "a fold-lane click on a line without a fold places the caret",
            "ab\ncd|",
            vec![click(fold_lane(0))],
            "|ab\ncd",
        ),
    ]);
}

#[test]
fn a_line_number_click_never_scrolls_the_text_under_it() {
    let text: String = (0..30).map(|n| format!("line {n}\n")).collect();
    let mut rig = Rig::new(&format!("|{text}"));
    let top = rig.scroll_line();
    let rows = rig.app.tabs[rig.app.active].editor.visible_lines();
    // The last visible row: its line break ends on the row below the viewport.
    let last = top + rows - 1;
    // The second of its two digits.
    rig.run(&[click(At::Offset(last, 0, -2, 0))]);
    assert_eq!(rig.scroll_line(), top);
    let selection = rig.tab().editor.selection_range();
    assert_eq!(
        selection.map(|range| (range.start, range.end)),
        Some((LineCol::new(last, 0), LineCol::new(last + 1, 0)))
    );
}

/// A rig over `marked` whose first `end + 1` lines form one fold region.
fn folding_rig(marked: &str, end: u32) -> Rig {
    let mut rig = Rig::new(marked);
    let active = rig.app.active;
    if let TabKind::Code { folds, .. } = &mut rig.app.tabs[active].kind {
        *folds = FoldRegions::from_sorted_regions(vec![FoldRegion { start: 0, end }]);
    }
    rig.render();
    rig
}

fn folded(rig: &Rig) -> bool {
    matches!(&rig.tab().kind, TabKind::Code { folded, .. } if folded.contains(&0))
}

#[test]
fn the_chevron_and_the_ellipsis_toggle_the_fold() {
    let mut rig = folding_rig("fn a() {\n    x\n}\n|tail", 2);
    rig.run(&[click(fold_lane(0))]);
    assert!(folded(&rig), "the chevron collapses the fold");
    assert!(app_breakpoints(&rig).is_empty(), "and sets no breakpoint");
    assert_eq!(
        rig.marked(),
        "fn a() {\n    x\n}\n|tail",
        "nor moves the caret"
    );
    // ` ⋯` is painted one cell after the header's end.
    rig.run(&[click(At::Offset(0, 8, 1, 0))]);
    assert!(!folded(&rig), "the ellipsis expands it");
    assert_eq!(rig.marked(), "fn a() {\n    x\n}\n|tail");
    // A quick click right after on the chevron toggles again, not a double-click.
    rig.run(&[click(fold_lane(0)), click(fold_lane(0))]);
    assert!(!folded(&rig));
}

#[test]
fn collapsing_a_fold_over_the_caret_moves_it_onto_the_header() {
    let mut rig = folding_rig("fn a() {\n  |  x\n}\ntail", 2);
    rig.run(&[click(fold_lane(0))]);
    assert!(folded(&rig));
    assert_eq!(rig.marked(), "fn| a() {\n    x\n}\ntail");
}

fn app_breakpoints(rig: &Rig) -> Vec<(PathBuf, u32)> {
    rig.app
        .breakpoints
        .iter()
        .flat_map(|(path, lines)| lines.keys().map(move |line| (path.clone(), *line)))
        .collect()
}

#[test]
fn the_marker_lane_toggles_a_breakpoint_and_leaves_the_caret() {
    let mut rig = Rig::new("|alpha\nbravo");
    rig.run(&[click(marker(1, 0))]);
    assert_eq!(app_breakpoints(&rig), [(PathBuf::from("t.rs"), 1)]);
    assert_eq!(rig.marked(), "|alpha\nbravo");
    rig.run(&[PAUSE, click(marker(1, 0))]);
    assert!(app_breakpoints(&rig).is_empty());
}

#[test]
fn the_marker_lane_below_the_last_line_sets_no_breakpoint() {
    let mut rig = Rig::new("|alpha\nbravo");
    rig.run(&[click(marker(1, 1)), PAUSE, click(marker(1, 3))]);
    assert!(app_breakpoints(&rig).is_empty());
    assert_eq!(rig.marked(), "alpha\nbravo|");
}

#[test]
fn a_file_no_debugger_runs_takes_no_breakpoint() {
    let mut rig = Rig::new("|- notes\nbody");
    let active = rig.app.active;
    if let TabKind::Code { path, .. } = &mut rig.app.tabs[active].kind {
        *path = PathBuf::from("notes.md");
    }
    rig.run(&[click(marker(0, 0))]);
    assert!(app_breakpoints(&rig).is_empty());
    assert_eq!(rig.marked(), "|- notes\nbody");
    assert_eq!(
        last_message(&rig.app).as_deref(),
        Some("breakpoints need a source file")
    );
    // F9 is refused the same way.
    rig.run(&[Step::Cmd(Command::DebugToggleBreakpoint)]);
    assert!(app_breakpoints(&rig).is_empty());
}
