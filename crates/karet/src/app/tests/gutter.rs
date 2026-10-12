//! The editor gutter's lanes (`docs/editing-behavior.md`, "Click"): line
//! numbers select whole lines, the marker lane toggles breakpoints, a change
//! marker opens the file's changes against `HEAD`, and the fold chevron and a
//! collapsed fold's `⋯` toggle the fold.

use karet_core::FoldRegion;
use karet_core::FoldRegions;

use super::harness::*;
use super::support::*;
use crate::app::*;

/// The line-number cell of `line`, in a gutter of under ten lines: the
/// marker, one digit, the change lane, then the fold lane right before the
/// text.
fn number(line: u32) -> At {
    At::Offset(line, 0, -3, 0)
}

/// The marker (breakpoint) lane of `line`'s row, `rows` rows further down.
fn marker(line: u32, rows: i16) -> At {
    At::Offset(line, 0, -4, rows)
}

/// The change lane of `line`'s row.
fn change_lane(line: u32) -> At {
    At::Offset(line, 0, -2, 0)
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
            vec![click(At::Offset(1, 0, -3, 2))],
            "alpha\nbravo|",
        ),
        case(
            "a fold-lane click on a line without a fold places the caret",
            "ab\ncd|",
            vec![click(fold_lane(0))],
            "|ab\ncd",
        ),
        case(
            "a change-lane click on a line without a change marker places the caret",
            "ab\ncd|",
            vec![click(change_lane(0))],
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
    rig.run(&[click(At::Offset(last, 0, -3, 0))]);
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

/// An added-lines change marker over `first..=last`, as the VCS worker
/// publishes it.
fn added(first: u32, last: u32) -> Decoration {
    Decoration {
        range: Range {
            start: LineCol::new(first, 0),
            end: LineCol::new(last, 0),
        },
        kind: DecorationKind::ChangeMarker { glyph: '\u{258e}' },
        role: Some(ThemeRole::GutterAdded),
    }
}

/// A rig over `marked` whose document (9) holds `markers`, on a recording
/// backend the test can read.
fn marked_rig(marked: &str, markers: Vec<Decoration>) -> (Rig, Arc<RecordingBackend>) {
    let mut rig = Rig::new(marked);
    let backend = Arc::new(RecordingBackend::new());
    rig.app.backend = Some(backend.clone());
    rig.app.docs.line_changes.insert(DocumentId(9), markers);
    rig.render();
    (rig, backend)
}

/// The revisions every `DiffWithRev` sent so far asked for.
fn diff_revs(backend: &RecordingBackend) -> Vec<String> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::DiffWithRev { rev, .. } => Some(rev.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_breakpoint_and_a_change_marker_on_one_line_are_both_drawn() {
    let (mut rig, _) = marked_rig("|alpha\nbravo", vec![added(0, 1)]);
    rig.run(&[click(marker(0, 0))]);
    assert_eq!(app_breakpoints(&rig), [(PathBuf::from("t.rs"), 0)]);
    let (bx, y) = rig.cell(marker(0, 0));
    let (cx, _) = rig.cell(change_lane(0));
    let screen = frame(&mut rig.app, 60, 12);
    assert_eq!(screen[(cx, y)].symbol(), "\u{258e}", "the change marker");
    assert_eq!(
        screen[(cx, y)].fg,
        rig.app.theme.role(ThemeRole::GutterAdded).to_ratatui()
    );
    assert_ne!(screen[(bx, y)].symbol(), " ", "the breakpoint beside it");
    assert_ne!(screen[(bx, y)].symbol(), "\u{258e}");
}

#[test]
fn a_change_marker_click_opens_the_changes_against_head() {
    let (mut rig, backend) = marked_rig("al|pha\nbravo\ncharlie", vec![added(1, 2)]);
    rig.run(&[click(change_lane(2))]);
    assert_eq!(diff_revs(&backend), ["HEAD"]);
    assert!(
        matches!(&rig.tab().kind, TabKind::Diff { .. }),
        "the diff tab opens in front"
    );
    assert_eq!(rig.tab().title, "t.rs (HEAD \u{2194} working)");
    assert!(app_breakpoints(&rig).is_empty(), "and sets no breakpoint");
    let code = rig
        .app
        .tabs
        .iter()
        .find(|tab| tab.path() == Some(Path::new("t.rs")));
    let caret = code.map(|tab| tab.editor.cursor());
    assert_eq!(caret, Some(LineCol::new(0, 2)), "nor moves the caret");
}

#[test]
fn the_change_lane_beside_an_unmarked_line_opens_nothing() {
    let (mut rig, backend) = marked_rig("al|pha\nbravo", vec![added(1, 1)]);
    rig.run(&[click(change_lane(0))]);
    assert!(diff_revs(&backend).is_empty());
    assert_eq!(
        rig.marked(),
        "|alpha\nbravo",
        "an ordinary click at the line start"
    );
}

/// The live buffer of the landing cases: 24 lines, `line 0` to `line 23`.
fn landing_buffer() -> String {
    (0..24).map(|n| format!("line {n}\n")).collect()
}

/// `HEAD`'s side of the landing cases: lines 1 and 12 differ and a line
/// below line 19 was deleted since, three hunks far enough apart that context
/// never merges them.
fn landing_head() -> String {
    landing_buffer()
        .replacen("line 1\n", "old 1\n", 1)
        .replacen("line 12\n", "old 12\n", 1)
        .replacen("line 19\n", "line 19\ndropped\n", 1)
}

/// The answer to the last `DiffWithRev` sent: `HEAD` diffed against the live
/// buffer with `context` lines around each change.
fn landing_answer(
    backend: &RecordingBackend,
    context: usize,
) -> (Option<RequestId>, PreparedChange) {
    let id = backend.sent.lock().ok().and_then(|sent| {
        sent.iter().rev().find_map(|(id, command)| {
            matches!(command, SessionCommand::DiffWithRev { .. }).then_some(*id)
        })
    });
    let diff = karet_diff::diff_text(
        &landing_head(),
        &landing_buffer(),
        &karet_diff::DiffOptions {
            context_lines: context,
            ..Default::default()
        },
    );
    let change = PreparedChange {
        path: PathBuf::from("t.rs"),
        old_path: None,
        status: karet_vcs::StatusKind::Modified,
        language: "plaintext".to_string(),
        diff: karet_diff::PreparedDiff::new(diff, Vec::new(), Vec::new()),
    };
    (id, change)
}

/// Open the landing file's changes in `layout` by `open` (a gesture or a
/// command) and answer them with a `context`-line diff; the filled diff
/// tab's scroll, and the row each hunk of that diff opens on.
fn landed_scroll(
    layout: ViewMode,
    context: usize,
    open: impl FnOnce(&mut Rig),
) -> (Option<u16>, Vec<Option<usize>>) {
    let mut rig = Rig::sized(&format!("|{}", landing_buffer()), 60, 32);
    let backend = Arc::new(RecordingBackend::new());
    rig.app.backend = Some(backend.clone());
    rig.app.diff_layout = layout;
    rig.app.docs.line_changes.insert(
        DocumentId(9),
        vec![added(1, 1), added(12, 12), added(19, 19)],
    );
    rig.render();
    open(&mut rig);
    let (id, change) = landing_answer(&backend, context);
    let rows = (0..change.diff.diff.hunks.len())
        .map(|index| match layout {
            ViewMode::Unified => karet_diff::unified_hunk_row(&change.diff, index),
            ViewMode::SideBySide => karet_diff::side_by_side_hunk_row(&change.diff, index),
        })
        .collect();
    rig.app.apply_diff_prepared(id, Ok(Box::new(change)));
    let scroll = match &rig.tab().kind {
        TabKind::Diff {
            file: Some(_),
            pager,
            ..
        } => Some(pager.scroll),
        _ => None,
    };
    (scroll, rows)
}

/// The scroll a change-marker click on `line` lands the diff on.
fn clicked_scroll(
    layout: ViewMode,
    context: usize,
    line: u32,
) -> (Option<u16>, Vec<Option<usize>>) {
    landed_scroll(layout, context, |rig| rig.run(&[click(change_lane(line))]))
}

fn as_scroll(row: Option<usize>) -> Option<u16> {
    row.and_then(|row| u16::try_from(row).ok())
}

#[test]
fn a_change_marker_click_lands_the_diff_on_the_clicked_hunk() {
    for layout in [ViewMode::Unified, ViewMode::SideBySide] {
        let (scroll, rows) = clicked_scroll(layout, 3, 12);
        assert_eq!(rows.len(), 3, "three hunks");
        assert_ne!(as_scroll(rows[1]), Some(0));
        assert_eq!(
            scroll,
            as_scroll(rows[1]),
            "{layout:?}: the middle hunk on top"
        );
        let (scroll, rows) = clicked_scroll(layout, 3, 1);
        assert_eq!(
            scroll,
            as_scroll(rows[0]),
            "{layout:?}: the first hunk on top"
        );
        let (scroll, rows) = clicked_scroll(layout, 3, 19);
        assert_eq!(
            scroll,
            as_scroll(rows[2]),
            "{layout:?}: the deletion's hunk on top"
        );
    }
}

#[test]
fn a_deletion_marker_lands_on_the_hunk_just_below_its_line() {
    // Without context, the deletion's hunk holds no new-side line at all: it
    // sits in the gap below line 19, where its marker is drawn.
    for layout in [ViewMode::Unified, ViewMode::SideBySide] {
        let (scroll, rows) = clicked_scroll(layout, 0, 19);
        assert_eq!(rows.len(), 3);
        assert_eq!(scroll, as_scroll(rows[2]), "{layout:?}");
    }
}

#[test]
fn the_command_palette_open_changes_still_opens_at_the_top() {
    for layout in [ViewMode::Unified, ViewMode::SideBySide] {
        let (scroll, _) = landed_scroll(layout, 3, |rig| {
            rig.run(&[Step::Cmd(Command::OpenChangesWithPrevious)]);
        });
        assert_eq!(scroll, Some(0), "{layout:?}");
    }
}

#[test]
fn without_git_decorations_the_gutter_has_no_change_lane() {
    let mut rig = Rig::new("|alpha\nbravo");
    let with_lane = rig.cell(text(0, 0));
    rig.app.settings.git.decorations = false;
    rig.render();
    let without = rig.cell(text(0, 0));
    assert_eq!(without.0 + 1, with_lane.0, "the text moves one cell left");
    // The cell left of the fold lane is the line number again.
    rig.run(&[click(At::Offset(1, 0, -2, 0))]);
    assert_eq!(rig.marked(), "alpha\n[bravo|]");
}
